// sqlStore garbage-collection surface (blobs_gc.go, plan §6). The on-disk
// /data/blobs directory is SHARED by every content-addressed payload, so GC's
// reference set must union EVERY structural ref column (documents, thumbnails,
// snapshots, mesh-job STLs, ticket artifacts) with the mesh/font hashes embedded
// in project documents. Any query error is propagated so GC fails closed rather
// than delete live data from an incomplete set.

package main

import (
	"database/sql"
	"encoding/json"
	"os"
	"path/filepath"
)

// AllProjectDocuments loads every project's stored document (from its blob) so
// GC can walk it for embedded mesh/font hashes.
func (s *sqlStore) AllProjectDocuments() ([]json.RawMessage, error) {
	rows, err := s.db.Query(`SELECT document_ref FROM projects WHERE document_ref IS NOT NULL`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []json.RawMessage
	for rows.Next() {
		var ref sql.NullString
		if err := rows.Scan(&ref); err != nil {
			return nil, err
		}
		if !ref.Valid || ref.String == "" {
			continue
		}
		data, err := s.getBlob(ref.String)
		if err != nil {
			return nil, err
		}
		if data == nil {
			continue
		}
		out = append(out, json.RawMessage(data))
	}
	return out, rows.Err()
}

// ListReferencedRefs returns every blob ref held directly in a store column:
// project documents + thumbnails, snapshot documents, mesh-job STLs and ticket
// artifacts. Deleting one of these would drop live data, so GC treats them all
// as referenced. Any query error is returned (⇒ GC aborts, deletes nothing).
func (s *sqlStore) ListReferencedRefs() ([]string, error) {
	queries := []string{
		`SELECT document_ref  FROM projects         WHERE document_ref  IS NOT NULL`,
		`SELECT thumbnail_ref FROM projects         WHERE thumbnail_ref IS NOT NULL`,
		`SELECT document_ref  FROM snapshots        WHERE document_ref  IS NOT NULL`,
		`SELECT blob_ref      FROM mesh_jobs        WHERE blob_ref <> ''`,
		`SELECT blob_ref      FROM ticket_artifacts WHERE blob_ref <> ''`,
	}
	var refs []string
	for _, q := range queries {
		rows, err := s.db.Query(q)
		if err != nil {
			return nil, err
		}
		for rows.Next() {
			var ref sql.NullString
			if err := rows.Scan(&ref); err != nil {
				rows.Close()
				return nil, err
			}
			if ref.Valid && ref.String != "" {
				refs = append(refs, ref.String)
			}
		}
		if err := rows.Err(); err != nil {
			rows.Close()
			return nil, err
		}
		rows.Close()
	}
	return refs, nil
}

// ListBlobRefs walks the blob directory (flat, one file per sha256 hex) and
// returns each blob's ref + mtime + size. Non-blob entries (in-flight `.tmp-*`
// writes, directories) are skipped via the sha256-hex name filter so a partial
// upload is never treated as an orphan.
func (s *sqlStore) ListBlobRefs() ([]BlobRef, error) {
	entries, err := os.ReadDir(s.blobDir)
	if err != nil {
		return nil, err
	}
	out := make([]BlobRef, 0, len(entries))
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		name := e.Name()
		if !isSha256Hex(name) {
			continue // skip .tmp-* and anything not a content-addressed blob
		}
		info, err := e.Info()
		if err != nil {
			continue // vanished between ReadDir and Info — ignore
		}
		out = append(out, BlobRef{Ref: name, ModTime: info.ModTime(), Size: info.Size()})
	}
	return out, nil
}

// DeleteBlob removes a blob file (GC only). A missing file is not an error.
func (s *sqlStore) DeleteBlob(ref string) error {
	if !isSha256Hex(ref) {
		return nil // never touch a non-blob path
	}
	err := os.Remove(filepath.Join(s.blobDir, ref))
	if os.IsNotExist(err) {
		return nil
	}
	return err
}
