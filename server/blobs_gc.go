// Orphaned-asset-blob garbage collection (docs/PROJECT-ASSET-STORAGE-PLAN §6, P3e).
//
// The content-addressed blob store (sqlStore's /data/blobs, memStore's blobs
// map) is SHARED: it holds project-document blobs, thumbnails, snapshot
// documents, mesh-job STLs, AND the mesh/font asset blobs a lean project
// document references by sha256 hash. (Ticket artifacts left the SQL store with
// ticket #346 — they live in IssueVault now; the in-memory test double still
// keeps them, and still reports them as referenced.) Over time a save ORPHANS an
// asset blob — a newer document version stops referencing a mesh/font hash, but
// the immutable blob lingers forever (the store only grows, see the former
// blobs_handlers.go TODO(GC)).
//
// GC reclaims exactly those unreachable asset blobs, and NOTHING else. It is
// deliberately conservative because it DELETES data:
//
//   1. Two reference sources are UNIONed into the live set (collectReferenced-
//      BlobHashes): every ref held directly in a store column (documents,
//      thumbnails, snapshots, mesh artifacts) PLUS every
//      embedded mesh/font hash found by walking each project document. A blob in
//      EITHER set is never touched. The walk biases toward OVER-collection — a
//      stray extra hash only spares an orphan, it can never delete a live blob.
//   2. A grace window protects freshly uploaded blobs: a client PUTs its asset
//      blobs BEFORE it PATCHes the referencing document, so a just-uploaded blob
//      is legitimately unreferenced for a short time. Blobs younger than the
//      grace are always kept.
//   3. Fail closed: if the live set or the blob listing cannot be built (a DB /
//      filesystem error), GC aborts and deletes nothing — an incomplete
//      reference set could otherwise delete live data.
//   4. The endpoint is admin-only and DRY-RUN BY DEFAULT (POST /api/admin/blobs/
//      gc): it reports what WOULD be deleted unless dry_run=false is passed.
//      There is deliberately NO background sweep — GC is strictly opt-in.
//
// Snapshots are self-contained since P3a (a published snapshot embeds full
// meshes, no __asset refs), so their CONTENT is not walked; their document blob
// is still protected structurally via ListReferencedRefs.

package main

import (
	"encoding/json"
	"log"
	"net/http"
	"strconv"
	"strings"
	"time"
)

// BlobRef identifies one stored blob plus the metadata GC needs: its content
// address, last-modified time (grace-window test) and byte size (freed-bytes
// accounting).
type BlobRef struct {
	Ref     string
	ModTime time.Time
	Size    int64
}

// gcDefaultGrace shields blobs uploaded within this window from GC. A document
// PATCH normally lands seconds after its asset PUTs; 48h is a very safe margin
// against ever collecting a blob whose referencing save has not arrived yet.
const gcDefaultGrace = 48 * time.Hour

// maxDeletedRefsReported caps the ref list echoed in the JSON response; the
// counts stay exact regardless of this cap.
const maxDeletedRefsReported = 200

// gcResult is the JSON report POST /api/admin/blobs/gc returns.
type gcResult struct {
	Scanned      int      `json:"scanned"`       // physical blobs examined
	Referenced   int      `json:"referenced"`    // distinct live hashes (both sources)
	Orphaned     int      `json:"orphaned"`      // blobs referenced by nothing (any age)
	Deleted      int      `json:"deleted"`       // orphans past the grace window (deleted, or WOULD-be on dry run)
	FreedBytes   int64    `json:"freed_bytes"`   // total size of the deleted blobs
	DryRun       bool     `json:"dry_run"`       // true = nothing was actually deleted
	GraceSeconds float64  `json:"grace_seconds"` // protection window applied
	DeletedRefs  []string `json:"deleted_refs"`  // the collected blobs (capped for the response)
}

// collectReferencedBlobHashes builds the set of blob refs that MUST NOT be
// deleted: the union of every ref held directly in a store column and every
// embedded mesh/font hash inside a project document. Returns an error (⇒ GC
// aborts) if either source cannot be read — an incomplete set is unsafe.
func collectReferencedBlobHashes(store Store) (map[string]struct{}, error) {
	set := make(map[string]struct{})

	refs, err := store.ListReferencedRefs()
	if err != nil {
		return nil, err
	}
	for _, ref := range refs {
		if ref != "" {
			set[ref] = struct{}{}
		}
	}

	docs, err := store.AllProjectDocuments()
	if err != nil {
		return nil, err
	}
	for _, doc := range docs {
		collectDocRefs(doc, set)
	}
	return set, nil
}

// collectDocRefs walks a project document (a v2 envelope {v,doc,fonts}, a plain
// project blob, or a bare kernel document — all tolerated) and adds every
// embedded blob hash to set:
//   - any "__asset" string value (an externalized mesh/font ref), and
//   - every value of a "fonts" object (the fontId→hash manifest).
//
// The lean project json (envelope "doc") and each part-tab's geometry
// (PartTab.documentJson) are carried as JSON-encoded STRINGS, so the walk also
// parses any string that looks like embedded JSON and recurses into it — that is
// how it reaches the __asset refs nested inside each documentJson. Only sha256-
// hex values are recorded (a real blob ref can only be sha256 hex), keeping the
// referenced count meaningful; a non-hex value could never match a blob anyway.
func collectDocRefs(raw json.RawMessage, set map[string]struct{}) {
	if len(raw) == 0 {
		return
	}
	var v any
	if err := json.Unmarshal(raw, &v); err != nil {
		return
	}
	walkDocValue(v, set)
}

