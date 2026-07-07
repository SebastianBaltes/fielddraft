package main

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"unicode/utf8"
)

// ticketTestEnv spins up a server against the in-memory store and registers a
// plain user, returning the user's session cookies. Reused across ticket tests.
type ticketTestEnv struct {
	store   Store
	srv     *httptest.Server
	client  *http.Client
	cookies []*http.Cookie
	userID  string
}

func newTicketEnv(t *testing.T, email string) *ticketTestEnv {
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
	resp, err := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"`+email+`","password":"pass12345","display_name":"T","invite":"`+newInvite(t, store)+`"}`))
	if err != nil {
		t.Fatalf("register: %v", err)
	}
	var u User
	json.NewDecoder(resp.Body).Decode(&u)
	cookies := resp.Cookies()
	resp.Body.Close()
	if len(cookies) == 0 {
		t.Fatal("no session cookie after register")
	}
	return &ticketTestEnv{store: store, srv: ts, client: client, cookies: cookies, userID: u.ID}
}

func (e *ticketTestEnv) do(t *testing.T, method, path, body string, cookies []*http.Cookie) *http.Response {
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
	for _, c := range cookies {
		r.AddCookie(c)
	}
	resp, err := e.client.Do(r)
	if err != nil {
		t.Fatalf("%s %s: %v", method, path, err)
	}
	return resp
}

// TestCreateTicketPlainPath — the GitHub-style title+body creation path.
func TestCreateTicketPlainPath(t *testing.T) {
	e := newTicketEnv(t, "plain@test.com")

	resp := e.do(t, "POST", "/api/tickets",
		`{"title":"Crash on extrude","body":"Steps to reproduce...","category":"bug"}`, e.cookies)
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create: got %d, want 201", resp.StatusCode)
	}
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()
	if tk.Title != "Crash on extrude" {
		t.Fatalf("title = %q", tk.Title)
	}
	if tk.State != StateTriage {
		t.Fatalf("initial state = %q, want triage", tk.State)
	}
	if tk.Category == nil || *tk.Category != "bug" {
		t.Fatalf("category = %v", tk.Category)
	}
	if tk.AuthorID != e.userID {
		t.Fatalf("author = %q, want %q", tk.AuthorID, e.userID)
	}
}

// TestCreateTicketReportPath — the rich "Report Issue" payload: title derived
// from the first line, repro artifacts persisted as ticket_artifacts blobs.
func TestCreateTicketReportPath(t *testing.T) {
	e := newTicketEnv(t, "report@test.com")

	png := base64.StdEncoding.EncodeToString([]byte{0x89, 0x50, 0x4E, 0x47})
	fdraft := base64.StdEncoding.EncodeToString([]byte("PK\x03\x04 fake zip"))
	payload := `{
		"description":"Something broke\nmore detail here",
		"screenshot":"data:image/png;base64,` + png + `",
		"projectFdraft":"` + fdraft + `",
		"settings":{"renderMode":"webgpu"},
		"uiState":{"camera":[1,2,3]},
		"actionLog":{"events":[]},
		"attachments":[{"name":"note.txt","mime":"text/plain","dataUrl":"data:text/plain;base64,` +
		base64.StdEncoding.EncodeToString([]byte("hi")) + `"}],
		"version":"0.6.0","url":"http://x","userAgent":"UA"
	}`
	resp := e.do(t, "POST", "/api/tickets", payload, e.cookies)
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("create: got %d, want 201", resp.StatusCode)
	}
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	if tk.Title != "Something broke" {
		t.Fatalf("derived title = %q, want first line", tk.Title)
	}
	// screenshot + fdraft + settings + ui_state + actions + 1 attachment = 6.
	if len(tk.Artifacts) != 6 {
		t.Fatalf("artifacts = %d, want 6: %+v", len(tk.Artifacts), tk.Artifacts)
	}
	kinds := map[ArtifactKind]bool{}
	for _, a := range tk.Artifacts {
		kinds[a.Kind] = true
	}
	for _, want := range []ArtifactKind{ArtifactScreenshot, ArtifactFdraft, ArtifactSettings, ArtifactUIState, ArtifactActions, ArtifactAttachment} {
		if !kinds[want] {
			t.Fatalf("missing artifact kind %q", want)
		}
	}

	// Download a screenshot artifact and verify bytes round-trip.
	var shotID string
	for _, a := range tk.Artifacts {
		if a.Kind == ArtifactScreenshot {
			shotID = a.ID
		}
	}
	resp = e.do(t, "GET", "/api/tickets/"+tk.ID+"/artifacts/"+shotID, "", e.cookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("artifact download: got %d", resp.StatusCode)
	}
	if ct := resp.Header.Get("Content-Type"); ct != "image/png" {
		t.Fatalf("artifact content-type = %q", ct)
	}
	resp.Body.Close()
}

// TestCreateTicketRequiresAuth — anonymous POST is rejected (now authenticated).
func TestCreateTicketRequiresAuth(t *testing.T) {
	e := newTicketEnv(t, "auth@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"x","body":"y"}`, nil)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("anon create: got %d, want 401", resp.StatusCode)
	}
	resp.Body.Close()
}

// TestListTicketsFilters — author=me + state filter.
func TestListTicketsFilters(t *testing.T) {
	e := newTicketEnv(t, "lister@test.com")

	// Author creates two tickets.
	for _, b := range []string{`{"title":"A","body":"a"}`, `{"title":"B","body":"b","category":"feature"}`} {
		resp := e.do(t, "POST", "/api/tickets", b, e.cookies)
		resp.Body.Close()
	}

	// A second user creates one (different author).
	e2 := registerSecondUser(t, e)
	resp := e.do(t, "POST", "/api/tickets", `{"title":"C","body":"c"}`, e2)
	resp.Body.Close()

	// author=me → only this user's 2 tickets.
	resp = e.do(t, "GET", "/api/tickets?author=me", "", e.cookies)
	var mine []ticketSummary
	json.NewDecoder(resp.Body).Decode(&mine)
	resp.Body.Close()
	if len(mine) != 2 {
		t.Fatalf("author=me: got %d, want 2", len(mine))
	}
	for _, tk := range mine {
		if tk.AuthorID != e.userID {
			t.Fatalf("author=me returned a foreign ticket: %+v", tk)
		}
	}

	// No filter → all 3 visible (all public, none rejected).
	resp = e.do(t, "GET", "/api/tickets", "", e.cookies)
	var all []ticketSummary
	json.NewDecoder(resp.Body).Decode(&all)
	resp.Body.Close()
	if len(all) != 3 {
		t.Fatalf("list all: got %d, want 3", len(all))
	}

	// category=feature → 1.
	resp = e.do(t, "GET", "/api/tickets?category=feature", "", e.cookies)
	var feats []ticketSummary
	json.NewDecoder(resp.Body).Decode(&feats)
	resp.Body.Close()
	if len(feats) != 1 || feats[0].Title != "B" {
		t.Fatalf("category=feature: %+v", feats)
	}

	// state=triage → all 3 (all start in triage).
	resp = e.do(t, "GET", "/api/tickets?state=triage", "", e.cookies)
	var triaged []ticketSummary
	json.NewDecoder(resp.Body).Decode(&triaged)
	resp.Body.Close()
	if len(triaged) != 3 {
		t.Fatalf("state=triage: got %d, want 3", len(triaged))
	}

	// state=accepted → 0 (none accepted).
	resp = e.do(t, "GET", "/api/tickets?state=accepted", "", e.cookies)
	var accepted []ticketSummary
	json.NewDecoder(resp.Body).Decode(&accepted)
	resp.Body.Close()
	if len(accepted) != 0 {
		t.Fatalf("state=accepted: got %d, want 0", len(accepted))
	}
}

// registerSecondUser registers a second plain user against the same store and
// returns their cookies.
func registerSecondUser(t *testing.T, e *ticketTestEnv) []*http.Cookie {
	t.Helper()
	resp, err := e.client.Post(e.srv.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"second-`+newID()+`@test.com","password":"pass12345","invite":"`+newInvite(t, e.store)+`"}`))
	if err != nil {
		t.Fatalf("register second: %v", err)
	}
	c := resp.Cookies()
	resp.Body.Close()
	return c
}

// TestCommentRoundTrip — add a comment, then list the thread.
func TestCommentRoundTrip(t *testing.T) {
	e := newTicketEnv(t, "commenter@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"Discuss","body":"please"}`, e.cookies)
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	resp = e.do(t, "POST", "/api/tickets/"+tk.ID+"/comments", `{"body":"a reply"}`, e.cookies)
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("add comment: got %d, want 201", resp.StatusCode)
	}
	var c TicketComment
	json.NewDecoder(resp.Body).Decode(&c)
	resp.Body.Close()
	if c.AuthorType != AuthorUser {
		t.Fatalf("comment author_type = %q, want user", c.AuthorType)
	}
	if c.AuthorID == nil || *c.AuthorID != e.userID {
		t.Fatalf("comment author_id = %v, want %q", c.AuthorID, e.userID)
	}

	resp = e.do(t, "GET", "/api/tickets/"+tk.ID+"/comments", "", e.cookies)
	var thread []TicketComment
	json.NewDecoder(resp.Body).Decode(&thread)
	resp.Body.Close()
	if len(thread) != 1 || thread[0].Body != "a reply" {
		t.Fatalf("comment thread: %+v", thread)
	}

	// Empty comment body → 400.
	resp = e.do(t, "POST", "/api/tickets/"+tk.ID+"/comments", `{"body":"   "}`, e.cookies)
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("empty comment: got %d, want 400", resp.StatusCode)
	}
	resp.Body.Close()
}

