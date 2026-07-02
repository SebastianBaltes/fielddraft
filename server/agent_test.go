package main

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

const (
	testPOToken     = "po-secret-token-0123456789"
	testCodingToken = "coding-secret-token-9876543210"
)

// agentEnv is a server backed by the in-memory store with both Service-Tokens
// configured, plus a registered plain user who can author tickets.
type agentEnv struct {
	store   Store
	tickets TicketStore // same underlying store, ticket-domain view (WP15)
	srv     *httptest.Server
	client  *http.Client
	cookies []*http.Cookie
	userID  string
}

func newAgentEnv(t *testing.T) *agentEnv {
	t.Helper()
	// Configure the agent tokens BEFORE NewServer reads them.
	t.Setenv("PO_SERVICE_TOKEN", testPOToken)
	t.Setenv("CODING_SERVICE_TOKEN", testCodingToken)

	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	t.Cleanup(ts.Close)

	client := &http.Client{
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
	resp, err := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"agentuser@test.com","password":"pass12345","display_name":"U","invite":"`+newInvite(t, store)+`"}`))
	if err != nil {
		t.Fatalf("register: %v", err)
	}
	var u User
	json.NewDecoder(resp.Body).Decode(&u)
	cookies := resp.Cookies()
	resp.Body.Close()
	return &agentEnv{store: store, tickets: store.(TicketStore), srv: ts, client: client, cookies: cookies, userID: u.ID}
}

// agentReq issues a request with an optional Bearer token and optional session
// cookies.
func (e *agentEnv) agentReq(t *testing.T, method, path, token, body string, cookies []*http.Cookie) *http.Response {
	t.Helper()
	var r *http.Request
	var err error
	if body == "" {
		r, err = http.NewRequest(method, e.srv.URL+path, nil)
	} else {
		r, err = http.NewRequest(method, e.srv.URL+path, strings.NewReader(body))
	}
	if err != nil {
		t.Fatalf("new request: %v", err)
	}
	r.Header.Set("Content-Type", "application/json")
	if token != "" {
		r.Header.Set("Authorization", "Bearer "+token)
	}
	for _, c := range cookies {
		r.AddCookie(c)
	}
	resp, err := e.client.Do(r)
	if err != nil {
		t.Fatalf("%s %s: %v", method, path, err)
	}
	return resp
}

// createTicket makes a ticket as the registered user and returns its id.
func (e *agentEnv) createTicket(t *testing.T, body string) string {
	t.Helper()
	resp := e.agentReq(t, "POST", "/api/tickets", "", body, e.cookies)
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create ticket: got %d", resp.StatusCode)
	}
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	return tk.ID
}

// ─── auth model ─────────────────────────────────────────────────

func TestAgentAuth(t *testing.T) {
	e := newAgentEnv(t)

	// Missing token → 401.
	resp := e.agentReq(t, "GET", "/api/agent/queue", "", "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("no token: got %d, want 401", resp.StatusCode)
	}

	// Wrong token → 401.
	resp = e.agentReq(t, "GET", "/api/agent/queue", "totally-bogus", "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("bad token: got %d, want 401", resp.StatusCode)
	}

	// Correct PO token → 200 on a po-or-coding route.
	resp = e.agentReq(t, "GET", "/api/agent/queue", testPOToken, "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("po queue: got %d, want 200", resp.StatusCode)
	}

	// Wrong scope: PO token on a coding-only route (create deploy) → 403.
	resp = e.agentReq(t, "POST", "/api/deploys", testPOToken,
		`{"version":"1.0","git_sha":"abc","status":"live"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("po on coding route: got %d, want 403", resp.StatusCode)
	}

	// Correct coding token on the coding route → 201.
	resp = e.agentReq(t, "POST", "/api/deploys", testCodingToken,
		`{"version":"1.0","git_sha":"abc","status":"live"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("coding deploy: got %d, want 201", resp.StatusCode)
	}
}

// TestAgentAuthDisabledWhenUnset — with no tokens configured, every token is
// rejected (the agent API is off).
func TestAgentAuthDisabledWhenUnset(t *testing.T) {
	t.Setenv("PO_SERVICE_TOKEN", "")
	t.Setenv("CODING_SERVICE_TOKEN", "")
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	req, _ := http.NewRequest("GET", ts.URL+"/api/agent/queue", nil)
	req.Header.Set("Authorization", "Bearer ") // empty token
	resp, _ := http.DefaultClient.Do(req)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("empty token with disabled API: got %d, want 401", resp.StatusCode)
	}
}

// ─── verdict → accepted → work-order ────────────────────────────

func TestVerdictAcceptedCreatesWorkOrder(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"Extrude crash","body":"crashes on extrude"}`)

	verdict := `{
		"spam": false, "malicious": false, "vision_fit": "fit",
		"confidence": 0.9, "category": "bug",
		"reason": "legit repro", "public_reply": "Thanks, we are on it!",
		"next_state": "accepted",
		"work_order": {"ticket_id":"` + id + `","type":"bug","summary":"fix extrude crash","acceptance_criteria":["no crash"]}
	}`
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken, verdict, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("verdict: got %d, want 200", resp.StatusCode)
	}
	var out struct {
		Ticket    *Ticket    `json:"ticket"`
		WorkOrder *WorkOrder `json:"work_order"`
	}
	json.NewDecoder(resp.Body).Decode(&out)
	resp.Body.Close()

	// State moved to accepted, classification recorded.
	if out.Ticket.State != StateAccepted {
		t.Fatalf("state = %q, want accepted", out.Ticket.State)
	}
	if out.Ticket.Category == nil || *out.Ticket.Category != "bug" {
		t.Fatalf("category = %v, want bug", out.Ticket.Category)
	}
	if out.Ticket.POVisionFit == nil || *out.Ticket.POVisionFit != "fit" {
		t.Fatalf("vision_fit = %v, want fit", out.Ticket.POVisionFit)
	}
	if out.Ticket.POVerdictAt == nil {
		t.Fatal("po_verdict_at not set")
	}
	// Work-order created.
	if out.WorkOrder == nil || out.WorkOrder.TicketID != id {
		t.Fatalf("work-order not created: %+v", out.WorkOrder)
	}
	if e.tickets.GetWorkOrder(id) == nil {
		t.Fatal("work-order not in store")
	}

	// A visible po comment carries the public reply.
	full := e.tickets.GetTicket(id)
	var foundPO bool
	for _, c := range full.Comments {
		if c.AuthorType == AuthorPO && !c.Withheld && strings.Contains(c.Body, "on it") {
			foundPO = true
		}
	}
	if !foundPO {
		t.Fatal("public po reply not posted as a visible comment")
	}
}

