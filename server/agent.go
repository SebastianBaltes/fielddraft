// Agent / work-order API (WP6, §8–§11).
//
// The machine-to-machine layer the two desktop ClaudeDraft agents drive. Both
// authenticate with a least-privilege Service-Token (Authorization: Bearer …),
// NOT a user session (§11):
//
//   - scope "po"     — fielddraft-po: read the queue/tickets/threads, post `po`
//     comments (leak-filtered), write verdicts, transition state.
//   - scope "coding" — fielddraft-coding: read its work queue, claim/complete
//     work-orders, post internal `dev` notes, transition state, write deploys.
//
// Tokens come from PO_SERVICE_TOKEN / CODING_SERVICE_TOKEN. An empty/unset token
// disables that scope (the agent API is off until tokens are configured), so a
// missing-secret deploy never silently grants access.

package main

import (
	"crypto/subtle"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"
)

// agentScope identifies which Service-Token authenticated a request.
type agentScope string

const (
	scopePO     agentScope = "po"
	scopeCoding agentScope = "coding"
)

// agentTokens holds the configured Service-Tokens. An empty token disables that
// scope entirely (constant-time compare never matches "").
type agentTokens struct {
	po     string
	coding string
}

// loadAgentTokens reads the Service-Tokens from the environment.
func loadAgentTokens() agentTokens {
	return agentTokens{
		po:     os.Getenv("PO_SERVICE_TOKEN"),
		coding: os.Getenv("CODING_SERVICE_TOKEN"),
	}
}

// bearerToken extracts the token from an "Authorization: Bearer <token>" header.
func bearerToken(r *http.Request) string {
	h := r.Header.Get("Authorization")
	const prefix = "Bearer "
	if len(h) <= len(prefix) || !strings.EqualFold(h[:len(prefix)], prefix) {
		return ""
	}
	return strings.TrimSpace(h[len(prefix):])
}

// constEq compares two strings in constant time. An empty configured token
// never matches (disabled scope).
func constEq(presented, configured string) bool {
	if configured == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(presented), []byte(configured)) == 1
}

// authResult is the outcome of requireAgent.
type authResult int

const (
	authOK         authResult = iota // token valid AND scope allowed
	authNoToken                      // missing/unknown token → 401
	authWrongScope                   // valid token, wrong scope → 403
)

// requireAgent authenticates the request's Bearer token and checks it carries
// one of the allowed scopes. It returns the resolved scope and a result code so
// the caller can answer 401 (no/unknown token) vs 403 (wrong scope).
func (s *Server) requireAgent(r *http.Request, allowed ...agentScope) (agentScope, authResult) {
	tok := bearerToken(r)
	if tok == "" {
		return "", authNoToken
	}
	var resolved agentScope
	switch {
	case constEq(tok, s.agentTokens.po):
		resolved = scopePO
	case constEq(tok, s.agentTokens.coding):
		resolved = scopeCoding
	default:
		return "", authNoToken
	}
	for _, a := range allowed {
		if a == resolved {
			return resolved, authOK
		}
	}
	return resolved, authWrongScope
}

// writeAgentAuth writes the appropriate error for a failed requireAgent, and
// reports whether the caller should stop (true = not authorized).
func writeAgentAuth(w http.ResponseWriter, res authResult) bool {
	switch res {
	case authNoToken:
		writeErr(w, http.StatusUnauthorized, "agent unauthorized")
		return true
	case authWrongScope:
		writeErr(w, http.StatusForbidden, "agent scope forbidden")
		return true
	default:
		return false
	}
}

// maxAgentBodyBytes caps an agent request body (verdicts/comments are small).
const maxAgentBodyBytes = 1 << 20

// ─── GET /api/agent/queue (po | coding, scope-aware) ────────────

// queueResponse is the agent queue: a scope-aware list of actionable tickets,
// each with its thread + artifact refs so the agent can act without extra round
// trips.
type queueResponse struct {
	Scope string             `json:"scope"`
	Items []agentQueueTicket `json:"items"`
}

