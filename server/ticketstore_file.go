// fileTicketStore — a file-backed TicketStore for local dev (WP15).
//
// Selected via TICKET_BACKEND=files (base dir from TICKET_DIR, default
// ./tickets). It implements the SAME TicketStore contract as the DB backend, so
// the identical ticket CRUD/UI/API/agent-harvest runs over files locally. Prod
// keeps the DB backend and is untouched.
//
// ─── on-disk layout (matches the materialized layout, §3.1) ─────────────────
//
//	<base>/{open,done}/<id>/
//	  ticket.json   — the AUTHORITATIVE structured record: every ticket field +
//	                  comments[] (author_type/internal/withheld/created_at) +
//	                  artifact metadata + work_order + po-verdict fields. This is
//	                  the only file read back.
//	  ticket.md     — rendered (heading + meta + body). Derived, agent-facing.
//	  comments.md   — rendered thread (user/po/dev/system, chronological). Derived.
//	  screenshot.png / project.fdraft / actions.json / settings.json /
//	  ui_state.json / attachment-NN-<name> — artifact blobs at their canonical
//	                  filenames; GetArtifactBlob reads these back.
//
// A ticket lives under open/<id>/ until it reaches a terminal state (closed /
// resolved / rejected_*), then it is moved to done/<id>/. Lookups probe both.
//
// Concurrency: one process-wide mutex (local dev, single process). Every method
// takes it; heavy work is fine because this is a dev-only backend.

package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// fileTicketStore is a file-backed TicketStore. All ticket state lives in
// per-ticket ticket.json files under <base>/{open,done}/<id>/.
type fileTicketStore struct {
	mu   sync.Mutex
	base string
	// deploys are process-global (not per-ticket); persisted to <base>/deploys.json.
	deploys []*Deploy
}

var _ TicketStore = (*fileTicketStore)(nil)

// fileTicket is the ticket.json schema — the authoritative record. It embeds all
// ticket fields plus the full thread, artifact metadata, and the work-order.
type fileTicket struct {
	Ticket
	Comments  []*TicketComment  `json:"comments"`
	Artifacts []*TicketArtifact `json:"artifacts"`
	WorkOrder *WorkOrder        `json:"work_order,omitempty"`
}

// newFileTicketStore opens (creating if needed) a file-backed store rooted at
// base, with open/ and done/ subdirs.
func newFileTicketStore(base string) (*fileTicketStore, error) {
	abs, err := filepath.Abs(base)
	if err != nil {
		return nil, err
	}
	for _, d := range []string{abs, filepath.Join(abs, "open"), filepath.Join(abs, "done")} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return nil, err
		}
	}
	s := &fileTicketStore{base: abs}
	s.deploys = s.loadDeploys()
	return s, nil
}

// ─── path helpers ───────────────────────────────────────────────

func (s *fileTicketStore) openDir(id string) string { return filepath.Join(s.base, "open", id) }
func (s *fileTicketStore) doneDir(id string) string { return filepath.Join(s.base, "done", id) }

// dirFor returns the on-disk directory holding ticket id (open/ preferred), and
// whether it exists at all.
func (s *fileTicketStore) dirFor(id string) (string, bool) {
	if d := s.openDir(id); dirExists(d) {
		return d, true
	}
	if d := s.doneDir(id); dirExists(d) {
		return d, true
	}
	return "", false
}

