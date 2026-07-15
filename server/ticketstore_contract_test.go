// Contract suite for the TicketStore business rules (WP15).
//
// It runs against the in-memory store (ticketstore_mem.go, the test double) and
// pins the SHARED behavior the ticket domain must have whatever backs it — state
// machine + audit comments, PO-review rule, verdict semantics, work-order
// lifecycle, literal-substring search, UTC timestamps. Since ticket #346 the one
// PRODUCTION backend is IssueVault; its own adapter behavior (scoping, lenient
// meta, status mapping) lives in store_issuevault_test.go.
//
// The sqlStore run shares a database with other tests/runs, so every list
// assertion filters by a per-fixture author id or checks containment instead of
// exact global counts.

package main

import (
	"encoding/json"
	"testing"
	"time"
)

// ticketStoreFixture is the store-under-test plus a factory for valid ticket
// author ids.
type ticketStoreFixture struct {
	store     TicketStore
	newAuthor func(t *testing.T) string
}

func memFixture(t *testing.T) ticketStoreFixture {
	t.Helper()
	store := NewStore()
	return ticketStoreFixture{
		store: store.(TicketStore),
		newAuthor: func(t *testing.T) string {
			t.Helper()
			u, err := store.CreateUser("author-"+newID()+"@test.com", "A", "pass12345", RoleUser)
			if err != nil {
				t.Fatalf("CreateUser: %v", err)
			}
			return u.ID
		},
	}
}

func TestTicketStoreContractMem(t *testing.T) { testTicketStoreContract(t, memFixture) }

// mkTicket creates a ticket and registers cleanup (keeps a shared test DB tidy).
func mkTicket(t *testing.T, f ticketStoreFixture, author, title, body, category string) *Ticket {
	t.Helper()
	tk := f.store.CreateTicket(author, title, body, category)
	if tk == nil {
		t.Fatal("CreateTicket returned nil")
	}
	t.Cleanup(func() { f.store.DeleteTicket(tk.ID) })
	return tk
}

func ticketIDs(ts []*Ticket) map[string]bool {
	out := map[string]bool{}
	for _, tk := range ts {
		out[tk.ID] = true
	}
	return out
}

