// M11: FieldDraft backend — Auth, User, Project CRUD.
//
// Minimal Go server with chi router, Argon2id password hashing,
// session cookies, and a JSON project store. MariaDB comes in
// M11.1; this first slice runs with an in-memory store so the
// server is immediately testable without Docker.

package main

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/argon2"
)

// ─── models ─────────────────────────────────────────────────────

type User struct {
	ID           string    `json:"id"`
	Email        string    `json:"email"`
	DisplayName  string    `json:"display_name"`
	PasswordHash []byte    `json:"-"`
	Salt         []byte    `json:"-"`
	CreatedAt    time.Time `json:"created_at"`
}

type Project struct {
	ID         string          `json:"id"`
	OwnerID    string          `json:"owner_id"`
	Name       string          `json:"name"`
	Visibility string          `json:"visibility"` // "private" | "public"
	Document   json.RawMessage `json:"document"`
	UpdatedAt  time.Time       `json:"updated_at"`
	CreatedAt  time.Time       `json:"created_at"`
}

type Session struct {
	Token   string
	UserID  string
	Expires time.Time
}

// ─── in-memory store (M11.0 — MariaDB comes in M11.1) ──────────

// M13: password reset token.
type ResetToken struct {
	Token   string
	UserID  string
	Expires time.Time
}

// P2.5: published snapshot for shareable viewer.
type Snapshot struct {
	ID        string          `json:"id"`
	ProjectID string          `json:"project_id"`
	Document  json.RawMessage `json:"document"`
	CreatedAt time.Time       `json:"created_at"`
}

type Store struct {
	mu          sync.RWMutex
	users       map[string]*User    // keyed by ID
	emails      map[string]string   // email → user ID
	projects    map[string]*Project // keyed by ID
	sessions    map[string]*Session // token → session
	resetTokens map[string]*ResetToken // token → reset
	thumbnails  map[string][]byte  // project ID → PNG data
	snapshots   map[string]*Snapshot // snapshot ID → snapshot
}

func NewStore() *Store {
	return &Store{
		users:       make(map[string]*User),
		emails:      make(map[string]string),
		projects:    make(map[string]*Project),
		sessions:    make(map[string]*Session),
		resetTokens: make(map[string]*ResetToken),
		thumbnails:  make(map[string][]byte),
		snapshots:   make(map[string]*Snapshot),
	}
}

func (s *Store) CreateUser(email, displayName, password string) (*User, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if _, exists := s.emails[email]; exists {
		return nil, errConflict
	}

	salt := make([]byte, 16)
	rand.Read(salt)
	hash := argon2.IDKey([]byte(password), salt, 1, 64*1024, 4, 32)

	id := newID()
	user := &User{
		ID:           id,
		Email:        email,
		DisplayName:  displayName,
		PasswordHash: hash,
		Salt:         salt,
		CreatedAt:    time.Now(),
	}
	s.users[id] = user
	s.emails[email] = id
	return user, nil
}

func (s *Store) AuthenticateUser(email, password string) (*User, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	uid, ok := s.emails[email]
	if !ok {
		return nil, errNotFound
	}
	user := s.users[uid]
	hash := argon2.IDKey([]byte(password), user.Salt, 1, 64*1024, 4, 32)
	if !bytesEqual(hash, user.PasswordHash) {
		return nil, errUnauthorized
	}
	return user, nil
}

func (s *Store) GetUserByEmail(email string) *User {
	s.mu.RLock()
	defer s.mu.RUnlock()
	uid, ok := s.emails[email]
	if !ok {
		return nil
	}
	return s.users[uid]
}

func (s *Store) UpdatePassword(userID, newPassword string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	user, ok := s.users[userID]
	if !ok {
		return
	}
	salt := make([]byte, 16)
	rand.Read(salt)
	user.PasswordHash = argon2.IDKey([]byte(newPassword), salt, 1, 64*1024, 4, 32)
	user.Salt = salt
}

func (s *Store) CreateResetToken(userID string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	token := newID()
	s.resetTokens[token] = &ResetToken{
		Token:   token,
		UserID:  userID,
		Expires: time.Now().Add(1 * time.Hour),
	}
	return token
}

