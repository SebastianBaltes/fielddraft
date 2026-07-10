package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// gcEnvelope builds a v2 project-document envelope that references a mesh blob
// (via a nested part-tab documentJson `__asset`) and a font blob (via the
// `fonts` manifest) — the exact shape the client persists. GC must recognize
// BOTH hashes as referenced.
func gcEnvelope(meshHash, fontHash string) json.RawMessage {
	innerDoc, _ := json.Marshal(map[string]any{
		"feature_calls": []any{
			map[string]any{
				"type":   "imported_mesh",
				"params": map[string]any{"__asset": meshHash, "__assetKind": "mesh"},
			},
		},
	})
	leanProject, _ := json.Marshal(map[string]any{
		"schemaVersion": 1,
		"tabs": []any{
			map[string]any{"id": "tab_1", "kind": "part", "documentJson": string(innerDoc)},
		},
	})
	envelope, _ := json.Marshal(map[string]any{
		"v":     2,
		"doc":   string(leanProject),
		"fonts": map[string]any{"uploaded:MyFont.ttf": fontHash},
	})
	return json.RawMessage(envelope)
}

// TestGCReferencedNeverDeleted is the core safety property: a blob referenced by
// a project document — a nested mesh `__asset` or a `fonts` manifest hash — is
// never deleted, even when it is older than the grace window; an unreferenced
// OLD blob is deleted; an unreferenced FRESH blob (inside the grace window) is
// kept.
func TestGCReferencedNeverDeleted(t *testing.T) {
	store := NewStore()
	ms := store.(*memStore)

	meshHash, _ := store.PutBlob([]byte("mesh-payload-referenced"))
	fontHash, _ := store.PutBlob([]byte("font-payload-referenced"))
	orphanOld, _ := store.PutBlob([]byte("orphan-old-unreferenced"))
	orphanFresh, _ := store.PutBlob([]byte("orphan-fresh-unreferenced"))

	// A project referencing the mesh + font hashes.
	store.CreateProject("owner1", "P", gcEnvelope(meshHash, fontHash))

	// Age the referenced blobs AND the old orphan past the grace window; leave
	// the fresh orphan at now().
	old := time.Now().Add(-72 * time.Hour)
	ms.setBlobModForTest(meshHash, old)
	ms.setBlobModForTest(fontHash, old)
	ms.setBlobModForTest(orphanOld, old)

	res, err := gcOrphanBlobs(store, 48*time.Hour, false /* dryRun */)
	if err != nil {
		t.Fatalf("gcOrphanBlobs: %v", err)
	}

	// Referenced (even old) survive.
	if !store.HasBlob(meshHash) {
		t.Error("referenced mesh blob was deleted")
	}
	if !store.HasBlob(fontHash) {
		t.Error("referenced font blob was deleted")
	}
	// Fresh orphan (inside grace) survives.
	if !store.HasBlob(orphanFresh) {
		t.Error("fresh orphan (within grace) was deleted")
	}
	// Old orphan is reclaimed.
	if store.HasBlob(orphanOld) {
		t.Error("old orphan blob should have been deleted")
	}

	if res.Referenced != 2 {
		t.Errorf("referenced = %d, want 2 (mesh+font)", res.Referenced)
	}
	if res.Scanned != 4 {
		t.Errorf("scanned = %d, want 4", res.Scanned)
	}
	if res.Orphaned != 2 {
		t.Errorf("orphaned = %d, want 2 (old+fresh)", res.Orphaned)
	}
	if res.Deleted != 1 {
		t.Errorf("deleted = %d, want 1 (old orphan only)", res.Deleted)
	}
	want := int64(len("orphan-old-unreferenced"))
	if res.FreedBytes != want {
		t.Errorf("freedBytes = %d, want %d", res.FreedBytes, want)
	}
	if len(res.DeletedRefs) != 1 || res.DeletedRefs[0] != orphanOld {
		t.Errorf("deletedRefs = %v, want [%s]", res.DeletedRefs, orphanOld)
	}
}

