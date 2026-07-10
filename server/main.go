// FieldDraft backend — Auth, Invites, Roles, Projects, Gallery, Tickets.
//
// HTTP layer: chi-free net/http with method-routed ServeMux, Argon2id
// password hashing, DB-backed sessions, per-IP rate limiting. Persistence is
// behind the Store interface (store.go): memStore for tests, sqlStore (MariaDB
// + content-addressed blobs) in production. main() wires up sqlStore from
// DB_DSN and seeds an admin from ADMIN_EMAIL/ADMIN_PASSWORD.

package main

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
)

// ─── server ─────────────────────────────────────────────────────

type Server struct {
	store       Store
	tickets     TicketStore // pluggable ticket domain (WP15); see selectTicketStore
	mux         *http.ServeMux
	rateLimiter *RateLimiter
	agentTokens agentTokens // least-privilege Service-Tokens (§11)
	// trustProxy (TRUST_PROXY=1) enables reading the client IP from
	// X-Forwarded-For. Only correct behind a trusted reverse proxy (Caddy in
	// prod) — an untrusted client can spoof the header otherwise.
	trustProxy bool
	// mesh is the running mesh-job dispatcher (mesher.go); nil = feature off
	// (MESHER_URL unset), the mesh routes answer 503.
	mesh *meshDispatch
}