func dirExists(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

// isTerminalState reports whether a ticket in state s belongs under done/.
func isTerminalState(st TicketState) bool {
	switch st {
	case StateClosed, StateResolved, StateRejectedSpam, StateRejectedMalicious:
		return true
	}
	return false
}

// ─── low-level load/save ────────────────────────────────────────

// load reads a ticket.json by id, or nil if the ticket is unknown.
func (s *fileTicketStore) load(id string) *fileTicket {
	dir, ok := s.dirFor(id)
	if !ok {
		return nil
	}
	raw, err := os.ReadFile(filepath.Join(dir, "ticket.json"))
	if err != nil {
		return nil
	}
	var ft fileTicket
	if err := json.Unmarshal(raw, &ft); err != nil {
		return nil
	}
	return &ft
}

// save writes the authoritative ticket.json plus the derived ticket.md and
// comments.md, and relocates the directory between open/ and done/ to match the
// ticket's terminal-ness.
func (s *fileTicketStore) save(ft *fileTicket) error {
	// Pick the target dir based on terminal-ness, migrating if it moved.
	want := s.openDir(ft.ID)
	if isTerminalState(ft.State) {
		want = s.doneDir(ft.ID)
	}
	cur, exists := s.dirFor(ft.ID)
	if exists && cur != want {
		if err := os.MkdirAll(filepath.Dir(want), 0o755); err != nil {
			return err
		}
		if err := os.Rename(cur, want); err != nil {
			return err
		}
	}
	if err := os.MkdirAll(want, 0o755); err != nil {
		return err
	}

	raw, err := json.MarshalIndent(ft, "", "  ")
	if err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(want, "ticket.json"), raw, 0o644); err != nil {
		return err
	}
	// Derived, agent-facing renders (not read back).
	if err := os.WriteFile(filepath.Join(want, "ticket.md"), []byte(renderTicketMD(ft)), 0o644); err != nil {
		return err
	}
	if err := os.WriteFile(filepath.Join(want, "comments.md"), []byte(renderCommentsMD(ft)), 0o644); err != nil {
		return err
	}
	return nil
}

// ─── rendered markdown (derived) ────────────────────────────────

// renderTicketMD renders ticket.md: heading + meta + body. Matches the layout
// the file flow produces today (web/vite.config.ts ticketWriter).
func renderTicketMD(ft *fileTicket) string {
	var b strings.Builder
	fmt.Fprintf(&b, "# %s\n\n", ticketHeading(ft.ID, ft.Title))
	fmt.Fprintf(&b, "- **ID:** %s\n", ft.ID)
	fmt.Fprintf(&b, "- **Author:** %s\n", ft.AuthorID)
	fmt.Fprintf(&b, "- **State:** %s\n", ft.State)
	if ft.Category != nil {
		fmt.Fprintf(&b, "- **Category:** %s\n", *ft.Category)
	}
	fmt.Fprintf(&b, "- **Created:** %s\n", ft.CreatedAt.Format(time.RFC3339))
	fmt.Fprintf(&b, "- **Updated:** %s\n\n", ft.UpdatedAt.Format(time.RFC3339))
	body := ft.Body
	if strings.TrimSpace(body) == "" {
		body = "_(none)_"
	}
	fmt.Fprintf(&b, "## Description\n\n%s\n", body)
	if len(ft.Artifacts) > 0 {
		b.WriteString("\n## Artifacts\n\n")
		for _, a := range ft.Artifacts {
			fmt.Fprintf(&b, "- %s (%s)\n", a.Name, a.Kind)
		}
	}
	return b.String()
}

// renderCommentsMD renders comments.md: the thread chronologically, tagged by
// author type; withheld/internal notes are labeled for the agent's awareness.
func renderCommentsMD(ft *fileTicket) string {
	var b strings.Builder
	b.WriteString("# Comments\n\n")
	if len(ft.Comments) == 0 {
		b.WriteString("_(none)_\n")
		return b.String()
	}
	for _, c := range ft.Comments {
		tags := ""
		if c.Internal {
			tags += " [internal]"
		}
		if c.Withheld {
			tags += " [withheld]"
		}
		fmt.Fprintf(&b, "## %s%s — %s\n\n%s\n\n", c.AuthorType, tags, c.CreatedAt.Format(time.RFC3339), c.Body)
	}
	return b.String()
}

// ─── enumerating all tickets ────────────────────────────────────

// allTickets loads every ticket.json under open/ and done/.
func (s *fileTicketStore) allTickets() []*fileTicket {
	var out []*fileTicket
	for _, sub := range []string{"open", "done"} {
		entries, err := os.ReadDir(filepath.Join(s.base, sub))
		if err != nil {
			continue
		}
		for _, e := range entries {
			if !e.IsDir() {
				continue
			}
			if ft := s.load(e.Name()); ft != nil {
				out = append(out, ft)
			}
		}
	}
	return out
}