// agentQueueTicket is one ticket in the queue, hydrated with thread + artifacts
// (and, for the coding scope, its work-order). UnreviewedComments surfaces the
// not-yet-PO-moderated user comments so the PO can judge them (B1).
type agentQueueTicket struct {
	*Ticket
	Comments           []*TicketComment  `json:"comments"`
	Artifacts          []*TicketArtifact `json:"artifacts"`
	WorkOrder          *WorkOrder        `json:"work_order,omitempty"`
	UnreviewedComments []*TicketComment  `json:"unreviewed_comments,omitempty"`
}

// queueLongPollMax caps how long a `?wait=` request may block. A blocked curl
// costs the agent NO tokens (tokens are per turn, not per wait-second), so a long
// hold lets a coarse wake schedule catch tickets near-instantly while staying
// idle-cheap. We cap it so a held connection can't live forever.
const queueLongPollMax = 300 * time.Second

// queueLongPollInterval is how often the handler re-checks the store while a
// `?wait=` request blocks. With only a handful of agents this internal poll is
// negligible; it never touches the model.
const queueLongPollInterval = 1 * time.Second

func (s *Server) handleAgentQueue(w http.ResponseWriter, r *http.Request) {
	scope, res := s.requireAgent(r, scopePO, scopeCoding)
	if writeAgentAuth(w, res) {
		return
	}

	items := s.queueItemsFor(scope)

	// Long-poll: if `?wait=<sec>` is set and the queue is currently empty, hold
	// the connection until an item appears, the wait elapses, or the client
	// disconnects — then return whatever the queue holds. This is the event-driven
	// read primitive: one held connection wakes the instant a ticket lands, so the
	// desktop agent needs only a coarse re-arm cadence (no busy polling, no idle
	// model turns while blocked).
	if wait := parseWaitSeconds(r); wait > 0 && len(items) == 0 {
		deadline := time.Now().Add(wait)
		ticker := time.NewTicker(queueLongPollInterval)
		defer ticker.Stop()
		for len(items) == 0 && time.Now().Before(deadline) {
			select {
			case <-r.Context().Done(): // client hung up
				return
			case <-ticker.C:
				items = s.queueItemsFor(scope)
			}
		}
	}

	json.NewEncoder(w).Encode(queueResponse{Scope: string(scope), Items: items})
}

// queueItemsFor returns the scope's actionable queue, never nil.
func (s *Server) queueItemsFor(scope agentScope) []agentQueueTicket {
	var items []agentQueueTicket
	switch scope {
	case scopePO:
		items = s.poQueue()
	case scopeCoding:
		items = s.codingQueue()
	}
	if items == nil {
		items = []agentQueueTicket{}
	}
	return items
}

// parseWaitSeconds reads the `?wait=<sec>` long-poll hint, clamped to
// [0, queueLongPollMax]. Absent/invalid/≤0 means "return immediately".
func parseWaitSeconds(r *http.Request) time.Duration {
	raw := r.URL.Query().Get("wait")
	if raw == "" {
		return 0
	}
	n, err := strconv.Atoi(raw)
	if err != nil || n <= 0 {
		return 0
	}
	d := time.Duration(n) * time.Second
	if d > queueLongPollMax {
		d = queueLongPollMax
	}
	return d
}

// poQueue: everything awaiting PO attention (§8.1) —
//   - tickets in `triage` (always the PO's job),
//   - tickets in `needs_info`/`declined` whose most recent comment is a user
//     reply (the dialogic states),
//   - ANY ticket (any state, even `resolved`) carrying an unreviewed user
//     comment, so post-moderation reaches spam on closed threads too (B1).
//
// Each ticket appears at most once; its unreviewed user comments are attached
// so the PO can moderate them directly.
func (s *Server) poQueue() []agentQueueTicket {
	var out []agentQueueTicket
	seen := map[string]bool{}
	add := func(t *Ticket) {
		if seen[t.ID] {
			return
		}
		seen[t.ID] = true
		item := s.hydrateQueueTicket(t, false)
		item.UnreviewedComments = unreviewedUserComments(item.Comments)
		out = append(out, item)
	}
	// triage — always the PO's job.
	for _, t := range s.tickets.ListTickets(TicketFilter{State: StateTriage, ViewerIsAdmin: true}) {
		add(t)
	}
	// needs_info with a new user comment since the last po/system one. (A rejected
	// ticket is NOT auto-revived on a comment — that would drag spam/malicious back
	// into the queue on any reply; objecting to a rejection is an explicit reopen
	// rejected→triage, ticket #… IssueVault 0.4.x collapse.)
	for _, st := range []TicketState{StateNeedsInfo} {
		for _, t := range s.tickets.ListTickets(TicketFilter{State: st, ViewerIsAdmin: true}) {
			full := s.tickets.GetTicket(t.ID)
			if full != nil && hasNewUserComment(full.Comments) {
				add(full)
			}
		}
	}
	// Any ticket (any state) with an unreviewed user comment — post-moderation.
	for _, t := range s.tickets.ListTicketsWithUnreviewedComments() {
		add(t)
	}
	return out
}