// TestVerdictAcceptedRequiresWorkOrder — accepting without a work_order is 400.
func TestVerdictAcceptedRequiresWorkOrder(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)

	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken,
		`{"vision_fit":"fit","category":"bug","next_state":"accepted"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("accept without work_order: got %d, want 400", resp.StatusCode)
	}
}

// TestVerdictDeclined — a decline transitions state, posts the reply, and must
// NOT carry a work_order.
func TestVerdictDeclined(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)

	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken,
		`{"vision_fit":"off","category":"feature","reason":"out of scope","public_reply":"Not a fit, sorry!","next_state":"declined"}`, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("decline verdict: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
	if e.tickets.GetTicket(id).State != StateDeclined {
		t.Fatal("state not declined")
	}
	if e.tickets.GetWorkOrder(id) != nil {
		t.Fatal("declined ticket should have no work-order")
	}

	// Providing a work_order on a non-accept verdict is rejected.
	id2 := e.createTicket(t, `{"title":"a","body":"b"}`)
	resp = e.agentReq(t, "POST", "/api/tickets/"+id2+"/verdict", testPOToken,
		`{"next_state":"declined","work_order":{"x":1}}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("decline with work_order: got %d, want 400", resp.StatusCode)
	}
}

// ─── leak-filter on public_reply ────────────────────────────────

func TestVerdictPublicReplyLeakFiltered(t *testing.T) {
	e := newAgentEnv(t)

	// Tripping reply (repo path) → comment stored withheld, not user-visible.
	id := e.createTicket(t, `{"title":"x","body":"y"}`)
	leak := `{"next_state":"needs_info","public_reply":"The bug is in server/main.go, will patch."}`
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken, leak, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("leak verdict: got %d, want 200", resp.StatusCode)
	}
	if !commentWithheld(e.tickets.GetTicket(id).Comments, AuthorPO) {
		t.Fatal("leaky po reply was NOT withheld")
	}
	// The user (non-admin) must not see the withheld comment via the HTTP view.
	getResp := e.agentReq(t, "GET", "/api/tickets/"+id, "", "", e.cookies)
	var detail ticketDetailResponse
	json.NewDecoder(getResp.Body).Decode(&detail)
	getResp.Body.Close()
	for _, c := range detail.Comments {
		if c.AuthorType == AuthorPO {
			t.Fatal("withheld po comment leaked to the user view")
		}
	}

	// Clean reply → visible.
	id2 := e.createTicket(t, `{"title":"a","body":"b"}`)
	clean := `{"next_state":"needs_info","public_reply":"Could you tell us which OS you are on?"}`
	resp = e.agentReq(t, "POST", "/api/tickets/"+id2+"/verdict", testPOToken, clean, nil)
	resp.Body.Close()
	if commentWithheld(e.tickets.GetTicket(id2).Comments, AuthorPO) {
		t.Fatal("clean po reply was wrongly withheld")
	}
}

