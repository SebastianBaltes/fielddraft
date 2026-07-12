package main

// Onshape import proxy: the browser cannot talk to Onshape directly (no CORS
// headers, and the API secret must never reach a bundle), so the server signs
// and forwards the read-only calls the import dialog needs.
//
// Auth today is a single server-side API key — the operator's. That key is an
// *identity*, not a public-data pass: it reads whatever the operator's Onshape
// account may read. Two consequences the routes below depend on:
//   - every route is behind requireAuth (registration is invite-gated), so the
//     proxy is never an open relay signing arbitrary reads for strangers;
//   - the day a private document is shared with the operator, it becomes
//     reachable here with no code change.
//
// The swap-in for a multi-tenant world is per-user OAuth: replace signRequest
// with an Authorization: Bearer of the calling user's token and keep the
// handlers as they are.

import (
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"strings"
	"time"

	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
)

const onshapeMaxBody = 32 << 20 // features JSON of a large part studio

type onshapeClient struct {
	baseURL   string
	accessKey string
	secretKey string
	http      *http.Client
}

// newOnshapeClient returns nil when no key is configured; the routes then
// answer 503 rather than failing at start-up.
func newOnshapeClient() *onshapeClient {
	access := os.Getenv("ONSHAPE_ACCESS_KEY")
	secret := os.Getenv("ONSHAPE_SECRET_KEY")
	if access == "" || secret == "" {
		return nil
	}
	base := os.Getenv("ONSHAPE_URL")
	if base == "" {
		base = "https://cad.onshape.com"
	}
	return &onshapeClient{
		baseURL:   strings.TrimRight(base, "/"),
		accessKey: access,
		secretKey: secret,
		http:      &http.Client{Timeout: 30 * time.Second},
	}
}

// signRequest builds Onshape's HMAC-SHA256 headers.
//
// The signed string is lowercased *in full* — including the RFC-1123 date,
// which carries uppercase ("Mon, 13 Jul 2026 … GMT"). Signing it as-is yields a
// bare 401 "Unauthenticated API request" with no hint as to why.
func (c *onshapeClient) signRequest(req *http.Request, path, query string) {
	nonceBytes := make([]byte, 20)
	_, _ = rand.Read(nonceBytes)
	nonce := hex.EncodeToString(nonceBytes)
	date := time.Now().UTC().Format(http.TimeFormat)
	const contentType = "application/json"

	toSign := strings.ToLower(fmt.Sprintf("get\n%s\n%s\n%s\n%s\n%s\n",
		nonce, date, contentType, path, query))
	mac := hmac.New(sha256.New, []byte(c.secretKey))
	mac.Write([]byte(toSign))
	sig := base64.StdEncoding.EncodeToString(mac.Sum(nil))

	req.Header.Set("Date", date)
	req.Header.Set("On-Nonce", nonce)
	req.Header.Set("Content-Type", contentType)
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Authorization",
		"On "+c.accessKey+":HmacSHA256:"+sig)
}

// get performs a signed GET and returns the raw JSON body.
func (c *onshapeClient) get(path, query string) ([]byte, error) {
	url := c.baseURL + path
	if query != "" {
		url += "?" + query
	}
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	c.signRequest(req, path, query)

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	data, err := io.ReadAll(io.LimitReader(resp.Body, onshapeMaxBody))
	if err != nil {
		return nil, err
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("onshape GET %s: %d %s",
			path, resp.StatusCode, strings.TrimSpace(string(data)))
	}
	return data, nil
}

// onshapeRef is a document/workspace/element triple, as parsed by the client
// from a pasted Onshape URL (…/documents/{d}/w/{w}/e/{e}).
type onshapeRef struct {
	DocumentID  string `json:"documentId"`
	WorkspaceID string `json:"workspaceId"`
	ElementID   string `json:"elementId"`
}

// onshapeIDRe guards the path we interpolate into the upstream URL: Onshape IDs
// are 24-char hex. Rejecting anything else keeps a crafted "documentId" from
// reaching out to an attacker-chosen upstream path under our signature.
func validOnshapeID(id string) bool {
	if len(id) != 24 {
		return false
	}
	for _, r := range id {
		if !strings.ContainsRune("0123456789abcdefABCDEF", r) {
			return false
		}
	}
	return true
}

// onshapeGuard is the shared preamble of the routes below: authenticated user,
// configured key, valid IDs. Returns false when it has already written a reply.
func (s *Server) onshapeGuard(w http.ResponseWriter, r *http.Request, ref *onshapeRef) bool {
	_, user := s.requireAuth(r)
	if user == nil {
		writeErr(w, http.StatusUnauthorized, "not authenticated")
		return false
	}
	if s.onshape == nil {
		writeErr(w, http.StatusServiceUnavailable, "onshape not configured")
		return false
	}
	q := r.URL.Query()
	ref.DocumentID = q.Get("documentId")
	ref.WorkspaceID = q.Get("workspaceId")
	ref.ElementID = q.Get("elementId")
	if !validOnshapeID(ref.DocumentID) || !validOnshapeID(ref.WorkspaceID) {
		writeErr(w, http.StatusBadRequest, "invalid document or workspace id")
		return false
	}
	return true
}

// handleOnshapeElements lists the elements (tabs) of a document, so the import
// dialog can offer the part studios of a pasted URL.
func (s *Server) handleOnshapeElements(w http.ResponseWriter, r *http.Request) {
	var ref onshapeRef
	if !s.onshapeGuard(w, r, &ref) {
		return
	}
	path := fmt.Sprintf("/api/documents/d/%s/w/%s/elements",
		ref.DocumentID, ref.WorkspaceID)
	data, err := s.onshape.get(path, "")
	if err != nil {
		log.Printf("onshape elements: %v", err)
		writeErr(w, http.StatusBadGateway, "onshape request failed")
		return
	}
	writeOnshapeJSON(w, data)
}

// handleOnshapeFeatures returns the feature tree (incl. sketches) of a part
// studio. This is the payload the sketch translator consumes.
func (s *Server) handleOnshapeFeatures(w http.ResponseWriter, r *http.Request) {
	var ref onshapeRef
	if !s.onshapeGuard(w, r, &ref) {
		return
	}
	if !validOnshapeID(ref.ElementID) {
		writeErr(w, http.StatusBadRequest, "invalid element id")
		return
	}
	path := fmt.Sprintf("/api/v9/partstudios/d/%s/w/%s/e/%s/features",
		ref.DocumentID, ref.WorkspaceID, ref.ElementID)
	data, err := s.onshape.get(path, "")
	if err != nil {
		log.Printf("onshape features: %v", err)
		writeErr(w, http.StatusBadGateway, "onshape request failed")
		return
	}
	writeOnshapeJSON(w, data)
}

// writeOnshapeJSON forwards an upstream body that we have already validated as
// 2xx. It is passed through verbatim (it is Onshape's JSON, not ours), but only
// after json.Valid — never hand the client bytes we did not confirm are JSON.
func writeOnshapeJSON(w http.ResponseWriter, data []byte) {
	if !json.Valid(data) {
		writeErr(w, http.StatusBadGateway, "onshape returned malformed json")
		return
	}
	_, _ = w.Write(data)
}
