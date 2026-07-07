// fd-mesher-serve — minimal HTTP wrapper around fd-prepare + fd-cgal-mesh
// (docs/CGAL-SERVER-MESHING-PLAN.md, P3).
//
// POST /mesh  {"document": <part document json>, "params": {"cellSizeMm":…}}
//   → 200 binary STL (application/octet-stream), mesher report JSON in the
//     X-Fd-Report response header
//   → 4xx/5xx {"error": …}
// GET /healthz → 200 ok
//
// No auth: the service lives on the internal compose network only and is
// never exposed. Hard limits instead: request size cap, a concurrency
// semaphore and a per-job process timeout.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"time"
)

var (
	prepareBin = envStr("FD_PREPARE_BIN", "/usr/local/bin/fd-prepare")
	meshBin    = envStr("FD_MESH_BIN", "/usr/local/bin/fd-cgal-mesh")
	maxBody    = int64(envInt("FD_MESH_MAX_BODY_MB", 50)) << 20
	jobTimeout = time.Duration(envInt("FD_MESH_TIMEOUT_SEC", 300)) * time.Second
	slots      chan struct{}
)

func envStr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envInt(key string, def int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return n
		}
	}
	return def
}

func httpError(w http.ResponseWriter, status int, format string, args ...any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	msg, _ := json.Marshal(fmt.Sprintf(format, args...))
	fmt.Fprintf(w, "{\"error\":%s}\n", msg)
}

// run executes one CLI step under the job deadline and returns its stderr
// (fd-cgal-mesh emits its report JSON there).
func run(ctx context.Context, stderr *bytes.Buffer, bin string, args ...string) error {
	cmd := exec.CommandContext(ctx, bin, args...)
	cmd.Stderr = stderr
	if err := cmd.Run(); err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return fmt.Errorf("timeout after %s", jobTimeout)
		}
		return fmt.Errorf("%s: %w", filepath.Base(bin), err)
	}
	return nil
}

func handleMesh(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		httpError(w, http.StatusMethodNotAllowed, "POST only")
		return
	}
	select {
	case slots <- struct{}{}:
		defer func() { <-slots }()
	default:
		httpError(w, http.StatusTooManyRequests, "mesher busy, retry later")
		return
	}

	var req struct {
		Document json.RawMessage `json:"document"`
		Params   json.RawMessage `json:"params"`
	}
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxBody))
	if err := dec.Decode(&req); err != nil {
		httpError(w, http.StatusBadRequest, "bad request body: %v", err)
		return
	}
	if len(req.Document) == 0 {
		httpError(w, http.StatusBadRequest, "missing \"document\"")
		return
	}
	params := "{}"
	if len(req.Params) > 0 {
		params = string(req.Params)
	}

	dir, err := os.MkdirTemp("", "fdmesh-*")
	if err != nil {
		httpError(w, http.StatusInternalServerError, "tempdir: %v", err)
		return
	}
	defer os.RemoveAll(dir)
	docPath := filepath.Join(dir, "document.json")
	jobPath := filepath.Join(dir, "job.fdmesh")
	stlPath := filepath.Join(dir, "out.stl")
	if err := os.WriteFile(docPath, req.Document, 0o600); err != nil {
		httpError(w, http.StatusInternalServerError, "write document: %v", err)
		return
	}

	ctx, cancel := context.WithTimeout(r.Context(), jobTimeout)
	defer cancel()
	start := time.Now()

	// Step 1: document → interchange file. fd-prepare writes to stdout.
	var prepErr bytes.Buffer
	prep := exec.CommandContext(ctx, prepareBin, docPath, params)
	prep.Stderr = &prepErr
	jobFile, err := os.Create(jobPath)
	if err != nil {
		httpError(w, http.StatusInternalServerError, "create job file: %v", err)
		return
	}
	prep.Stdout = jobFile
	err = prep.Run()
	jobFile.Close()
	if err != nil {
		log.Printf("prepare failed: %v; stderr: %s", err, prepErr.String())
		httpError(w, http.StatusUnprocessableEntity, "prepare failed: %v: %s",
			err, prepErr.String())
		return
	}

	// Step 2: interchange file → STL. Report JSON lands on stderr.
	var meshErr bytes.Buffer
	if err := run(ctx, &meshErr, meshBin, jobPath, stlPath); err != nil {
		log.Printf("mesh failed: %v; stderr: %s", err, meshErr.String())
		httpError(w, http.StatusUnprocessableEntity, "mesh failed: %v: %s",
			err, lastLine(meshErr.Bytes()))
		return
	}

	stl, err := os.ReadFile(stlPath)
	if err != nil {
		httpError(w, http.StatusInternalServerError, "read stl: %v", err)
		return
	}
	report := lastLine(meshErr.Bytes())
	log.Printf("meshed %d bytes in %s; report %s", len(stl),
		time.Since(start).Round(time.Millisecond), report)

	w.Header().Set("Content-Type", "application/octet-stream")
	w.Header().Set("X-Fd-Report", report)
	w.Header().Set("Content-Length", strconv.Itoa(len(stl)))
	w.Write(stl)
}

// lastLine extracts the final non-empty line — fd-cgal-mesh logs progress
// markers first and the report JSON last.
func lastLine(b []byte) string {
	lines := bytes.Split(bytes.TrimSpace(b), []byte("\n"))
	if len(lines) == 0 {
		return "{}"
	}
	return string(bytes.TrimSpace(lines[len(lines)-1]))
}

func main() {
	slots = make(chan struct{}, envInt("FD_MESH_CONCURRENCY", 2))
	addr := envStr("FD_MESH_ADDR", ":9090")
	http.HandleFunc("/mesh", handleMesh)
	http.HandleFunc("/healthz", func(w http.ResponseWriter, _ *http.Request) {
		fmt.Fprintln(w, "ok")
	})
	log.Printf("fd-mesher-serve on %s (concurrency=%d, timeout=%s, max=%dMB)",
		addr, cap(slots), jobTimeout, maxBody>>20)
	log.Fatal(http.ListenAndServe(addr, nil))
}