// TestLegalStateTransition — an admin drives triage → accepted; the audit
// `system` comment is appended.
func TestLegalStateTransition(t *testing.T) {
	e := newTicketEnv(t, "author@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"Fix me","body":"bug"}`, e.cookies)
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	adminCookies := seedAdminCookies(t, e, "boss@test.com")

	// Admin: triage → accepted (legal).
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"accepted"}`, adminCookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("legal transition: got %d, want 200", resp.StatusCode)
	}
	var updated ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&updated)
	resp.Body.Close()
	if updated.State != StateAccepted {
		t.Fatalf("state after transition = %q, want accepted", updated.State)
	}
	// An audit system comment was appended.
	foundSystem := false
	for _, c := range updated.Comments {
		if c.AuthorType == AuthorSystem && strings.Contains(c.Body, "accepted") {
			foundSystem = true
		}
	}
	if !foundSystem {
		t.Fatalf("no system audit comment after transition: %+v", updated.Comments)
	}

	// Continue the dev chain: accepted → in_progress → deploying → resolved.
	for _, st := range []string{"in_progress", "deploying", "resolved"} {
		resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"`+st+`"}`, adminCookies)
		if resp.StatusCode != http.StatusOK {
			t.Fatalf("transition to %s: got %d, want 200", st, resp.StatusCode)
		}
		resp.Body.Close()
	}

	// Author may close a resolved ticket (resolved → closed).
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"closed"}`, e.cookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("author close: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
}

// TestIllegalStateTransition — a disallowed move is rejected with 400.
func TestIllegalStateTransition(t *testing.T) {
	e := newTicketEnv(t, "illegal@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"X","body":"y"}`, e.cookies)
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	adminCookies := seedAdminCookies(t, e, "boss2@test.com")

	// triage → resolved is NOT a legal edge → 400.
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"resolved"}`, adminCookies)
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("illegal transition: got %d, want 400", resp.StatusCode)
	}
	resp.Body.Close()

	// An unknown state value is also rejected.
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"banana"}`, adminCookies)
	if resp.StatusCode != http.StatusBadRequest {
		t.Fatalf("unknown state: got %d, want 400", resp.StatusCode)
	}
	resp.Body.Close()

	// A non-admin may not set arbitrary state (only close).
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"accepted"}`, e.cookies)
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("non-admin state: got %d, want 403", resp.StatusCode)
	}
	resp.Body.Close()

	// The ticket is still in triage (no illegal move took effect).
	resp = e.do(t, "GET", "/api/tickets/"+tk.ID, "", e.cookies)
	var after ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&after)
	resp.Body.Close()
	if after.State != StateTriage {
		t.Fatalf("state after rejected moves = %q, want triage", after.State)
	}
}

// TestRejectedTicketVisibility — a rejected_spam ticket is hidden from other
// users' lists but visible to the author and to admins (§7.3).
func TestRejectedTicketVisibility(t *testing.T) {
	e := newTicketEnv(t, "spammer@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"Buy now","body":"spam"}`, e.cookies)
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	adminCookies := seedAdminCookies(t, e, "boss3@test.com")

	// Admin marks it rejected_spam (legal triage → rejected_spam).
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"rejected_spam"}`, adminCookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("reject: got %d", resp.StatusCode)
	}
	resp.Body.Close()

	// A third, unrelated user does NOT see it in the public list.
	other := registerSecondUser(t, e)
	resp = e.do(t, "GET", "/api/tickets", "", other)
	var otherList []ticketSummary
	json.NewDecoder(resp.Body).Decode(&otherList)
	resp.Body.Close()
	for _, s := range otherList {
		if s.ID == tk.ID {
			t.Fatalf("rejected ticket leaked into another user's list")
		}
	}
	// And cannot fetch it directly.
	resp = e.do(t, "GET", "/api/tickets/"+tk.ID, "", other)
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("other user GET rejected: got %d, want 404", resp.StatusCode)
	}
	resp.Body.Close()

	// The author still sees their own rejected ticket.
	resp = e.do(t, "GET", "/api/tickets?author=me", "", e.cookies)
	var mine []ticketSummary
	json.NewDecoder(resp.Body).Decode(&mine)
	resp.Body.Close()
	if len(mine) != 1 || mine[0].ID != tk.ID {
		t.Fatalf("author cannot see own rejected ticket: %+v", mine)
	}

	// Admin sees it too.
	resp = e.do(t, "GET", "/api/tickets/"+tk.ID, "", adminCookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("admin GET rejected: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
}

// TestTicketEditAndDelete — author edits title/body; author/admin delete.
func TestTicketEditAndDelete(t *testing.T) {
	e := newTicketEnv(t, "editor@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"Old","body":"old body"}`, e.cookies)
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	// Author edits title + body.
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"title":"New","body":"new body"}`, e.cookies)
	var updated ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&updated)
	resp.Body.Close()
	if updated.Title != "New" || updated.Body != "new body" {
		t.Fatalf("edit failed: %+v", updated)
	}

	// A non-author non-admin cannot edit.
	other := registerSecondUser(t, e)
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"title":"Hijack"}`, other)
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("foreign edit: got %d, want 403", resp.StatusCode)
	}
	resp.Body.Close()

	// Author deletes.
	resp = e.do(t, "DELETE", "/api/tickets/"+tk.ID, "", e.cookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("delete: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
	resp = e.do(t, "GET", "/api/tickets/"+tk.ID, "", e.cookies)
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("get after delete: got %d, want 404", resp.StatusCode)
	}
	resp.Body.Close()
}

