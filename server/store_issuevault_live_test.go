package main

// Live end-to-end test of the issuevaultStore adapter against a REAL IssueVault.
// Gated behind ISSUEVAULT_LIVE=1 so normal `go test` / CI never touch the network.
//
// Run (from server/):
//   ISSUEVAULT_LIVE=1 \
//   ISSUEVAULT_TOKEN=<agent-token> \
//   ISSUEVAULT_INSTALLATION=<installation-uuid> \
//   go test -run TestIssuevaultLiveE2E -v .
//
// It creates a probe ticket in the given installation and DELETES it at the end
// (defer), so it is self-cleaning except for one deploy record (records have no
// delete in the adapter) — created with a marker version for easy pruning.

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
)

func TestIssuevaultLiveE2E(t *testing.T) {
	if os.Getenv("ISSUEVAULT_LIVE") == "" {
		t.Skip("set ISSUEVAULT_LIVE=1 (+ ISSUEVAULT_TOKEN/INSTALLATION) to run the live adapter test")
	}
	s := newIssuevaultStore()
	if s.token == "" || s.installationID == "" {
		t.Fatal("need ISSUEVAULT_TOKEN and ISSUEVAULT_INSTALLATION")
	}

	// ── create ──
	tk := s.CreateTicket("e2e-user", "__e2e__ adapter probe", "probe body", "bug")
	if tk == nil {
		t.Fatal("CreateTicket returned nil")
	}
	id := tk.ID
	defer func() {
		if !s.DeleteTicket(id) {
			t.Logf("WARN cleanup: DeleteTicket(%s) failed — prune manually", id)
		}
	}()
	if tk.State != StateTriage {
		t.Fatalf("new ticket state=%q want triage", tk.State)
	}
	if tk.Category == nil || *tk.Category != "bug" {
		t.Fatalf("category=%v want bug", tk.Category)
	}

	// ── get (round-trip) ──
	got := s.GetTicket(id)
	if got == nil || got.Title != "__e2e__ adapter probe" {
		t.Fatalf("GetTicket mismatch: %+v", got)
	}

	// ── user comment (enters moderation: po_reviewed=false) ──
	uc := s.AddComment(id, AuthorUser, strPtr("e2e-user"), "a user comment", false)
	if uc == nil || uc.POReviewed {
		t.Fatalf("user comment should be unreviewed: %+v", uc)
	}
	if uc.AuthorType != AuthorUser {
		t.Fatalf("comment author_type=%q want user", uc.AuthorType)
	}

	// ── PO verdict triage→in_progress + meta round-trip (proves meta is an OBJECT) ──
	reason := "looks legit"
	fit := "fit"
	acc, err := s.SetPOVerdict(id, POVerdict{Spam: false, Malicious: false, VisionFit: &fit, Reason: reason, State: StateInProgress})
	if err != nil {
		t.Fatalf("SetPOVerdict: %v", err)
	}
	if acc.State != StateInProgress {
		t.Fatalf("post-verdict state=%q want in_progress", acc.State)
	}
	if acc.POReason == nil || *acc.POReason != reason {
		t.Fatalf("po_reason did not round-trip through meta: %v (meta likely not decoded as object)", acc.POReason)
	}
	if acc.POVisionFit == nil || *acc.POVisionFit != fit {
		t.Fatalf("po_vision_fit did not round-trip: %v", acc.POVisionFit)
	}

	// ── list by state (array filter) — must find the in_progress ticket ──
	list := s.ListTickets(TicketFilter{State: StateInProgress, ViewerIsAdmin: true})
	if !containsID(list, id) {
		t.Fatalf("ListTickets(state=in_progress) did not return %s (state filter shape?) — got %d", id, len(list))
	}

	// ── work-order lifecycle: create → claim → complete ──
	wo, err := s.CreateWorkOrder(id, json.RawMessage(`{"task":"probe"}`))
	if err != nil || !wo.Open() {
		t.Fatalf("CreateWorkOrder: wo=%+v err=%v", wo, err)
	}
	if _, err := s.CreateWorkOrder(id, json.RawMessage(`{}`)); err != errWorkOrderExists {
		t.Fatalf("second CreateWorkOrder err=%v want errWorkOrderExists", err)
	}
	// appears in the coding queue
	if !containsID(s.ListAcceptedWithOpenWorkOrder(), id) {
		t.Fatalf("ListAcceptedWithOpenWorkOrder did not return %s (meta predicate?)", id)
	}
	claimed, err := s.ClaimWorkOrder(id)
	if err != nil || claimed.PickedUpAt == nil {
		t.Fatalf("ClaimWorkOrder: %+v err=%v", claimed, err)
	}
	if _, err := s.ClaimWorkOrder(id); err != errWorkOrderClaimed {
		t.Fatalf("double claim err=%v want errWorkOrderClaimed", err)
	}
	// payload must survive the merge-patches
	if strings.TrimSpace(string(claimed.Payload)) != `{"task":"probe"}` {
		t.Fatalf("work-order payload clobbered by merge-patch: %s", claimed.Payload)
	}
	done, err := s.CompleteWorkOrder(id, "e2e-deploy")
	if err != nil || done.CompletedAt == nil || done.Open() {
		t.Fatalf("CompleteWorkOrder: %+v err=%v", done, err)
	}
	// now gone from the coding queue
	if containsID(s.ListAcceptedWithOpenWorkOrder(), id) {
		t.Fatalf("completed work-order still in coding queue")
	}

	// ── deploy record (records + new token scope) ──
	dep, err := s.CreateDeploy("0.0.0-e2e-DELETEME", "abc1234", []string{id}, DeployStatus("live"))
	if err != nil || dep.ID == "" {
		t.Fatalf("CreateDeploy: %+v err=%v", dep, err)
	}
	if found := findDeploy(s.ListDeploys(), dep.ID); found == nil || found.Version != "0.0.0-e2e-DELETEME" {
		t.Fatalf("ListDeploys missing the new record %s", dep.ID)
	}

	// ── comment moderation: flip po_reviewed false→true ──
	rev := s.SetCommentReviewed(id, uc.ID, false)
	if rev == nil || !rev.POReviewed {
		t.Fatalf("SetCommentReviewed did not flip po_reviewed: %+v", rev)
	}

	// ── artifact upload + blob round-trip ──
	blob := []byte("PNG-PROBE-BYTES")
	art := s.AddArtifact(id, ArtifactScreenshot, "shot.png", "image/png", blob)
	if art == nil || art.BlobRef == "" || art.Kind != ArtifactScreenshot {
		t.Fatalf("AddArtifact: %+v", art)
	}
	gotMeta, gotBytes := s.GetArtifactBlob(id, art.ID)
	if gotMeta == nil || string(gotBytes) != string(blob) {
		t.Fatalf("GetArtifactBlob mismatch: meta=%+v bytes=%q", gotMeta, gotBytes)
	}

	t.Logf("live e2e OK — ticket %s (deploy record %s = 0.0.0-e2e-DELETEME left for pruning)", id, dep.ID)
}

func strPtr(s string) *string { return &s }

func containsID(ts []*Ticket, id string) bool {
	for _, t := range ts {
		if t.ID == id {
			return true
		}
	}
	return false
}

func findDeploy(ds []*Deploy, id string) *Deploy {
	for _, d := range ds {
		if d.ID == id {
			return d
		}
	}
	return nil
}