// commentWithheld reports whether the latest comment of the given author type
// is withheld.
func commentWithheld(comments []*TicketComment, at AuthorType) bool {
	for i := len(comments) - 1; i >= 0; i-- {
		if comments[i].AuthorType == at {
			return comments[i].Withheld
		}
	}
	return false
}

// ─── work-order claim + complete ────────────────────────────────

func TestWorkOrderClaimComplete(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)

	// Accept (creates the work-order) via the verdict route.
	accept := `{"vision_fit":"fit","category":"bug","next_state":"accepted","work_order":{"summary":"do it"}}`
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken, accept, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("accept: got %d", resp.StatusCode)
	}

	// Coding agent appears in the queue.
	q := e.agentReq(t, "GET", "/api/agent/queue", testCodingToken, "", nil)
	var queue queueResponse
	json.NewDecoder(q.Body).Decode(&queue)
	q.Body.Close()
	if queue.Scope != "coding" || len(queue.Items) != 1 || queue.Items[0].WorkOrder == nil {
		t.Fatalf("coding queue: %+v", queue)
	}

	// Claim → accepted→in_progress, picked_up_at set.
	resp = e.agentReq(t, "POST", "/api/work-orders/"+id+"/claim", testCodingToken, "", nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("claim: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
	if e.tickets.GetTicket(id).State != StateInProgress {
		t.Fatal("ticket not in_progress after claim")
	}
	if wo := e.tickets.GetWorkOrder(id); wo == nil || wo.PickedUpAt == nil {
		t.Fatal("picked_up_at not set after claim")
	}

	// Double-claim → 409.
	resp = e.agentReq(t, "POST", "/api/work-orders/"+id+"/claim", testCodingToken, "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusConflict {
		t.Fatalf("double claim: got %d, want 409", resp.StatusCode)
	}

	// Record a deploy then complete linking it (in_progress→deploying→… we
	// complete to deploying, the legal next from in_progress).
	depResp := e.agentReq(t, "POST", "/api/deploys", testCodingToken,
		`{"version":"1.1","git_sha":"deadbeef","ticket_ids":["`+id+`"],"status":"live"}`, nil)
	var dep Deploy
	json.NewDecoder(depResp.Body).Decode(&dep)
	depResp.Body.Close()
	if dep.ID == "" {
		t.Fatal("deploy id empty")
	}

	// Complete → deploying (legal from in_progress), completed_at + deploy link.
	complete := `{"deploy_id":"` + dep.ID + `","next_state":"deploying"}`
	resp = e.agentReq(t, "POST", "/api/work-orders/"+id+"/complete", testCodingToken, complete, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("complete: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
	wo := e.tickets.GetWorkOrder(id)
	if wo.CompletedAt == nil || wo.DeployID == nil || *wo.DeployID != dep.ID {
		t.Fatalf("work-order not completed/linked: %+v", wo)
	}
	if e.tickets.GetTicket(id).State != StateDeploying {
		t.Fatal("ticket not deploying after complete")
	}
	// No longer in the coding queue.
	q = e.agentReq(t, "GET", "/api/agent/queue", testCodingToken, "", nil)
	json.NewDecoder(q.Body).Decode(&queue)
	q.Body.Close()
	if len(queue.Items) != 0 {
		t.Fatalf("completed ticket still in coding queue: %+v", queue.Items)
	}

	// Double-complete → 409, and the original completion stays untouched.
	resp = e.agentReq(t, "POST", "/api/work-orders/"+id+"/complete", testCodingToken,
		`{"next_state":"resolved"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusConflict {
		t.Fatalf("double complete: got %d, want 409", resp.StatusCode)
	}
	after := e.tickets.GetWorkOrder(id)
	if !after.CompletedAt.Equal(*wo.CompletedAt) || after.DeployID == nil || *after.DeployID != dep.ID {
		t.Fatalf("double complete mutated the work-order: %+v", after)
	}
}

// TestCompleteWorkOrderValidatesTransitionFirst — an invalid next_state is
// rejected BEFORE the work-order is completed: the order stays open and the
// ticket state is unchanged (the old handler completed first, then failed).
func TestCompleteWorkOrderValidatesTransitionFirst(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)
	accept := `{"vision_fit":"fit","category":"bug","next_state":"accepted","work_order":{"summary":"do it"}}`
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken, accept, nil)
	resp.Body.Close()
	resp = e.agentReq(t, "POST", "/api/work-orders/"+id+"/claim", testCodingToken, "", nil)
	resp.Body.Close()

	// in_progress → closed is not a legal move → 400, nothing mutated.
	resp = e.agentReq(t, "POST", "/api/work-orders/"+id+"/complete", testCodingToken,
		`{"next_state":"closed"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("invalid next_state: got %d, want 400", resp.StatusCode)
	}
	if wo := e.tickets.GetWorkOrder(id); wo.CompletedAt != nil {
		t.Fatal("work-order was completed despite the invalid transition")
	}
	if e.tickets.GetTicket(id).State != StateInProgress {
		t.Fatal("ticket state changed despite the invalid transition")
	}

	// Unknown work-order → 404.
	resp = e.agentReq(t, "POST", "/api/work-orders/does-not-exist/complete", testCodingToken, "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("unknown ticket complete: got %d, want 404", resp.StatusCode)
	}
}

// ─── deploy record round-trip + list authz ──────────────────────

func TestDeployRoundTrip(t *testing.T) {
	e := newAgentEnv(t)

	mk := func(v string) string {
		resp := e.agentReq(t, "POST", "/api/deploys", testCodingToken,
			`{"version":"`+v+`","git_sha":"sha-`+v+`","ticket_ids":[],"status":"building"}`, nil)
		defer resp.Body.Close()
		if resp.StatusCode != http.StatusCreated {
			t.Fatalf("create deploy %s: got %d", v, resp.StatusCode)
		}
		var d Deploy
		json.NewDecoder(resp.Body).Decode(&d)
		return d.ID
	}
	id1 := mk("1.0")
	id2 := mk("1.1")
	if id1 == "" || id2 == "" {
		t.Fatal("deploy ids empty")
	}

	// Coding agent lists deploys.
	resp := e.agentReq(t, "GET", "/api/deploys", testCodingToken, "", nil)
	var list []Deploy
	json.NewDecoder(resp.Body).Decode(&list)
	resp.Body.Close()
	if len(list) != 2 {
		t.Fatalf("deploy list: %d, want 2", len(list))
	}

	// Invalid status → 400.
	resp = e.agentReq(t, "POST", "/api/deploys", testCodingToken, `{"version":"x","status":"bogus"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("bad status: got %d, want 400", resp.StatusCode)
	}

	// A plain user (no agent token, non-admin session) cannot list deploys.
	resp = e.agentReq(t, "GET", "/api/deploys", "", "", e.cookies)
	resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("user list deploys: got %d, want 403", resp.StatusCode)
	}

	// A guest with NO credentials at all is unauthenticated → 401 (not 403).
	resp = e.agentReq(t, "GET", "/api/deploys", "", "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("guest list deploys: got %d, want 401", resp.StatusCode)
	}

	// A valid PO token is authenticated but wrong-scope → 403.
	resp = e.agentReq(t, "GET", "/api/deploys", testPOToken, "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("po token list deploys: got %d, want 403", resp.StatusCode)
	}
}

// ─── agent-comment + agent-state ────────────────────────────────

func TestAgentCommentAndState(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)

	// Coding posts an internal dev note (default kind for coding scope).
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/agent-comment", testCodingToken,
		`{"body":"working on it locally"}`, nil)
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("dev comment: got %d, want 201", resp.StatusCode)
	}
	var c TicketComment
	json.NewDecoder(resp.Body).Decode(&c)
	resp.Body.Close()
	if c.AuthorType != AuthorDev || !c.Internal {
		t.Fatalf("dev comment shape: %+v", c)
	}

	// Coding may NOT post a public po comment.
	resp = e.agentReq(t, "POST", "/api/tickets/"+id+"/agent-comment", testCodingToken,
		`{"body":"hi user","kind":"po"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("coding posting po comment: got %d, want 403", resp.StatusCode)
	}

	// PO posts a public comment (default kind for po scope).
	resp = e.agentReq(t, "POST", "/api/tickets/"+id+"/agent-comment", testPOToken,
		`{"body":"We are looking into this."}`, nil)
	json.NewDecoder(resp.Body).Decode(&c)
	resp.Body.Close()
	if c.AuthorType != AuthorPO {
		t.Fatalf("po comment author: %q", c.AuthorType)
	}

	// agent-state: PO drives triage→needs_info.
	resp = e.agentReq(t, "PATCH", "/api/tickets/"+id+"/agent-state", testPOToken,
		`{"state":"needs_info"}`, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("agent-state: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
	if e.tickets.GetTicket(id).State != StateNeedsInfo {
		t.Fatal("state not needs_info")
	}

	// Illegal transition → 400.
	resp = e.agentReq(t, "PATCH", "/api/tickets/"+id+"/agent-state", testPOToken,
		`{"state":"resolved"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("illegal agent-state: got %d, want 400", resp.StatusCode)
	}
}

// ─── po queue ───────────────────────────────────────────────────

func TestPOQueue(t *testing.T) {
	e := newAgentEnv(t)
	// Two triage tickets.
	e.createTicket(t, `{"title":"a","body":"a"}`)
	id2 := e.createTicket(t, `{"title":"b","body":"b"}`)

	resp := e.agentReq(t, "GET", "/api/agent/queue", testPOToken, "", nil)
	var queue queueResponse
	json.NewDecoder(resp.Body).Decode(&queue)
	resp.Body.Close()
	if queue.Scope != "po" || len(queue.Items) != 2 {
		t.Fatalf("po queue triage: scope=%q items=%d", queue.Scope, len(queue.Items))
	}

	// Move id2 to needs_info, then have the user reply → it should re-surface.
	if _, err := e.tickets.SetTicketState(id2, StateNeedsInfo); err != nil {
		t.Fatalf("to needs_info: %v", err)
	}
	resp = e.agentReq(t, "GET", "/api/agent/queue", testPOToken, "", nil)
	json.NewDecoder(resp.Body).Decode(&queue)
	resp.Body.Close()
	// Only the remaining triage ticket — needs_info with no fresh user reply is
	// not in the queue.
	if len(queue.Items) != 1 {
		t.Fatalf("po queue after needs_info (no reply): %d, want 1", len(queue.Items))
	}

	// User replies on id2.
	cr := e.agentReq(t, "POST", "/api/tickets/"+id2+"/comments", "", `{"body":"more details here"}`, e.cookies)
	cr.Body.Close()
	resp = e.agentReq(t, "GET", "/api/agent/queue", testPOToken, "", nil)
	json.NewDecoder(resp.Body).Decode(&queue)
	resp.Body.Close()
	if len(queue.Items) != 2 {
		t.Fatalf("po queue after user reply: %d, want 2", len(queue.Items))
	}
}

// TestGetArtifactAgentToken — the coding/po agents fetch any ticket's artifacts
// with their Bearer token (materialize/repro flow); the user-session path + the
// no-auth rejection still hold. Regression for the WP14 artifact-auth gap.
func TestGetArtifactAgentToken(t *testing.T) {
	e := newAgentEnv(t)
	tid := e.createTicket(t, `{"title":"bug","body":"x","category":"bug"}`)
	art := e.tickets.AddArtifact(tid, ArtifactScreenshot, "screenshot.png", "image/png", []byte("PNGDATA"))
	path := "/api/tickets/" + tid + "/artifacts/" + art.ID

	// coding Bearer token → 200 + the raw bytes.
	resp := e.agentReq(t, "GET", path, testCodingToken, "", nil)
	body, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK || string(body) != "PNGDATA" {
		t.Fatalf("coding token: got %d %q", resp.StatusCode, body)
	}
	// po Bearer token → 200.
	resp = e.agentReq(t, "GET", path, testPOToken, "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("po token: got %d", resp.StatusCode)
	}
	// no auth → 401.
	resp = e.agentReq(t, "GET", path, "", "", nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("no auth: got %d", resp.StatusCode)
	}
	// user session (cookie) still works → 200.
	resp = e.agentReq(t, "GET", path, "", "", e.cookies)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("user session: got %d", resp.StatusCode)
	}
}