func NewServer(store Store) *Server {
	s := &Server{
		store:       store,
		tickets:     selectTicketStore(store),
		mux:         http.NewServeMux(),
		rateLimiter: NewRateLimiter(1.0, 60), // 60 burst, 1/sec refill
		agentTokens: loadAgentTokens(),
		trustProxy:  os.Getenv("TRUST_PROXY") == "1",
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
	// Content-addressed asset blobs (client cloud-save asset sync, §5.5).
	s.mux.HandleFunc("POST /api/blobs/missing", s.handleBlobsMissing)
	s.mux.HandleFunc("PUT /api/blobs/{sha256}", s.handlePutBlob)
	s.mux.HandleFunc("GET /api/blobs/{sha256}", s.handleGetBlob)
	s.mux.HandleFunc("POST /api/projects/{id}/mesh", s.handleCreateMeshJob)
	s.mux.HandleFunc("GET /api/mesh-jobs/{id}", s.handleGetMeshJob)
	s.mux.HandleFunc("GET /api/mesh-jobs/{id}/artifact", s.handleGetMeshArtifact)
	s.mux.HandleFunc("DELETE /api/mesh-jobs/{id}", s.handleCancelMeshJob)
	s.mux.HandleFunc("POST /api/auth/password-reset-request", s.handlePasswordResetRequest)
	s.mux.HandleFunc("POST /api/auth/password-reset", s.handlePasswordReset)
	s.mux.HandleFunc("GET /api/projects/{id}/thumbnail", s.handleGetThumbnail)
	s.mux.HandleFunc("PUT /api/projects/{id}/thumbnail", s.handlePutThumbnail)
	s.mux.HandleFunc("POST /api/projects/{id}/publish", s.handlePublish)
	s.mux.HandleFunc("GET /api/v/{id}", s.handleViewSnapshot)
	// Public invite lookup (no auth): the self-registration form reads the
	// invitee's name/email to prefill the register screen from an invite link.
	s.mux.HandleFunc("GET /api/invites/{token}/info", s.handleInviteInfo)
	// Admin invite management.
	s.mux.HandleFunc("POST /api/invites", s.handleCreateInvite)
	s.mux.HandleFunc("GET /api/invites", s.handleListInvites)
	s.mux.HandleFunc("DELETE /api/invites/{token}", s.handleDeleteInvite)
	// Admin moderation: alert feed + user ban (WP8, §4.4).
	s.mux.HandleFunc("GET /api/admin/alerts", s.handleListAdminAlerts)
	s.mux.HandleFunc("POST /api/admin/alerts/{id}/seen", s.handleMarkAdminAlertSeen)
	s.mux.HandleFunc("GET /api/admin/users", s.handleListUsers)
	s.mux.HandleFunc("POST /api/admin/users/{id}/ban", s.handleBanUser)
	s.mux.HandleFunc("POST /api/admin/users/{id}/unban", s.handleUnbanUser)
	// Tickets (DB-backed, authenticated; §7).
	s.mux.HandleFunc("POST /api/tickets", s.handleCreateTicket)
	s.mux.HandleFunc("GET /api/tickets", s.handleListTickets)
	s.mux.HandleFunc("GET /api/tickets/{id}", s.handleGetTicket)
	s.mux.HandleFunc("PATCH /api/tickets/{id}", s.handlePatchTicket)
	s.mux.HandleFunc("DELETE /api/tickets/{id}", s.handleDeleteTicket)
	s.mux.HandleFunc("GET /api/tickets/{id}/comments", s.handleListComments)
	s.mux.HandleFunc("POST /api/tickets/{id}/comments", s.handleAddComment)
	s.mux.HandleFunc("GET /api/tickets/{id}/artifacts/{artifactId}", s.handleGetArtifact)
	// PO per-comment moderation (Service-Token, po scope; WP8 §4.2/§8).
	s.mux.HandleFunc("POST /api/tickets/{ticketId}/comments/{commentId}/verdict", s.handleCommentVerdict)
	// Agent / work-order API (Service-Token auth; WP6, §8–§11).
	s.mux.HandleFunc("GET /api/agent/queue", s.handleAgentQueue)
	s.mux.HandleFunc("POST /api/tickets/{id}/verdict", s.handleVerdict)
	s.mux.HandleFunc("POST /api/tickets/{id}/agent-comment", s.handleAgentComment)
	s.mux.HandleFunc("PATCH /api/tickets/{id}/agent-state", s.handleAgentState)
	s.mux.HandleFunc("POST /api/work-orders/{ticketId}/claim", s.handleClaimWorkOrder)
	s.mux.HandleFunc("POST /api/work-orders/{ticketId}/complete", s.handleCompleteWorkOrder)
	s.mux.HandleFunc("POST /api/deploys", s.handleCreateDeploy)
	s.mux.HandleFunc("GET /api/deploys", s.handleListDeploys)
	return s
}

// selectTicketStore picks the ticket-domain backend from the environment (WP15):
//
//   - TICKET_BACKEND unset or "db" (the default, and the only prod path) → the
//     existing DB/mem store's own ticket methods. Byte-identical to before the
//     TicketStore split, so PROD IS UNAFFECTED.
//   - TICKET_BACKEND=files → a file-backed store under TICKET_DIR (default
//     ./tickets), for local dev. The materialized layout doubles as the on-disk
//     tickets the desktop agents harvest (docs/TICKET-SYNC-AND-AGENTS.md §3.1).
//
// The same ticket CRUD/UI/API/agent-harvest runs unchanged over whichever
// backend is installed.
func selectTicketStore(store Store) TicketStore {
	switch os.Getenv("TICKET_BACKEND") {
	case "files":
		dir := os.Getenv("TICKET_DIR")
		if dir == "" {
			dir = "./tickets"
		}
		fs, err := newFileTicketStore(dir)
		if err != nil {
			log.Fatalf("ticket store (files) init failed: %v", err)
		}
		log.Printf("ticket store: file backend at %s", dir)
		return fs
	case "issuevault":
		// External IssueVault service (store_issuevault.go) instead of MariaDB.
		return newIssuevaultStore()
	default:
		// db (default): the DB/mem store's own ticket methods. Both memStore and
		// sqlStore implement every TicketStore method (see the compile-time
		// assertions in ticketstore.go), so this assertion always holds.
		ts, ok := store.(TicketStore)
		if !ok {
			log.Fatalf("ticket store (db): %T does not implement TicketStore", store)
		}
		return ts
	}
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	// Per-IP rate limiting (token bucket, 60 req/min).
	if !s.rateLimiter.Allow(s.clientIP(r)) {
		writeErr(w, http.StatusTooManyRequests, "rate limit exceeded")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	s.mux.ServeHTTP(w, r)
}

// clientIP resolves the caller's IP for rate limiting. X-Forwarded-For is
// attacker-controlled unless a trusted reverse proxy sets it, so it is only
// honored with TRUST_PROXY=1 — and then only its FIRST element (Caddy ≥2.5
// strips client-supplied XFF from untrusted peers, so the first element is
// the real client). Without a proxy the TCP peer address (host only, no
// port — one bucket per client, not per connection) is used.
func (s *Server) clientIP(r *http.Request) string {
	if s.trustProxy {
		if fwd := r.Header.Get("X-Forwarded-For"); fwd != "" {
			first := fwd
			if i := strings.IndexByte(fwd, ','); i >= 0 {
				first = fwd[:i]
			}
			if ip := strings.TrimSpace(first); ip != "" {
				return ip
			}
		}
	}
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

// ─── auth handlers ──────────────────────────────────────────────

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email       string `json:"email"`
		DisplayName string `json:"display_name"`
		Password    string `json:"password"`
		Invite      string `json:"invite"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	if body.Email == "" || body.Password == "" {
		writeErr(w, http.StatusBadRequest, "email and password required")
		return
	}

	// Invite-gated registration: token from body or ?invite= query.
	token := body.Invite
	if token == "" {
		token = r.URL.Query().Get("invite")
	}
	if token == "" {
		writeErr(w, http.StatusForbidden, "invite required")
		return
	}
	// Validate + burn the invite and create the user as ONE atomic store
	// operation — two racing registrations with the same token can never both
	// create a user, and a duplicate email never burns the token.
	user, err := s.store.RegisterWithInvite(body.Email, body.DisplayName, body.Password, token)
	if err != nil {
		writeStoreError(w, err)
		return
	}

	sess := s.store.CreateSession(user.ID)
	setSessionCookie(w, sess)
	json.NewEncoder(w).Encode(user)
}

func (s *Server) handleLogin(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email    string `json:"email"`
		Password string `json:"password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	user, err := s.store.AuthenticateUser(body.Email, body.Password)
	if err != nil {
		writeErr(w, http.StatusUnauthorized, "invalid credentials")
		return
	}
	sess := s.store.CreateSession(user.ID)
	setSessionCookie(w, sess)
	json.NewEncoder(w).Encode(user)
}

func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	sess, _ := s.requireAuth(r)
	if sess != nil {
		s.store.DeleteSession(sess.Token)
	}
	clearSessionCookie(w)
	w.Write([]byte(`{"ok":true}`))
}

func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	json.NewEncoder(w).Encode(user)
}

