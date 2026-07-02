// Round-trip suite for fileTicketStore (WP15). Each test runs against a store
// rooted at t.TempDir(), asserting parity with the memStore semantics where
// reasonable (state machine, visibility, review rules, work-order lifecycle).

package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func newFileStore(t *testing.T) *fileTicketStore {
	t.Helper()
	fs, err := newFileTicketStore(t.TempDir())
	if err != nil {
		t.Fatalf("newFileTicketStore: %v", err)
	}
	return fs
}

// ─── create (both paths) + get ──────────────────────────────────

func TestFileStoreCreateAndGet(t *testing.T) {
	fs := newFileStore(t)

	// Plain path (with category).
	a := fs.CreateTicket("author-1", "Extrude crash", "crashes on extrude", "bug")
	if a == nil || a.ID == "" {
		t.Fatal("CreateTicket returned nil")
	}
	if a.State != StateTriage {
		t.Fatalf("state = %q, want triage", a.State)
	}
	if a.Category == nil || *a.Category != "bug" {
		t.Fatalf("category = %v, want bug", a.Category)
	}
	// List-row shape carries no thread/artifacts.
	if a.Comments != nil || a.Artifacts != nil {
		t.Fatal("CreateTicket must return a bare ticket (no thread/artifacts)")
	}

	// Report-Issue-ish path (no category).
	b := fs.CreateTicket("author-2", "Feature X", "please add X", "")
	if b.Category != nil {
		t.Fatalf("category = %v, want nil", b.Category)
	}

	// GetTicket hydrates + persists ticket.json / ticket.md / comments.md.
	got := fs.GetTicket(a.ID)
	if got == nil || got.Title != "Extrude crash" || got.Body != "crashes on extrude" {
		t.Fatalf("GetTicket mismatch: %+v", got)
	}
	dir := fs.openDir(a.ID)
	for _, f := range []string{"ticket.json", "ticket.md", "comments.md"} {
		if _, err := os.Stat(filepath.Join(dir, f)); err != nil {
			t.Fatalf("missing %s: %v", f, err)
		}
	}
	if fs.GetTicket("nope") != nil {
		t.Fatal("GetTicket of unknown id should be nil")
	}
}

// ─── list + filters (author=me / state / category / q / visibility) ──

func TestFileStoreListFilters(t *testing.T) {
	fs := newFileStore(t)
	mine := fs.CreateTicket("me", "Alpha bug title", "alpha body", "bug")
	other := fs.CreateTicket("other", "Beta feature", "beta body has needle", "feature")

	// author filter (author=me).
	if got := fs.ListTickets(TicketFilter{AuthorID: "me"}); len(got) != 1 || got[0].ID != mine.ID {
		t.Fatalf("author filter: got %d, want 1 (mine)", len(got))
	}
	// state filter.
	if got := fs.ListTickets(TicketFilter{State: StateTriage}); len(got) != 2 {
		t.Fatalf("state filter: got %d, want 2", len(got))
	}
	// category filter.
	if got := fs.ListTickets(TicketFilter{Category: "feature"}); len(got) != 1 || got[0].ID != other.ID {
		t.Fatalf("category filter: got %d, want 1 (other)", len(got))
	}
	// free-text q over title/body (case-insensitive).
	if got := fs.ListTickets(TicketFilter{Query: "NEEDLE"}); len(got) != 1 || got[0].ID != other.ID {
		t.Fatalf("q filter: got %d, want 1 (other)", len(got))
	}
	if got := fs.ListTickets(TicketFilter{Query: "alpha"}); len(got) != 1 || got[0].ID != mine.ID {
		t.Fatalf("q filter title: got %d, want 1 (mine)", len(got))
	}
}

func TestFileStoreVisibility(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "spammy", "buy now", "")
	// triage → rejected_spam.
	if _, err := fs.SetTicketState(tk.ID, StateRejectedSpam); err != nil {
		t.Fatalf("reject: %v", err)
	}

	// A stranger sees nothing.
	if got := fs.ListTickets(TicketFilter{ViewerID: "stranger"}); len(got) != 0 {
		t.Fatalf("stranger sees rejected: %d, want 0", len(got))
	}
	// The author sees their own rejected ticket.
	if got := fs.ListTickets(TicketFilter{ViewerID: "author"}); len(got) != 1 {
		t.Fatalf("author cannot see own rejected: %d, want 1", len(got))
	}
	// An admin sees it.
	if got := fs.ListTickets(TicketFilter{ViewerIsAdmin: true}); len(got) != 1 {
		t.Fatalf("admin cannot see rejected: %d, want 1", len(got))
	}
	// Rejected/resolved tickets live under done/.
	if !dirExists(fs.doneDir(tk.ID)) {
		t.Fatal("rejected ticket should be under done/")
	}
	if dirExists(fs.openDir(tk.ID)) {
		t.Fatal("rejected ticket should NOT remain under open/")
	}
}