// TestGCDryRun asserts a dry run REPORTS the collectable orphan but deletes
// nothing; a real run (dry_run=false) then deletes it.
func TestGCDryRun(t *testing.T) {
	store := NewStore()
	ms := store.(*memStore)

	orphan, _ := store.PutBlob([]byte("orphan-dryrun"))
	ms.setBlobModForTest(orphan, time.Now().Add(-72*time.Hour))

	// Dry run: reports 1, keeps the blob.
	res, err := gcOrphanBlobs(store, 48*time.Hour, true /* dryRun */)
	if err != nil {
		t.Fatalf("gcOrphanBlobs (dry): %v", err)
	}
	if !res.DryRun {
		t.Error("res.DryRun = false, want true")
	}
	if res.Deleted != 1 {
		t.Errorf("dry-run deleted = %d, want 1 (would-delete)", res.Deleted)
	}
	if !store.HasBlob(orphan) {
		t.Fatal("dry run must NOT delete the blob")
	}

	// Real run: actually deletes.
	res, err = gcOrphanBlobs(store, 48*time.Hour, false)
	if err != nil {
		t.Fatalf("gcOrphanBlobs (real): %v", err)
	}
	if res.Deleted != 1 {
		t.Errorf("real-run deleted = %d, want 1", res.Deleted)
	}
	if store.HasBlob(orphan) {
		t.Error("real run should have deleted the orphan")
	}
}

// TestGCEndpointAdminGate covers the HTTP endpoint: unauthenticated → 401,
// non-admin → 403, admin default → dry-run (keeps), admin dry_run=false → delete.
func TestGCEndpointAdminGate(t *testing.T) {
	store := NewStore()
	ms := store.(*memStore)
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()
	client := &http.Client{
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse },
	}

	orphan, _ := store.PutBlob([]byte("orphan-http"))
	ms.setBlobModForTest(orphan, time.Now().Add(-72*time.Hour))

	postGC := func(cookie *http.Cookie, query string) *http.Response {
		req, _ := http.NewRequest("POST", ts.URL+"/api/admin/blobs/gc"+query, nil)
		if cookie != nil {
			req.AddCookie(cookie)
		}
		resp, err := client.Do(req)
		if err != nil {
			t.Fatalf("POST gc: %v", err)
		}
		return resp
	}

	// No auth → 401.
	resp := postGC(nil, "")
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("no auth: got %d, want 401", resp.StatusCode)
	}
	resp.Body.Close()

	// Non-admin user → 403.
	invite := newInvite(t, store)
	rresp, err := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"gcuser@example.com","password":"secret123","display_name":"U","invite":"`+invite+`"}`))
	if err != nil {
		t.Fatal(err)
	}
	userCookie := rresp.Cookies()[0]
	rresp.Body.Close()
	resp = postGC(userCookie, "")
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("non-admin: got %d, want 403", resp.StatusCode)
	}
	resp.Body.Close()
	if !store.HasBlob(orphan) {
		t.Fatal("non-admin request must not delete anything")
	}

	// Admin session cookie.
	admin, err := store.CreateUser("gcadmin@example.com", "Admin", "adminpass123", RoleAdmin)
	if err != nil {
		t.Fatal(err)
	}
	adminCookie := &http.Cookie{Name: "session", Value: store.CreateSession(admin.ID).Token}

	// Admin, default (no dry_run param) → dry-run: reports but keeps.
	resp = postGC(adminCookie, "")
	if resp.StatusCode != 200 {
		t.Fatalf("admin dry-run: got %d", resp.StatusCode)
	}
	var out gcResult
	json.NewDecoder(resp.Body).Decode(&out)
	resp.Body.Close()
	if !out.DryRun {
		t.Error("default must be dry_run=true")
	}
	if out.Deleted != 1 {
		t.Errorf("admin dry-run deleted = %d, want 1 (would-delete)", out.Deleted)
	}
	if !store.HasBlob(orphan) {
		t.Fatal("dry run (default) must NOT delete the blob")
	}

	// Admin, dry_run=false → actually deletes.
	resp = postGC(adminCookie, "?dry_run=false")
	if resp.StatusCode != 200 {
		t.Fatalf("admin real: got %d", resp.StatusCode)
	}
	out = gcResult{}
	json.NewDecoder(resp.Body).Decode(&out)
	resp.Body.Close()
	if out.DryRun {
		t.Error("dry_run=false must actually delete")
	}
	if out.Deleted != 1 {
		t.Errorf("admin real deleted = %d, want 1", out.Deleted)
	}
	if store.HasBlob(orphan) {
		t.Error("dry_run=false should have deleted the orphan")
	}
}
