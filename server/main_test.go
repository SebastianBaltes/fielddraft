package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// newInvite mints a single-use invite token directly through the store so a
// test can register a user. Registration is invite-gated, so every register
// call in these tests needs a fresh token.
func newInvite(t *testing.T, store Store) string {
	t.Helper()
	admin, err := store.CreateUser("admin-"+newID()+"@test.com", "Admin", "adminpass123", RoleAdmin)
	if err != nil {
		t.Fatalf("seed admin: %v", err)
	}
	inv := store.CreateInvite(admin.ID, "", "", "", time.Now().Add(time.Hour))
	if inv == nil {
		t.Fatal("CreateInvite returned nil")
	}
	return inv.Token
}

func TestRegisterLoginProjectCRUD(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	client := &http.Client{
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}

	// Register (invite-gated).
	invite := newInvite(t, store)
	resp, err := client.Post(ts.URL+"/api/auth/register",
		"application/json",
		strings.NewReader(`{"email":"test@example.com","password":"secret123","display_name":"Tester","invite":"`+invite+`"}`))
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("register: got %d", resp.StatusCode)
	}
	cookies := resp.Cookies()
	if len(cookies) == 0 {
		t.Fatal("no session cookie")
	}
	sessionCookie := cookies[0]
	resp.Body.Close()

	// Me.
	req, _ := http.NewRequest("GET", ts.URL+"/api/me", nil)
	req.AddCookie(sessionCookie)
	resp, err = client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("me: got %d", resp.StatusCode)
	}
	var me User
	json.NewDecoder(resp.Body).Decode(&me)
	resp.Body.Close()
	if me.Email != "test@example.com" {
		t.Fatalf("me.email = %q", me.Email)
	}
	if me.Role != RoleUser {
		t.Fatalf("me.role = %q, want %q", me.Role, RoleUser)
	}

	// Create project.
	req, _ = http.NewRequest("POST", ts.URL+"/api/projects",
		strings.NewReader(`{"name":"My Part","document":{"schema_version":1}}`))
	req.AddCookie(sessionCookie)
	resp, err = client.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 201 {
		t.Fatalf("create project: got %d", resp.StatusCode)
	}
	var proj Project
	json.NewDecoder(resp.Body).Decode(&proj)
	resp.Body.Close()
	if proj.Name != "My Part" {
		t.Fatalf("project.name = %q", proj.Name)
	}

	// List projects.
	req, _ = http.NewRequest("GET", ts.URL+"/api/projects", nil)
	req.AddCookie(sessionCookie)
	resp, _ = client.Do(req)
	var projects []Project
	json.NewDecoder(resp.Body).Decode(&projects)
	resp.Body.Close()
	if len(projects) != 1 {
		t.Fatalf("expected 1 project, got %d", len(projects))
	}

	// Update project.
	req, _ = http.NewRequest("PATCH", ts.URL+"/api/projects/"+proj.ID,
		strings.NewReader(`{"name":"Renamed"}`))
	req.AddCookie(sessionCookie)
	resp, _ = client.Do(req)
	var updated Project
	json.NewDecoder(resp.Body).Decode(&updated)
	resp.Body.Close()
	if updated.Name != "Renamed" {
		t.Fatalf("updated.name = %q", updated.Name)
	}

	// Delete project.
	req, _ = http.NewRequest("DELETE", ts.URL+"/api/projects/"+proj.ID, nil)
	req.AddCookie(sessionCookie)
	resp, _ = client.Do(req)
	if resp.StatusCode != 200 {
		t.Fatalf("delete: got %d", resp.StatusCode)
	}
	resp.Body.Close()

	// List should be empty.
	req, _ = http.NewRequest("GET", ts.URL+"/api/projects", nil)
	req.AddCookie(sessionCookie)
	resp, _ = client.Do(req)
	json.NewDecoder(resp.Body).Decode(&projects)
	resp.Body.Close()
	if len(projects) != 0 {
		t.Fatalf("expected 0 projects after delete, got %d", len(projects))
	}

	// Logout.
	req, _ = http.NewRequest("POST", ts.URL+"/api/auth/logout", nil)
	req.AddCookie(sessionCookie)
	resp, _ = client.Do(req)
	resp.Body.Close()

	// Me after logout → 401.
	req, _ = http.NewRequest("GET", ts.URL+"/api/me", nil)
	req.AddCookie(sessionCookie)
	resp, _ = client.Do(req)
	if resp.StatusCode != 401 {
		t.Fatalf("me after logout: got %d, want 401", resp.StatusCode)
	}
	resp.Body.Close()
}