func walkDocValue(v any, set map[string]struct{}) {
	switch t := v.(type) {
	case map[string]any:
		for k, val := range t {
			switch k {
			case "__asset":
				if s, ok := val.(string); ok && isSha256Hex(s) {
					set[s] = struct{}{}
				}
			case "fonts":
				if fonts, ok := val.(map[string]any); ok {
					for _, fv := range fonts {
						if s, ok := fv.(string); ok && isSha256Hex(s) {
							set[s] = struct{}{}
						}
					}
				}
			}
			walkDocValue(val, set)
		}
	case []any:
		for _, e := range t {
			walkDocValue(e, set)
		}
	case string:
		// A part-tab's documentJson (and the envelope's doc field) is embedded as
		// a JSON string; dive in so nested __asset refs are reached. Gate on a
		// leading brace/bracket so we only attempt to parse actual JSON payloads.
		if s := strings.TrimSpace(t); strings.HasPrefix(s, "{") || strings.HasPrefix(s, "[") {
			var inner any
			if json.Unmarshal([]byte(t), &inner) == nil {
				walkDocValue(inner, set)
			}
		}
	}
}

// gcOrphanBlobs deletes (or, on dry-run, only reports) every blob that is
// referenced by nothing AND older than grace. Never deletes a referenced or
// freshly uploaded blob. Returns an error (deleting nothing) if the reference
// set or the blob listing could not be built — fail closed.
func gcOrphanBlobs(store Store, grace time.Duration, dryRun bool) (gcResult, error) {
	referenced, err := collectReferencedBlobHashes(store)
	if err != nil {
		return gcResult{}, err
	}
	blobs, err := store.ListBlobRefs()
	if err != nil {
		return gcResult{}, err
	}
	cutoff := now().Add(-grace)

	res := gcResult{
		Scanned:      len(blobs),
		Referenced:   len(referenced),
		DryRun:       dryRun,
		GraceSeconds: grace.Seconds(),
		DeletedRefs:  []string{},
	}
	for _, b := range blobs {
		if _, live := referenced[b.Ref]; live {
			continue
		}
		res.Orphaned++
		// Grace window: keep anything modified after the cutoff — a client uploads
		// its asset blobs BEFORE it PATCHes the referencing document.
		if b.ModTime.After(cutoff) {
			continue
		}
		if !dryRun {
			if err := store.DeleteBlob(b.Ref); err != nil {
				log.Printf("blobs GC: delete %s: %v", b.Ref, err)
				continue
			}
		}
		res.Deleted++
		res.FreedBytes += b.Size
		if len(res.DeletedRefs) < maxDeletedRefsReported {
			res.DeletedRefs = append(res.DeletedRefs, b.Ref)
		}
	}
	return res, nil
}

// handleBlobsGC runs orphaned-asset-blob garbage collection (plan §6). ADMIN
// ONLY and DRY-RUN BY DEFAULT: it reports what would be reclaimed and deletes
// nothing unless dry_run=false is passed explicitly. A grace window (default
// gcDefaultGrace) shields freshly uploaded blobs whose referencing document
// PATCH has not landed yet. There is deliberately NO background sweep — GC is
// opt-in to keep a data-destroying operation under an operator's control.
//
// Params (JSON body, overridable by query string so a URL flag always wins):
//
//	dry_run     bool  — default TRUE; only dry_run=false actually deletes.
//	grace_hours float — default 48; the freshness window, in hours.
func (s *Server) handleBlobsGC(w http.ResponseWriter, r *http.Request) {
	if s.requireAdmin(w, r) == nil {
		return
	}
	dryRun := true
	grace := gcDefaultGrace

	// Body params (optional, best-effort — an empty/invalid body keeps defaults).
	var body struct {
		DryRun     *bool    `json:"dry_run"`
		GraceHours *float64 `json:"grace_hours"`
	}
	if r.Body != nil {
		_ = json.NewDecoder(r.Body).Decode(&body)
	}
	if body.DryRun != nil {
		dryRun = *body.DryRun
	}
	if body.GraceHours != nil && *body.GraceHours >= 0 {
		grace = time.Duration(*body.GraceHours * float64(time.Hour))
	}
	// Query overrides body.
	q := r.URL.Query()
	if v := q.Get("dry_run"); v != "" {
		dryRun = !(v == "false" || v == "0" || v == "no")
	}
	if v := q.Get("grace_hours"); v != "" {
		if h, err := strconv.ParseFloat(v, 64); err == nil && h >= 0 {
			grace = time.Duration(h * float64(time.Hour))
		}
	}

	res, err := gcOrphanBlobs(s.store, grace, dryRun)
	if err != nil {
		log.Printf("blobs GC: aborted (reference set incomplete): %v", err)
		writeErr(w, http.StatusInternalServerError, "gc aborted: could not build reference set")
		return
	}
	json.NewEncoder(w).Encode(res)
}