// ─── tickets ────────────────────────────────────────────────────

// hydratedTicket returns a *Ticket with Comments + Artifacts populated (copies).
func hydratedTicket(ft *fileTicket) *Ticket {
	t := ft.Ticket
	t.Comments = cloneComments(ft.Comments)
	t.Artifacts = cloneArtifacts(ft.Artifacts)
	return &t
}

// bareTicket returns a *Ticket with no thread/artifacts (list-row shape).
func bareTicket(ft *fileTicket) *Ticket {
	t := ft.Ticket
	t.Comments = nil
	t.Artifacts = nil
	return &t
}

func cloneComments(in []*TicketComment) []*TicketComment {
	if len(in) == 0 {
		return nil
	}
	out := make([]*TicketComment, 0, len(in))
	for _, c := range in {
		cc := *c
		out = append(out, &cc)
	}
	return out
}

func cloneArtifacts(in []*TicketArtifact) []*TicketArtifact {
	if len(in) == 0 {
		return nil
	}
	out := make([]*TicketArtifact, 0, len(in))
	for _, a := range in {
		aa := *a
		out = append(out, &aa)
	}
	return out
}

func (s *fileTicketStore) CreateTicket(authorID, title, body, category string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	at := now()
	ft := &fileTicket{Ticket: Ticket{
		ID:        newID(),
		AuthorID:  authorID,
		Title:     title,
		Body:      body,
		State:     StateTriage,
		CreatedAt: at,
		UpdatedAt: at,
	}}
	if category != "" {
		c := category
		ft.Category = &c
	}
	if err := s.save(ft); err != nil {
		return nil
	}
	return bareTicket(ft)
}

func (s *fileTicketStore) GetTicket(id string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(id)
	if ft == nil {
		return nil
	}
	return hydratedTicket(ft)
}

func (s *fileTicketStore) ListTickets(f TicketFilter) []*Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	q := strings.ToLower(f.Query)
	var out []*Ticket
	for _, ft := range s.allTickets() {
		t := &ft.Ticket
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
		// Visibility (§7.3): hide rejected tickets unless caller is admin or the
		// ticket's own author.
		if isRejected(t.State) && !f.ViewerIsAdmin && t.AuthorID != f.ViewerID {
			continue
		}
		out = append(out, bareTicket(ft))
	}
	sortTicketsDesc(out)
	return out
}

func (s *fileTicketStore) UpdateTicket(id, title, body string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(id)
	if ft == nil {
		return nil
	}
	if title != "" {
		ft.Title = title
	}
	if body != "" {
		ft.Body = body
	}
	ft.UpdatedAt = now()
	if err := s.save(ft); err != nil {
		return nil
	}
	return bareTicket(ft)
}

func (s *fileTicketStore) SetTicketCategory(id, category string) *Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(id)
	if ft == nil {
		return nil
	}
	if category != "" {
		c := category
		ft.Category = &c
	} else {
		ft.Category = nil
	}
	ft.UpdatedAt = now()
	if err := s.save(ft); err != nil {
		return nil
	}
	return bareTicket(ft)
}

func (s *fileTicketStore) SetTicketState(id string, to TicketState) (*Ticket, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(id)
	if ft == nil {
		return nil, errTicketMissing
	}
	audit, err := applyStateChange(&ft.Ticket, to, now())
	if err != nil {
		return nil, err
	}
	s.appendComment(ft, AuthorSystem, nil, audit, false, false)
	if err := s.save(ft); err != nil {
		return nil, err
	}
	return bareTicket(ft), nil
}

func (s *fileTicketStore) SetPOVerdict(id string, v POVerdict) (*Ticket, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(id)
	if ft == nil {
		return nil, errTicketMissing
	}
	audit, err := applyPOVerdict(&ft.Ticket, v, now())
	if err != nil {
		return nil, err
	}
	s.appendComment(ft, AuthorSystem, nil, audit, false, false)
	if err := s.save(ft); err != nil {
		return nil, err
	}
	return bareTicket(ft), nil
}