// unreviewedUserComments filters a thread to the user comments still awaiting
// PO post-moderation (B1).
func unreviewedUserComments(comments []*TicketComment) []*TicketComment {
	var out []*TicketComment
	for _, c := range comments {
		if c.AuthorType == AuthorUser && !c.POReviewed {
			out = append(out, c)
		}
	}
	return out
}

// codingQueue: accepted tickets with an open (unclaimed/incomplete) work-order.
func (s *Server) codingQueue() []agentQueueTicket {
	var out []agentQueueTicket
	for _, t := range s.tickets.ListAcceptedWithOpenWorkOrder() {
		out = append(out, s.hydrateQueueTicket(t, true))
	}
	return out
}

// hydrateQueueTicket builds a queue entry. If withWorkOrder, the ticket's
// work-order is attached (for the coding agent).
func (s *Server) hydrateQueueTicket(t *Ticket, withWorkOrder bool) agentQueueTicket {
	full := t
	if full.Comments == nil && full.Artifacts == nil {
		if hydrated := s.tickets.GetTicket(t.ID); hydrated != nil {
			full = hydrated
		}
	}
	comments := full.Comments
	if comments == nil {
		comments = []*TicketComment{}
	}
	artifacts := full.Artifacts
	if artifacts == nil {
		artifacts = []*TicketArtifact{}
	}
	item := agentQueueTicket{Ticket: full, Comments: comments, Artifacts: artifacts}
	if withWorkOrder {
		item.WorkOrder = s.tickets.GetWorkOrder(t.ID)
	}
	return item
}

// hasNewUserComment reports whether the latest non-system comment is a user
// reply — i.e. the PO owes the user an answer. System (audit) comments are
// ignored so a state-change entry doesn't mask a pending user reply.
func hasNewUserComment(comments []*TicketComment) bool {
	for i := len(comments) - 1; i >= 0; i-- {
		switch comments[i].AuthorType {
		case AuthorSystem:
			continue // audit noise — keep looking
		case AuthorUser:
			return true
		default: // po / dev — the PO already responded after the user
			return false
		}
	}
	return false
}

// ─── POST /api/tickets/{id}/verdict (po) ────────────────────────

// verdictRequest is the §8.3 PO verdict JSON.
type verdictRequest struct {
	Spam        bool            `json:"spam"`
	Malicious   bool            `json:"malicious"`
	VisionFit   string          `json:"vision_fit"`   // fit|partial|off|""
	Confidence  float64         `json:"confidence"`   // advisory, logged in reason context
	Category    string          `json:"category"`     // bug|feature|question|other|""
	Reason      string          `json:"reason"`       // internal
	PublicReply string          `json:"public_reply"` // user-facing, leak-filtered
	NextState   string          `json:"next_state"`
	WorkOrder   json.RawMessage `json:"work_order"` // required iff next_state==accepted
}