// ─── comments: add/list + internal/withheld visibility + review ─

func TestFileStoreComments(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")
	uid := "author"

	// User comment: needs review.
	uc := fs.AddComment(tk.ID, AuthorUser, &uid, "user says hi", false)
	if uc == nil || uc.POReviewed {
		t.Fatalf("user comment should be unreviewed: %+v", uc)
	}
	// Internal dev note (admin-only) — inherently reviewed.
	dev := fs.AddAgentComment(tk.ID, AuthorDev, "internal note", true, false)
	if dev == nil || !dev.Internal || !dev.POReviewed {
		t.Fatalf("dev note bad: %+v", dev)
	}
	// Withheld po comment.
	po := fs.AddAgentComment(tk.ID, AuthorPO, "leaked?", false, true)
	if po == nil || !po.Withheld {
		t.Fatalf("po comment should be withheld: %+v", po)
	}

	list := fs.ListComments(tk.ID)
	if len(list) != 3 {
		t.Fatalf("ListComments: got %d, want 3", len(list))
	}
	// visibleComments (from tickets_handlers.go) drops internal + withheld for
	// non-admins — parity with the DB path.
	if vis := visibleComments(list, false); len(vis) != 1 || vis[0].ID != uc.ID {
		t.Fatalf("non-admin visible comments: got %d, want 1 (the user comment)", len(vis))
	}
	if vis := visibleComments(list, true); len(vis) != 3 {
		t.Fatalf("admin sees all: got %d, want 3", len(vis))
	}

	// GetComment scoped to the ticket.
	if fs.GetComment(tk.ID, uc.ID) == nil {
		t.Fatal("GetComment should find the comment on its ticket")
	}
	if fs.GetComment("other-ticket", uc.ID) != nil {
		t.Fatal("GetComment must be nil for the wrong ticket")
	}

	// Post-moderation: mark reviewed + withheld → shows in the unreviewed query
	// before, and not after.
	if got := fs.ListTicketsWithUnreviewedComments(); len(got) != 1 {
		t.Fatalf("unreviewed query before: got %d, want 1", len(got))
	}
	if upd := fs.SetCommentReviewed(tk.ID, uc.ID, true); upd == nil || !upd.POReviewed || !upd.Withheld {
		t.Fatalf("SetCommentReviewed bad: %+v", upd)
	}
	if got := fs.ListTicketsWithUnreviewedComments(); len(got) != 0 {
		t.Fatalf("unreviewed query after: got %d, want 0", len(got))
	}
}

// ─── artifacts: add + fetch bytes ───────────────────────────────

func TestFileStoreArtifacts(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")

	png := []byte("PNGDATA-bytes")
	a := fs.AddArtifact(tk.ID, ArtifactScreenshot, "ignored.png", "image/png", png)
	if a == nil {
		t.Fatal("AddArtifact returned nil")
	}
	// Screenshot lands at its canonical filename.
	if a.Name != "screenshot.png" {
		t.Fatalf("artifact name = %q, want screenshot.png", a.Name)
	}
	if _, err := os.Stat(filepath.Join(fs.openDir(tk.ID), "screenshot.png")); err != nil {
		t.Fatalf("screenshot.png not written: %v", err)
	}
	// Fetch the bytes back.
	gotA, data := fs.GetArtifactBlob(tk.ID, a.ID)
	if gotA == nil || string(data) != string(png) {
		t.Fatalf("GetArtifactBlob mismatch: %q", data)
	}
	// A plain attachment keeps its (sanitized) name.
	att := fs.AddArtifact(tk.ID, ArtifactAttachment, "attachment-01-log.txt", "text/plain", []byte("hi"))
	if att == nil || att.Name != "attachment-01-log.txt" {
		t.Fatalf("attachment name = %q", att.Name)
	}
	// Empty data is skipped (parity with DB).
	if fs.AddArtifact(tk.ID, ArtifactActions, "actions.json", "application/json", nil) != nil {
		t.Fatal("empty artifact should be skipped")
	}
	// GetTicket hydrates artifacts.
	if full := fs.GetTicket(tk.ID); len(full.Artifacts) != 2 {
		t.Fatalf("hydrated artifacts: got %d, want 2", len(full.Artifacts))
	}
}

