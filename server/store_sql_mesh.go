// sqlStore implementation of the mesh-job queue (plan §P4). The STL result
// goes through the shared content-addressed blob store (putBlob/getBlob).

package main

import (
	"database/sql"
	"log"
)

const meshJobCols = `id, owner_id, project_id, part_id, params_json, status,
	blob_ref, COALESCE(report_json,''), COALESCE(error,''),
	created_at, started_at, finished_at`

func scanMeshJob(row interface{ Scan(...any) error }) *MeshJob {
	var j MeshJob
	var started, finished sql.NullTime
	if err := row.Scan(&j.ID, &j.OwnerID, &j.ProjectID, &j.PartID,
		&j.ParamsJSON, &j.Status, &j.BlobRef, &j.ReportJSON, &j.Error,
		&j.CreatedAt, &started, &finished); err != nil {
		if err != sql.ErrNoRows {
			log.Printf("sqlStore: scanMeshJob: %v", err)
		}
		return nil
	}
	if started.Valid {
		j.StartedAt = &started.Time
	}
	if finished.Valid {
		j.FinishedAt = &finished.Time
	}
	return &j
}

func (s *sqlStore) CreateMeshJob(ownerID, projectID, partID, paramsJSON string) *MeshJob {
	j := &MeshJob{
		ID:         newID(),
		OwnerID:    ownerID,
		ProjectID:  projectID,
		PartID:     partID,
		ParamsJSON: paramsJSON,
		Status:     MeshQueued,
		CreatedAt:  now(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO mesh_jobs (id, owner_id, project_id, part_id, params_json, status, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		j.ID, j.OwnerID, j.ProjectID, j.PartID, j.ParamsJSON, j.Status, j.CreatedAt); err != nil {
		log.Printf("sqlStore: CreateMeshJob: %v", err)
		return nil
	}
	return j
}

func (s *sqlStore) GetMeshJob(id, ownerID string) *MeshJob {
	return scanMeshJob(s.db.QueryRow(
		`SELECT `+meshJobCols+` FROM mesh_jobs WHERE id = ? AND owner_id = ?`,
		id, ownerID))
}

// ClaimNextMeshJob atomically flips the oldest queued job to running. The
// guarded UPDATE makes concurrent claimers (several dispatcher workers, or
// several server replicas) never grab the same job.
func (s *sqlStore) ClaimNextMeshJob() *MeshJob {
	for range 5 { // retry a lost race against another claimer
		var id string
		err := s.db.QueryRow(
			`SELECT id FROM mesh_jobs WHERE status = ? ORDER BY created_at LIMIT 1`,
			MeshQueued).Scan(&id)
		if err == sql.ErrNoRows {
			return nil
		}
		if err != nil {
			log.Printf("sqlStore: ClaimNextMeshJob select: %v", err)
			return nil
		}
		res, err := s.db.Exec(
			`UPDATE mesh_jobs SET status = ?, started_at = ? WHERE id = ? AND status = ?`,
			MeshRunning, now(), id, MeshQueued)
		if err != nil {
			log.Printf("sqlStore: ClaimNextMeshJob update: %v", err)
			return nil
		}
		if n, _ := res.RowsAffected(); n == 1 {
			return scanMeshJob(s.db.QueryRow(
				`SELECT `+meshJobCols+` FROM mesh_jobs WHERE id = ?`, id))
		}
	}
	return nil
}

func (s *sqlStore) FinishMeshJob(id string, status MeshJobStatus, stl []byte, reportJSON, errMsg string) {
	ref := ""
	if stl != nil {
		var err error
		if ref, err = s.putBlob(stl); err != nil {
			log.Printf("sqlStore: FinishMeshJob putBlob: %v", err)
			status, errMsg = MeshError, "artifact store failed"
		}
	}
	if _, err := s.db.Exec(
		`UPDATE mesh_jobs SET status = ?, blob_ref = ?, report_json = ?, error = ?, finished_at = ?
		 WHERE id = ?`,
		status, ref, reportJSON, errMsg, now(), id); err != nil {
		log.Printf("sqlStore: FinishMeshJob: %v", err)
	}
}

func (s *sqlStore) CancelMeshJob(id, ownerID string) bool {
	res, err := s.db.Exec(
		`UPDATE mesh_jobs SET status = ?, error = 'canceled', finished_at = ?
		 WHERE id = ? AND owner_id = ? AND status = ?`,
		MeshError, now(), id, ownerID, MeshQueued)
	if err != nil {
		log.Printf("sqlStore: CancelMeshJob: %v", err)
		return false
	}
	n, _ := res.RowsAffected()
	return n == 1
}

func (s *sqlStore) ActiveMeshJobs(ownerID string) int {
	var n int
	if err := s.db.QueryRow(
		`SELECT COUNT(*) FROM mesh_jobs WHERE owner_id = ? AND status IN (?, ?)`,
		ownerID, MeshQueued, MeshRunning).Scan(&n); err != nil {
		log.Printf("sqlStore: ActiveMeshJobs: %v", err)
	}
	return n
}

func (s *sqlStore) MeshJobArtifact(id, ownerID string) []byte {
	j := s.GetMeshJob(id, ownerID)
	if j == nil || j.BlobRef == "" {
		return nil
	}
	data, err := s.getBlob(j.BlobRef)
	if err != nil {
		log.Printf("sqlStore: MeshJobArtifact getBlob: %v", err)
		return nil
	}
	return data
}

func (s *sqlStore) ResetStaleMeshJobs() {
	if _, err := s.db.Exec(
		`UPDATE mesh_jobs SET status = ?, started_at = NULL WHERE status = ?`,
		MeshQueued, MeshRunning); err != nil {
		log.Printf("sqlStore: ResetStaleMeshJobs: %v", err)
	}
}
