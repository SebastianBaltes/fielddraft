// memStore — in-memory map implementation of Store.
//
// Kept ONLY as a fast, hermetic test fixture (the handler tests in
// main_test.go run against it with no live database). NewStore() returns it.

package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"sync"
	"time"
)

type memStore struct {
	mu          sync.RWMutex
	users       map[string]*User       // keyed by ID
	emails      map[string]string      // email → user ID
	projects    map[string]*Project    // keyed by ID
	sessions    map[string]*Session    // token → session
	resetTokens map[string]*ResetToken // token → reset
	thumbnails  map[string][]byte      // project ID → PNG data
	snapshots   map[string]*Snapshot   // snapshot ID → snapshot
	invites     map[string]*Invite     // token → invite

	tickets   map[string]*Ticket           // ticket ID → ticket
	ticketSeq int                          // monotonic #N counter (ticket 066230d9)
	comments  map[string][]*TicketComment  // ticket ID → comments (in order)
	artifacts map[string][]*TicketArtifact // ticket ID → artifacts (in order)
	blobs     map[string][]byte            // sha256 hex → blob bytes

	workOrders map[string]*WorkOrder // ticket ID → work-order
	deploys    map[string]*Deploy    // deploy ID → deploy
	deploySeq  []string              // deploy IDs, insertion order (for stable list)

	alerts   map[string]*AdminAlert // alert ID → admin alert
	alertSeq []string               // alert IDs, insertion order (newest = last)
	userSeq  []string               // user IDs, insertion order (for stable list)

	meshJobs  map[string]*MeshJob // job ID → mesh job (store_mem_mesh.go)
	meshSeq   []string            // job IDs, insertion order (FIFO claim)
	meshBlobs map[string][]byte   // blob ref → STL bytes
}

// NewStore returns the in-memory store (test fixture).
func NewStore() Store {
	return &memStore{
		users:       make(map[string]*User),
		emails:      make(map[string]string),
		projects:    make(map[string]*Project),
		sessions:    make(map[string]*Session),
		resetTokens: make(map[string]*ResetToken),
		thumbnails:  make(map[string][]byte),
		snapshots:   make(map[string]*Snapshot),
		invites:     make(map[string]*Invite),
		tickets:     make(map[string]*Ticket),
		comments:    make(map[string][]*TicketComment),
		artifacts:   make(map[string][]*TicketArtifact),
		blobs:       make(map[string][]byte),
		workOrders:  make(map[string]*WorkOrder),
		deploys:     make(map[string]*Deploy),
		alerts:      make(map[string]*AdminAlert),
	}
}

// ─── content-addressed blobs (client asset sync, plan §5.5) ─────
//
// Reuses the same `blobs` map the ticket artifacts use — one content-addressed
// namespace keyed by sha256 hex.

func (s *memStore) PutBlob(data []byte) (string, error) {
	if len(data) == 0 {
		return "", nil
	}
	sum := sha256.Sum256(data)
	ref := hex.EncodeToString(sum[:])
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.blobs[ref]; !ok {
		cp := make([]byte, len(data))
		copy(cp, data)
		s.blobs[ref] = cp
	}
	return ref, nil
}

func (s *memStore) GetBlob(ref string) ([]byte, error) {
	if ref == "" {
		return nil, nil
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.blobs[ref], nil
}

func (s *memStore) HasBlob(ref string) bool {
	if ref == "" {
		return false
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	_, ok := s.blobs[ref]
	return ok
}

func (s *memStore) CreateUser(email, displayName, password string, role Role) (*User, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if _, exists := s.emails[email]; exists {
		return nil, errConflict
	}
	user := s.createUserLocked(email, displayName, password, role)
	return user, nil
}

// createUserLocked builds and stores a user. Caller holds the write lock and
// has already checked the email is free.
func (s *memStore) createUserLocked(email, displayName, password string, role Role) *User {
	hash, salt := hashPassword(password)
	id := newID()
	user := &User{
		ID:           id,
		Email:        email,
		DisplayName:  displayName,
		Role:         role,
		PasswordHash: hash,
		Salt:         salt,
		CreatedAt:    now(),
	}
	s.users[id] = user
	s.emails[email] = id
	s.userSeq = append(s.userSeq, id)
	return user
}

func (s *memStore) AuthenticateUser(email, password string) (*User, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	uid, ok := s.emails[email]
	if !ok {
		return nil, errNotFound
	}
	user := s.users[uid]
	if !verifyPassword(password, user.Salt, user.PasswordHash) {
		return nil, errUnauthorized
	}
	// A banned user cannot authenticate (B3, §4.4).
	if user.Disabled {
		return nil, errUserDisabled
	}
	return user, nil
}

func (s *memStore) GetUserByEmail(email string) *User {
	s.mu.RLock()
	defer s.mu.RUnlock()
	uid, ok := s.emails[email]
	if !ok {
		return nil
	}
	return s.users[uid]
}

func (s *memStore) GetUser(id string) *User {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.users[id]
}

func (s *memStore) UpdatePassword(userID, newPassword string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	user, ok := s.users[userID]
	if !ok {
		return
	}
	user.PasswordHash, user.Salt = hashPassword(newPassword)
}

func (s *memStore) HasAdmin() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, u := range s.users {
		if u.Role == RoleAdmin {
			return true
		}
	}
	return false
}

// ─── user administration / ban (B3) ─────────────────────────────

// cloneUser returns a copy so callers can't mutate stored state.
func cloneUser(u *User) *User {
	c := *u
	return &c
}

func (s *memStore) ListUsers() []*User {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]*User, 0, len(s.userSeq))
	// Newest first.
	for i := len(s.userSeq) - 1; i >= 0; i-- {
		if u := s.users[s.userSeq[i]]; u != nil {
			out = append(out, cloneUser(u))
		}
	}
	return out
}

