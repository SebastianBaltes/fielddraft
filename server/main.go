// M11: FieldDraft backend — Auth, User, Project CRUD.
//
// Minimal Go server with chi router, Argon2id password hashing,
// session cookies, and a JSON project store. MariaDB comes in
// M11.1; this first slice runs with an in-memory store so the
// server is immediately testable without Docker.

package main

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"log"
	"net/http"
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

type Store struct {
	mu       sync.RWMutex
	users    map[string]*User    // keyed by ID
	emails   map[string]string   // email → user ID
	projects map[string]*Project // keyed by ID
	sessions map[string]*Session // token → session
}

func NewStore() *Store {
	return &Store{
		users:    make(map[string]*User),
		emails:   make(map[string]string),
		projects: make(map[string]*Project),
		sessions: make(map[string]*Session),
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

func (s *Server) handleCreateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
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

func (s *Server) handleUpdateProject(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
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
