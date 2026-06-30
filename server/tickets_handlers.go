// Ticket HTTP handlers (§7.4).
//
// DB-backed, authenticated, GitHub-Issues-like. Two creation paths share one
// model (§7.1): the rich "Report Issue" payload (auto-captures screenshot /
// project.fdraft / actions / settings / ui_state / attachments → persisted as
// ticket_artifacts blobs) and a plain title+body issue. Repro artifacts ride
// the same content-addressed blob storage WP1 uses for project documents.

package main

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"time"
)

// maxTicketBytes caps a ticket-creation body (matches the old file flow).
const maxTicketBytes = 25 << 20

// ─── creation (§7.1: two paths, one model) ──────────────────────

// createTicketRequest covers BOTH creation paths. The "Report Issue" dialog
// fills Description + the repro fields; the plain Issues-UI fills Title + Body.
type createTicketRequest struct {
	// Plain issue path.
	Title    string `json:"title"`
	Body     string `json:"body"`
	Category string `json:"category"`

	// Report-Issue path (auto-repro). Description is the free-form text; when
	// Title/Body are absent it seeds both (title = first line, body = full text).
	Description   string             `json:"description"`
	ProjectFdraft string             `json:"projectFdraft"` // base64 .fdraft ZIP
	Screenshot    string             `json:"screenshot"`    // PNG data URL
	Attachments   []ticketAttachment `json:"attachments"`
	Version       string             `json:"version"`
	Timestamp     string             `json:"timestamp"`
	UserAgent     string             `json:"userAgent"`
	URL           string             `json:"url"`
	Settings      json.RawMessage    `json:"settings"`
	UIState       json.RawMessage    `json:"uiState"`
	ActionLog     json.RawMessage    `json:"actionLog"`
}

func (s *Server) handleCreateTicket(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	var req createTicketRequest
	if err := json.NewDecoder(io.LimitReader(r.Body, maxTicketBytes)).Decode(&req); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}

	// Resolve title + body across both paths. The Report-Issue path carries
	// only Description; the plain path carries Title + Body.
	title := strings.TrimSpace(req.Title)
	body := req.Body
	if body == "" {
		body = req.Description
	}
	if strings.TrimSpace(body) == "" {
		http.Error(w, `{"error":"body or description required"}`, http.StatusBadRequest)
		return
	}
	if title == "" {
		title = ticketHeading("", body)
	}
	if !validCategory(req.Category) {
		http.Error(w, `{"error":"invalid category"}`, http.StatusBadRequest)
		return
	}

	t := s.store.CreateTicket(user.ID, title, body, req.Category)
	if t == nil {
		http.Error(w, `{"error":"could not create ticket"}`, http.StatusInternalServerError)
		return
	}

	// Persist repro artifacts (Report-Issue path). Each is content-addressed.
	s.persistArtifacts(t.ID, &req)

	// Re-fetch so the response carries any artifacts/category just written.
	full := s.store.GetTicket(t.ID)
	if full == nil {
		full = t
	}
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(ticketDetail(full, user, false))
}

// persistArtifacts decodes and stores every repro blob carried by a
// Report-Issue payload. Invalid/empty fields are silently skipped.
func (s *Server) persistArtifacts(ticketID string, req *createTicketRequest) {
	if png := decodeDataURL(req.Screenshot); png != nil {
		s.store.AddArtifact(ticketID, ArtifactScreenshot, "screenshot.png", "image/png", png)
	}
	if req.ProjectFdraft != "" {
		if blob, err := base64.StdEncoding.DecodeString(req.ProjectFdraft); err == nil && len(blob) > 0 {
			s.store.AddArtifact(ticketID, ArtifactFdraft, "project.fdraft", "application/zip", blob)
		}
	}
	if isJSONPresent(req.ActionLog) {
		s.store.AddArtifact(ticketID, ArtifactActions, "actions.json", "application/json", req.ActionLog)
	}
	if isJSONPresent(req.Settings) {
		s.store.AddArtifact(ticketID, ArtifactSettings, "settings.json", "application/json", req.Settings)
	}
	if isJSONPresent(req.UIState) {
		s.store.AddArtifact(ticketID, ArtifactUIState, "ui_state.json", "application/json", req.UIState)
	}
	for i, a := range req.Attachments {
		blob := decodeDataURL(a.DataURL)
		if blob == nil {
			continue
		}
		name := attachmentFilename(i, a.Name)
		mime := a.Mime
		if mime == "" {
			mime = "application/octet-stream"
		}
		s.store.AddArtifact(ticketID, ArtifactAttachment, name, mime, blob)
	}
}

