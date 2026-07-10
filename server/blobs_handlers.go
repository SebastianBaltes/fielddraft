// Content-addressed asset-blob endpoints (client cloud-save asset sync, §5.5).
//
// A server-bound project stores a LEAN document (features + asset hashes); the
// large mesh/font payloads it references are uploaded here separately, keyed by
// their sha256 hex, deduplicated across projects and users. The client's
// externalize/inline path (web/src/projectPersistence.ts) drives these:
//
//   POST /api/blobs/missing   {hashes:[…]} → {missing:[…]}   (skip re-uploads)
//   PUT  /api/blobs/{sha256}  raw bytes    → upload-if-missing
//   GET  /api/blobs/{sha256}               → raw bytes (application/octet-stream)
//
// All three require an authenticated session. The blobs themselves are reused
// from the existing content-addressed store (Store.PutBlob/GetBlob/HasBlob →
// sqlStore's on-disk /data/blobs, the same store ticket artifacts and thumbnails
// use).
//
// TODO(GC): orphaned blobs are never reclaimed here (plan §6). A blob that a
// later save orphans (its project document no longer references the hash)
// accumulates until a future sweep cross-references every live project/snapshot
// document against the blob dir and deletes the unreachable ones. Until then the
// store only grows.

package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
)

// maxBlobBytes caps a single blob upload (64 MB). A mesh/font payload is
// content-addressed and immutable; this bounds one PUT so a client cannot
// exhaust disk with one request.
const maxBlobBytes = 64 * 1024 * 1024

// isSha256Hex reports whether s is a lowercase 64-char hex string — the exact
// shape of a blob ref (sha256). Guards the path parameter so a stray value can
// never touch the filesystem via an unfiltered ref.
func isSha256Hex(s string) bool {
	if len(s) != 64 {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')) {
			return false
		}
	}
	return true
}

// handlePutBlob stores an uploaded asset blob. Upload-if-missing: an already
// present hash is a 200 no-op; otherwise the raw body is stored. The {sha256}
// path MUST equal sha256(body) — content-addressed integrity — else 400.
func (s *Server) handlePutBlob(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	want := r.PathValue("sha256")
	if !isSha256Hex(want) {
		writeErr(w, http.StatusBadRequest, "invalid blob hash")
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, maxBlobBytes)
	data, err := readAll(r.Body)
	if err != nil {
		writeErr(w, http.StatusRequestEntityTooLarge, "blob too large (max 64 MB)")
		return
	}
	sum := sha256.Sum256(data)
	got := hex.EncodeToString(sum[:])
	if got != want {
		writeErr(w, http.StatusBadRequest, "blob hash mismatch")
		return
	}
	if s.store.HasBlob(want) {
		// Already stored — no-op (content-addressed ⇒ immutable).
		json.NewEncoder(w).Encode(map[string]any{"stored": false})
		return
	}
	if _, err := s.store.PutBlob(data); err != nil {
		writeErr(w, http.StatusInternalServerError, "could not store blob")
		return
	}
	json.NewEncoder(w).Encode(map[string]any{"stored": true})
}

// handleGetBlob returns a blob's raw bytes, or 404 when absent.
func (s *Server) handleGetBlob(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	ref := r.PathValue("sha256")
	if !isSha256Hex(ref) {
		writeErr(w, http.StatusBadRequest, "invalid blob hash")
		return
	}
	data, err := s.store.GetBlob(ref)
	if err != nil {
		writeErr(w, http.StatusInternalServerError, "could not read blob")
		return
	}
	if data == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	w.Header().Set("Content-Type", "application/octet-stream")
	w.Write(data)
}

// handleBlobsMissing answers which of the given hashes are NOT stored, so the
// client uploads only what the server lacks (dedup across projects/users) —
// saving a PUT round-trip per already-present blob.
func (s *Server) handleBlobsMissing(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	var body struct {
		Hashes []string `json:"hashes"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeErr(w, http.StatusBadRequest, "invalid JSON")
		return
	}
	missing := make([]string, 0, len(body.Hashes))
	seen := make(map[string]struct{}, len(body.Hashes))
	for _, h := range body.Hashes {
		if _, dup := seen[h]; dup {
			continue
		}
		seen[h] = struct{}{}
		if !s.store.HasBlob(h) {
			missing = append(missing, h)
		}
	}
	json.NewEncoder(w).Encode(map[string][]string{"missing": missing})
}
