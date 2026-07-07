// memStore implementation of the mesh-job queue (plan §P4) — test fixture.

package main

import "time"

func (s *memStore) ensureMeshMaps() {
	if s.meshJobs == nil {
		s.meshJobs = make(map[string]*MeshJob)
		s.meshBlobs = make(map[string][]byte)
	}
}

func (s *memStore) CreateMeshJob(ownerID, projectID, partID, paramsJSON string) *MeshJob {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ensureMeshMaps()
	j := &MeshJob{
		ID:         newID(),
		OwnerID:    ownerID,
		ProjectID:  projectID,
		PartID:     partID,
		ParamsJSON: paramsJSON,
		Status:     MeshQueued,
		CreatedAt:  time.Now(),
	}
	s.meshJobs[j.ID] = j
	s.meshSeq = append(s.meshSeq, j.ID)
	return j
}

func (s *memStore) GetMeshJob(id, ownerID string) *MeshJob {
	s.mu.RLock()
	defer s.mu.RUnlock()
	j := s.meshJobs[id]
	if j == nil || j.OwnerID != ownerID {
		return nil
	}
	cp := *j
	return &cp
}

func (s *memStore) ClaimNextMeshJob() *MeshJob {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, id := range s.meshSeq {
		j := s.meshJobs[id]
		if j != nil && j.Status == MeshQueued {
			at := time.Now()
			j.Status = MeshRunning
			j.StartedAt = &at
			cp := *j
			return &cp
		}
	}
	return nil
}

func (s *memStore) FinishMeshJob(id string, status MeshJobStatus, stl []byte, reportJSON, errMsg string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	j := s.meshJobs[id]
	if j == nil {
		return
	}
	at := time.Now()
	j.Status = status
	j.ReportJSON = reportJSON
	j.Error = errMsg
	j.FinishedAt = &at
	if stl != nil {
		j.BlobRef = id
		s.meshBlobs[id] = stl
	}
}

func (s *memStore) CancelMeshJob(id, ownerID string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	j := s.meshJobs[id]
	if j == nil || j.OwnerID != ownerID || j.Status != MeshQueued {
		return false
	}
	at := time.Now()
	j.Status = MeshError
	j.Error = "canceled"
	j.FinishedAt = &at
	return true
}

func (s *memStore) ActiveMeshJobs(ownerID string) int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	n := 0
	for _, j := range s.meshJobs {
		if j.OwnerID == ownerID && (j.Status == MeshQueued || j.Status == MeshRunning) {
			n++
		}
	}
	return n
}

func (s *memStore) MeshJobArtifact(id, ownerID string) []byte {
	s.mu.RLock()
	defer s.mu.RUnlock()
	j := s.meshJobs[id]
	if j == nil || j.OwnerID != ownerID || j.BlobRef == "" {
		return nil
	}
	return s.meshBlobs[j.BlobRef]
}

func (s *memStore) ResetStaleMeshJobs() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, j := range s.meshJobs {
		if j.Status == MeshRunning {
			j.Status = MeshQueued
			j.StartedAt = nil
		}
	}
}