// decodeDataURL extracts the bytes from a "data:...;base64,<payload>" URL.
// Returns nil for anything that isn't a base64 data URL.
func decodeDataURL(s string) []byte {
	if !strings.HasPrefix(s, "data:") {
		return nil
	}
	ci := strings.IndexByte(s, ',')
	if ci < 0 {
		return nil
	}
	blob, err := base64.StdEncoding.DecodeString(s[ci+1:])
	if err != nil || len(blob) == 0 {
		return nil
	}
	return blob
}

// isJSONPresent reports whether a raw JSON field carries real content.
func isJSONPresent(raw json.RawMessage) bool {
	return len(raw) > 0 && string(raw) != "null"
}

// ─── list / detail ──────────────────────────────────────────────

func (s *Server) handleListTickets(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	q := r.URL.Query()
	f := TicketFilter{
		State:         TicketState(q.Get("state")),
		Category:      q.Get("category"),
		Query:         q.Get("q"),
		ViewerID:      user.ID,
		ViewerIsAdmin: user.Role == RoleAdmin,
	}
	switch author := q.Get("author"); author {
	case "me":
		f.AuthorID = user.ID
	case "":
		// any author
	default:
		f.AuthorID = author
	}
	tickets := s.store.ListTickets(f)
	out := make([]ticketSummary, 0, len(tickets))
	for _, t := range tickets {
		out = append(out, summarize(t))
	}
	json.NewEncoder(w).Encode(out)
}

