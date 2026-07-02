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
	if _, err := fs.SetTicketState(tk.ID, StateRejectedSpam); err != nil {
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