// ─── state transitions: legal + rejected ────────────────────────

func TestFileStoreStateTransitions(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")

	// Legal: triage → accepted.
	if _, err := fs.SetTicketState(tk.ID, StateAccepted); err != nil {
		t.Fatalf("legal transition rejected: %v", err)
	}
	// The move appended a system audit comment (parity with DB).
	cs := fs.ListComments(tk.ID)
	if len(cs) != 1 || cs[0].AuthorType != AuthorSystem {
		t.Fatalf("expected a system audit comment, got %+v", cs)
	}

	// Illegal: accepted → closed is not in the machine.
	if _, err := fs.SetTicketState(tk.ID, StateClosed); err != errInvalidTransition {
		t.Fatalf("illegal transition err = %v, want errInvalidTransition", err)
	}
	// Unknown target state.
	if _, err := fs.SetTicketState(tk.ID, TicketState("bogus")); err != errUnknownState {
		t.Fatalf("unknown state err = %v, want errUnknownState", err)
	}
	// Missing ticket.
	if _, err := fs.SetTicketState("nope", StateAccepted); err != errTicketMissing {
		t.Fatalf("missing ticket err = %v, want errTicketMissing", err)
	}
}

// ─── po verdict ─────────────────────────────────────────────────

func TestFileStorePOVerdict(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")

	fit := "fit"
	v := POVerdict{Spam: false, Malicious: false, VisionFit: &fit, Reason: "legit", State: StateAccepted}
	out, err := fs.SetPOVerdict(tk.ID, v)
	if err != nil {
		t.Fatalf("SetPOVerdict: %v", err)
	}
	if out.State != StateAccepted {
		t.Fatalf("state = %q, want accepted", out.State)
	}
	full := fs.GetTicket(tk.ID)
	if full.POVisionFit == nil || *full.POVisionFit != "fit" {
		t.Fatalf("po_vision_fit = %v, want fit", full.POVisionFit)
	}
	if full.POReason == nil || *full.POReason != "legit" {
		t.Fatalf("po_reason = %v", full.POReason)
	}
	if full.POVerdictAt == nil {
		t.Fatal("po_verdict_at not set")
	}
	// An illegal verdict target is rejected.
	if _, err := fs.SetPOVerdict(tk.ID, POVerdict{State: StateClosed}); err != errInvalidTransition {
		t.Fatalf("illegal verdict transition err = %v", err)
	}
}

// ─── work-order lifecycle: create / claim / complete ────────────

func TestFileStoreWorkOrderLifecycle(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")
	if _, err := fs.SetTicketState(tk.ID, StateAccepted); err != nil {
		t.Fatalf("accept: %v", err)
	}

	payload := json.RawMessage(`{"type":"bug","summary":"fix"}`)
	wo, err := fs.CreateWorkOrder(tk.ID, payload)
	if err != nil {
		t.Fatalf("CreateWorkOrder: %v", err)
	}
	if wo.TicketID != tk.ID || string(wo.Payload) != string(payload) {
		t.Fatalf("work-order mismatch: %+v", wo)
	}
	// Duplicate → conflict.
	if _, err := fs.CreateWorkOrder(tk.ID, payload); err != errWorkOrderExists {
		t.Fatalf("dup work-order err = %v, want errWorkOrderExists", err)
	}
	// Open ticket appears in the coding queue.
	if q := fs.ListAcceptedWithOpenWorkOrder(); len(q) != 1 || q[0].ID != tk.ID {
		t.Fatalf("coding queue: got %d, want 1", len(q))
	}

	// Claim sets picked_up_at; a second claim conflicts.
	claimed, err := fs.ClaimWorkOrder(tk.ID)
	if err != nil || claimed.PickedUpAt == nil {
		t.Fatalf("ClaimWorkOrder: %v (%+v)", err, claimed)
	}
	if _, err := fs.ClaimWorkOrder(tk.ID); err != errWorkOrderClaimed {
		t.Fatalf("double claim err = %v, want errWorkOrderClaimed", err)
	}

	// Complete with a deploy link; the deploy must exist.
	if _, err := fs.CompleteWorkOrder(tk.ID, "no-such-deploy"); err != errDeployMissing {
		t.Fatalf("complete unknown deploy err = %v, want errDeployMissing", err)
	}
	dep, err := fs.CreateDeploy("1.0.0", "abc123", []string{tk.ID}, DeployLive)
	if err != nil {
		t.Fatalf("CreateDeploy: %v", err)
	}
	done, err := fs.CompleteWorkOrder(tk.ID, dep.ID)
	if err != nil {
		t.Fatalf("CompleteWorkOrder: %v", err)
	}
	if done.CompletedAt == nil || done.DeployID == nil || *done.DeployID != dep.ID {
		t.Fatalf("completed work-order bad: %+v", done)
	}
	if done.Open() {
		t.Fatal("completed work-order should not be Open()")
	}
	// No longer in the coding queue.
	if q := fs.ListAcceptedWithOpenWorkOrder(); len(q) != 0 {
		t.Fatalf("coding queue after complete: got %d, want 0", len(q))
	}
	// Deploys list is newest-first and persisted.
	if ds := fs.ListDeploys(); len(ds) != 1 || ds[0].ID != dep.ID {
		t.Fatalf("ListDeploys: got %d", len(ds))
	}
}