func (s *Server) handleGetTicket(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	t := s.store.GetTicket(r.PathValue("id"))
	if t == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	isAdmin := user.Role == RoleAdmin
	// Rejected tickets are visible only to admins or the author (§7.3).
	if isRejected(t.State) && !isAdmin && t.AuthorID != user.ID {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	json.NewEncoder(w).Encode(ticketDetail(t, user, isAdmin))
}

// ─── update / delete ────────────────────────────────────────────

func (s *Server) handlePatchTicket(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	t := s.store.GetTicket(id)
	if t == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	isAdmin := user.Role == RoleAdmin
	isAuthor := t.AuthorID == user.ID
	if !isAdmin && !isAuthor {
		http.Error(w, `{"error":"forbidden"}`, http.StatusForbidden)
		return
	}

	var body struct {
		Title *string `json:"title"`
		Body  *string `json:"body"`
		State *string `json:"state"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}

	// Author/admin may edit title + body.
	if body.Title != nil || body.Body != nil {
		newTitle, newBody := "", ""
		if body.Title != nil {
			newTitle = *body.Title
		}
		if body.Body != nil {
			newBody = *body.Body
		}
		if s.store.UpdateTicket(id, newTitle, newBody) == nil {
			http.Error(w, `{"error":"could not update ticket"}`, http.StatusInternalServerError)
			return
		}
	}

	// State change. The author may only close (resolved → closed); the admin
	// may drive any validated transition (PO/dev moves).
	if body.State != nil {
		to := TicketState(*body.State)
		if !isAdmin {
			if to != StateClosed {
				http.Error(w, `{"error":"only admins may set this state"}`, http.StatusForbidden)
				return
			}
		}
		if _, err := s.store.SetTicketState(id, to); err != nil {
			writeStoreError(w, err)
			return
		}
	}

	full := s.store.GetTicket(id)
	json.NewEncoder(w).Encode(ticketDetail(full, user, isAdmin))
}

func (s *Server) handleDeleteTicket(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	t := s.store.GetTicket(id)
	if t == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	if user.Role != RoleAdmin && t.AuthorID != user.ID {
		http.Error(w, `{"error":"forbidden"}`, http.StatusForbidden)
		return
	}
	if !s.store.DeleteTicket(id) {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	w.Write([]byte(`{"ok":true}`))
}

// ─── comments ───────────────────────────────────────────────────

func (s *Server) handleListComments(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	t := s.store.GetTicket(id)
	if t == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	isAdmin := user.Role == RoleAdmin
	if isRejected(t.State) && !isAdmin && t.AuthorID != user.ID {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	comments := s.store.ListComments(id)
	json.NewEncoder(w).Encode(visibleComments(comments, isAdmin))
}

func (s *Server) handleAddComment(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	t := s.store.GetTicket(id)
	if t == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	isAdmin := user.Role == RoleAdmin
	if isRejected(t.State) && !isAdmin && t.AuthorID != user.ID {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	var body struct {
		Body string `json:"body"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	if strings.TrimSpace(body.Body) == "" {
		http.Error(w, `{"error":"comment body required"}`, http.StatusBadRequest)
		return
	}
	// Any authenticated user comments as author_type='user'.
	uid := user.ID
	c := s.store.AddComment(id, AuthorUser, &uid, body.Body, false)
	if c == nil {
		http.Error(w, `{"error":"could not add comment"}`, http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(c)
}

// ─── artifact download ──────────────────────────────────────────

func (s *Server) handleGetArtifact(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	t := s.store.GetTicket(id)
	if t == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	isAdmin := user.Role == RoleAdmin
	if isRejected(t.State) && !isAdmin && t.AuthorID != user.ID {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	art, data := s.store.GetArtifactBlob(id, r.PathValue("artifactId"))
	if art == nil || data == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	mime := art.Mime
	if mime == "" {
		mime = "application/octet-stream"
	}
	w.Header().Set("Content-Type", mime)
	w.WriteHeader(http.StatusOK)
	w.Write(data)
}

// ─── response shaping (visibility, §7.3) ────────────────────────

// ticketSummary is one row in the list view (no thread, no body blob).
type ticketSummary struct {
	ID        string      `json:"id"`
	AuthorID  string      `json:"author_id"`
	Title     string      `json:"title"`
	State     TicketState `json:"state"`
	Category  *string     `json:"category"`
	CreatedAt time.Time   `json:"created_at"`
	UpdatedAt time.Time   `json:"updated_at"`
}

func summarize(t *Ticket) ticketSummary {
	return ticketSummary{
		ID:        t.ID,
		AuthorID:  t.AuthorID,
		Title:     t.Title,
		State:     t.State,
		Category:  t.Category,
		CreatedAt: t.CreatedAt,
		UpdatedAt: t.UpdatedAt,
	}
}

// ticketDetailResponse is the full ticket view with a visibility-filtered thread.
type ticketDetailResponse struct {
	*Ticket
	Comments  []*TicketComment  `json:"comments"`
	Artifacts []*TicketArtifact `json:"artifacts"`
}

// ticketDetail builds the detail payload, filtering internal `dev` comments for
// non-admins (§7.3: user-visible are user / po / system; dev internal notes are
// admin-only).
func ticketDetail(t *Ticket, viewer *User, isAdmin bool) ticketDetailResponse {
	visible := visibleComments(t.Comments, isAdmin)
	artifacts := t.Artifacts
	if artifacts == nil {
		artifacts = []*TicketArtifact{}
	}
	if visible == nil {
		visible = []*TicketComment{}
	}
	return ticketDetailResponse{Ticket: t, Comments: visible, Artifacts: artifacts}
}

// visibleComments drops internal (dev) and withheld (leak-filtered) comments for
// non-admin viewers. Admins see everything.
func visibleComments(comments []*TicketComment, isAdmin bool) []*TicketComment {
	if isAdmin {
		return comments
	}
	out := make([]*TicketComment, 0, len(comments))
	for _, c := range comments {
		if c.Internal || c.Withheld {
			continue
		}
		out = append(out, c)
	}
	return out
}