func (s *Server) handleVerdict(w http.ResponseWriter, r *http.Request) {
	if _, res := s.requireAgent(r, scopePO); writeAgentAuth(w, res) {
		return
	}
	id := r.PathValue("id")
	t := s.tickets.GetTicket(id)
	if t == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}

	var req verdictRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxAgentBodyBytes)).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}

	// ── validation ──
	next := TicketState(req.NextState)
	if !validState(next) {
		writeErr(w, http.StatusBadRequest, "unknown next_state")
		return
	}
	if err := validateTransition(t.State, next); err != nil {
		writeStoreError(w, err)
		return
	}
	if !validVisionFit(req.VisionFit) {
		writeErr(w, http.StatusBadRequest, "invalid vision_fit")
		return
	}
	if !validCategory(req.Category) {
		writeErr(w, http.StatusBadRequest, "invalid category")
		return
	}
	// work_order present iff accepting. "Accept" is now the move INTO in_progress
	// via this PO-verdict path (accepted folded into in_progress); a reopen to
	// in_progress goes through the separate SetTicketState path, which carries no
	// work-order. The WO's existence is what the coding queue keys on.
	hasWO := isJSONPresent(req.WorkOrder)
	if next == StateInProgress && !hasWO {
		writeErr(w, http.StatusBadRequest, "work_order required when accepting (next_state in_progress)")
		return
	}
	if next != StateInProgress && hasWO {
		writeErr(w, http.StatusBadRequest, "work_order only allowed when accepting (next_state in_progress)")
		return
	}

	// ── apply verdict (sets po_* fields + category + transitions state) ──
	verdict := POVerdict{
		Spam:      req.Spam,
		Malicious: req.Malicious,
		Reason:    req.Reason,
		State:     next,
	}
	if req.VisionFit != "" {
		vf := req.VisionFit
		verdict.VisionFit = &vf
	}
	// Category is part of the verdict (§8.3): classify before the state move so
	// the audit comment reflects a fully-classified ticket.
	if req.Category != "" {
		s.tickets.SetTicketCategory(id, req.Category)
	}
	if _, err := s.tickets.SetPOVerdict(id, verdict); err != nil {
		writeStoreError(w, err)
		return
	}

	// A malicious verdict notifies the admin (§4.4 admin-awareness).
	if req.Malicious {
		tid := id
		s.store.CreateAdminAlert(AlertMaliciousTicket, &tid,
			"ticket flagged malicious by PO verdict"+reasonSuffix(req.Reason))
	}

	// ── public reply, through the leak filter (§4.2) ──
	var replyComment *TicketComment
	if strings.TrimSpace(req.PublicReply) != "" {
		replyComment = s.postPOComment(id, req.PublicReply)
	}

	// ── on accept (→ in_progress): create the work-order from the payload ──
	var wo *WorkOrder
	if next == StateInProgress {
		created, err := s.tickets.CreateWorkOrder(id, req.WorkOrder)
		if err != nil {
			writeStoreError(w, err)
			return
		}
		wo = created
	}

	full := s.tickets.GetTicket(id)
	resp := map[string]any{
		"ticket":     full,
		"work_order": wo,
	}
	if replyComment != nil {
		resp["public_reply"] = replyComment
	}
	json.NewEncoder(w).Encode(resp)
}

// postPOComment writes a user-facing `po` comment through the leak filter
// (§4.2). If it trips, the comment is stored withheld=1 (invisible to users),
// logged, AND written to the admin alert feed for review (§4.4); a clean
// comment is visible.
func (s *Server) postPOComment(ticketID, body string) *TicketComment {
	clean, reasons := leakScan(body)
	c := s.tickets.AddAgentComment(ticketID, AuthorPO, body, false, !clean)
	if !clean {
		logLeakHold(ticketID, reasons)
		tid := ticketID
		s.store.CreateAdminAlert(AlertWithheldComment, &tid,
			"withheld outgoing po comment: "+strings.Join(reasons, ", "))
	}
	return c
}

// reasonSuffix appends a "(reason)" clause to an alert detail when a reason is
// given, else "".
func reasonSuffix(reason string) string {
	if strings.TrimSpace(reason) == "" {
		return ""
	}
	return " (" + reason + ")"
}

// ─── POST /api/tickets/{ticketId}/comments/{commentId}/verdict (po) ──

// commentVerdictRequest is the §8 per-comment PO moderation verdict (B1).
type commentVerdictRequest struct {
	Spam      bool   `json:"spam"`
	Malicious bool   `json:"malicious"`
	Reason    string `json:"reason"`
}