func (s *Store) ConsumeResetToken(token string) (string, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rt, ok := s.resetTokens[token]
	if !ok || time.Now().After(rt.Expires) {
		delete(s.resetTokens, token)
		return "", false
	}
	delete(s.resetTokens, token)
	return rt.UserID, true
}

func (s *Store) SetThumbnail(projectID string, data []byte) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.thumbnails[projectID] = data
}

func (s *Store) GetThumbnail(projectID string) []byte {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.thumbnails[projectID]
}

func (s *Store) CreateSnapshot(projectID string, doc json.RawMessage) *Snapshot {
	s.mu.Lock()
	defer s.mu.Unlock()
	snap := &Snapshot{
		ID:        newID(),
		ProjectID: projectID,
		Document:  doc,
		CreatedAt: time.Now(),
	}
	s.snapshots[snap.ID] = snap
	return snap
}

func (s *Store) GetSnapshot(id string) *Snapshot {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.snapshots[id]
}

func (s *Store) CreateSession(userID string) *Session {
	s.mu.Lock()
	defer s.mu.Unlock()

	token := newID()
	sess := &Session{
		Token:   token,
		UserID:  userID,
		Expires: time.Now().Add(7 * 24 * time.Hour),
	}
	s.sessions[token] = sess
	return sess
}

func (s *Store) GetSession(token string) *Session {
	s.mu.RLock()
	defer s.mu.RUnlock()

	sess, ok := s.sessions[token]
	if !ok || time.Now().After(sess.Expires) {
		return nil
	}
	return sess
}

func (s *Store) DeleteSession(token string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.sessions, token)
}

func (s *Store) GetUser(id string) *User {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.users[id]
}

func (s *Store) CreateProject(ownerID, name string, doc json.RawMessage) *Project {
	s.mu.Lock()
	defer s.mu.Unlock()

	id := newID()
	now := time.Now()
	p := &Project{
		ID:         id,
		OwnerID:    ownerID,
		Name:       name,
		Visibility: "private",
		Document:   doc,
		UpdatedAt:  now,
		CreatedAt:  now,
	}
	s.projects[id] = p
	return p
}

func (s *Store) ListProjects(ownerID string) []*Project {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var result []*Project
	for _, p := range s.projects {
		if p.OwnerID == ownerID {
			result = append(result, p)
		}
	}
	return result
}

func (s *Store) GetProject(id, ownerID string) *Project {
	s.mu.RLock()
	defer s.mu.RUnlock()

	p, ok := s.projects[id]
	if !ok || p.OwnerID != ownerID {
		return nil
	}
	return p
}

func (s *Store) UpdateProject(id, ownerID string, name string, doc json.RawMessage, visibility string) *Project {
	s.mu.Lock()
	defer s.mu.Unlock()

	p, ok := s.projects[id]
	if !ok || p.OwnerID != ownerID {
		return nil
	}
	if name != "" {
		p.Name = name
	}
	if doc != nil {
		p.Document = doc
	}
	if visibility == "public" || visibility == "private" {
		p.Visibility = visibility
	}
	p.UpdatedAt = time.Now()
	return p
}

func (s *Store) ListPublicProjects() []*Project {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var result []*Project
	for _, p := range s.projects {
		if p.Visibility == "public" {
			result = append(result, p)
		}
	}
	return result
}

func (s *Store) DeleteProject(id, ownerID string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()

	p, ok := s.projects[id]
	if !ok || p.OwnerID != ownerID {
		return false
	}
	delete(s.projects, id)
	return true
}

// ─── HTTP handlers ──────────────────────────────────────────────

var (
	errConflict     = &httpError{http.StatusConflict, "email already registered"}
	errNotFound     = &httpError{http.StatusNotFound, "not found"}
	errUnauthorized = &httpError{http.StatusUnauthorized, "invalid credentials"}
)

type httpError struct {
	Code    int
	Message string
}

func (e *httpError) Error() string { return e.Message }

type Server struct {
	store       *Store
	mux         *http.ServeMux
	rateLimiter *RateLimiter
}