func (s *memStore) SetUserDisabled(userID string, disabled bool) *User {
	s.mu.Lock()
	defer s.mu.Unlock()
	u, ok := s.users[userID]
	if !ok {
		return nil
	}
	u.Disabled = disabled
	return cloneUser(u)
}

// AdminCount returns the number of (enabled) admins, not counting excludeUserID.
// Used to guard against removing the last admin (B3).
func (s *memStore) AdminCount(excludeUserID string) int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	n := 0
	for _, u := range s.users {
		if u.ID == excludeUserID {
			continue
		}
		if u.Role == RoleAdmin && !u.Disabled {
			n++
		}
	}
	return n
}

func (s *memStore) DeleteUserSessions(userID string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for tok, sess := range s.sessions {
		if sess.UserID == userID {
			delete(s.sessions, tok)
		}
	}
}

func (s *memStore) CreateResetToken(userID string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	token := newID()
	s.resetTokens[token] = &ResetToken{
		Token:   token,
		UserID:  userID,
		Expires: now().Add(resetTokenTTL),
	}
	return token
}

func (s *memStore) ConsumeResetToken(token string) (string, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rt, ok := s.resetTokens[token]
	if !ok || now().After(rt.Expires) {
		delete(s.resetTokens, token)
		return "", false
	}
	delete(s.resetTokens, token)
	return rt.UserID, true
}

// ─── invites ────────────────────────────────────────────────────

func (s *memStore) CreateInvite(createdBy, inviteeName, inviteeEmail, message string, expiresAt time.Time) *Invite {
	s.mu.Lock()
	defer s.mu.Unlock()
	inv := &Invite{
		Token:        newID(),
		CreatedBy:    createdBy,
		InviteeName:  inviteeName,
		InviteeEmail: inviteeEmail,
		Message:      message,
		ExpiresAt:    expiresAt,
		CreatedAt:    now(),
	}
	s.invites[inv.Token] = inv
	return inv
}

func (s *memStore) GetInvite(token string) *Invite {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.invites[token]
}

func (s *memStore) ListInvites() []*Invite {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]*Invite, 0, len(s.invites))
	for _, inv := range s.invites {
		out = append(out, inv)
	}
	return out
}

func (s *memStore) ConsumeInvite(token, usedBy string) (*Invite, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.consumeInviteLocked(token, usedBy)
}

// consumeInviteLocked burns a single-use invite. Caller holds the write lock.
func (s *memStore) consumeInviteLocked(token, usedBy string) (*Invite, error) {
	inv, ok := s.invites[token]
	if !ok || now().After(inv.ExpiresAt) {
		return nil, errInviteInvalid
	}
	if inv.UsedBy != nil {
		return nil, errInviteUsed
	}
	usedAt := now()
	inv.UsedBy = &usedBy
	inv.UsedAt = &usedAt
	return inv, nil
}

// RegisterWithInvite validates + burns the invite and creates the user under
// ONE lock section — the single-use guarantee and the user creation cannot
// tear apart (fixes the register race, see Store interface).
func (s *memStore) RegisterWithInvite(email, displayName, password, token string) (*User, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	// Invite first (a duplicate-email failure must not burn the token), then
	// the email check, then create + burn.
	inv, ok := s.invites[token]
	if !ok || now().After(inv.ExpiresAt) {
		return nil, errInviteInvalid
	}
	if inv.UsedBy != nil {
		return nil, errInviteUsed
	}
	if _, exists := s.emails[email]; exists {
		return nil, errConflict
	}
	user := s.createUserLocked(email, displayName, password, RoleUser)
	if _, err := s.consumeInviteLocked(token, user.ID); err != nil {
		// Unreachable: the invite was validated under this same lock.
		delete(s.users, user.ID)
		delete(s.emails, email)
		s.userSeq = s.userSeq[:len(s.userSeq)-1]
		return nil, err
	}
	return user, nil
}

