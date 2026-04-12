package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

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

	// Register.
	resp, err := client.Post(ts.URL+"/api/auth/register",
		"application/json",
		strings.NewReader(`{"email":"test@example.com","password":"secret123","display_name":"Tester"}`))
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

	body := `{"email":"dup@test.com","password":"pass"}`
	resp, _ := http.Post(ts.URL+"/api/auth/register", "application/json", strings.NewReader(body))
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("first register: %d", resp.StatusCode)
	}
	resp, _ = http.Post(ts.URL+"/api/auth/register", "application/json", strings.NewReader(body))
	resp.Body.Close()
	if resp.StatusCode != 409 {
		t.Fatalf("duplicate register: got %d, want 409", resp.StatusCode)
	}
}