func NewServer(store *Store) *Server {
	s := &Server{
		store:       store,
		mux:         http.NewServeMux(),
		rateLimiter: NewRateLimiter(1.0, 60), // 60 burst, 1/sec refill
	}
	s.mux.HandleFunc("POST /api/auth/register", s.handleRegister)
	s.mux.HandleFunc("POST /api/auth/login", s.handleLogin)
	s.mux.HandleFunc("POST /api/auth/logout", s.handleLogout)
	s.mux.HandleFunc("GET /api/me", s.handleMe)
	s.mux.HandleFunc("GET /api/projects", s.handleListProjects)
	s.mux.HandleFunc("POST /api/projects", s.handleCreateProject)
	s.mux.HandleFunc("GET /api/projects/{id}", s.handleGetProject)
	s.mux.HandleFunc("PATCH /api/projects/{id}", s.handleUpdateProject)
	s.mux.HandleFunc("DELETE /api/projects/{id}", s.handleDeleteProject)
	s.mux.HandleFunc("GET /api/gallery", s.handleGallery)
	s.mux.HandleFunc("POST /api/auth/password-reset-request", s.handlePasswordResetRequest)
	s.mux.HandleFunc("POST /api/auth/password-reset", s.handlePasswordReset)
	s.mux.HandleFunc("GET /api/projects/{id}/thumbnail", s.handleGetThumbnail)
	s.mux.HandleFunc("PUT /api/projects/{id}/thumbnail", s.handlePutThumbnail)
	s.mux.HandleFunc("POST /api/projects/{id}/publish", s.handlePublish)
	s.mux.HandleFunc("GET /api/v/{id}", s.handleViewSnapshot)
	s.mux.HandleFunc("POST /api/tickets", s.handleCreateTicket)
	return s
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	// M13: per-IP rate limiting (token bucket, 60 req/min).
	ip := r.RemoteAddr
	if fwd := r.Header.Get("X-Forwarded-For"); fwd != "" {
		ip = fwd
	}
	if !s.rateLimiter.Allow(ip) {
		http.Error(w, `{"error":"rate limit exceeded"}`, http.StatusTooManyRequests)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	s.mux.ServeHTTP(w, r)
}

func (s *Server) requireAuth(r *http.Request) (*Session, *User) {
	cookie, err := r.Cookie("session")
	if err != nil {
		return nil, nil
	}
	sess := s.store.GetSession(cookie.Value)
	if sess == nil {
		return nil, nil
	}
	user := s.store.GetUser(sess.UserID)
	return sess, user
}

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email       string `json:"email"`
		DisplayName string `json:"display_name"`
		Password    string `json:"password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	if body.Email == "" || body.Password == "" {
		http.Error(w, `{"error":"email and password required"}`, http.StatusBadRequest)
		return
	}
	user, err := s.store.CreateUser(body.Email, body.DisplayName, body.Password)
	if err != nil {
		if he, ok := err.(*httpError); ok {
			http.Error(w, `{"error":"`+he.Message+`"}`, he.Code)
			return
		}
		http.Error(w, `{"error":"internal"}`, http.StatusInternalServerError)
		return
	}
	sess := s.store.CreateSession(user.ID)
	http.SetCookie(w, &http.Cookie{
		Name:     "session",
		Value:    sess.Token,
		Path:     "/",
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
		Expires:  sess.Expires,
	})
	json.NewEncoder(w).Encode(user)
}

func (s *Server) handleLogin(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email    string `json:"email"`
		Password string `json:"password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	user, err := s.store.AuthenticateUser(body.Email, body.Password)
	if err != nil {
		http.Error(w, `{"error":"invalid credentials"}`, http.StatusUnauthorized)
		return
	}
	sess := s.store.CreateSession(user.ID)
	http.SetCookie(w, &http.Cookie{
		Name:     "session",
		Value:    sess.Token,
		Path:     "/",
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
		Expires:  sess.Expires,
	})
	json.NewEncoder(w).Encode(user)
}

func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	sess, _ := s.requireAuth(r)
	if sess != nil {
		s.store.DeleteSession(sess.Token)
	}
	http.SetCookie(w, &http.Cookie{
		Name:     "session",
		Value:    "",
		Path:     "/",
		HttpOnly: true,
		MaxAge:   -1,
	})
	w.Write([]byte(`{"ok":true}`))
}