// ─── project handlers ───────────────────────────────────────────

func (s *Server) handleListProjects(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	projects := s.store.ListProjects(user.ID)
	if projects == nil {
		projects = []*Project{}
	}
	json.NewEncoder(w).Encode(projects)
}

// Per-user quota — max projects.
const maxProjectsPerUser = 100

func (s *Server) handleCreateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	existing := s.store.ListProjects(user.ID)
	if len(existing) >= maxProjectsPerUser {
		writeErr(w, http.StatusForbidden, "project quota exceeded (max 100)")
		return
	}
	var body struct {
		Name     string          `json:"name"`
		Document json.RawMessage `json:"document"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	if body.Name == "" {
		body.Name = "Untitled"
	}
	p := s.store.CreateProject(user.ID, body.Name, body.Document)
	if p == nil {
		writeErr(w, http.StatusInternalServerError, "could not create project")
		return
	}
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(p)
}

func (s *Server) handleGetProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	id := r.PathValue("id")
	p := s.store.GetProject(id, user.ID)
	if p == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	json.NewEncoder(w).Encode(p)
}

// Max document size per project (50 MB).
const maxDocumentBytes = 50 * 1024 * 1024

func (s *Server) handleUpdateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	if r.ContentLength > maxDocumentBytes {
		writeErr(w, http.StatusRequestEntityTooLarge, "document too large (max 50 MB)")
		return
	}
	id := r.PathValue("id")
	var body struct {
		Name       string          `json:"name"`
		Document   json.RawMessage `json:"document"`
		Visibility string          `json:"visibility"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	p := s.store.UpdateProject(id, user.ID, body.Name, body.Document, body.Visibility)
	if p == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	json.NewEncoder(w).Encode(p)
}

func (s *Server) handleDeleteProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	id := r.PathValue("id")
	if !s.store.DeleteProject(id, user.ID) {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	w.Write([]byte(`{"ok":true}`))
}

func (s *Server) handleGallery(w http.ResponseWriter, r *http.Request) {
	// Public gallery — no auth required.
	projects := s.store.ListPublicProjects()
	if projects == nil {
		projects = []*Project{}
	}
	json.NewEncoder(w).Encode(projects)
}

// ─── rate limiter ───────────────────────────────────────────────

// RateLimiter is a simple per-IP token bucket. Each IP gets `burst` tokens;
// one is consumed per request. Tokens refill at `rate` per second. When the
// bucket is empty, Allow returns false and the server responds 429. Buckets
// idle for bucketIdleTTL are evicted on access, so the map cannot grow without
// bound across many client IPs.
type RateLimiter struct {
	mu        sync.Mutex
	buckets   map[string]*bucket
	rate      float64
	burst     int
	lastSweep time.Time
}

type bucket struct {
	tokens float64
	last   time.Time
}