// handleCommentVerdict lets the PO post-moderate a single user comment (B1).
// Effect: the comment is always marked po_reviewed; spam OR malicious also
// withholds it (hidden from non-admins via visibleComments) and raises an admin
// alert (§4.4). The comment must belong to the named ticket.
func (s *Server) handleCommentVerdict(w http.ResponseWriter, r *http.Request) {
	if _, res := s.requireAgent(r, scopePO); writeAgentAuth(w, res) {
		return
	}
	ticketID := r.PathValue("ticketId")
	commentID := r.PathValue("commentId")
	// Validate the comment belongs to the ticket.
	c := s.tickets.GetComment(ticketID, commentID)
	if c == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}

	var req commentVerdictRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxAgentBodyBytes)).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}

	bad := req.Spam || req.Malicious
	updated := s.tickets.SetCommentReviewed(ticketID, commentID, bad)
	if updated == nil {
		writeErr(w, http.StatusInternalServerError, "could not moderate comment")
		return
	}
	if bad {
		tid := ticketID
		label := "comment withheld as spam"
		if req.Malicious {
			label = "comment withheld as malicious"
		}
		s.store.CreateAdminAlert(AlertMaliciousComment, &tid, label+reasonSuffix(req.Reason))
	}
	json.NewEncoder(w).Encode(updated)
}

// ─── POST /api/tickets/{id}/agent-comment (po | coding) ─────────

type agentCommentRequest struct {
	Body string `json:"body"`
	Kind string `json:"kind"` // "po" (public, leak-filtered) | "dev" (internal)
}

func (s *Server) handleAgentComment(w http.ResponseWriter, r *http.Request) {
	scope, res := s.requireAgent(r, scopePO, scopeCoding)
	if writeAgentAuth(w, res) {
		return
	}
	id := r.PathValue("id")
	if s.tickets.GetTicket(id) == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	var req agentCommentRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxAgentBodyBytes)).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	if strings.TrimSpace(req.Body) == "" {
		writeErr(w, http.StatusBadRequest, "comment body required")
		return
	}

	// Resolve the kind. Default per scope: po→po, coding→dev.
	kind := req.Kind
	if kind == "" {
		if scope == scopePO {
			kind = "po"
		} else {
			kind = "dev"
		}
	}

	var c *TicketComment
	switch kind {
	case "po":
		// Only the PO authors public comments (§4.1: single public author).
		if scope != scopePO {
			writeErr(w, http.StatusForbidden, "only the po scope may post po comments")
			return
		}
		c = s.postPOComment(id, req.Body)
	case "dev":
		// Internal dev note (admin-only). Both scopes may post one, but in
		// practice the coding agent does.
		c = s.tickets.AddAgentComment(id, AuthorDev, req.Body, true, false)
	default:
		writeErr(w, http.StatusBadRequest, "kind must be po or dev")
		return
	}
	if c == nil {
		writeErr(w, http.StatusInternalServerError, "could not add comment")
		return
	}
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(c)
}

// ─── PATCH /api/tickets/{id}/agent-state (po | coding) ──────────