// seedAdminCookies creates an admin directly in the store and logs in for cookies.
func seedAdminCookies(t *testing.T, e *ticketTestEnv, email string) []*http.Cookie {
	t.Helper()
	if _, err := e.store.CreateUser(email, "Admin", "adminpass123", RoleAdmin); err != nil {
		t.Fatalf("seed admin: %v", err)
	}
	resp, err := e.client.Post(e.srv.URL+"/api/auth/login", "application/json",
		strings.NewReader(`{"email":"`+email+`","password":"adminpass123"}`))
	if err != nil {
		t.Fatalf("admin login: %v", err)
	}
	c := resp.Cookies()
	resp.Body.Close()
	if len(c) == 0 {
		t.Fatal("no admin session cookie")
	}
	return c
}

// A multi-byte character straddling the 80-char boundary must not be cut in
// half — invalid UTF-8 in the derived title made MariaDB reject the INSERT
// (Error 1366) and "Report an issue" failed with a 500.
func TestTicketHeadingRuneSafeTruncation(t *testing.T) {
	// 79 ASCII chars, then "ä…" — a byte slice at 80 would split the "ä".
	long := strings.Repeat("x", 79) + "äöü und noch mehr Text dahinter"
	got := ticketHeading("id1", long)
	if !utf8.ValidString(got) {
		t.Fatalf("heading is not valid UTF-8: %q", got)
	}
	if want := strings.Repeat("x", 79) + "ä…"; got != want {
		t.Fatalf("heading = %q, want %q", got, want)
	}
	// Short lines pass through untouched.
	if got := ticketHeading("id2", "kurz"); got != "kurz" {
		t.Fatalf("short heading = %q", got)
	}
}