func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	json.NewEncoder(w).Encode(user)
}

func (s *Server) handleListProjects(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	projects := s.store.ListProjects(user.ID)
	if projects == nil {
		projects = []*Project{}
	}
	json.NewEncoder(w).Encode(projects)
}

// M13: per-user quota — max projects.
const maxProjectsPerUser = 100

func (s *Server) handleCreateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	// M13: quota check.
	existing := s.store.ListProjects(user.ID)
	if len(existing) >= maxProjectsPerUser {
		http.Error(w, `{"error":"project quota exceeded (max 100)"}`, http.StatusForbidden)
		return
	}
	var body struct {
		Name     string          `json:"name"`
		Document json.RawMessage `json:"document"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	if body.Name == "" {
		body.Name = "Untitled"
	}
	p := s.store.CreateProject(user.ID, body.Name, body.Document)
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(p)
}

func (s *Server) handleGetProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	p := s.store.GetProject(id, user.ID)
	if p == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	json.NewEncoder(w).Encode(p)
}

// M13: max document size per project (50 MB).
const maxDocumentBytes = 50 * 1024 * 1024

func (s *Server) handleUpdateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	// M13: reject oversized payloads early.
	if r.ContentLength > maxDocumentBytes {
		http.Error(w, `{"error":"document too large (max 50 MB)"}`, http.StatusRequestEntityTooLarge)
		return
	}
	id := r.PathValue("id")
	var body struct {
		Name       string          `json:"name"`
		Document   json.RawMessage `json:"document"`
		Visibility string          `json:"visibility"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	p := s.store.UpdateProject(id, user.ID, body.Name, body.Document, body.Visibility)
	if p == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	json.NewEncoder(w).Encode(p)
}

func (s *Server) handleDeleteProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	if !s.store.DeleteProject(id, user.ID) {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	w.Write([]byte(`{"ok":true}`))
}

// ─── helpers ────────────────────────────────────────────────────

func (s *Server) handleGallery(w http.ResponseWriter, r *http.Request) {
	// Public gallery — no auth required.
	projects := s.store.ListPublicProjects()
	if projects == nil {
		projects = []*Project{}
	}
	json.NewEncoder(w).Encode(projects)
}

// ─── rate limiter ───────────────────────────────────────────────

// RateLimiter is a simple per-IP token bucket. Each IP gets
// `burst` tokens; one is consumed per request. Tokens refill
// at `rate` per second. When the bucket is empty, Allow returns
// false and the server responds 429.
type RateLimiter struct {
	mu      sync.Mutex
	buckets map[string]*bucket
	rate    float64
	burst   int
}

type bucket struct {
	tokens float64
	last   time.Time
}

func NewRateLimiter(ratePerSec float64, burst int) *RateLimiter {
	return &RateLimiter{
		buckets: make(map[string]*bucket),
		rate:    ratePerSec,
		burst:   burst,
	}
}

func (rl *RateLimiter) Allow(ip string) bool {
	rl.mu.Lock()
	defer rl.mu.Unlock()

	now := time.Now()
	b, ok := rl.buckets[ip]
	if !ok {
		b = &bucket{tokens: float64(rl.burst), last: now}
		rl.buckets[ip] = b
	}
	elapsed := now.Sub(b.last).Seconds()
	b.tokens += elapsed * rl.rate
	if b.tokens > float64(rl.burst) {
		b.tokens = float64(rl.burst)
	}
	b.last = now
	if b.tokens < 1 {
		return false
	}
	b.tokens--
	return true
}

