// memStore — in-memory map implementation of Store.
//
// Kept ONLY as a fast, hermetic test fixture (the handler tests in
// main_test.go run against it with no live database). NewStore() returns it.

package main

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"strings"
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

	tickets   map[string]*Ticket           // ticket ID → ticket
	comments  map[string][]*TicketComment  // ticket ID → comments (in order)
	artifacts map[string][]*TicketArtifact // ticket ID → artifacts (in order)
	blobs     map[string][]byte            // sha256 hex → blob bytes

	workOrders map[string]*WorkOrder // ticket ID → work-order
	deploys    map[string]*Deploy    // deploy ID → deploy
	deploySeq  []string              // deploy IDs, insertion order (for stable list)

	alerts   map[string]*AdminAlert // alert ID → admin alert
	alertSeq []string               // alert IDs, insertion order (newest = last)
	userSeq  []string               // user IDs, insertion order (for stable list)
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
	s.userSeq = append(s.userSeq, id)
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

// ─── tickets ────────────────────────────────────────────────────

// cloneTicket returns a shallow copy so callers can't mutate stored state
// (and so concurrent reads stay race-free under the RWMutex contract).
func cloneTicket(t *Ticket) *Ticket {
	c := *t
	c.Comments = nil
	c.Artifacts = nil
	return &c
}

func (s *memStore) CreateTicket(authorID, title, body, category string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now().UTC()
	t := &Ticket{
		ID:        newID(),
		AuthorID:  authorID,
		Title:     title,
		Body:      body,
		State:     StateTriage,
		CreatedAt: now,
		UpdatedAt: now,
	}
	if category != "" {
		c := category
		t.Category = &c
	}
	s.tickets[t.ID] = t
	return cloneTicket(t)
}

func (s *memStore) GetTicket(id string) *Ticket {
	s.mu.RLock()
	defer s.mu.RUnlock()
	t, ok := s.tickets[id]
	if !ok {
		return nil
	}
	out := cloneTicket(t)
	for _, c := range s.comments[id] {
		cc := *c
		out.Comments = append(out.Comments, &cc)
	}
	for _, a := range s.artifacts[id] {
		aa := *a
		out.Artifacts = append(out.Artifacts, &aa)
	}
	return out
}

func (s *memStore) ListTickets(f TicketFilter) []*Ticket {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []*Ticket
	q := strings.ToLower(f.Query)
	for _, t := range s.tickets {
		if f.AuthorID != "" && t.AuthorID != f.AuthorID {
			continue
		}
		if f.State != "" && t.State != f.State {
			continue
		}
		if f.Category != "" && (t.Category == nil || *t.Category != f.Category) {
			continue
		}
		if q != "" && !strings.Contains(strings.ToLower(t.Title), q) &&
			!strings.Contains(strings.ToLower(t.Body), q) {
			continue
		}
		// Visibility (§7.3): hide rejected tickets unless caller is admin or
		// the ticket's own author.
		if isRejected(t.State) && !f.ViewerIsAdmin && t.AuthorID != f.ViewerID {
			continue
		}
		out = append(out, cloneTicket(t))
	}
	// Newest first (chronological by creation).
	sortTicketsDesc(out)
	return out
}

func (s *memStore) UpdateTicket(id, title, body string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	t, ok := s.tickets[id]
	if !ok {
		return nil
	}
	if title != "" {
		t.Title = title
	}
	if body != "" {
		t.Body = body
	}
	t.UpdatedAt = time.Now().UTC()
	return cloneTicket(t)
}

func (s *memStore) SetTicketCategory(id, category string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	t, ok := s.tickets[id]
	if !ok {
		return nil
	}
	if category != "" {
		c := category
		t.Category = &c
	} else {
		t.Category = nil
	}
	t.UpdatedAt = time.Now().UTC()
	return cloneTicket(t)
}

func (s *memStore) SetTicketState(id string, to TicketState) (*Ticket, error) {
	s.mu.Lock()
	t, ok := s.tickets[id]
	if !ok {
		s.mu.Unlock()
		return nil, errTicketMissing
	}
	if err := validateTransition(t.State, to); err != nil {
		s.mu.Unlock()
		return nil, err
	}
	from := t.State
	t.State = to
	t.UpdatedAt = time.Now().UTC()
	out := cloneTicket(t)
	s.mu.Unlock()
	s.appendSystemComment(id, "state: "+string(from)+" → "+string(to))
	return out, nil
}

