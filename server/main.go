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
	"net/http"
	"os"
	"strings"
	"sync"
	"time"
)

// ─── server ─────────────────────────────────────────────────────

type Server struct {
	store       Store
	mux         *http.ServeMux
	rateLimiter *RateLimiter
	agentTokens agentTokens // least-privilege Service-Tokens (§11)
}

func NewServer(store Store) *Server {
	s := &Server{
		store:       store,
		mux:         http.NewServeMux(),
		rateLimiter: NewRateLimiter(1.0, 60), // 60 burst, 1/sec refill
		agentTokens: loadAgentTokens(),
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

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	// Per-IP rate limiting (token bucket, 60 req/min).
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

// ─── auth handlers ──────────────────────────────────────────────

func (s *Server) handleRegister(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Email       string `json:"email"`
		DisplayName string `json:"display_name"`
		Password    string `json:"password"`
		Invite      string `json:"invite"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	if body.Email == "" || body.Password == "" {
		http.Error(w, `{"error":"email and password required"}`, http.StatusBadRequest)
		return
	}

	// Invite-gated registration: token from body or ?invite= query.
	token := body.Invite
	if token == "" {
		token = r.URL.Query().Get("invite")
	}
	if token == "" {
		http.Error(w, `{"error":"invite required"}`, http.StatusForbidden)
		return
	}
	// Validate (exists, unused, not expired) before creating the user so a
	// duplicate-email failure doesn't burn the token.
	inv := s.store.GetInvite(token)
	if inv == nil || time.Now().UTC().After(inv.ExpiresAt) {
		writeStoreError(w, errInviteInvalid)
		return
	}
	if inv.UsedBy != nil {
		writeStoreError(w, errInviteUsed)
		return
	}

	user, err := s.store.CreateUser(body.Email, body.DisplayName, body.Password, RoleUser)
	if err != nil {
		writeStoreError(w, err)
		return
	}

	// Burn the invite atomically with the real user id. The single-use guard
	// lives in ConsumeInvite; on a lost race we delete the just-created user.
	if _, err := s.store.ConsumeInvite(token, user.ID); err != nil {
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
		http.Error(w, `{"error":"invalid JSON"}`, http.StatusBadRequest)
		return
	}
	user, err := s.store.AuthenticateUser(body.Email, body.Password)
	if err != nil {
		http.Error(w, `{"error":"invalid credentials"}`, http.StatusUnauthorized)
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
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
	json.NewEncoder(w).Encode(user)
}

// ─── project handlers ───────────────────────────────────────────

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

// Per-user quota — max projects.
const maxProjectsPerUser = 100

func (s *Server) handleCreateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
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
	if p == nil {
		http.Error(w, `{"error":"could not create project"}`, http.StatusInternalServerError)
		return
	}
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

// Max document size per project (50 MB).
const maxDocumentBytes = 50 * 1024 * 1024

func (s *Server) handleUpdateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return
	}
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
// bucket is empty, Allow returns false and the server responds 429.
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

// ─── shared helpers ─────────────────────────────────────────────

func newID() string {
	b := make([]byte, 16)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// writeStoreError maps a store *httpError to an HTTP response.
func writeStoreError(w http.ResponseWriter, err error) {
	if he, ok := err.(*httpError); ok {
		http.Error(w, `{"error":"`+he.Message+`"}`, he.Code)
		return
	}
	http.Error(w, `{"error":"internal"}`, http.StatusInternalServerError)
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
	log.Printf("password reset token for %s: %s", body.Email, token)
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

// ─── project thumbnails ─────────────────────────────────────────

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
	// Max 2MB thumbnail.
	r.Body = http.MaxBytesReader(w, r.Body, 2<<20)
	data, err := readAll(r.Body)
	if err != nil {
		http.Error(w, `{"error":"thumbnail too large (max 2MB)"}`, http.StatusBadRequest)
		return
	}
	s.store.SetThumbnail(id, data)
	json.NewEncoder(w).Encode(map[string]string{"status": "ok"})
}

// ─── publish + viewer ───────────────────────────────────────────

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
	if snap == nil {
		http.Error(w, `{"error":"could not publish"}`, http.StatusInternalServerError)
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
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
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
// non-empty line. Falls back to a generic "Issue <id>".
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

	addr := ":8080"
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