func newID() string {
	b := make([]byte, 16)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// ─── tickets ────────────────────────────────────────────────────

// ticketRequest is the payload POSTed by the in-app "Report Issue" dialog.
// ticket xqll: no title field — all info lives in the description.
type ticketRequest struct {
	Description string             `json:"description"`
	ProjectJSON string             `json:"projectJson"`
	Screenshot  string             `json:"screenshot"` // PNG data URL, or empty
	Attachments []ticketAttachment `json:"attachments"`
	Version     string             `json:"version"`
	Timestamp   string             `json:"timestamp"`
	UserAgent   string             `json:"userAgent"`
	URL         string             `json:"url"`
}

// ticketAttachment is one user-attached file (picked or pasted, ticket 8onw),
// transported as a data URL inside the ticket JSON.
type ticketAttachment struct {
	Name    string `json:"name"`
	Mime    string `json:"mime"`
	DataURL string `json:"dataUrl"`
}

// attachmentFilename sanitizes a user-supplied attachment name into a safe
// on-disk filename: basename only, conservative character set, indexed so
// attachments never collide with each other or the ticket files.
func attachmentFilename(index int, name string) string {
	base := name
	if i := strings.LastIndexAny(base, "/\\"); i >= 0 {
		base = base[i+1:]
	}
	var b strings.Builder
	for _, r := range base {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9',
			r == '.', r == '-', r == '_', r == ' ':
			b.WriteRune(r)
		default:
			b.WriteRune('_')
		}
	}
	safe := b.String()
	if len(safe) > 80 {
		safe = safe[len(safe)-80:]
	}
	if safe == "" {
		safe = "file"
	}
	return fmt.Sprintf("attachment-%02d-%s", index+1, safe)
}

// ticketHeading derives the ticket.md `# ` heading from the description's
// first non-empty line (ticket xqll — there is no user title anymore). Falls
// back to a generic "Issue <id>" when the description is empty.
func ticketHeading(id, description string) string {
	for _, line := range strings.Split(description, "\n") {
		if t := strings.TrimSpace(line); t != "" {
			if len(t) > 80 {
				t = strings.TrimSpace(t[:80]) + "…"
			}
			return t
		}
	}
	return "Issue " + id
}

// ticketsDir is where filed tickets are written. Override with TICKETS_DIR;
// defaults to "tickets" relative to the server's working directory.
func ticketsDir() string {
	if d := os.Getenv("TICKETS_DIR"); d != "" {
		return d
	}
	return "tickets"
}

// handleCreateTicket persists a bug / feature-request ticket to
// <ticketsDir>/open/<id>/ — the same layout the dev Vite plugin writes, so a
// human or the agent processes both identically. Anonymous but rate-limited
// (60/min per IP via ServeHTTP).
func (s *Server) handleCreateTicket(w http.ResponseWriter, r *http.Request) {
	var req ticketRequest
	if err := json.NewDecoder(io.LimitReader(r.Body, 25<<20)).Decode(&req); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	if strings.TrimSpace(req.Description) == "" {
		http.Error(w, `{"error":"description required"}`, http.StatusBadRequest)
		return
	}

	id := time.Now().UTC().Format("20060102-150405") + "-" + newID()[:4]
	dir := filepath.Join(ticketsDir(), "open", id)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		http.Error(w, `{"error":"could not create ticket"}`, http.StatusInternalServerError)
		return
	}

	ts := req.Timestamp
	if ts == "" {
		ts = time.Now().UTC().Format(time.RFC3339)
	}

	// ticket xqll: ticket.json is gone — ticket.md (below) carries all the
	// metadata (ID/version/time/URL/UA) plus the description and attachments.

	// User attachments (ticket 8onw): each data URL becomes a file next to
	// the screenshot, listed in ticket.md for whoever picks the ticket up.
	var attachmentNames []string
	for i, a := range req.Attachments {
		ci := strings.IndexByte(a.DataURL, ',')
		if !strings.HasPrefix(a.DataURL, "data:") || ci < 0 {
			continue
		}
		blob, err := base64.StdEncoding.DecodeString(a.DataURL[ci+1:])
		if err != nil {
			continue
		}
		fname := attachmentFilename(i, a.Name)
		if os.WriteFile(filepath.Join(dir, fname), blob, 0o644) == nil {
			attachmentNames = append(attachmentNames, fname)
		}
	}

	md := fmt.Sprintf("# %s\n\n- **ID:** %s\n- **Version:** %s\n- **Time:** %s\n- **URL:** %s\n- **UA:** %s\n\n## Description\n\n%s\n",
		ticketHeading(id, req.Description), id, req.Version, ts, req.URL, req.UserAgent, req.Description)
	if len(attachmentNames) > 0 {
		md += "\n## Attachments\n\n"
		for _, n := range attachmentNames {
			md += "- " + n + "\n"
		}
	}
	os.WriteFile(filepath.Join(dir, "ticket.md"), []byte(md), 0o644)

	if req.ProjectJSON != "" {
		os.WriteFile(filepath.Join(dir, "project.fdraft"), []byte(req.ProjectJSON), 0o644)
	}

	if strings.HasPrefix(req.Screenshot, "data:image/") {
		if i := strings.IndexByte(req.Screenshot, ','); i >= 0 {
			if png, err := base64.StdEncoding.DecodeString(req.Screenshot[i+1:]); err == nil {
				os.WriteFile(filepath.Join(dir, "screenshot.png"), png, 0o644)
			}
		}
	}

	log.Printf("ticket saved: %s — %s", id, ticketHeading(id, req.Description))
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]string{"ticket_id": id})
}