// testTicketStoreContract is the shared behavior contract. newFixture is called
// per subtest so every case runs on an isolated store (sql: same DB, fresh
// author ids).
func testTicketStoreContract(t *testing.T, newFixture func(t *testing.T) ticketStoreFixture) {
	t.Run("CreateAndGet", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "Crash", "it crashes", "bug")
		if tk.State != StateTriage {
			t.Fatalf("initial state = %q, want triage", tk.State)
		}
		if tk.Category == nil || *tk.Category != "bug" {
			t.Fatalf("category = %v, want bug", tk.Category)
		}
		// Timestamps are UTC (divergence b: memStore used local time.Now()).
		if tk.CreatedAt.Location() != time.UTC || tk.UpdatedAt.Location() != time.UTC {
			t.Fatalf("timestamps not UTC: %v / %v", tk.CreatedAt.Location(), tk.UpdatedAt.Location())
		}
		// Create returns a bare list-row shape.
		if tk.Comments != nil || tk.Artifacts != nil {
			t.Fatal("CreateTicket must return a bare ticket (no thread/artifacts)")
		}
		got := f.store.GetTicket(tk.ID)
		if got == nil || got.Title != "Crash" || got.Body != "it crashes" || got.AuthorID != author {
			t.Fatalf("GetTicket mismatch: %+v", got)
		}
		if f.store.GetTicket("does-not-exist") != nil {
			t.Fatal("GetTicket of unknown id should be nil")
		}
		// No category → nil.
		none := mkTicket(t, f, author, "X", "y", "")
		if none.Category != nil {
			t.Fatalf("category = %v, want nil", none.Category)
		}
	})

	t.Run("UpdateAndCategory", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "Old", "old body", "")
		upd := f.store.UpdateTicket(tk.ID, "New", "new body")
		if upd == nil || upd.Title != "New" || upd.Body != "new body" {
			t.Fatalf("UpdateTicket: %+v", upd)
		}
		// Empty fields keep the old values.
		upd = f.store.UpdateTicket(tk.ID, "", "")
		if upd.Title != "New" || upd.Body != "new body" {
			t.Fatalf("empty update overwrote fields: %+v", upd)
		}
		if f.store.UpdateTicket("does-not-exist", "a", "b") != nil {
			t.Fatal("UpdateTicket of unknown id should be nil")
		}
		// Set + clear category.
		if c := f.store.SetTicketCategory(tk.ID, "feature"); c == nil || c.Category == nil || *c.Category != "feature" {
			t.Fatalf("SetTicketCategory: %+v", c)
		}
		if c := f.store.SetTicketCategory(tk.ID, ""); c == nil || c.Category != nil {
			t.Fatalf("clear category: %+v", c)
		}
	})

	t.Run("ListFilters", func(t *testing.T) {
		f := newFixture(t)
		me := f.newAuthor(t)
		other := f.newAuthor(t)
		mine := mkTicket(t, f, me, "Alpha bug title", "alpha body", "bug")
		theirs := mkTicket(t, f, other, "Beta feature", "beta body has needle", "feature")

		// Author filter.
		if got := f.store.ListTickets(TicketFilter{AuthorID: me}); len(got) != 1 || got[0].ID != mine.ID {
			t.Fatalf("author filter: got %d, want 1 (mine)", len(got))
		}
		// State + category filters (author-scoped so a shared DB can't interfere).
		if got := f.store.ListTickets(TicketFilter{AuthorID: me, State: StateTriage}); len(got) != 1 {
			t.Fatalf("state filter: got %d, want 1", len(got))
		}
		if got := f.store.ListTickets(TicketFilter{AuthorID: other, Category: "feature"}); len(got) != 1 || got[0].ID != theirs.ID {
			t.Fatalf("category filter: got %d, want 1 (theirs)", len(got))
		}
		// Free-text query is case-insensitive over title/body.
		if got := f.store.ListTickets(TicketFilter{AuthorID: other, Query: "NEEDLE"}); len(got) != 1 {
			t.Fatalf("q filter body: got %d, want 1", len(got))
		}
		if got := f.store.ListTickets(TicketFilter{AuthorID: me, Query: "alpha BUG"}); len(got) != 1 {
			t.Fatalf("q filter title: got %d, want 1", len(got))
		}
	})

	t.Run("ListQueryLiteralSubstring", func(t *testing.T) {
		// Divergence c: sqlStore's LIKE must escape %/_ so the query is a literal
		// substring — parity with strings.Contains in mem/file.
		f := newFixture(t)
		author := f.newAuthor(t)
		percent := mkTicket(t, f, author, "discount", "sale is 100% off today", "")
		underscore := mkTicket(t, f, author, "snake", "uses snake_case naming", "")
		mkTicket(t, f, author, "plain", "uses snakeXcase naming", "")

		if got := f.store.ListTickets(TicketFilter{AuthorID: author, Query: "100% off"}); len(got) != 1 || got[0].ID != percent.ID {
			t.Fatalf("literal %% query: got %d, want exactly the percent ticket", len(got))
		}
		// An unescaped LIKE would treat _ as a wildcard and also match snakeXcase.
		if got := f.store.ListTickets(TicketFilter{AuthorID: author, Query: "snake_case"}); len(got) != 1 || got[0].ID != underscore.ID {
			t.Fatalf("literal _ query: got %d, want exactly the underscore ticket", len(got))
		}
		// A metacharacter-only query matches nothing literal.
		if got := f.store.ListTickets(TicketFilter{AuthorID: author, Query: "sn%ng"}); len(got) != 0 {
			t.Fatalf("wildcard-style query must not match: got %d", len(got))
		}
	})

	t.Run("RejectedVisibility", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "spammy", "buy now", "")
		if _, err := f.store.SetTicketState(tk.ID, StateRejected, false); err != nil {
			t.Fatalf("reject: %v", err)
		}
		// A stranger doesn't see it.
		if ticketIDs(f.store.ListTickets(TicketFilter{ViewerID: "stranger"}))[tk.ID] {
			t.Fatal("rejected ticket leaked to a stranger")
		}
		// The author sees their own rejected ticket; an admin sees it too.
		if !ticketIDs(f.store.ListTickets(TicketFilter{ViewerID: author}))[tk.ID] {
			t.Fatal("author cannot see own rejected ticket")
		}
		if !ticketIDs(f.store.ListTickets(TicketFilter{ViewerIsAdmin: true}))[tk.ID] {
			t.Fatal("admin cannot see rejected ticket")
		}
	})

	t.Run("StateTransitions", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "t", "b", "")

		// Legal move appends a system audit comment.
		moved, err := f.store.SetTicketState(tk.ID, StateInProgress, false)
		if err != nil || moved.State != StateInProgress {
			t.Fatalf("legal transition: %v / %+v", err, moved)
		}
		cs := f.store.ListComments(tk.ID)
		if len(cs) != 1 || cs[0].AuthorType != AuthorSystem || cs[0].Body != "state: triage → in_progress" {
			t.Fatalf("audit comment: %+v", cs)
		}
		// The audit comment is born reviewed (PO-review rule).
		if !cs[0].POReviewed {
			t.Fatal("system audit comment must be po_reviewed")
		}

		// Illegal move / unknown state / no-op / missing ticket.
		if _, err := f.store.SetTicketState(tk.ID, StateTriage, false); err != errInvalidTransition {
			t.Fatalf("illegal transition err = %v, want errInvalidTransition", err)
		}
		if _, err := f.store.SetTicketState(tk.ID, TicketState("bogus"), false); err != errUnknownState {
			t.Fatalf("unknown state err = %v, want errUnknownState", err)
		}
		if _, err := f.store.SetTicketState(tk.ID, StateInProgress, false); err != errInvalidTransition {
			t.Fatalf("no-op transition err = %v, want errInvalidTransition", err)
		}
		if _, err := f.store.SetTicketState("does-not-exist", StateInProgress, false); err != errTicketMissing {
			t.Fatalf("missing ticket err = %v, want errTicketMissing", err)
		}
		// Failed moves left no extra audit comments and no state change.
		if got := f.store.GetTicket(tk.ID); got.State != StateInProgress || len(got.Comments) != 1 {
			t.Fatalf("state/comments after rejected moves: %+v", got)
		}
	})

	t.Run("POVerdict", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "t", "b", "")

		fit := "fit"
		out, err := f.store.SetPOVerdict(tk.ID, POVerdict{VisionFit: &fit, Reason: "legit", State: StateInProgress})
		if err != nil || out.State != StateInProgress {
			t.Fatalf("SetPOVerdict: %v / %+v", err, out)
		}
		full := f.store.GetTicket(tk.ID)
		if full.POVisionFit == nil || *full.POVisionFit != "fit" {
			t.Fatalf("po_vision_fit = %v, want fit", full.POVisionFit)
		}
		if full.POReason == nil || *full.POReason != "legit" {
			t.Fatalf("po_reason = %v, want legit", full.POReason)
		}
		if full.POVerdictAt == nil {
			t.Fatal("po_verdict_at not set")
		}
		// Audit comment.
		cs := f.store.ListComments(tk.ID)
		if len(cs) != 1 || cs[0].Body != "po verdict: triage → in_progress" {
			t.Fatalf("verdict audit comment: %+v", cs)
		}
		// A second verdict with an empty reason clears the previous one
		// (divergence d: sql did, mem/file kept the stale value).
		if _, err := f.store.SetPOVerdict(tk.ID, POVerdict{State: StateRejected}); err != nil {
			t.Fatalf("second verdict: %v", err)
		}
		full = f.store.GetTicket(tk.ID)
		if full.POReason != nil {
			t.Fatalf("po_reason not cleared by empty-reason verdict: %v", *full.POReason)
		}
		// Illegal verdict target / missing ticket.
		if _, err := f.store.SetPOVerdict(tk.ID, POVerdict{State: StateClosed}); err != errInvalidTransition {
			t.Fatalf("illegal verdict err = %v", err)
		}
		if _, err := f.store.SetPOVerdict("does-not-exist", POVerdict{State: StateInProgress}); err != errTicketMissing {
			t.Fatalf("missing ticket verdict err = %v", err)
		}
	})

	t.Run("Comments", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "t", "b", "")

		// User comment: unreviewed, carries the author id, bumps updated_at.
		uc := f.store.AddComment(tk.ID, AuthorUser, &author, "user says hi", false)
		if uc == nil || uc.POReviewed || uc.AuthorID == nil || *uc.AuthorID != author {
			t.Fatalf("user comment: %+v", uc)
		}
		// Agent comments: no author id, inherently reviewed; withheld/internal kept.
		dev := f.store.AddAgentComment(tk.ID, AuthorDev, "internal note", true, false)
		if dev == nil || !dev.Internal || !dev.POReviewed || dev.AuthorID != nil {
			t.Fatalf("dev note: %+v", dev)
		}
		po := f.store.AddAgentComment(tk.ID, AuthorPO, "held reply", false, true)
		if po == nil || !po.Withheld || !po.POReviewed {
			t.Fatalf("withheld po comment: %+v", po)
		}
		// Unknown ticket → nil.
		if f.store.AddComment("does-not-exist", AuthorUser, &author, "x", false) != nil {
			t.Fatal("AddComment on unknown ticket should be nil")
		}
		// The thread carries all three comments. (No strict-order assertion: the
		// DB column is second-precision DATETIME, so same-second inserts have no
		// defined order.)
		list := f.store.ListComments(tk.ID)
		if len(list) != 3 {
			t.Fatalf("thread size: %d, want 3", len(list))
		}
		// GetComment is ticket-scoped.
		if f.store.GetComment(tk.ID, uc.ID) == nil {
			t.Fatal("GetComment missed an existing comment")
		}
		if f.store.GetComment("other-ticket", uc.ID) != nil {
			t.Fatal("GetComment matched a comment on the wrong ticket")
		}

		// Post-moderation: the ticket surfaces while uc is unreviewed, and drops
		// out once it is reviewed+withheld.
		if !ticketIDs(f.store.ListTicketsWithUnreviewedComments())[tk.ID] {
			t.Fatal("ticket with unreviewed user comment not surfaced")
		}
		// SetCommentReviewed is ticket-scoped too: a wrong ticketID must not moderate it.
		if f.store.SetCommentReviewed("other-ticket", uc.ID, true) != nil {
			t.Fatal("SetCommentReviewed moderated a comment via the wrong ticket")
		}
		mod := f.store.SetCommentReviewed(tk.ID, uc.ID, true)
		if mod == nil || !mod.POReviewed || !mod.Withheld {
			t.Fatalf("SetCommentReviewed: %+v", mod)
		}
		if ticketIDs(f.store.ListTicketsWithUnreviewedComments())[tk.ID] {
			t.Fatal("ticket still surfaced after its only user comment was reviewed")
		}
	})

	t.Run("Artifacts", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "t", "b", "")

		png := []byte("PNGDATA-bytes")
		a := f.store.AddArtifact(tk.ID, ArtifactScreenshot, "screenshot.png", "image/png", png)
		if a == nil || a.Size != int64(len(png)) || a.Name != "screenshot.png" {
			t.Fatalf("AddArtifact: %+v", a)
		}
		gotA, data := f.store.GetArtifactBlob(tk.ID, a.ID)
		if gotA == nil || string(data) != string(png) {
			t.Fatalf("GetArtifactBlob: %q", data)
		}
		// Empty data is skipped; unknown ids are nil.
		if f.store.AddArtifact(tk.ID, ArtifactActions, "actions.json", "application/json", nil) != nil {
			t.Fatal("empty artifact should be skipped")
		}
		if art, _ := f.store.GetArtifactBlob(tk.ID, "does-not-exist"); art != nil {
			t.Fatal("unknown artifact should be nil")
		}
		// GetTicket hydrates artifacts.
		if full := f.store.GetTicket(tk.ID); len(full.Artifacts) != 1 {
			t.Fatalf("hydrated artifacts: %d, want 1", len(full.Artifacts))
		}
	})

	t.Run("WorkOrderLifecycle", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "t", "b", "")
		if _, err := f.store.SetTicketState(tk.ID, StateInProgress, false); err != nil {
			t.Fatalf("accept: %v", err)
		}

		payload := json.RawMessage(`{"type":"bug","summary":"fix"}`)
		if _, err := f.store.CreateWorkOrder("does-not-exist", payload); err != errTicketMissing {
			t.Fatalf("wo on missing ticket err = %v", err)
		}
		wo, err := f.store.CreateWorkOrder(tk.ID, payload)
		if err != nil || wo.TicketID != tk.ID || string(wo.Payload) != string(payload) {
			t.Fatalf("CreateWorkOrder: %v / %+v", err, wo)
		}
		if _, err := f.store.CreateWorkOrder(tk.ID, payload); err != errWorkOrderExists {
			t.Fatalf("dup work-order err = %v", err)
		}
		if got := f.store.GetWorkOrder(tk.ID); got == nil || !got.Open() {
			t.Fatalf("GetWorkOrder: %+v", got)
		}
		// In the coding queue (hydrated).
		q := f.store.ListAcceptedWithOpenWorkOrder()
		if !ticketIDs(q)[tk.ID] {
			t.Fatal("accepted ticket with open WO not in queue")
		}
		for _, item := range q {
			if item.ID == tk.ID && item.Comments == nil {
				t.Fatal("queue ticket should be hydrated with comments")
			}
		}

		// Claim once; a second claim conflicts; claiming a missing WO is a 404.
		claimed, err := f.store.ClaimWorkOrder(tk.ID)
		if err != nil || claimed.PickedUpAt == nil {
			t.Fatalf("ClaimWorkOrder: %v / %+v", err, claimed)
		}
		if _, err := f.store.ClaimWorkOrder(tk.ID); err != errWorkOrderClaimed {
			t.Fatalf("double claim err = %v", err)
		}
		if _, err := f.store.ClaimWorkOrder("does-not-exist"); err != errWorkOrderMissing {
			t.Fatalf("claim missing err = %v", err)
		}

		// Complete with an unknown deploy → clean 404 (divergence a: sql used to
		// hit the FK and return a 500).
		if _, err := f.store.CompleteWorkOrder(tk.ID, "no-such-deploy"); err != errDeployMissing {
			t.Fatalf("complete unknown deploy err = %v, want errDeployMissing", err)
		}
		dep, err := f.store.CreateDeploy("1.0.0", "abc123", []string{tk.ID}, DeployLive)
		if err != nil {
			t.Fatalf("CreateDeploy: %v", err)
		}
		done, err := f.store.CompleteWorkOrder(tk.ID, dep.ID)
		if err != nil || done.CompletedAt == nil || done.DeployID == nil || *done.DeployID != dep.ID {
			t.Fatalf("CompleteWorkOrder: %v / %+v", err, done)
		}
		if done.Open() {
			t.Fatal("completed work-order should not be Open()")
		}
		// A second complete is a conflict — completed_at / the deploy link are
		// never silently overwritten.
		if _, err := f.store.CompleteWorkOrder(tk.ID, ""); err != errWorkOrderCompleted {
			t.Fatalf("double complete err = %v, want errWorkOrderCompleted", err)
		}
		if after := f.store.GetWorkOrder(tk.ID); !after.CompletedAt.Equal(*done.CompletedAt) || *after.DeployID != dep.ID {
			t.Fatalf("double complete mutated the work-order: %+v", after)
		}
		if _, err := f.store.CompleteWorkOrder("does-not-exist", ""); err != errWorkOrderMissing {
			t.Fatalf("complete missing err = %v", err)
		}
		// Out of the queue now.
		if ticketIDs(f.store.ListAcceptedWithOpenWorkOrder())[tk.ID] {
			t.Fatal("completed work-order still in queue")
		}
		// Deleting the ticket takes the work-order with it.
		if !f.store.DeleteTicket(tk.ID) {
			t.Fatal("DeleteTicket returned false")
		}
		if f.store.GetWorkOrder(tk.ID) != nil {
			t.Fatal("work-order survived ticket delete")
		}
	})

	t.Run("Deploys", func(t *testing.T) {
		f := newFixture(t)
		d1, err := f.store.CreateDeploy("1.0", "sha1", nil, DeployBuilding)
		if err != nil || d1 == nil {
			t.Fatalf("CreateDeploy 1: %v", err)
		}
		d2, err := f.store.CreateDeploy("1.1", "sha2", []string{"tid"}, DeployLive)
		if err != nil || d2 == nil {
			t.Fatalf("CreateDeploy 2: %v", err)
		}
		if len(d2.TicketIDs) != 1 || d2.TicketIDs[0] != "tid" {
			t.Fatalf("deploy ticket_ids: %+v", d2.TicketIDs)
		}
		// Both deploys are listed. (No order assertion: sqlStore sorts by the
		// second-precision deployed_at with an id tie-break, so same-second
		// inserts have no portable order across backends.)
		found1, found2 := false, false
		for _, d := range f.store.ListDeploys() {
			switch d.ID {
			case d1.ID:
				found1 = true
			case d2.ID:
				found2 = true
			}
		}
		if !found1 || !found2 {
			t.Fatalf("deploys missing from list: d1=%v d2=%v", found1, found2)
		}
	})

	t.Run("Delete", func(t *testing.T) {
		f := newFixture(t)
		author := f.newAuthor(t)
		tk := mkTicket(t, f, author, "t", "b", "")
		if !f.store.DeleteTicket(tk.ID) {
			t.Fatal("DeleteTicket returned false")
		}
		if f.store.GetTicket(tk.ID) != nil {
			t.Fatal("ticket still present after delete")
		}
		if f.store.DeleteTicket(tk.ID) {
			t.Fatal("DeleteTicket of a gone ticket should be false")
		}
	})
}
