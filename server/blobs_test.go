package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// registerClient spins up a test server, registers one invite-gated user, and
// returns a cookie-jar client already carrying the session — the auth boilerplate
// every blob-endpoint test needs.
func registerClient(t *testing.T) (*httptest.Server, *http.Client, *http.Cookie) {
	t.Helper()
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	t.Cleanup(ts.Close)

	client := &http.Client{
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
	invite := newInvite(t, store)
	resp, err := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"blob@example.com","password":"secret123","display_name":"Blob","invite":"`+invite+`"}`))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("register: got %d", resp.StatusCode)
	}
	cookies := resp.Cookies()
	if len(cookies) == 0 {
		t.Fatal("no session cookie")
	}
	return ts, client, cookies[0]
}

func sha256HexOf(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

// putBlob PUTs raw bytes to /api/blobs/{hash}; returns the response.
func putBlob(t *testing.T, ts *httptest.Server, client *http.Client, cookie *http.Cookie, hash string, data []byte) *http.Response {
	t.Helper()
	req, _ := http.NewRequest("PUT", ts.URL+"/api/blobs/"+hash, bytes.NewReader(data))
	req.AddCookie(cookie)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	return resp
}

func TestBlobPutGetRoundTrip(t *testing.T) {
	ts, client, cookie := registerClient(t)
	data := []byte("hello mesh blob \x00\x01\x02")
	hash := sha256HexOf(data)

	// PUT new → 200, stored:true.
	resp := putBlob(t, ts, client, cookie, hash, data)
	if resp.StatusCode != 200 {
		t.Fatalf("PUT new: got %d", resp.StatusCode)
	}
	var body struct {
		Stored bool `json:"stored"`
	}
	json.NewDecoder(resp.Body).Decode(&body)
	resp.Body.Close()
	if !body.Stored {
		t.Fatalf("PUT new: stored = false, want true")
	}

	// GET → exact bytes back.
	req, _ := http.NewRequest("GET", ts.URL+"/api/blobs/"+hash, nil)
	req.AddCookie(cookie)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("GET: got %d", resp.StatusCode)
	}
	got, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if !bytes.Equal(got, data) {
		t.Fatalf("GET body = %q, want %q", got, data)
	}
	if ct := resp.Header.Get("Content-Type"); ct != "application/octet-stream" {
		t.Fatalf("GET content-type = %q", ct)
	}

	// PUT existing → 200, stored:false (no-op dedup).
	resp = putBlob(t, ts, client, cookie, hash, data)
	if resp.StatusCode != 200 {
		t.Fatalf("PUT existing: got %d", resp.StatusCode)
	}
	body.Stored = true
	json.NewDecoder(resp.Body).Decode(&body)
	resp.Body.Close()
	if body.Stored {
		t.Fatalf("PUT existing: stored = true, want false (no-op)")
	}
}

func TestBlobGetMissing404(t *testing.T) {
	ts, client, cookie := registerClient(t)
	hash := sha256HexOf([]byte("never uploaded"))
	req, _ := http.NewRequest("GET", ts.URL+"/api/blobs/"+hash, nil)
	req.AddCookie(cookie)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 404 {
		t.Fatalf("GET missing: got %d, want 404", resp.StatusCode)
	}
}

func TestBlobPutHashMismatch400(t *testing.T) {
	ts, client, cookie := registerClient(t)
	data := []byte("payload A")
	wrongHash := sha256HexOf([]byte("payload B")) // valid hex, wrong content
	resp := putBlob(t, ts, client, cookie, wrongHash, data)
	defer resp.Body.Close()
	if resp.StatusCode != 400 {
		t.Fatalf("PUT hash mismatch: got %d, want 400", resp.StatusCode)
	}
	// Nothing was stored.
	req, _ := http.NewRequest("GET", ts.URL+"/api/blobs/"+wrongHash, nil)
	req.AddCookie(cookie)
	g, _ := client.Do(req)
	if g.StatusCode != 404 {
		t.Fatalf("after mismatch, GET should 404, got %d", g.StatusCode)
	}
	g.Body.Close()
}

func TestBlobPutInvalidHash400(t *testing.T) {
	ts, client, cookie := registerClient(t)
	resp := putBlob(t, ts, client, cookie, "not-a-hash", []byte("x"))
	defer resp.Body.Close()
	if resp.StatusCode != 400 {
		t.Fatalf("PUT invalid hash: got %d, want 400", resp.StatusCode)
	}
}

func TestBlobMissingEndpoint(t *testing.T) {
	ts, client, cookie := registerClient(t)
	present := []byte("stored blob")
	presentHash := sha256HexOf(present)
	putBlob(t, ts, client, cookie, presentHash, present).Body.Close()

	absentHash := sha256HexOf([]byte("absent blob"))
	reqBody := `{"hashes":["` + presentHash + `","` + absentHash + `","` + presentHash + `"]}`
	req, _ := http.NewRequest("POST", ts.URL+"/api/blobs/missing", strings.NewReader(reqBody))
	req.AddCookie(cookie)
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("missing: got %d", resp.StatusCode)
	}
	var out struct {
		Missing []string `json:"missing"`
	}
	json.NewDecoder(resp.Body).Decode(&out)
	resp.Body.Close()
	if len(out.Missing) != 1 || out.Missing[0] != absentHash {
		t.Fatalf("missing = %v, want [%s]", out.Missing, absentHash)
	}
}

func TestBlobEndpointsRequireAuth(t *testing.T) {
	ts, client, cookie := registerClient(t)
	data := []byte("auth-gated")
	hash := sha256HexOf(data)
	// Seed one blob (authenticated) so GET has something to 401 on.
	putBlob(t, ts, client, cookie, hash, data).Body.Close()

	// No cookie on any endpoint → 401.
	cases := []struct {
		method, path, body string
	}{
		{"PUT", "/api/blobs/" + hash, "bytes"},
		{"GET", "/api/blobs/" + hash, ""},
		{"POST", "/api/blobs/missing", `{"hashes":["` + hash + `"]}`},
	}
	for _, c := range cases {
		var r io.Reader
		if c.body != "" {
			r = strings.NewReader(c.body)
		}
		req, _ := http.NewRequest(c.method, ts.URL+c.path, r)
		resp, err := client.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		if resp.StatusCode != 401 {
			t.Fatalf("%s %s without auth: got %d, want 401", c.method, c.path, resp.StatusCode)
		}
		resp.Body.Close()
	}
}