// ─── M13: password reset ────────────────────────────────────────

func (s *Server) handlePasswordResetRequest(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email string `json:"email"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Email == "" {
		http.Error(w, `{"error":"email required"}`, http.StatusBadRequest)
		return
	}
	user := s.store.GetUserByEmail(body.Email)
	if user == nil {
		// Don't reveal whether the email exists — always return 200.
		json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
		return
	}
	token := s.store.CreateResetToken(user.ID)
	// In production, send via SMTP. For now, log the token.
	log.Printf("M13: password reset token for %s: %s", body.Email, token)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

func (s *Server) handlePasswordReset(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Token       string `json:"token"`
		NewPassword string `json:"new_password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	if body.Token == "" || body.NewPassword == "" {
		http.Error(w, `{"error":"token and new_password required"}`, http.StatusBadRequest)
		return
	}
	if len(body.NewPassword) < 8 {
		http.Error(w, `{"error":"password must be at least 8 characters"}`, http.StatusBadRequest)
		return
	}
	userID, ok := s.store.ConsumeResetToken(body.Token)
	if !ok {
		http.Error(w, `{"error":"invalid or expired token"}`, http.StatusBadRequest)
		return
	}
	s.store.UpdatePassword(userID, body.NewPassword)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

// ─── M13: project thumbnails ────────────────────────────────────

func (s *Server) handleGetThumbnail(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	data := s.store.GetThumbnail(id)
	if data == nil {
		http.Error(w, `{"error":"no thumbnail"}`, http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", "image/png")
	w.Write(data)
}

func (s *Server) handlePutThumbnail(w http.ResponseWriter, r *http.Request) {
	sess, _ := s.requireAuth(r)
	if sess == nil {
		http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	proj := s.store.GetProject(id, sess.UserID)
	if proj == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	// Max 2MB thumbnail
	r.Body = http.MaxBytesReader(w, r.Body, 2<<20)
	data, err := readAll(r.Body)
	if err != nil {
		http.Error(w, `{"error":"thumbnail too large (max 2MB)"}`, http.StatusBadRequest)
		return
	}
	s.store.SetThumbnail(id, data)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

// ─── P2.5: publish + viewer ─────────────────────────────────────

func (s *Server) handlePublish(w http.ResponseWriter, r *http.Request) {
	sess, _ := s.requireAuth(r)
	if sess == nil {
		http.Error(w, `{"error":"unauthorized"}`, http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	proj := s.store.GetProject(id, sess.UserID)
	if proj == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	snap := s.store.CreateSnapshot(proj.ID, proj.Document)
	json.NewEncoder(w).Encode(map[string]string{
		"snapshot_id": snap.ID,
		"view_url":    "/api/v/" + snap.ID,
	})
}

func (s *Server) handleViewSnapshot(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	snap := s.store.GetSnapshot(id)
	if snap == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	json.NewEncoder(w).Encode(snap)
}

func readAll(r io.Reader) ([]byte, error) {
	return io.ReadAll(r)
}

func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	var diff byte
	for i := range a {
		diff |= a[i] ^ b[i]
	}
	return diff == 0
}

// ─── main ───────────────────────────────────────────────────────

func main() {
	store := NewStore()
	srv := NewServer(store)

	addr := ":8080"
	log.Printf("fielddraft server listening on %s", addr)
	log.Fatal(http.ListenAndServe(addr, srv))
}
