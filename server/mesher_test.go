// Integration test for the mesh-job queue (plan §P4 / M3): submit → dispatch
// against a FAKE mesher service → poll to done → download the artifact.

package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// meshTestEnv boots a memStore server with a running dispatcher pointed at a
// fake mesher, plus one registered user with one saved project.
func meshTestEnv(t *testing.T, mesher http.HandlerFunc) (ts *httptest.Server, client *http.Client, cookie *http.Cookie, projectID string) {
	t.Helper()
	fake := httptest.NewServer(mesher)
	t.Cleanup(fake.Close)

	store := NewStore()
	srv := NewServerWithTickets(store, store.(TicketStore))
	srv.startMeshDispatcher(fake.URL, 1)
	ts = httptest.NewServer(srv)
	t.Cleanup(ts.Close)

	client = &http.Client{}
	invite := newInvite(t, store)
	resp, err := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"mesh@example.com","password":"secret123","display_name":"M","invite":"`+invite+`"}`))
	if err != nil || resp.StatusCode != 200 {
		t.Fatalf("register failed: %v %v", err, resp)
	}
	cookie = resp.Cookies()[0]
	resp.Body.Close()

	// One project whose document holds a part tab "p1".
	doc := `{"tabs":[{"id":"p1","kind":"part","name":"Part 1","documentJson":"{\"features\":[]}"}],"activeTabId":"p1"}`
	req, _ := http.NewRequest("POST", ts.URL+"/api/projects",
		strings.NewReader(`{"name":"T","document":`+doc+`}`))
	req.Header.Set("Content-Type", "application/json")
	req.AddCookie(cookie)
	resp, err = client.Do(req)
	if err != nil || resp.StatusCode != http.StatusCreated {
		t.Fatalf("create project failed: %v %v", err, resp)
	}
	var p Project
	json.NewDecoder(resp.Body).Decode(&p)
	resp.Body.Close()
	return ts, client, cookie, p.ID
}

func meshPost(t *testing.T, client *http.Client, cookie *http.Cookie, url, body string) (*http.Response, []byte) {
	t.Helper()
	req, _ := http.NewRequest("POST", url, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.AddCookie(cookie)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	data, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	return resp, data
}

func meshGet(t *testing.T, client *http.Client, cookie *http.Cookie, url string) (*http.Response, []byte) {
	t.Helper()
	req, _ := http.NewRequest("GET", url, nil)
	req.AddCookie(cookie)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	data, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	return resp, data
}

func TestMeshJobRoundtrip(t *testing.T) {
	fakeSTL := []byte("FAKE-BINARY-STL")
	var gotDoc string
	ts, client, cookie, projectID := meshTestEnv(t, func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Document json.RawMessage `json:"document"`
			Params   json.RawMessage `json:"params"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		gotDoc = string(req.Document)
		w.Header().Set("X-Fd-Report", `{"triangles":42}`)
		w.Write(fakeSTL)
	})

	resp, body := meshPost(t, client, cookie,
		ts.URL+"/api/projects/"+projectID+"/mesh",
		`{"partId":"p1","params":{"cellSizeMm":0.5}}`)
	if resp.StatusCode != http.StatusAccepted {
		t.Fatalf("submit: got %d %s", resp.StatusCode, body)
	}
	var sub struct{ JobID string `json:"jobId"` }
	json.Unmarshal(body, &sub)
	if sub.JobID == "" {
		t.Fatalf("no jobId in %s", body)
	}

	// Poll to done.
	var status struct {
		Status string          `json:"status"`
		URL    string          `json:"url"`
		Report json.RawMessage `json:"report"`
		Error  string          `json:"error"`
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		_, body = meshGet(t, client, cookie, ts.URL+"/api/mesh-jobs/"+sub.JobID)
		json.Unmarshal(body, &status)
		if status.Status == "done" || status.Status == "error" {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("job stuck: %s", body)
		}
		time.Sleep(10 * time.Millisecond)
	}
	if status.Status != "done" {
		t.Fatalf("job failed: %s", body)
	}
	if gotDoc != `{"features":[]}` {
		t.Fatalf("mesher got document %q", gotDoc)
	}
	if string(status.Report) != `{"triangles":42}` {
		t.Fatalf("report = %s", status.Report)
	}

	// Download the artifact via the reported URL.
	resp, body = meshGet(t, client, cookie, ts.URL+status.URL)
	if resp.StatusCode != 200 || string(body) != string(fakeSTL) {
		t.Fatalf("artifact: %d %q", resp.StatusCode, body)
	}
	if ct := resp.Header.Get("Content-Type"); ct != "model/stl" {
		t.Fatalf("artifact content-type = %q", ct)
	}

	// Unauthenticated access must fail.
	r2, err := http.Get(ts.URL + status.URL)
	if err != nil {
		t.Fatal(err)
	}
	r2.Body.Close()
	if r2.StatusCode != http.StatusUnauthorized {
		t.Fatalf("unauthed artifact: got %d", r2.StatusCode)
	}
}