func (s *fileTicketStore) DeleteTicket(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	dir, ok := s.dirFor(id)
	if !ok {
		return false
	}
	return os.RemoveAll(dir) == nil
}

// ─── comments ───────────────────────────────────────────────────

// appendComment adds a comment to ft in memory (caller saves). The PO-review
// rule (only user comments need moderation) lives in newComment.
func (s *fileTicketStore) appendComment(ft *fileTicket, authorType AuthorType, authorID *string, body string, internal, withheld bool) *TicketComment {
	c := newComment(ft.ID, authorType, authorID, body, internal, withheld)
	ft.Comments = append(ft.Comments, c)
	ft.UpdatedAt = c.CreatedAt
	return c
}

func (s *fileTicketStore) AddComment(ticketID string, authorType AuthorType, authorID *string, body string, internal bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil
	}
	c := s.appendComment(ft, authorType, authorID, body, internal, false)
	if err := s.save(ft); err != nil {
		return nil
	}
	cc := *c
	return &cc
}

func (s *fileTicketStore) AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil
	}
	// po/dev/system have no user id and are inherently reviewed (newComment rule).
	c := s.appendComment(ft, authorType, nil, body, internal, withheld)
	if err := s.save(ft); err != nil {
		return nil
	}
	cc := *c
	return &cc
}

func (s *fileTicketStore) ListComments(ticketID string) []*TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil
	}
	return cloneComments(ft.Comments)
}

func (s *fileTicketStore) GetComment(ticketID, commentID string) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil
	}
	for _, c := range ft.Comments {
		if c.ID == commentID {
			cc := *c
			return &cc
		}
	}
	return nil
}

func (s *fileTicketStore) SetCommentReviewed(commentID string, withheld bool) *TicketComment {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, ft := range s.allTickets() {
		for _, c := range ft.Comments {
			if c.ID == commentID {
				c.POReviewed = true
				if withheld {
					c.Withheld = true
				}
				if err := s.save(ft); err != nil {
					return nil
				}
				cc := *c
				return &cc
			}
		}
	}
	return nil
}

func (s *fileTicketStore) ListTicketsWithUnreviewedComments() []*Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []*Ticket
	for _, ft := range s.allTickets() {
		has := false
		for _, c := range ft.Comments {
			if c.AuthorType == AuthorUser && !c.POReviewed {
				has = true
				break
			}
		}
		if has {
			out = append(out, hydratedTicket(ft))
		}
	}
	sortTicketsDesc(out)
	return out
}

// ─── artifacts ──────────────────────────────────────────────────

// artifactFilename picks the canonical on-disk name for an artifact (§3.1). For
// screenshot/fdraft/actions/settings/ui_state the kind fixes the name; for a
// plain attachment the (already-sanitized) Name is used.
func artifactFilename(kind ArtifactKind, name string) string {
	switch kind {
	case ArtifactScreenshot:
		return "screenshot.png"
	case ArtifactFdraft:
		return "project.fdraft"
	case ArtifactActions:
		return "actions.json"
	case ArtifactSettings:
		return "settings.json"
	case ArtifactUIState:
		return "ui_state.json"
	default:
		return name
	}
}

func (s *fileTicketStore) AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact {
	if len(data) == 0 {
		return nil
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil
	}
	dir, _ := s.dirFor(ticketID)
	fname := artifactFilename(kind, name)
	if err := os.WriteFile(filepath.Join(dir, fname), data, 0o644); err != nil {
		return nil
	}
	sum := sha256.Sum256(data)
	a := &TicketArtifact{
		ID:        newID(),
		TicketID:  ticketID,
		Kind:      kind,
		BlobRef:   hex.EncodeToString(sum[:]),
		Mime:      mime,
		Size:      int64(len(data)),
		Name:      fname,
		CreatedAt: now(),
	}
	ft.Artifacts = append(ft.Artifacts, a)
	if err := s.save(ft); err != nil {
		return nil
	}
	aa := *a
	return &aa
}