func (s *memStore) SetPOVerdict(id string, v POVerdict) (*Ticket, error) {
	s.mu.Lock()
	t, ok := s.tickets[id]
	if !ok {
		s.mu.Unlock()
		return nil, errTicketMissing
	}
	if err := validateTransition(t.State, v.State); err != nil {
		s.mu.Unlock()
		return nil, err
	}
	from := t.State
	now := time.Now().UTC()
	t.POSpam = v.Spam
	t.POMalicious = v.Malicious
	t.POVisionFit = v.VisionFit
	if v.Reason != "" {
		r := v.Reason
		t.POReason = &r
	}
	t.POVerdictAt = &now
	t.State = v.State
	t.UpdatedAt = now
	out := cloneTicket(t)
	s.mu.Unlock()
	s.appendSystemComment(id, "po verdict: "+string(from)+" → "+string(v.State))
	return out, nil
}

func (s *memStore) DeleteTicket(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.tickets[id]; !ok {
		return false
	}
	delete(s.tickets, id)
	delete(s.comments, id)
	delete(s.artifacts, id)
	delete(s.workOrders, id)
	return true
}

// appendSystemComment records a state-change audit entry. Locks internally.
func (s *memStore) appendSystemComment(ticketID, body string) {
	s.AddComment(ticketID, AuthorSystem, nil, body, false)
}

func (s *memStore) AddComment(ticketID string, authorType AuthorType, authorID *string, body string, internal bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.tickets[ticketID]; !ok {
		return nil
	}
	c := &TicketComment{
		ID:         newID(),
		TicketID:   ticketID,
		AuthorType: authorType,
		AuthorID:   authorID,
		Internal:   internal,
		Body:       body,
		CreatedAt:  time.Now().UTC(),
		// Only user comments need PO post-moderation; po/dev/system are inherently
		// reviewed (B1, §4.2).
		POReviewed: authorType != AuthorUser,
	}
	s.comments[ticketID] = append(s.comments[ticketID], c)
	if t := s.tickets[ticketID]; t != nil {
		t.UpdatedAt = c.CreatedAt
	}
	cc := *c
	return &cc
}

func (s *memStore) GetComment(ticketID, commentID string) *TicketComment {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, c := range s.comments[ticketID] {
		if c.ID == commentID {
			cc := *c
			return &cc
		}
	}
	return nil
}

// SetCommentReviewed marks a comment PO-reviewed and, when withheld, hides it
// (B1, §4.2). Returns the updated comment (nil if unknown).
func (s *memStore) SetCommentReviewed(commentID string, withheld bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, list := range s.comments {
		for _, c := range list {
			if c.ID == commentID {
				c.POReviewed = true
				if withheld {
					c.Withheld = true
				}
				cc := *c
				return &cc
			}
		}
	}
	return nil
}

// ListTicketsWithUnreviewedComments returns every ticket (any state) that has
// at least one unreviewed USER comment — so spam on a resolved ticket still
// reaches the PO (B1, §4.2). Hydrated with comments + artifacts.
func (s *memStore) ListTicketsWithUnreviewedComments() []*Ticket {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []*Ticket
	for id, t := range s.tickets {
		has := false
		for _, c := range s.comments[id] {
			if c.AuthorType == AuthorUser && !c.POReviewed {
				has = true
				break
			}
		}
		if !has {
			continue
		}
		full := cloneTicket(t)
		for _, c := range s.comments[id] {
			cc := *c
			full.Comments = append(full.Comments, &cc)
		}
		for _, a := range s.artifacts[id] {
			aa := *a
			full.Artifacts = append(full.Artifacts, &aa)
		}
		out = append(out, full)
	}
	sortTicketsDesc(out)
	return out
}

func (s *memStore) ListComments(ticketID string) []*TicketComment {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []*TicketComment
	for _, c := range s.comments[ticketID] {
		cc := *c
		out = append(out, &cc)
	}
	return out
}

func (s *memStore) AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact {
	if len(data) == 0 {
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.tickets[ticketID]; !ok {
		return nil
	}
	sum := sha256.Sum256(data)
	ref := hex.EncodeToString(sum[:])
	s.blobs[ref] = data
	a := &TicketArtifact{
		ID:        newID(),
		TicketID:  ticketID,
		Kind:      kind,
		BlobRef:   ref,
		Mime:      mime,
		Size:      int64(len(data)),
		Name:      name,
		CreatedAt: time.Now().UTC(),
	}
	s.artifacts[ticketID] = append(s.artifacts[ticketID], a)
	return a
}

func (s *memStore) GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, a := range s.artifacts[ticketID] {
		if a.ID == artifactID {
			aa := *a
			return &aa, s.blobs[a.BlobRef]
		}
	}
	return nil, nil
}

// sortTicketsDesc sorts tickets newest-first by CreatedAt (insertion sort —
// ticket counts are small).
func sortTicketsDesc(ts []*Ticket) {
	for i := 1; i < len(ts); i++ {
		for j := i; j > 0 && ts[j].CreatedAt.After(ts[j-1].CreatedAt); j-- {
			ts[j], ts[j-1] = ts[j-1], ts[j]
		}
	}
}