func (s *Server) handleAgentState(w http.ResponseWriter, r *http.Request) {
	if _, res := s.requireAgent(r, scopePO, scopeCoding); writeAgentAuth(w, res) {
		return
	}
	id := r.PathValue("id")
	if s.tickets.GetTicket(id) == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	var body struct {
		State string `json:"state"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxAgentBodyBytes)).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	to := TicketState(body.State)
	if _, err := s.tickets.SetTicketState(id, to, false); err != nil {
		writeStoreError(w, err)
		return
	}
	json.NewEncoder(w).Encode(s.tickets.GetTicket(id))
}

// ─── POST /api/work-orders/{ticketId}/claim (coding) ────────────

func (s *Server) handleClaimWorkOrder(w http.ResponseWriter, r *http.Request) {
	if _, res := s.requireAgent(r, scopeCoding); writeAgentAuth(w, res) {
		return
	}
	ticketID := r.PathValue("ticketId")
	wo, err := s.tickets.ClaimWorkOrder(ticketID)
	if err != nil {
		writeStoreError(w, err)
		return
	}
	// Drive the ticket accepted→in_progress (§10.1). If it's already moved on,
	// keep the claim but don't fail.
	if _, terr := s.tickets.SetTicketState(ticketID, StateInProgress, false); terr != nil {
		logStateNote(ticketID, "claim", terr)
	}
	json.NewEncoder(w).Encode(map[string]any{
		"work_order": wo,
		"ticket":     s.tickets.GetTicket(ticketID),
	})
}

// ─── POST /api/work-orders/{ticketId}/complete (coding) ─────────

type completeRequest struct {
	DeployID  string `json:"deploy_id"`  // optional link to the resolving deploy
	NextState string `json:"next_state"` // optional override; default resolved
}

func (s *Server) handleCompleteWorkOrder(w http.ResponseWriter, r *http.Request) {
	if _, res := s.requireAgent(r, scopeCoding); writeAgentAuth(w, res) {
		return
	}
	ticketID := r.PathValue("ticketId")
	var req completeRequest
	// Body is optional.
	json.NewDecoder(http.MaxBytesReader(w, r.Body, maxAgentBodyBytes)).Decode(&req)

	// Validate the ticket transition BEFORE completing the work-order: a bad
	// next_state must not leave the order completed while the ticket move
	// fails (the old order mutated first and errored after).
	t := s.tickets.GetTicket(ticketID)
	if t == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	// Default target is `closed` (resolved folded into closed on the 0.4.x
	// collapse); the body may pick another legal state (e.g. needs_info if the dev
	// needs clarification, or parked).
	target := StateClosed
	if req.NextState != "" {
		target = TicketState(req.NextState)
	}
	if err := validateTransition(t.State, target); err != nil {
		writeStoreError(w, err)
		return
	}

	// Completing twice is a 409 (errWorkOrderCompleted) — never a silent
	// overwrite of completed_at / the deploy link.
	wo, err := s.tickets.CompleteWorkOrder(ticketID, req.DeployID)
	if err != nil {
		writeStoreError(w, err)
		return
	}
	// The move was validated above; if a concurrent actor won the race in
	// between, keep the completion and just log — consistent with claim.
	if _, terr := s.tickets.SetTicketState(ticketID, target, false); terr != nil {
		logStateNote(ticketID, "complete", terr)
	}
	json.NewEncoder(w).Encode(map[string]any{
		"work_order": wo,
		"ticket":     s.tickets.GetTicket(ticketID),
	})
}

// ─── deploys (coding write, admin|coding read) ──────────────────

type deployRequest struct {
	Version   string   `json:"version"`
	GitSHA    string   `json:"git_sha"`
	TicketIDs []string `json:"ticket_ids"`
	Status    string   `json:"status"`
}

func (s *Server) handleCreateDeploy(w http.ResponseWriter, r *http.Request) {
	if _, res := s.requireAgent(r, scopeCoding); writeAgentAuth(w, res) {
		return
	}
	var req deployRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxAgentBodyBytes)).Decode(&req); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	status := req.Status
	if status == "" {
		status = string(DeployBuilding)
	}
	if !validDeployStatus(status) {
		writeErr(w, http.StatusBadRequest, "invalid deploy status")
		return
	}
	d, err := s.tickets.CreateDeploy(req.Version, req.GitSHA, req.TicketIDs, DeployStatus(status))
	if err != nil {
		writeStoreError(w, err)
		return
	}
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(d)
}

// handleListDeploys serves the deploy audit list to either an admin user
// session OR the coding agent token (for its own rollback decisions).
func (s *Server) handleListDeploys(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil || user.Role != RoleAdmin {
		// Not an admin session — allow the coding agent token instead.
		if _, res := s.requireAgent(r, scopeCoding); res != authOK {
			if user != nil {
				// Authenticated but insufficient rights.
				writeErr(w, http.StatusForbidden, "admin or coding agent only")
			} else {
				// No session: report the agent-auth failure — 401 for a
				// missing/unknown token, 403 for a wrong-scope token.
				writeAgentAuth(w, res)
			}
			return
		}
	}
	deploys := s.tickets.ListDeploys()
	if deploys == nil {
		deploys = []*Deploy{}
	}
	json.NewEncoder(w).Encode(deploys)
}

// ─── logging hooks (admin-awareness, §4.4) ──────────────────────

// logLeakHold records a withheld outgoing comment for admin review (§4.4
// admin-awareness: a held comment is something the admin should see).
func logLeakHold(ticketID string, reasons []string) {
	log.Printf("leak-filter: HELD po comment on ticket %s — %s", ticketID, strings.Join(reasons, ", "))
}

// logStateNote records a non-fatal state-transition note (e.g. a claim where
// the ticket has already moved past accepted).
func logStateNote(ticketID, op string, err error) {
	log.Printf("agent: %s on ticket %s skipped state move: %v", op, ticketID, err)
}
