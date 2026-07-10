// memStore garbage-collection surface (blobs_gc.go, plan §6). The in-memory
// fixture keeps most categories in dedicated maps (projects/snapshots/thumbnails
// inline, mesh STLs in meshBlobs), so its shared `blobs` map holds only the
// content-addressed asset blobs (PutBlob) and ticket artifacts (AddArtifact
// writes there directly). ListReferencedRefs therefore reports the ticket-
// artifact + mesh-job refs so GC never reclaims them.

package main

import (
	"encoding/json"
	"time"
)

// AllProjectDocuments returns every project's stored document (inline in
// memStore) so GC can scan it for embedded mesh/font hashes.
func (s *memStore) AllProjectDocuments() ([]json.RawMessage, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]json.RawMessage, 0, len(s.projects))
	for _, p := range s.projects {
		if len(p.Document) > 0 {
			out = append(out, p.Document)
		}
	}
	return out, nil
}

// ListReferencedRefs returns every blob ref held directly by the store: ticket
// artifacts (share the blobs map) and mesh-job STLs. These are never GC-able.
func (s *memStore) ListReferencedRefs() ([]string, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var refs []string
	for _, arts := range s.artifacts {
		for _, a := range arts {
			if a.BlobRef != "" {
				refs = append(refs, a.BlobRef)
			}
		}
	}
	for _, j := range s.meshJobs {
		if j.BlobRef != "" {
			refs = append(refs, j.BlobRef)
		}
	}
	return refs, nil
}

// ListBlobRefs enumerates the physical asset/artifact blobs with size + first-
// write time (falling back to now() for any that predate blobMod tracking).
func (s *memStore) ListBlobRefs() ([]BlobRef, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]BlobRef, 0, len(s.blobs))
	for ref, data := range s.blobs {
		mt, ok := s.blobMod[ref]
		if !ok {
			mt = now()
		}
		out = append(out, BlobRef{Ref: ref, ModTime: mt, Size: int64(len(data))})
	}
	return out, nil
}

// DeleteBlob removes a blob (GC only).
func (s *memStore) DeleteBlob(ref string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.blobs, ref)
	delete(s.blobMod, ref)
	return nil
}

// setBlobModForTest backdates a blob's recorded mtime so a test can exercise the
// grace window without sleeping. Test-only helper (see blobs_gc_test.go).
func (s *memStore) setBlobModForTest(ref string, t time.Time) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.blobMod[ref] = t
}