// ─── work-orders ────────────────────────────────────────────────

// cloneWorkOrder returns a copy so callers can't mutate stored state.
func cloneWorkOrder(wo *WorkOrder) *WorkOrder {
	c := *wo
	return &c
}

func (s *memStore) CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.tickets[ticketID]; !ok {
		return nil, errTicketMissing
	}
	if _, exists := s.workOrders[ticketID]; exists {
		return nil, errWorkOrderExists
	}
	wo := &WorkOrder{
		TicketID:  ticketID,
		Payload:   append(json.RawMessage(nil), payload...),
		CreatedAt: time.Now().UTC(),
	}
	s.workOrders[ticketID] = wo
	return cloneWorkOrder(wo), nil
}

func (s *memStore) GetWorkOrder(ticketID string) *WorkOrder {
	s.mu.RLock()
	defer s.mu.RUnlock()
	wo, ok := s.workOrders[ticketID]
	if !ok {
		return nil
	}
	return cloneWorkOrder(wo)
}

func (s *memStore) ClaimWorkOrder(ticketID string) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	wo, ok := s.workOrders[ticketID]
	if !ok {
		return nil, errWorkOrderMissing
	}
	if wo.PickedUpAt != nil {
		return nil, errWorkOrderClaimed
	}
	now := time.Now().UTC()
	wo.PickedUpAt = &now
	return cloneWorkOrder(wo), nil
}

func (s *memStore) CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	wo, ok := s.workOrders[ticketID]
	if !ok {
		return nil, errWorkOrderMissing
	}
	if deployID != "" {
		if _, ok := s.deploys[deployID]; !ok {
			return nil, errDeployMissing
		}
	}
	now := time.Now().UTC()
	wo.CompletedAt = &now
	if deployID != "" {
		d := deployID
		wo.DeployID = &d
	}
	return cloneWorkOrder(wo), nil
}

// ─── deploys ────────────────────────────────────────────────────

func cloneDeploy(d *Deploy) *Deploy {
	c := *d
	c.TicketIDs = append([]string(nil), d.TicketIDs...)
	return &c
}

func (s *memStore) CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	d := &Deploy{
		ID:         newID(),
		Version:    version,
		GitSHA:     gitSHA,
		TicketIDs:  append([]string(nil), ticketIDs...),
		Status:     status,
		DeployedAt: time.Now().UTC(),
	}
	s.deploys[d.ID] = d
	s.deploySeq = append(s.deploySeq, d.ID)
	return cloneDeploy(d), nil
}

func (s *memStore) ListDeploys() []*Deploy {
	s.mu.RLock()
	defer s.mu.RUnlock()
	// Newest first.
	out := make([]*Deploy, 0, len(s.deploySeq))
	for i := len(s.deploySeq) - 1; i >= 0; i-- {
		if d := s.deploys[s.deploySeq[i]]; d != nil {
			out = append(out, cloneDeploy(d))
		}
	}
	return out
}

// ─── agent comments ─────────────────────────────────────────────

func (s *memStore) AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.tickets[ticketID]; !ok {
		return nil
	}
	c := &TicketComment{
		ID:         newID(),
		TicketID:   ticketID,
		AuthorType: authorType,
		AuthorID:   nil, // po/dev/system have no user id
		Internal:   internal,
		Body:       body,
		CreatedAt:  time.Now().UTC(),
		Withheld:   withheld,
		POReviewed: true, // po/dev are authored by the agents — inherently reviewed
	}
	s.comments[ticketID] = append(s.comments[ticketID], c)
	if t := s.tickets[ticketID]; t != nil {
		t.UpdatedAt = c.CreatedAt
	}
	cc := *c
	return &cc
}

// ListAcceptedWithOpenWorkOrder returns accepted tickets that still carry an
// open (uncompleted) work-order — the coding agent's queue (§10.1).
func (s *memStore) ListAcceptedWithOpenWorkOrder() []*Ticket {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var out []*Ticket
	for _, t := range s.tickets {
		if t.State != StateAccepted {
			continue
		}
		wo, ok := s.workOrders[t.ID]
		if !ok || !wo.Open() {
			continue
		}
		full := cloneTicket(t)
		for _, c := range s.comments[t.ID] {
			cc := *c
			full.Comments = append(full.Comments, &cc)
		}
		for _, a := range s.artifacts[t.ID] {
			aa := *a
			full.Artifacts = append(full.Artifacts, &aa)
		}
		out = append(out, full)
	}
	sortTicketsDesc(out)
	return out
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
		CreatedAt: time.Now().UTC(),
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
