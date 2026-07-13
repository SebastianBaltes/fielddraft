// memStore — ticket domain (TicketStore implementation over in-memory maps).
//
// Split out of store_mem.go purely mechanically: tickets, comments, artifacts,
// work-orders, deploys, and the agent queue queries. Users/projects/sessions/
// invites/admin-alerts stay in store_mem.go.

package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"strings"
)

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
	at := now()
	s.ticketSeq++
	t := &Ticket{
		ID:        newID(),
		Number:    s.ticketSeq,
		AuthorID:  authorID,
		Title:     title,
		Body:      body,
		State:     StateTriage,
		CreatedAt: at,
		UpdatedAt: at,
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
	t.UpdatedAt = now()
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
	t.UpdatedAt = now()
	return cloneTicket(t)
}

func (s *memStore) SetTicketState(id string, to TicketState, direct bool) (*Ticket, error) {
	s.mu.Lock()
	t, ok := s.tickets[id]
	if !ok {
		s.mu.Unlock()
		return nil, errTicketMissing
	}
	audit, err := applyStateChange(t, to, now(), direct)
	if err != nil {
		s.mu.Unlock()
		return nil, err
	}
	out := cloneTicket(t)
	s.mu.Unlock()
	s.appendSystemComment(id, audit)
	return out, nil
}

func (s *memStore) SetPOVerdict(id string, v POVerdict) (*Ticket, error) {
	s.mu.Lock()
	t, ok := s.tickets[id]
	if !ok {
		s.mu.Unlock()
		return nil, errTicketMissing
	}
	audit, err := applyPOVerdict(t, v, now())
	if err != nil {
		s.mu.Unlock()
		return nil, err
	}
	out := cloneTicket(t)
	s.mu.Unlock()
	s.appendSystemComment(id, audit)
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
	// The PO-review rule (only user comments need moderation) lives in newComment.
	c := newComment(ticketID, authorType, authorID, body, internal, false)
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
func (s *memStore) SetCommentReviewed(ticketID, commentID string, withheld bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	// Scope to the named ticket's comments: a commentID from another ticket must
	// not be moderated here (mirrors GetComment's "nil if not on that ticket").
	for _, c := range s.comments[ticketID] {
		if c.ID == commentID {
			c.POReviewed = true
			if withheld {
				c.Withheld = true
			}
			cc := *c
			return &cc
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
		CreatedAt: now(),
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
		CreatedAt: now(),
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
	at := now()
	wo.PickedUpAt = &at
	return cloneWorkOrder(wo), nil
}

func (s *memStore) CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	wo, ok := s.workOrders[ticketID]
	if !ok {
		return nil, errWorkOrderMissing
	}
	if wo.CompletedAt != nil {
		return nil, errWorkOrderCompleted
	}
	if deployID != "" {
		if _, ok := s.deploys[deployID]; !ok {
			return nil, errDeployMissing
		}
	}
	at := now()
	wo.CompletedAt = &at
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
		DeployedAt: now(),
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
	// po/dev/system have no user id and are inherently reviewed (newComment rule).
	c := newComment(ticketID, authorType, nil, body, internal, withheld)
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