// Ticket d22a909e: the AUTHOR may reopen a resolved ticket (resolved →
// in_progress, "the fix didn't stick") — but no other transition.
func TestAuthorReopensResolvedTicket(t *testing.T) {
	e := newTicketEnv(t, "reopen@test.com")
	resp := e.do(t, "POST", "/api/tickets", `{"title":"Fix me","body":"bug"}`, e.cookies)
	var tk ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&tk)
	resp.Body.Close()

	// Author may NOT push a triage ticket to in_progress.
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"in_progress"}`, e.cookies)
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("author triage→in_progress: got %d, want 403", resp.StatusCode)
	}
	resp.Body.Close()

	// Drive to resolved via admin.
	adminCookies := seedAdminCookies(t, e, "reopen-admin@test.com")
	for _, s := range []string{"accepted", "in_progress", "deploying", "resolved"} {
		resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"`+s+`"}`, adminCookies)
		if resp.StatusCode != http.StatusOK {
			t.Fatalf("admin →%s: got %d, want 200", s, resp.StatusCode)
		}
		resp.Body.Close()
	}

	// Author reopens: resolved → in_progress.
	resp = e.do(t, "PATCH", "/api/tickets/"+tk.ID, `{"state":"in_progress"}`, e.cookies)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("author reopen: got %d, want 200", resp.StatusCode)
	}
	var updated ticketDetailResponse
	json.NewDecoder(resp.Body).Decode(&updated)
	resp.Body.Close()
	if updated.State != StateInProgress {
		t.Fatalf("state after reopen = %q, want in_progress", updated.State)
	}
}
