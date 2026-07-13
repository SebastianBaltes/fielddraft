package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// onshapeTestEnv wires a fake Onshape upstream into a server and registers a
// user, returning the session cookie. handler sees the signed request, so a
// test can assert on the signature the same way Onshape does.
func onshapeTestEnv(t *testing.T, handler http.HandlerFunc) (*httptest.Server, *http.Cookie) {
	t.Helper()
	upstream := httptest.NewServer(handler)
	t.Cleanup(upstream.Close)

	store := NewStore()
	srv := NewServer(store)
	srv.onshape = &onshapeClient{
		baseURL:   upstream.URL,
		accessKey: "test-access",
		secretKey: "test-secret",
		http:      upstream.Client(),
	}
	ts := httptest.NewServer(srv)
	t.Cleanup(ts.Close)

	invite := newInvite(t, store)
	resp, err := http.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"o@example.com","password":"secret123","display_name":"O","invite":"`+invite+`"}`))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("register: got %d", resp.StatusCode)
	}
	if len(resp.Cookies()) == 0 {
		t.Fatal("no session cookie")
	}
	return ts, resp.Cookies()[0]
}

func getWith(t *testing.T, ts *httptest.Server, path string, cookie *http.Cookie) (int, string) {
	t.Helper()
	req, err := http.NewRequest(http.MethodGet, ts.URL+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	if cookie != nil {
		req.AddCookie(cookie)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	return resp.StatusCode, string(body)
}

const (
	testDoc  = "4646f5c34c7fce51b5e015b1"
	testWs   = "df2ec03857256ae2247d57e6"
	testElem = "6b5cf69f7aeb6f9c6caf393d"
)

func featuresPath() string {
	return "/api/onshape/features?documentId=" + testDoc +
		"&workspaceId=" + testWs + "&elementId=" + testElem
}

// The proxy signs with our key, so an unauthenticated caller must never get
// through — otherwise it is an open relay reading Onshape as the operator.
func TestOnshapeRequiresAuth(t *testing.T) {
	reached := false
	ts, _ := onshapeTestEnv(t, func(w http.ResponseWriter, _ *http.Request) {
		reached = true
		_, _ = w.Write([]byte(`{}`))
	})
	code, _ := getWith(t, ts, featuresPath(), nil)
	if code != http.StatusUnauthorized {
		t.Fatalf("want 401 without session, got %d", code)
	}
	if reached {
		t.Fatal("unauthenticated request reached Onshape — proxy is an open relay")
	}
}

func TestOnshapeUnconfiguredIs503(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	srv.onshape = nil // no API key in the environment
	ts := httptest.NewServer(srv)
	defer ts.Close()

	invite := newInvite(t, store)
	resp, err := http.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"n@example.com","password":"secret123","display_name":"N","invite":"`+invite+`"}`))
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()

	code, _ := getWith(t, ts, featuresPath(), resp.Cookies()[0])
	if code != http.StatusServiceUnavailable {
		t.Fatalf("want 503 when unconfigured, got %d", code)
	}
}

// Onshape lowercases the whole signed string — date and nonce included. Getting
// this wrong yields a bare 401 upstream, so the test reproduces Onshape's own
// verification rather than merely asserting the header exists.
func TestOnshapeSignatureMatchesOnshapesOwnRecipe(t *testing.T) {
	var sigOK bool
	ts, cookie := onshapeTestEnv(t, func(w http.ResponseWriter, r *http.Request) {
		date := r.Header.Get("Date")
		nonce := r.Header.Get("On-Nonce")
		query := r.URL.RawQuery
		toSign := strings.ToLower("get\n" + nonce + "\n" + date + "\n" +
			"application/json\n" + r.URL.Path + "\n" + query + "\n")
		mac := hmac.New(sha256.New, []byte("test-secret"))
		mac.Write([]byte(toSign))
		want := "On test-access:HmacSHA256:" +
			base64.StdEncoding.EncodeToString(mac.Sum(nil))
		sigOK = r.Header.Get("Authorization") == want
		_, _ = w.Write([]byte(`{"features":[]}`))
	})

	code, body := getWith(t, ts, featuresPath(), cookie)
	if code != http.StatusOK {
		t.Fatalf("want 200, got %d: %s", code, body)
	}
	if !sigOK {
		t.Fatal("HMAC signature did not match Onshape's recipe")
	}
	if !strings.Contains(body, `"features"`) {
		t.Fatalf("upstream body not forwarded: %s", body)
	}
}

// A crafted id must not steer the signed request at an attacker-chosen path.
func TestOnshapeRejectsMalformedIDs(t *testing.T) {
	reached := false
	ts, cookie := onshapeTestEnv(t, func(w http.ResponseWriter, _ *http.Request) {
		reached = true
		_, _ = w.Write([]byte(`{}`))
	})
	bad := "/api/onshape/features?documentId=../../users/sessioninfo" +
		"&workspaceId=" + testWs + "&elementId=" + testElem
	code, _ := getWith(t, ts, bad, cookie)
	if code != http.StatusBadRequest {
		t.Fatalf("want 400 for a malformed id, got %d", code)
	}
	if reached {
		t.Fatal("malformed id reached Onshape")
	}
}

// Upstream failures must not leak Onshape's body (which can carry account
// detail) to the client.
func TestOnshapeUpstreamErrorIsNotLeaked(t *testing.T) {
	ts, cookie := onshapeTestEnv(t, func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusForbidden)
		_, _ = w.Write([]byte(`{"message":"secret account detail"}`))
	})
	code, body := getWith(t, ts, featuresPath(), cookie)
	if code != http.StatusBadGateway {
		t.Fatalf("want 502 on upstream failure, got %d", code)
	}
	if strings.Contains(body, "secret account detail") {
		t.Fatalf("upstream error body leaked to client: %s", body)
	}
}