func (s *memStore) DeleteInvite(token string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.invites[token]; !ok {
		return false
	}
	delete(s.invites, token)
	return true
}

// ─── thumbnails & snapshots ─────────────────────────────────────

func (s *memStore) SetThumbnail(projectID string, data []byte) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.thumbnails[projectID] = data
}

func (s *memStore) GetThumbnail(projectID string) []byte {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.thumbnails[projectID]
}

func (s *memStore) CreateSnapshot(projectID string, doc json.RawMessage) *Snapshot {
	s.mu.Lock()
	defer s.mu.Unlock()
	snap := &Snapshot{
		ID:        newID(),
		ProjectID: projectID,
		Document:  doc,
		CreatedAt: now(),
	}
	s.snapshots[snap.ID] = snap
	return snap
}

func (s *memStore) GetSnapshot(id string) *Snapshot {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.snapshots[id]
}

// ─── sessions ───────────────────────────────────────────────────

func (s *memStore) CreateSession(userID string) *Session {
	s.mu.Lock()
	defer s.mu.Unlock()

	token := newID()
	sess := &Session{
		Token:   token,
		UserID:  userID,
		Expires: now().Add(sessionTTL),
	}
	s.sessions[token] = sess
	return sess
}

func (s *memStore) GetSession(token string) *Session {
	s.mu.RLock()
	defer s.mu.RUnlock()

	sess, ok := s.sessions[token]
	if !ok || now().After(sess.Expires) {
		return nil
	}
	return sess
}

func (s *memStore) DeleteSession(token string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.sessions, token)
}

// ─── projects ───────────────────────────────────────────────────

func (s *memStore) CreateProject(ownerID, name string, doc json.RawMessage) *Project {
	s.mu.Lock()
	defer s.mu.Unlock()

	id := newID()
	at := now()
	p := &Project{
		ID:         id,
		OwnerID:    ownerID,
		Name:       name,
		Visibility: "private",
		Document:   doc,
		UpdatedAt:  at,
		CreatedAt:  at,
	}
	s.projects[id] = p
	return p
}

func (s *memStore) ListProjects(ownerID string) []*Project {
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

func (s *memStore) GetProject(id, ownerID string) *Project {
	s.mu.RLock()
	defer s.mu.RUnlock()

	p, ok := s.projects[id]
	if !ok || p.OwnerID != ownerID {
		return nil
	}
	return p
}

func (s *memStore) UpdateProject(id, ownerID, name string, doc json.RawMessage, visibility string) *Project {
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
	p.UpdatedAt = now()
	return p
}

func (s *memStore) ListPublicProjects() []*Project {
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

func (s *memStore) DeleteProject(id, ownerID string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()

	p, ok := s.projects[id]
	if !ok || p.OwnerID != ownerID {
		return false
	}
	delete(s.projects, id)
	return true
}

// ─── admin alerts (B2, §4.4) ────────────────────────────────────

func cloneAlert(a *AdminAlert) *AdminAlert {
	c := *a
	if a.TicketID != nil {
		tid := *a.TicketID
		c.TicketID = &tid
	}
	return &c
}

func (s *memStore) CreateAdminAlert(kind AlertKind, ticketID *string, detail string) *AdminAlert {
	s.mu.Lock()
	defer s.mu.Unlock()
	a := &AdminAlert{
		ID:        newID(),
		Kind:      kind,
		Detail:    detail,
		CreatedAt: now(),
	}
	if ticketID != nil {
		tid := *ticketID
		a.TicketID = &tid
	}
	s.alerts[a.ID] = a
	s.alertSeq = append(s.alertSeq, a.ID)
	return cloneAlert(a)
}

func (s *memStore) ListAdminAlerts(unseenOnly bool) []*AdminAlert {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]*AdminAlert, 0, len(s.alertSeq))
	// Newest first.
	for i := len(s.alertSeq) - 1; i >= 0; i-- {
		a := s.alerts[s.alertSeq[i]]
		if a == nil || (unseenOnly && a.Seen) {
			continue
		}
		out = append(out, cloneAlert(a))
	}
	return out
}

func (s *memStore) MarkAdminAlertSeen(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	a, ok := s.alerts[id]
	if !ok {
		return false
	}
	a.Seen = true
	return true
}