// bucketIdleTTL is how long a bucket may sit untouched before eviction. Any
// realistic refill completes well within this window, so eviction (= a fresh
// full bucket on the next request) never grants extra tokens.
const bucketIdleTTL = 10 * time.Minute

// bucketSweepEvery caps how often the eviction sweep runs (it is O(#buckets)).
const bucketSweepEvery = time.Minute

func NewRateLimiter(ratePerSec float64, burst int) *RateLimiter {
	return &RateLimiter{
		buckets: make(map[string]*bucket),
		rate:    ratePerSec,
		burst:   burst,
	}
}

func (rl *RateLimiter) Allow(ip string) bool {
	return rl.allowAt(ip, time.Now())
}

// allowAt is Allow with an injectable clock (tests).
func (rl *RateLimiter) allowAt(ip string, now time.Time) bool {
	rl.mu.Lock()
	defer rl.mu.Unlock()

	// Opportunistic eviction of idle buckets, at most once per sweep interval.
	if now.Sub(rl.lastSweep) >= bucketSweepEvery {
		rl.lastSweep = now
		for k, b := range rl.buckets {
			if now.Sub(b.last) >= bucketIdleTTL {
				delete(rl.buckets, k)
			}
		}
	}

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

// ─── shared helpers ─────────────────────────────────────────────

func newID() string {
	b := make([]byte, 16)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// writeErr writes the canonical JSON error body {"error": msg} with the given
// status. ALL handler error responses go through here: the message is
// json.Marshal-escaped (a `"` in a store message can no longer break out of
// the JSON string), and the Content-Type stays application/json (http.Error
// forced text/plain).
func writeErr(w http.ResponseWriter, code int, msg string) {
	body, err := json.Marshal(map[string]string{"error": msg})
	if err != nil {
		body = []byte(`{"error":"internal"}`)
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	w.Write(body)
}

// writeStoreError maps a store *httpError to an HTTP response.
func writeStoreError(w http.ResponseWriter, err error) {
	if he, ok := err.(*httpError); ok {
		writeErr(w, he.Code, he.Message)
		return
	}
	writeErr(w, http.StatusInternalServerError, "internal")
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

// ─── password reset ─────────────────────────────────────────────

func (s *Server) handlePasswordResetRequest(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email string `json:"email"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Email == "" {
		writeErr(w, http.StatusBadRequest, "email required")
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
	log.Printf("password reset token for %s: %s", body.Email, token)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

func (s *Server) handlePasswordReset(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Token       string `json:"token"`
		NewPassword string `json:"new_password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	if body.Token == "" || body.NewPassword == "" {
		writeErr(w, http.StatusBadRequest, "token and new_password required")
		return
	}
	if len(body.NewPassword) < 8 {
		writeErr(w, http.StatusBadRequest, "password must be at least 8 characters")
		return
	}
	userID, ok := s.store.ConsumeResetToken(body.Token)
	if !ok {
		writeErr(w, http.StatusBadRequest, "invalid or expired token")
		return
	}
	s.store.UpdatePassword(userID, body.NewPassword)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

// ─── project thumbnails ─────────────────────────────────────────

func (s *Server) handleGetThumbnail(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	data := s.store.GetThumbnail(id)
	if data == nil {
		writeErr(w, http.StatusNotFound, "no thumbnail")
		return
	}
	w.Header().Set("Content-Type", "image/png")
	w.Write(data)
}

func (s *Server) handlePutThumbnail(w http.ResponseWriter, r *http.Request) {
	sess, _ := s.requireAuth(r)
	if sess == nil {
		writeErr(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	id := r.PathValue("id")
	proj := s.store.GetProject(id, sess.UserID)
	if proj == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	// Max 2MB thumbnail.
	r.Body = http.MaxBytesReader(w, r.Body, 2<<20)
	data, err := readAll(r.Body)
	if err != nil {
		writeErr(w, http.StatusBadRequest, "thumbnail too large (max 2MB)")
		return
	}
	s.store.SetThumbnail(id, data)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

// ─── publish + viewer ───────────────────────────────────────────

func (s *Server) handlePublish(w http.ResponseWriter, r *http.Request) {
	sess, _ := s.requireAuth(r)
	if sess == nil {
		writeErr(w, http.StatusUnauthorized, "unauthorized")
		return
	}
	id := r.PathValue("id")
	proj := s.store.GetProject(id, sess.UserID)
	if proj == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	// docs/PROJECT-ASSET-STORAGE-PLAN §6 (P2 regression): a server-bound
	// project stores a LEAN document — meshes/fonts are content-addressed
	// blobs behind the owner-auth-gated /api/blobs. The public snapshot
	// viewer is UNAUTHENTICATED and cannot fetch those blobs, so an
	// asset-bearing published project would not load. The client therefore
	// POSTs the SELF-CONTAINED full document (serializeProject, assets
	// inline) as `{document}`; we snapshot that instead of the stored lean
	// doc. An absent/empty body (legacy client, asset-free project) falls
	// back to proj.Document, which is already self-contained.
	doc := proj.Document
	var body struct {
		Document json.RawMessage `json:"document"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		if err != io.EOF {
			writeErr(w, http.StatusBadRequest, "invalid JSON")
			return
		}
		// Empty body — keep the stored document (backward-compat).
	} else if len(body.Document) > 0 && string(body.Document) != "null" {
		doc = body.Document
	}
	snap := s.store.CreateSnapshot(proj.ID, doc)
	if snap == nil {
		writeErr(w, http.StatusInternalServerError, "could not publish")
		return
	}
	json.NewEncoder(w).Encode(map[string]string{
		"snapshot_id": snap.ID,
		"view_url":    "/api/v/" + snap.ID,
	})
}

func (s *Server) handleViewSnapshot(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	snap := s.store.GetSnapshot(id)
	if snap == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	json.NewEncoder(w).Encode(snap)
}

// ─── tickets (DB-backed; handlers live in tickets_handlers.go) ──

// ticketAttachment is one user-attached file, transported as a data URL.
type ticketAttachment struct {
	Name    string `json:"name"`
	Mime    string `json:"mime"`
	DataURL string `json:"dataUrl"`
}

// attachmentFilename sanitizes a user-supplied attachment name into a safe
// on-disk filename: basename only, conservative character set, indexed.
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

// ticketHeading derives the ticket.md heading from the description's first
// non-empty line. Falls back to a generic "Issue <id>". Truncation is
// RUNE-based — a byte slice (`t[:80]`) cuts multi-byte characters in half
// and the resulting invalid UTF-8 makes MariaDB reject the INSERT with
// Error 1366 ("Report an issue" then fails with a 500).
func ticketHeading(id, description string) string {
	for _, line := range strings.Split(description, "\n") {
		if t := strings.TrimSpace(line); t != "" {
			if runes := []rune(t); len(runes) > 80 {
				t = strings.TrimSpace(string(runes[:80])) + "…"
			}
			return t
		}
	}
	return "Issue " + id
}

// ─── main ───────────────────────────────────────────────────────

func main() {
	if os.Getenv("SECURE_COOKIES") == "1" {
		cookieSecure = true
	}

	dsn := os.Getenv("DB_DSN")
	if dsn == "" {
		log.Fatal("DB_DSN is required (e.g. user:pass@tcp(host:3306)/db?parseTime=true)")
	}
	store, err := NewSQLStore(dsn)
	if err != nil {
		log.Fatalf("database init failed: %v", err)
	}
	defer store.Close()

	seedAdmin(store)

	srv := NewServer(store)

	meshWorkers := 2
	if v := os.Getenv("MESH_WORKERS"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			meshWorkers = n
		}
	}
	srv.startMeshDispatcher(os.Getenv("MESHER_URL"), meshWorkers)

	addr := ":8080"
	if p := os.Getenv("PORT"); p != "" {
		addr = ":" + p
	}
	log.Printf("fielddraft server listening on %s", addr)
	log.Fatal(http.ListenAndServe(addr, srv))
}

// seedAdmin creates the operator admin from ADMIN_EMAIL/ADMIN_PASSWORD on
// first boot, if no admin exists yet.
func seedAdmin(store Store) {
	email := os.Getenv("ADMIN_EMAIL")
	password := os.Getenv("ADMIN_PASSWORD")
	if email == "" || password == "" {
		return
	}
	if store.HasAdmin() {
		log.Printf("seed admin: an admin already exists, skipping")
		return
	}
	if existing := store.GetUserByEmail(email); existing != nil {
		log.Printf("seed admin: user %s already exists but is not admin; not modifying", email)
		return
	}
	u, err := store.CreateUser(email, "Admin", password, RoleAdmin)
	if err != nil {
		log.Printf("seed admin: could not create admin %s: %v", email, err)
		return
	}
	log.Printf("seed admin: created admin user %s (id=%s)", u.Email, u.ID)
}