func (s *fileTicketStore) GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil, nil
	}
	dir, _ := s.dirFor(ticketID)
	for _, a := range ft.Artifacts {
		if a.ID == artifactID {
			data, err := os.ReadFile(filepath.Join(dir, a.Name))
			if err != nil {
				aa := *a
				return &aa, nil
			}
			aa := *a
			return &aa, data
		}
	}
	return nil, nil
}

// ─── work-orders ────────────────────────────────────────────────

func (s *fileTicketStore) CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil {
		return nil, errTicketMissing
	}
	if ft.WorkOrder != nil {
		return nil, errWorkOrderExists
	}
	wo := &WorkOrder{
		TicketID:  ticketID,
		Payload:   append(json.RawMessage(nil), payload...),
		CreatedAt: now(),
	}
	ft.WorkOrder = wo
	if err := s.save(ft); err != nil {
		return nil, err
	}
	return cloneWorkOrder(wo), nil
}

func (s *fileTicketStore) GetWorkOrder(ticketID string) *WorkOrder {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil || ft.WorkOrder == nil {
		return nil
	}
	return cloneWorkOrder(ft.WorkOrder)
}

func (s *fileTicketStore) ClaimWorkOrder(ticketID string) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil || ft.WorkOrder == nil {
		return nil, errWorkOrderMissing
	}
	if ft.WorkOrder.PickedUpAt != nil {
		return nil, errWorkOrderClaimed
	}
	at := now()
	ft.WorkOrder.PickedUpAt = &at
	if err := s.save(ft); err != nil {
		return nil, err
	}
	return cloneWorkOrder(ft.WorkOrder), nil
}

func (s *fileTicketStore) CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	ft := s.load(ticketID)
	if ft == nil || ft.WorkOrder == nil {
		return nil, errWorkOrderMissing
	}
	if ft.WorkOrder.CompletedAt != nil {
		return nil, errWorkOrderCompleted
	}
	if deployID != "" && !s.hasDeploy(deployID) {
		return nil, errDeployMissing
	}
	at := now()
	ft.WorkOrder.CompletedAt = &at
	if deployID != "" {
		d := deployID
		ft.WorkOrder.DeployID = &d
	}
	if err := s.save(ft); err != nil {
		return nil, err
	}
	return cloneWorkOrder(ft.WorkOrder), nil
}

func (s *fileTicketStore) ListAcceptedWithOpenWorkOrder() []*Ticket {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []*Ticket
	for _, ft := range s.allTickets() {
		if ft.State != StateAccepted {
			continue
		}
		if ft.WorkOrder == nil || !ft.WorkOrder.Open() {
			continue
		}
		out = append(out, hydratedTicket(ft))
	}
	sortTicketsDesc(out)
	return out
}

// ─── deploys ────────────────────────────────────────────────────
//
// Deploys are process-global (not ticket-scoped), persisted to
// <base>/deploys.json in insertion order.

func (s *fileTicketStore) deploysPath() string { return filepath.Join(s.base, "deploys.json") }

func (s *fileTicketStore) loadDeploys() []*Deploy {
	raw, err := os.ReadFile(s.deploysPath())
	if err != nil {
		return nil
	}
	var ds []*Deploy
	if err := json.Unmarshal(raw, &ds); err != nil {
		return nil
	}
	return ds
}

func (s *fileTicketStore) saveDeploys() error {
	raw, err := json.MarshalIndent(s.deploys, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(s.deploysPath(), raw, 0o644)
}

func (s *fileTicketStore) hasDeploy(id string) bool {
	for _, d := range s.deploys {
		if d.ID == id {
			return true
		}
	}
	return false
}

func (s *fileTicketStore) CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error) {
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
	s.deploys = append(s.deploys, d)
	if err := s.saveDeploys(); err != nil {
		return nil, err
	}
	return cloneDeploy(d), nil
}

func (s *fileTicketStore) ListDeploys() []*Deploy {
	s.mu.Lock()
	defer s.mu.Unlock()
	// Newest first.
	out := make([]*Deploy, 0, len(s.deploys))
	for i := len(s.deploys) - 1; i >= 0; i-- {
		out = append(out, cloneDeploy(s.deploys[i]))
	}
	return out
}