func TestRateLimiterRejects(t *testing.T) {
	rl := NewRateLimiter(0, 3) // 0 refill, 3 burst
	if !rl.Allow("1.2.3.4") {
		t.Fatal("first should pass")
	}
	if !rl.Allow("1.2.3.4") {
		t.Fatal("second should pass")
	}
	if !rl.Allow("1.2.3.4") {
		t.Fatal("third should pass")
	}
	if rl.Allow("1.2.3.4") {
		t.Fatal("fourth should be rejected (bucket empty)")
	}
	// Different IP should still pass.
	if !rl.Allow("5.6.7.8") {
		t.Fatal("different IP should pass")
	}
}

func TestDuplicateEmailRejects(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	mk := func() string {
		return `{"email":"dup@test.com","password":"pass","invite":"` + newInvite(t, store) + `"}`
	}
	resp, _ := http.Post(ts.URL+"/api/auth/register", "application/json", strings.NewReader(mk()))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("first register: %d", resp.StatusCode)
	}
	// Second register with a FRESH invite but the same email → 409. (A fresh
	// invite proves the conflict is the email, not a burned token.)
	resp, _ = http.Post(ts.URL+"/api/auth/register", "application/json", strings.NewReader(mk()))
	resp.Body.Close()
	if resp.StatusCode != 409 {
		t.Fatalf("duplicate register: got %d, want 409", resp.StatusCode)
	}
}

func TestPasswordReset(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	// Register a user.
	resp, _ := http.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"reset@test.com","password":"oldpass123","display_name":"Reset","invite":"`+newInvite(t, store)+`"}`))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatal("register failed")
	}

	// Request reset — returns 200 even for valid emails (no info leak).
	resp, _ = http.Post(ts.URL+"/api/auth/password-reset-request", "application/json",
		strings.NewReader(`{"email":"reset@test.com"}`))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("reset request: got %d, want 200", resp.StatusCode)
	}

	// Get the token from the store (in production it would be emailed).
	user := store.GetUserByEmail("reset@test.com")
	if user == nil {
		t.Fatal("user not found")
	}
	token := store.CreateResetToken(user.ID)

	// Reset with the token.
	resp, _ = http.Post(ts.URL+"/api/auth/password-reset", "application/json",
		strings.NewReader(`{"token":"`+token+`","new_password":"newpass456"}`))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("reset confirm: got %d, want 200", resp.StatusCode)
	}

	// Login with old password should fail.
	resp, _ = http.Post(ts.URL+"/api/auth/login", "application/json",
		strings.NewReader(`{"email":"reset@test.com","password":"oldpass123"}`))
	resp.Body.Close()
	if resp.StatusCode == 200 {
		t.Fatal("login with old password should fail after reset")
	}

	// Login with new password should succeed.
	resp, _ = http.Post(ts.URL+"/api/auth/login", "application/json",
		strings.NewReader(`{"email":"reset@test.com","password":"newpass456"}`))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("login with new password: got %d, want 200", resp.StatusCode)
	}
}

func TestPasswordResetInvalidToken(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	resp, _ := http.Post(ts.URL+"/api/auth/password-reset", "application/json",
		strings.NewReader(`{"token":"bogus","new_password":"newpass456"}`))
	resp.Body.Close()
	if resp.StatusCode != 400 {
		t.Fatalf("reset with bad token: got %d, want 400", resp.StatusCode)
	}
}

func TestThumbnailPutGet(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	client := &http.Client{
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}

	// Register + login to get a session cookie.
	resp, _ := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"thumb@test.com","password":"pass12345","display_name":"Thumb","invite":"`+newInvite(t, store)+`"}`))
	cookies := resp.Cookies()
	resp.Body.Close()

	// Create a project.
	req, _ := http.NewRequest("POST", ts.URL+"/api/projects", strings.NewReader(`{"name":"P1","document":{}}`))
	req.Header.Set("Content-Type", "application/json")
	for _, c := range cookies {
		req.AddCookie(c)
	}
	resp, _ = client.Do(req)
	var proj struct {
		ID string `json:"id"`
	}
	json.NewDecoder(resp.Body).Decode(&proj)
	resp.Body.Close()

	// Upload thumbnail.
	pngData := []byte{0x89, 0x50, 0x4E, 0x47} // fake PNG header
	req, _ = http.NewRequest("PUT", ts.URL+"/api/projects/"+proj.ID+"/thumbnail", strings.NewReader(string(pngData)))
	for _, c := range cookies {
		req.AddCookie(c)
	}
	resp, _ = client.Do(req)
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("put thumbnail: got %d, want 200", resp.StatusCode)
	}

	// Get thumbnail.
	resp, err := http.Get(ts.URL + "/api/projects/" + proj.ID + "/thumbnail")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("get thumbnail: got %d, want 200", resp.StatusCode)
	}
	if resp.Header.Get("Content-Type") != "image/png" {
		t.Fatalf("thumbnail content-type: %s", resp.Header.Get("Content-Type"))
	}
}