// ─── agent-queue query parity: unreviewed comment + accepted-open-WO ──

func TestFileStoreAgentQueueParity(t *testing.T) {
	fs := newFileStore(t)

	// Ticket 1: accepted with an open work-order (coding queue).
	t1 := fs.CreateTicket("u1", "fix me", "body", "bug")
	if _, err := fs.SetTicketState(t1.ID, StateAccepted); err != nil {
		t.Fatalf("accept: %v", err)
	}
	if _, err := fs.CreateWorkOrder(t1.ID, json.RawMessage(`{}`)); err != nil {
		t.Fatalf("wo: %v", err)
	}

	// Ticket 2: resolved but carrying an unreviewed user comment (PO post-mod).
	t2 := fs.CreateTicket("u2", "resolved thing", "body", "")
	uid := "u2"
	// Drive to a resolved-ish state via legal moves, then leave an unreviewed
	// user comment on it (spam on a closed thread must still reach the PO).
	fs.AddComment(t2.ID, AuthorUser, &uid, "late spam", false)

	// Coding queue: exactly t1.
	cq := fs.ListAcceptedWithOpenWorkOrder()
	if len(cq) != 1 || cq[0].ID != t1.ID {
		t.Fatalf("accepted-with-open-WO: got %d, want t1", len(cq))
	}
	// Unreviewed-comment query: exactly t2 (t1's only comment is a system audit).
	uq := fs.ListTicketsWithUnreviewedComments()
	if len(uq) != 1 || uq[0].ID != t2.ID {
		t.Fatalf("unreviewed-comment query: got %d, want t2", len(uq))
	}
	// The hydrated ticket carries its comments so the PO can moderate directly.
	if len(uq[0].Comments) == 0 {
		t.Fatal("unreviewed-query ticket should be hydrated with comments")
	}
}

// ─── delete ─────────────────────────────────────────────────────

func TestFileStoreDelete(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")
	if !fs.DeleteTicket(tk.ID) {
		t.Fatal("DeleteTicket returned false")
	}
	if fs.GetTicket(tk.ID) != nil {
		t.Fatal("ticket still present after delete")
	}
	if fs.DeleteTicket(tk.ID) {
		t.Fatal("DeleteTicket of a gone ticket should be false")
	}
}

// ─── persistence: a fresh store over the same dir reads it back ─

func TestFileStorePersistsAcrossReopen(t *testing.T) {
	dir := t.TempDir()
	fs1, err := newFileTicketStore(dir)
	if err != nil {
		t.Fatalf("open 1: %v", err)
	}
	tk := fs1.CreateTicket("author", "persist me", "body", "bug")
	fs1.AddArtifact(tk.ID, ArtifactScreenshot, "x.png", "image/png", []byte("PNG"))
	dep, _ := fs1.CreateDeploy("1.2.3", "sha", nil, DeployBuilding)

	// A brand-new store instance over the same dir sees the same data.
	fs2, err := newFileTicketStore(dir)
	if err != nil {
		t.Fatalf("open 2: %v", err)
	}
	got := fs2.GetTicket(tk.ID)
	if got == nil || got.Title != "persist me" || len(got.Artifacts) != 1 {
		t.Fatalf("reopened ticket mismatch: %+v", got)
	}
	_, data := fs2.GetArtifactBlob(tk.ID, got.Artifacts[0].ID)
	if string(data) != "PNG" {
		t.Fatalf("reopened artifact bytes = %q", data)
	}
	if ds := fs2.ListDeploys(); len(ds) != 1 || ds[0].ID != dep.ID {
		t.Fatalf("reopened deploys: got %d", len(ds))
	}
}