func TestMeshJobErrorAndLimits(t *testing.T) {
	block := make(chan struct{})
	ts, client, cookie, projectID := meshTestEnv(t, func(w http.ResponseWriter, r *http.Request) {
		<-block // hold the job "running" until released
		w.WriteHeader(http.StatusUnprocessableEntity)
		fmt.Fprint(w, `{"error":"mesh failed: no sign change"}`)
	})

	// Unknown part fails at submit time.
	resp, body := meshPost(t, client, cookie,
		ts.URL+"/api/projects/"+projectID+"/mesh", `{"partId":"nope"}`)
	if resp.StatusCode != http.StatusUnprocessableEntity {
		t.Fatalf("unknown part: got %d %s", resp.StatusCode, body)
	}

	// First job occupies the per-user slot…
	resp, body = meshPost(t, client, cookie,
		ts.URL+"/api/projects/"+projectID+"/mesh", `{"partId":"p1"}`)
	if resp.StatusCode != http.StatusAccepted {
		t.Fatalf("submit: got %d %s", resp.StatusCode, body)
	}
	var sub struct{ JobID string `json:"jobId"` }
	json.Unmarshal(body, &sub)

	// …so a second submit is rejected with 429.
	resp, _ = meshPost(t, client, cookie,
		ts.URL+"/api/projects/"+projectID+"/mesh", `{"partId":"p1"}`)
	if resp.StatusCode != http.StatusTooManyRequests {
		t.Fatalf("second submit: got %d", resp.StatusCode)
	}

	// Release the fake mesher; the job must end in error with the detail.
	close(block)
	var status struct {
		Status string `json:"status"`
		Error  string `json:"error"`
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		_, body = meshGet(t, client, cookie, ts.URL+"/api/mesh-jobs/"+sub.JobID)
		json.Unmarshal(body, &status)
		if status.Status == "error" {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("job stuck: %s", body)
		}
		time.Sleep(10 * time.Millisecond)
	}
	if !strings.Contains(status.Error, "no sign change") {
		t.Fatalf("error detail lost: %q", status.Error)
	}
}

func TestMeshJobCancelAndOwnership(t *testing.T) {
	// No dispatcher on this server: exercises the feature-off 503 and lets
	// queued jobs sit still so cancel/ownership can be tested race-free.
	store := NewStore()
	srv := NewServerWithTickets(store, store.(TicketStore)) // no startMeshDispatcher → mesh feature off
	ts := httptest.NewServer(srv)
	defer ts.Close()
	client := &http.Client{}
	invite := newInvite(t, store)
	resp, _ := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"a@example.com","password":"secret123","display_name":"A","invite":"`+invite+`"}`))
	cookie := resp.Cookies()[0]
	resp.Body.Close()

	// Feature off → 503 on submit.
	req, _ := http.NewRequest("POST", ts.URL+"/api/projects/xxx/mesh",
		strings.NewReader(`{"partId":"p1"}`))
	req.Header.Set("Content-Type", "application/json")
	req.AddCookie(cookie)
	resp, _ = client.Do(req)
	resp.Body.Close()
	if resp.StatusCode != http.StatusServiceUnavailable {
		t.Fatalf("feature off: got %d", resp.StatusCode)
	}

	// Queued job of user A: another user must neither see nor cancel it.
	jobA := store.CreateMeshJob("userA", "proj", "p1", "{}")
	if store.GetMeshJob(jobA.ID, "userB") != nil {
		t.Fatal("cross-user job visible")
	}
	if store.CancelMeshJob(jobA.ID, "userB") {
		t.Fatal("cross-user cancel allowed")
	}
	if !store.CancelMeshJob(jobA.ID, "userA") {
		t.Fatal("owner cancel refused")
	}
	if j := store.GetMeshJob(jobA.ID, "userA"); j.Status != MeshError || j.Error != "canceled" {
		t.Fatalf("canceled job = %+v", j)
	}
	// A canceled (finished) job is not claimable.
	if c := store.ClaimNextMeshJob(); c != nil {
		t.Fatalf("claimed canceled job %+v", c)
	}
}
