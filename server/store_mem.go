// memStore — in-memory map implementation of Store.
//
// Kept ONLY as a fast, hermetic test fixture (the handler tests in
// main_test.go run against it with no live database). NewStore() returns it.

package main

import (
	"crypto/rand"
	"encoding/json"
	"sync"
	"time"

	"golang.org/x/crypto/argon2"
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
	}
}

func (s *memStore) CreateUser(email, displayName, password string, role Role) (*User, error) {
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
		Role:         role,
		PasswordHash: hash,
		Salt:         salt,
		CreatedAt:    time.Now(),
	}
	s.users[id] = user
	s.emails[email] = id
	return user, nil
}

func (s *memStore) AuthenticateUser(email, password string) (*User, error) {
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
	salt := make([]byte, 16)
	rand.Read(salt)
	user.PasswordHash = argon2.IDKey([]byte(newPassword), salt, 1, 64*1024, 4, 32)
	user.Salt = salt
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

func (s *memStore) CreateResetToken(userID string) string {
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

func (s *memStore) ConsumeResetToken(token string) (string, bool) {
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

// ─── invites ────────────────────────────────────────────────────

func (s *memStore) CreateInvite(createdBy, note string, expiresAt time.Time) *Invite {
	s.mu.Lock()
	defer s.mu.Unlock()
	inv := &Invite{
		Token:     newID(),
		CreatedBy: createdBy,
		Note:      note,
		ExpiresAt: expiresAt,
		CreatedAt: time.Now(),
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
	inv, ok := s.invites[token]
	if !ok || time.Now().After(inv.ExpiresAt) {
		return nil, errInviteInvalid
	}
	if inv.UsedBy != nil {
		return nil, errInviteUsed
	}
	now := time.Now()
	inv.UsedBy = &usedBy
	inv.UsedAt = &now
	return inv, nil
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
		CreatedAt: time.Now(),
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
		Expires: time.Now().Add(7 * 24 * time.Hour),
	}
	s.sessions[token] = sess
	return sess
}

func (s *memStore) GetSession(token string) *Session {
	s.mu.RLock()
	defer s.mu.RUnlock()

	sess, ok := s.sessions[token]
	if !ok || time.Now().After(sess.Expires) {
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
	p.UpdatedAt = time.Now()
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