// TestRegisterRequiresInvite — registration without (or with a bad) invite is
// rejected, a valid invite works, and the same invite cannot be reused.
func TestRegisterRequiresInvite(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	// No invite → 403.
	resp, _ := http.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"a@test.com","password":"pass12345"}`))
	resp.Body.Close()
	if resp.StatusCode != 403 {
		t.Fatalf("no invite: got %d, want 403", resp.StatusCode)
	}

	// Bogus invite → 400.
	resp, _ = http.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"a@test.com","password":"pass12345","invite":"nope"}`))
	resp.Body.Close()
	if resp.StatusCode != 400 {
		t.Fatalf("bad invite: got %d, want 400", resp.StatusCode)
	}

	// Valid invite via ?invite= query → 200.
	tok := newInvite(t, store)
	resp, _ = http.Post(ts.URL+"/api/auth/register?invite="+tok, "application/json",
		strings.NewReader(`{"email":"a@test.com","password":"pass12345"}`))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("valid invite: got %d, want 200", resp.StatusCode)
	}

	// Reusing the burned invite → 400.
	resp, _ = http.Post(ts.URL+"/api/auth/register?invite="+tok, "application/json",
		strings.NewReader(`{"email":"b@test.com","password":"pass12345"}`))
	resp.Body.Close()
	if resp.StatusCode != 400 {
		t.Fatalf("reused invite: got %d, want 400", resp.StatusCode)
	}
}

