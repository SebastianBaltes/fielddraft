// fileTicketStore-SPECIFIC tests (WP15): the on-disk layout, canonical artifact
// filenames, open/→done/ relocation, and persistence across reopen. The shared
// business behavior (state machine, verdicts, comments, work-orders, filters,
// visibility) is covered by the TicketStore contract suite in
// ticketstore_contract_test.go.

package main

import (
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

// TestFileStoreOnDiskLayout — every save writes the authoritative ticket.json
// plus the derived ticket.md / comments.md under open/<id>/.
func TestFileStoreOnDiskLayout(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author-1", "Extrude crash", "crashes on extrude", "bug")
	if tk == nil || tk.ID == "" {
		t.Fatal("CreateTicket returned nil")
	}
	dir := fs.openDir(tk.ID)
	for _, f := range []string{"ticket.json", "ticket.md", "comments.md"} {
		if _, err := os.Stat(filepath.Join(dir, f)); err != nil {
			t.Fatalf("missing %s: %v", f, err)
		}
	}
}

// TestFileStoreTerminalStateMovesToDone — a ticket reaching a terminal state
// relocates from open/<id>/ to done/<id>/ and stays loadable.
func TestFileStoreTerminalStateMovesToDone(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "spammy", "buy now", "")
	if _, err := fs.SetTicketState(tk.ID, StateRejectedSpam, false); err != nil {
		t.Fatalf("reject: %v", err)
	}
	if !dirExists(fs.doneDir(tk.ID)) {
		t.Fatal("rejected ticket should be under done/")
	}
	if dirExists(fs.openDir(tk.ID)) {
		t.Fatal("rejected ticket should NOT remain under open/")
	}
	if fs.GetTicket(tk.ID) == nil {
		t.Fatal("relocated ticket no longer loadable")
	}
}

// TestFileStoreCanonicalArtifactFilenames — kind-typed artifacts land at their
// canonical §3.1 filenames; plain attachments keep their (sanitized) name.
func TestFileStoreCanonicalArtifactFilenames(t *testing.T) {
	fs := newFileStore(t)
	tk := fs.CreateTicket("author", "t", "b", "")

	png := []byte("PNGDATA-bytes")
	a := fs.AddArtifact(tk.ID, ArtifactScreenshot, "ignored.png", "image/png", png)
	if a == nil || a.Name != "screenshot.png" {
		t.Fatalf("artifact name = %q, want screenshot.png", a.Name)
	}
	if _, err := os.Stat(filepath.Join(fs.openDir(tk.ID), "screenshot.png")); err != nil {
		t.Fatalf("screenshot.png not written: %v", err)
	}
	if _, data := fs.GetArtifactBlob(tk.ID, a.ID); string(data) != string(png) {
		t.Fatalf("GetArtifactBlob mismatch: %q", data)
	}
	att := fs.AddArtifact(tk.ID, ArtifactAttachment, "attachment-01-log.txt", "text/plain", []byte("hi"))
	if att == nil || att.Name != "attachment-01-log.txt" {
		t.Fatalf("attachment name = %q", att.Name)
	}
}

// TestFileStoreReadsViteMiddlewareTicket — dev-parity pin: the Vite dev-server
// ticket middleware (web/vite.config.ts ticketWriter) writes ticket.json in
// EXACTLY this shape so tickets filed via `just dev` (no Go server) are
// visible under TICKET_BACKEND=files. If the fileTicketStore schema changes,
// this fixture (and the middleware) must move with it.
func TestFileStoreReadsViteMiddlewareTicket(t *testing.T) {
	base := t.TempDir()
	const id = "20260702-181530-ab3d"
	dir := filepath.Join(base, "open", id)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	png := []byte("PNGDATA")
	if err := os.WriteFile(filepath.Join(dir, "screenshot.png"), png, 0o644); err != nil {
		t.Fatal(err)
	}
	// Verbatim middleware output shape (sha256 of "PNGDATA" below).
	ticketJSON := `{
  "id": "` + id + `",
  "author_id": "local-dev",
  "title": "Extrude crashes",
  "body": "Extrude crashes\nwhen depth is negative",
  "state": "triage",
  "category": null,
  "po_spam": false,
  "po_malicious": false,
  "po_vision_fit": null,
  "po_reason": null,
  "po_verdict_at": null,
  "created_at": "2026-07-02T18:15:30.123Z",
  "updated_at": "2026-07-02T18:15:30.123Z",
  "comments": [],
  "artifacts": [
    {
      "id": "0123456789abcdef0123456789abcdef",
      "ticket_id": "` + id + `",
      "kind": "screenshot",
      "blob_ref": "2d4566582844690f8634a8b2534ea5221560038c6c0650c99140759bad603ae2",
      "mime": "image/png",
      "size": 7,
      "name": "screenshot.png",
      "created_at": "2026-07-02T18:15:30.123Z"
    }
  ]
}
`
	if err := os.WriteFile(filepath.Join(dir, "ticket.json"), []byte(ticketJSON), 0o644); err != nil {
		t.Fatal(err)
	}

	fs, err := newFileTicketStore(base)
	if err != nil {
		t.Fatalf("newFileTicketStore: %v", err)
	}
	got := fs.GetTicket(id)
	if got == nil {
		t.Fatal("middleware-written ticket invisible to fileTicketStore")
	}
	if got.Title != "Extrude crashes" || got.State != StateTriage || got.AuthorID != "local-dev" {
		t.Fatalf("ticket fields: %+v", got)
	}
	if len(got.Artifacts) != 1 || got.Artifacts[0].Kind != ArtifactScreenshot {
		t.Fatalf("artifacts: %+v", got.Artifacts)
	}
	// The list view sees it too.
	if got := fs.ListTickets(TicketFilter{}); len(got) != 1 || got[0].ID != id {
		t.Fatalf("ListTickets: %d", len(got))
	}
	// And the blob resolves via the artifact's on-disk name.
	if _, data := fs.GetArtifactBlob(id, got.Artifacts[0].ID); string(data) != string(png) {
		t.Fatalf("artifact bytes: %q", data)
	}
}

// TestFileStorePersistsAcrossReopen — a fresh store instance over the same dir
// reads back tickets, artifact bytes, and deploys.
func TestFileStorePersistsAcrossReopen(t *testing.T) {
	dir := t.TempDir()
	fs1, err := newFileTicketStore(dir)
	if err != nil {
		t.Fatalf("open 1: %v", err)
	}
	tk := fs1.CreateTicket("author", "persist me", "body", "bug")
	fs1.AddArtifact(tk.ID, ArtifactScreenshot, "x.png", "image/png", []byte("PNG"))
	dep, _ := fs1.CreateDeploy("1.2.3", "sha", nil, DeployBuilding)

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
