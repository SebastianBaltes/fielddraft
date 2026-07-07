// Server-side CGAL meshing: job routes + dispatcher (plan §P4 / M3).
//
// POST /api/projects/{id}/mesh  {partId, params} → 202 {jobId}   (owner-gated)
// GET  /api/mesh-jobs/{id}                       → job status (poll-cheap)
// GET  /api/mesh-jobs/{id}/artifact              → binary STL download
// DELETE /api/mesh-jobs/{id}                     → cancel (queued only)
//
// The dispatcher is a small worker pool inside the server process: it claims
// queued jobs FIFO from the store (atomic claim — safe across replicas),
// extracts the part document from the SAVED project, POSTs it to the mesher
// service (MESHER_URL) and stores the STL as a content-addressed blob.
// MESHER_URL empty = feature off, the POST route answers 503.

package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"strconv"
	"time"
)

// meshDispatch is the running dispatcher's handle (nil = feature off).
type meshDispatch struct {
	store     Store
	mesherURL string
	client    *http.Client
	wake      chan struct{}
}

var meshMaxParamsBytes = 4 << 10

// startMeshDispatcher launches `workers` claim loops. A wake signal is sent
// on job creation; a ticker sweeps every few seconds for jobs that arrived
// via another replica or survived a restart.
func (s *Server) startMeshDispatcher(mesherURL string, workers int) {
	if mesherURL == "" {
		log.Printf("mesh: MESHER_URL not set, server-side meshing disabled")
		return
	}
	if workers < 1 {
		workers = 1
	}
	s.store.ResetStaleMeshJobs()
	d := &meshDispatch{
		store:     s.store,
		mesherURL: mesherURL,
		client:    &http.Client{Timeout: meshJobTimeout()},
		wake:      make(chan struct{}, 16),
	}
	s.mesh = d
	for range workers {
		go d.loop()
	}
	log.Printf("mesh: dispatcher up (%d workers → %s)", workers, mesherURL)
}

func meshJobTimeout() time.Duration {
	if v := os.Getenv("MESH_JOB_TIMEOUT_SEC"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return time.Duration(n) * time.Second
		}
	}
	return 330 * time.Second // slightly above the mesher's own 300 s cap
}

func (d *meshDispatch) loop() {
	tick := time.NewTicker(5 * time.Second)
	defer tick.Stop()
	for {
		for {
			job := d.store.ClaimNextMeshJob()
			if job == nil {
				break
			}
			d.process(job)
		}
		select {
		case <-d.wake:
		case <-tick.C:
		}
	}
}

// process runs one claimed job to done/error. Never panics the loop.
func (d *meshDispatch) process(job *MeshJob) {
	start := time.Now()
	fail := func(format string, args ...any) {
		msg := fmt.Sprintf(format, args...)
		log.Printf("mesh job %s: %s", job.ID, msg)
		d.store.FinishMeshJob(job.ID, MeshError, nil, "", msg)
	}

	project := d.store.GetProject(job.ProjectID, job.OwnerID)
	if project == nil {
		fail("project %s gone", job.ProjectID)
		return
	}
	partDoc, err := extractPartDocument(project.Document, job.PartID)
	if err != nil {
		fail("%v", err)
		return
	}

	reqBody, err := json.Marshal(map[string]json.RawMessage{
		"document": partDoc,
		"params":   json.RawMessage(job.ParamsJSON),
	})
	if err != nil {
		fail("marshal request: %v", err)
		return
	}
	resp, err := d.client.Post(d.mesherURL+"/mesh", "application/json",
		bytes.NewReader(reqBody))
	if err != nil {
		fail("mesher unreachable: %v", err)
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4<<10))
		fail("mesher %d: %s", resp.StatusCode, bytes.TrimSpace(body))
		return
	}
	stl, err := io.ReadAll(io.LimitReader(resp.Body, 512<<20))
	if err != nil {
		fail("read mesher response: %v", err)
		return
	}
	report := resp.Header.Get("X-Fd-Report")
	d.store.FinishMeshJob(job.ID, MeshDone, stl, report, "")
	log.Printf("mesh job %s: done, %d bytes in %s", job.ID, len(stl),
		time.Since(start).Round(time.Millisecond))
}