// TestInviteAdminEndpoints — only admins may create/list/delete invites, and a
// created token actually works for registration.
func TestInviteAdminEndpoints(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	client := &http.Client{
		CheckRedirect: func(_ *http.Request, _ []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}

	// Seed an admin and a plain user directly, then log in for cookies.
	if _, err := store.CreateUser("boss@test.com", "Boss", "bosspass123", RoleAdmin); err != nil {
		t.Fatal(err)
	}
	plainInvite := newInvite(t, store)
	resp, _ := client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"plain@test.com","password":"plainpass123","invite":"`+plainInvite+`"}`))
	userCookies := resp.Cookies()
	resp.Body.Close()

	login := func(email, pass string) []*http.Cookie {
		r, _ := client.Post(ts.URL+"/api/auth/login", "application/json",
			strings.NewReader(`{"email":"`+email+`","password":"`+pass+`"}`))
		c := r.Cookies()
		r.Body.Close()
		return c
	}
	adminCookies := login("boss@test.com", "bosspass123")

	withCookies := func(req *http.Request, cs []*http.Cookie) *http.Request {
		for _, c := range cs {
			req.AddCookie(c)
		}
		return req
	}

	// Non-admin POST /api/invites → 403.
	req, _ := http.NewRequest("POST", ts.URL+"/api/invites", strings.NewReader(`{"invitee_name":"x"}`))
	resp, _ = client.Do(withCookies(req, userCookies))
	resp.Body.Close()
	if resp.StatusCode != 403 {
		t.Fatalf("non-admin create invite: got %d, want 403", resp.StatusCode)
	}

	// Admin POST /api/invites → 201 with a token + register URL. The recipient
	// fields (name/email/message) round-trip into the response.
	req, _ = http.NewRequest("POST", ts.URL+"/api/invites", strings.NewReader(
		`{"invitee_name":"Charlie","invitee_email":"charlie@test.com","message":"welcome aboard"}`))
	resp, _ = client.Do(withCookies(req, adminCookies))
	var created inviteResponse
	json.NewDecoder(resp.Body).Decode(&created)
	resp.Body.Close()
	if resp.StatusCode != 201 {
		t.Fatalf("admin create invite: got %d, want 201", resp.StatusCode)
	}
	if created.Token == "" || created.Status != "open" {
		t.Fatalf("invite response: %+v", created)
	}
	if created.InviteeName != "Charlie" || created.InviteeEmail != "charlie@test.com" || created.Message != "welcome aboard" {
		t.Fatalf("invite recipient fields not returned: %+v", created)
	}
	if !strings.Contains(created.RegisterURL, created.Token) {
		t.Fatalf("register_url missing token: %q", created.RegisterURL)
	}

	// The created token actually registers a user.
	resp, _ = client.Post(ts.URL+"/api/auth/register", "application/json",
		strings.NewReader(`{"email":"charlie@test.com","password":"charliepass","invite":"`+created.Token+`"}`))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("register with admin-created invite: got %d, want 200", resp.StatusCode)
	}

	// Admin GET /api/invites → list includes the now-used invite.
	req, _ = http.NewRequest("GET", ts.URL+"/api/invites", nil)
	resp, _ = client.Do(withCookies(req, adminCookies))
	var list []inviteResponse
	json.NewDecoder(resp.Body).Decode(&list)
	resp.Body.Close()
	foundUsed := false
	for _, inv := range list {
		if inv.Token == created.Token && inv.Status == "used" {
			foundUsed = true
			if inv.InviteeName != "Charlie" || inv.InviteeEmail != "charlie@test.com" || inv.Message != "welcome aboard" {
				t.Fatalf("list invite recipient fields lost: %+v", inv)
			}
		}
	}
	if !foundUsed {
		t.Fatalf("created invite not marked used in list: %+v", list)
	}

	// Admin DELETE a fresh invite → 200; non-admin delete → 403.
	req, _ = http.NewRequest("POST", ts.URL+"/api/invites", strings.NewReader(`{}`))
	resp, _ = client.Do(withCookies(req, adminCookies))
	var toDelete inviteResponse
	json.NewDecoder(resp.Body).Decode(&toDelete)
	resp.Body.Close()

	req, _ = http.NewRequest("DELETE", ts.URL+"/api/invites/"+toDelete.Token, nil)
	resp, _ = client.Do(withCookies(req, userCookies))
	resp.Body.Close()
	if resp.StatusCode != 403 {
		t.Fatalf("non-admin delete invite: got %d, want 403", resp.StatusCode)
	}

	req, _ = http.NewRequest("DELETE", ts.URL+"/api/invites/"+toDelete.Token, nil)
	resp, _ = client.Do(withCookies(req, adminCookies))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("admin delete invite: got %d, want 200", resp.StatusCode)
	}
}

// TestInviteInfoPublic — the public GET /api/invites/{token}/info endpoint
// prefills the self-registration form: an OPEN invite returns the invitee's
// name + email with valid=true (no auth needed), while unknown / used / expired
// tokens return a bare valid=false that leaks nothing else.
func TestInviteInfoPublic(t *testing.T) {
	store := NewStore()
	srv := NewServer(store)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	admin, err := store.CreateUser("boss@test.com", "Boss", "bosspass123", RoleAdmin)
	if err != nil {
		t.Fatal(err)
	}

	// Open invite → valid=true with the prefill fields, NO cookie/session sent.
	open := store.CreateInvite(admin.ID, "Ada Lovelace", "ada@test.com", "welcome", time.Now().Add(time.Hour))
	resp, err := http.Get(ts.URL + "/api/invites/" + open.Token + "/info")
	if err != nil {
		t.Fatal(err)
	}
	if resp.StatusCode != 200 {
		t.Fatalf("open invite info: got %d, want 200", resp.StatusCode)
	}
	var info struct {
		InviteeName  string `json:"invitee_name"`
		InviteeEmail string `json:"invitee_email"`
		Valid        bool   `json:"valid"`
	}
	json.NewDecoder(resp.Body).Decode(&info)
	resp.Body.Close()
	if !info.Valid {
		t.Fatalf("open invite: valid=%v, want true", info.Valid)
	}
	if info.InviteeName != "Ada Lovelace" || info.InviteeEmail != "ada@test.com" {
		t.Fatalf("open invite prefill: got %q / %q", info.InviteeName, info.InviteeEmail)
	}

	// Unknown token → valid=false, no prefill leaked.
	resp, _ = http.Get(ts.URL + "/api/invites/does-not-exist/info")
	info = struct {
		InviteeName  string `json:"invitee_name"`
		InviteeEmail string `json:"invitee_email"`
		Valid        bool   `json:"valid"`
	}{}
	json.NewDecoder(resp.Body).Decode(&info)
	resp.Body.Close()
	if info.Valid || info.InviteeName != "" || info.InviteeEmail != "" {
		t.Fatalf("unknown token: got %+v, want valid=false and no prefill", info)
	}

	// Used token → valid=false.
	used := store.CreateInvite(admin.ID, "Grace", "grace@test.com", "", time.Now().Add(time.Hour))
	if _, err := store.ConsumeInvite(used.Token, admin.ID); err != nil {
		t.Fatal(err)
	}
	resp, _ = http.Get(ts.URL + "/api/invites/" + used.Token + "/info")
	info.Valid = true
	info.InviteeEmail = ""
	json.NewDecoder(resp.Body).Decode(&info)
	resp.Body.Close()
	if info.Valid || info.InviteeEmail != "" {
		t.Fatalf("used token: got %+v, want valid=false and no prefill", info)
	}

	// Expired token → valid=false.
	expired := store.CreateInvite(admin.ID, "Alan", "alan@test.com", "", time.Now().Add(-time.Hour))
	resp, _ = http.Get(ts.URL + "/api/invites/" + expired.Token + "/info")
	info.Valid = true
	info.InviteeEmail = ""
	json.NewDecoder(resp.Body).Decode(&info)
	resp.Body.Close()
	if info.Valid || info.InviteeEmail != "" {
		t.Fatalf("expired token: got %+v, want valid=false and no prefill", info)
	}
}