// extractPartDocument pulls one part tab's kernel document out of a saved
// project document ({tabs: [{id, kind: "part", documentJson}, …]}).
func extractPartDocument(projectDoc json.RawMessage, partID string) (json.RawMessage, error) {
	var doc struct {
		Tabs []struct {
			ID           string `json:"id"`
			Kind         string `json:"kind"`
			DocumentJSON string `json:"documentJson"`
		} `json:"tabs"`
	}
	if err := json.Unmarshal(projectDoc, &doc); err != nil {
		return nil, fmt.Errorf("project document unreadable: %v", err)
	}
	for _, t := range doc.Tabs {
		if t.ID == partID && t.Kind == "part" {
			if !json.Valid([]byte(t.DocumentJSON)) {
				return nil, fmt.Errorf("part %s: documentJson is not valid JSON", partID)
			}
			return json.RawMessage(t.DocumentJSON), nil
		}
	}
	return nil, fmt.Errorf("part %s not found in project", partID)
}

// ─── handlers ───────────────────────────────────────────────────

func (s *Server) handleCreateMeshJob(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	if s.mesh == nil {
		writeErr(w, http.StatusServiceUnavailable, "mesher not configured")
		return
	}
	project := s.store.GetProject(r.PathValue("id"), user.ID)
	if project == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	var body struct {
		PartID string          `json:"partId"`
		Params json.RawMessage `json:"params"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.PartID == "" {
		writeErr(w, http.StatusBadRequest, "partId required")
		return
	}
	params := "{}"
	if len(body.Params) > 0 {
		if len(body.Params) > meshMaxParamsBytes || !json.Valid(body.Params) {
			writeErr(w, http.StatusBadRequest, "invalid params")
			return
		}
		params = string(body.Params)
	}
	// The part must exist in the SAVED document — meshing runs against the
	// server-side project state, so an unknown partId means the client did
	// not sync (or sent garbage). Fail at submit time, not in the worker.
	if _, err := extractPartDocument(project.Document, body.PartID); err != nil {
		writeErr(w, http.StatusUnprocessableEntity, err.Error())
		return
	}
	if s.store.ActiveMeshJobs(user.ID) >= 1 {
		writeErr(w, http.StatusTooManyRequests, "a mesh job is already queued or running")
		return
	}
	job := s.store.CreateMeshJob(user.ID, project.ID, body.PartID, params)
	if job == nil {
		writeErr(w, http.StatusInternalServerError, "could not create job")
		return
	}
	select {
	case s.mesh.wake <- struct{}{}:
	default:
	}
	w.WriteHeader(http.StatusAccepted)
	json.NewEncoder(w).Encode(map[string]string{"jobId": job.ID})
}

func (s *Server) handleGetMeshJob(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	job := s.store.GetMeshJob(r.PathValue("id"), user.ID)
	if job == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	resp := map[string]any{
		"id":     job.ID,
		"status": job.Status,
	}
	if job.Status == MeshDone {
		resp["url"] = "/api/mesh-jobs/" + job.ID + "/artifact"
		if job.ReportJSON != "" {
			resp["report"] = json.RawMessage(job.ReportJSON)
		}
	}
	if job.Status == MeshError {
		resp["error"] = job.Error
	}
	json.NewEncoder(w).Encode(resp)
}

func (s *Server) handleGetMeshArtifact(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	data := s.store.MeshJobArtifact(r.PathValue("id"), user.ID)
	if data == nil {
		writeErr(w, http.StatusNotFound, "not found")
		return
	}
	w.Header().Set("Content-Type", "model/stl")
	w.Header().Set("Content-Disposition", "attachment; filename=\"mesh.stl\"")
	w.Header().Set("Content-Length", strconv.Itoa(len(data)))
	w.Write(data)
}

func (s *Server) handleCancelMeshJob(w http.ResponseWriter, r *http.Request) {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return
	}
	if !s.store.CancelMeshJob(r.PathValue("id"), user.ID) {
		writeErr(w, http.StatusConflict, "job not cancelable")
		return
	}
	json.NewEncoder(w).Encode(map[string]string{"status": "canceled"})
}
