// WP8 moderation tests — PO post-moderation of user comments (B1), real admin
// alerts (B2), and user ban/disable (B3). All run against the in-memory store
// through the HTTP layer, reusing the agentEnv fixture from agent_test.go.

package main

import (
	"encoding/json"
	"net/http"
	"testing"
)

// adminLogin seeds an admin user directly in the store and logs it in, returning
// the session cookies.
func (e *agentEnv) adminLogin(t *testing.T, email string) []*http.Cookie {
	t.Helper()
	if _, err := e.store.CreateUser(email, "Admin", "adminpass123", RoleAdmin); err != nil {
		t.Fatalf("seed admin: %v", err)
	}
	resp := e.agentReq(t, "POST", "/api/auth/login", "",
		`{"email":"`+email+`","password":"adminpass123"}`, nil)
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("admin login: got %d", resp.StatusCode)
	}
	return resp.Cookies()
}

// addUserComment posts a user comment on a ticket as the registered plain user
// and returns the created comment.
func (e *agentEnv) addUserComment(t *testing.T, ticketID, body string) TicketComment {
	t.Helper()
	resp := e.agentReq(t, "POST", "/api/tickets/"+ticketID+"/comments", "",
		`{"body":"`+body+`"}`, e.cookies)
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusCreated {
		t.Fatalf("add comment: got %d", resp.StatusCode)
	}
	var c TicketComment
	json.NewDecoder(resp.Body).Decode(&c)
	return c
}

// ─── B1: PO post-moderation of EVERY user comment ───────────────

// TestUnreviewedCommentSurfacesInPOQueue — a fresh user comment is po_reviewed=0
// and makes its ticket appear in the PO queue even when the ticket is resolved.
func TestUnreviewedCommentSurfacesInPOQueue(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)

	// Drive the ticket to resolved (PO-less path via the store, walking the SM).
	for _, st := range []TicketState{StateAccepted, StateInProgress, StateDeploying, StateResolved} {
		if _, err := e.tickets.SetTicketState(id, st); err != nil {
			t.Fatalf("to %s: %v", st, err)
		}
	}

	// The user comments on the resolved ticket.
	c := e.addUserComment(t, id, "this regressed again")
	if c.POReviewed {
		t.Fatal("new user comment should start po_reviewed=0")
	}

	// PO queue surfaces the resolved ticket because of the unreviewed comment.
	resp := e.agentReq(t, "GET", "/api/agent/queue", testPOToken, "", nil)
	var queue queueResponse
	json.NewDecoder(resp.Body).Decode(&queue)
	resp.Body.Close()

	var found *agentQueueTicket
	for i := range queue.Items {
		if queue.Items[i].ID == id {
			found = &queue.Items[i]
		}
	}
	if found == nil {
		t.Fatalf("resolved ticket with unreviewed comment not in PO queue: %+v", queue.Items)
	}
	if len(found.UnreviewedComments) != 1 || found.UnreviewedComments[0].ID != c.ID {
		t.Fatalf("unreviewed comment not attached to queue item: %+v", found.UnreviewedComments)
	}
}

// TestModerateCommentSpamWithholds — moderating a comment spam/malicious marks
// it reviewed, withholds it (hidden from the user), and raises an admin alert.
func TestModerateCommentSpamWithholds(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)
	// Drive it out of triage so the ONLY reason it would be in the PO queue is an
	// unreviewed comment (isolates the post-moderation behaviour).
	for _, st := range []TicketState{StateAccepted, StateInProgress, StateDeploying, StateResolved} {
		if _, err := e.tickets.SetTicketState(id, st); err != nil {
			t.Fatalf("to %s: %v", st, err)
		}
	}
	c := e.addUserComment(t, id, "buy cheap watches at spam dot com")

	// PO moderates the comment as spam.
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/comments/"+c.ID+"/verdict", testPOToken,
		`{"spam":true,"malicious":false,"reason":"obvious spam"}`, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("comment verdict: got %d, want 200", resp.StatusCode)
	}
	var moderated TicketComment
	json.NewDecoder(resp.Body).Decode(&moderated)
	resp.Body.Close()
	if !moderated.POReviewed || !moderated.Withheld {
		t.Fatalf("moderated comment not reviewed+withheld: %+v", moderated)
	}

	// In store: reviewed + withheld.
	stored := e.tickets.GetComment(id, c.ID)
	if stored == nil || !stored.POReviewed || !stored.Withheld {
		t.Fatalf("stored comment state wrong: %+v", stored)
	}

	// The user (non-admin) no longer sees the withheld comment.
	getResp := e.agentReq(t, "GET", "/api/tickets/"+id, "", "", e.cookies)
	var detail ticketDetailResponse
	json.NewDecoder(getResp.Body).Decode(&detail)
	getResp.Body.Close()
	for _, vc := range detail.Comments {
		if vc.ID == c.ID {
			t.Fatal("withheld spam comment still visible to the user")
		}
	}

	// An admin alert was raised.
	if !hasAlert(e.store.ListAdminAlerts(false), AlertMaliciousComment, &id) {
		t.Fatal("no malicious_comment alert created")
	}

	// The ticket no longer surfaces in the PO queue for that comment.
	resp = e.agentReq(t, "GET", "/api/agent/queue", testPOToken, "", nil)
	var queue queueResponse
	json.NewDecoder(resp.Body).Decode(&queue)
	resp.Body.Close()
	for _, it := range queue.Items {
		if it.ID == id {
			t.Fatal("ticket still in PO queue after its only comment was reviewed")
		}
	}
}

// TestModerateCommentCleanNoWithhold — a clean moderation marks the comment
// reviewed but does NOT withhold it and raises no alert.
func TestModerateCommentCleanNoWithhold(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)
	c := e.addUserComment(t, id, "here is more repro detail")

	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/comments/"+c.ID+"/verdict", testPOToken,
		`{"spam":false,"malicious":false}`, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("clean comment verdict: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()

	stored := e.tickets.GetComment(id, c.ID)
	if stored == nil || !stored.POReviewed || stored.Withheld {
		t.Fatalf("clean comment should be reviewed but not withheld: %+v", stored)
	}
	if len(e.store.ListAdminAlerts(false)) != 0 {
		t.Fatal("clean moderation should raise no alert")
	}
}

// TestCommentVerdictValidation — wrong ticket, missing comment, and non-po
// scope are all rejected.
func TestCommentVerdictValidation(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)
	other := e.createTicket(t, `{"title":"a","body":"b"}`)
	c := e.addUserComment(t, id, "a comment")

	// Comment belongs to `id`, not `other` → 404.
	resp := e.agentReq(t, "POST", "/api/tickets/"+other+"/comments/"+c.ID+"/verdict", testPOToken,
		`{"spam":true}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("cross-ticket comment verdict: got %d, want 404", resp.StatusCode)
	}

	// Unknown comment → 404.
	resp = e.agentReq(t, "POST", "/api/tickets/"+id+"/comments/deadbeef/verdict", testPOToken,
		`{"spam":true}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusNotFound {
		t.Fatalf("unknown comment verdict: got %d, want 404", resp.StatusCode)
	}

	// Coding scope may not moderate comments → 403.
	resp = e.agentReq(t, "POST", "/api/tickets/"+id+"/comments/"+c.ID+"/verdict", testCodingToken,
		`{"spam":true}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusForbidden {
		t.Fatalf("coding comment verdict: got %d, want 403", resp.StatusCode)
	}
}

// ─── B2: real admin alerts ──────────────────────────────────────

// TestMaliciousVerdictCreatesAlert — a malicious ticket verdict writes an alert.
func TestMaliciousVerdictCreatesAlert(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"evil","body":"rm -rf /"}`)

	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken,
		`{"malicious":true,"reason":"prompt injection attempt","next_state":"rejected_malicious"}`, nil)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("malicious verdict: got %d, want 200", resp.StatusCode)
	}
	resp.Body.Close()
	if !hasAlert(e.store.ListAdminAlerts(false), AlertMaliciousTicket, &id) {
		t.Fatal("no malicious_ticket alert created")
	}
}

// TestWithheldOutgoingCommentCreatesAlert — a leaky po public_reply both
// withholds the comment and raises a withheld_comment alert.
func TestWithheldOutgoingCommentCreatesAlert(t *testing.T) {
	e := newAgentEnv(t)
	id := e.createTicket(t, `{"title":"x","body":"y"}`)

	leak := `{"next_state":"needs_info","public_reply":"The bug is in server/main.go, will patch."}`
	resp := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken, leak, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("leak verdict: got %d, want 200", resp.StatusCode)
	}
	if !hasAlert(e.store.ListAdminAlerts(false), AlertWithheldComment, &id) {
		t.Fatal("no withheld_comment alert created for the leaky reply")
	}
}

// TestAdminAlertsListAndSeen — admin lists alerts (newest first), filters by
// ?unseen=1, and marks one seen.
func TestAdminAlertsListAndSeen(t *testing.T) {
	e := newAgentEnv(t)
	admin := e.adminLogin(t, "boss-alerts@test.com")

	// Generate two alerts: a malicious ticket and a banned user.
	id := e.createTicket(t, `{"title":"x","body":"y"}`)
	r := e.agentReq(t, "POST", "/api/tickets/"+id+"/verdict", testPOToken,
		`{"malicious":true,"reason":"bad","next_state":"rejected_malicious"}`, nil)
	r.Body.Close()
	e.store.CreateAdminAlert(AlertUserBanned, nil, "user banned: x@test.com")

	// List all (newest first).
	resp := e.agentReq(t, "GET", "/api/admin/alerts", "", "", admin)
	var all []AdminAlert
	json.NewDecoder(resp.Body).Decode(&all)
	resp.Body.Close()
	if len(all) != 2 {
		t.Fatalf("alert list: %d, want 2", len(all))
	}
	if all[0].Kind != AlertUserBanned {
		t.Fatalf("alerts not newest-first: %+v", all)
	}

	// Mark the newest seen.
	resp = e.agentReq(t, "POST", "/api/admin/alerts/"+all[0].ID+"/seen", "", "", admin)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("mark seen: got %d, want 200", resp.StatusCode)
	}

	// ?unseen=1 now returns only the other one.
	resp = e.agentReq(t, "GET", "/api/admin/alerts?unseen=1", "", "", admin)
	var unseen []AdminAlert
	json.NewDecoder(resp.Body).Decode(&unseen)
	resp.Body.Close()
	if len(unseen) != 1 || unseen[0].ID != all[1].ID {
		t.Fatalf("unseen filter wrong: %+v", unseen)
	}
}

// TestAdminEndpointsRejectNonAdmins — alert + user endpoints reject a plain user
// (403) and a guest (401).
func TestAdminEndpointsRejectNonAdmins(t *testing.T) {
	e := newAgentEnv(t)

	endpoints := []struct{ method, path string }{
		{"GET", "/api/admin/alerts"},
		{"GET", "/api/admin/users"},
		{"POST", "/api/admin/users/abc/ban"},
		{"POST", "/api/admin/users/abc/unban"},
		{"POST", "/api/admin/alerts/abc/seen"},
	}
	for _, ep := range endpoints {
		// Plain user session → 403.
		resp := e.agentReq(t, ep.method, ep.path, "", "", e.cookies)
		resp.Body.Close()
		if resp.StatusCode != http.StatusForbidden {
			t.Fatalf("%s %s as user: got %d, want 403", ep.method, ep.path, resp.StatusCode)
		}
		// Guest (no session) → 401.
		resp = e.agentReq(t, ep.method, ep.path, "", "", nil)
		resp.Body.Close()
		if resp.StatusCode != http.StatusUnauthorized {
			t.Fatalf("%s %s as guest: got %d, want 401", ep.method, ep.path, resp.StatusCode)
		}
	}
}

// ─── B3: user ban / disable ─────────────────────────────────────

// TestBanBlocksLoginAndSession — banning a user blocks subsequent login and
// invalidates the existing session, and writes a user_banned alert.
func TestBanBlocksLoginAndSession(t *testing.T) {
	e := newAgentEnv(t)
	admin := e.adminLogin(t, "boss-ban@test.com")

	// The agentEnv's plain user is logged in (e.cookies). Confirm it works first.
	resp := e.agentReq(t, "GET", "/api/me", "", "", e.cookies)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("pre-ban /api/me: got %d, want 200", resp.StatusCode)
	}

	// Admin bans the user.
	resp = e.agentReq(t, "POST", "/api/admin/users/"+e.userID+"/ban", "", "", admin)
	var banned User
	json.NewDecoder(resp.Body).Decode(&banned)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK || !banned.Disabled {
		t.Fatalf("ban: got %d disabled=%v", resp.StatusCode, banned.Disabled)
	}

	// The existing session is now invalid.
	resp = e.agentReq(t, "GET", "/api/me", "", "", e.cookies)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("post-ban /api/me: got %d, want 401", resp.StatusCode)
	}

	// Subsequent login fails.
	resp = e.agentReq(t, "POST", "/api/auth/login", "",
		`{"email":"agentuser@test.com","password":"pass12345"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusUnauthorized {
		t.Fatalf("banned login: got %d, want 401", resp.StatusCode)
	}

	// A user_banned alert was written.
	if !hasAlert(e.store.ListAdminAlerts(false), AlertUserBanned, nil) {
		t.Fatal("no user_banned alert created")
	}

	// Unban restores login.
	resp = e.agentReq(t, "POST", "/api/admin/users/"+e.userID+"/unban", "", "", admin)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("unban: got %d, want 200", resp.StatusCode)
	}
	resp = e.agentReq(t, "POST", "/api/auth/login", "",
		`{"email":"agentuser@test.com","password":"pass12345"}`, nil)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("post-unban login: got %d, want 200", resp.StatusCode)
	}
}

// TestLastAdminBanGuard — an admin cannot be banned if it would remove the last
// remaining admin; with a second admin present the ban is allowed.
func TestLastAdminBanGuard(t *testing.T) {
	e := newAgentEnv(t)
	// newAgentEnv's newInvite already seeded one admin; adminLogin adds a second.
	// To test the guard cleanly we need a known single-admin situation, so we ban
	// down to the last admin and prove the final one is protected.
	admin1 := e.adminLogin(t, "admin1-guard@test.com")
	e.adminLogin(t, "admin2-guard@test.com")

	// Find their ids via the admin user list.
	resp := e.agentReq(t, "GET", "/api/admin/users", "", "", admin1)
	var users []adminUser
	json.NewDecoder(resp.Body).Decode(&users)
	resp.Body.Close()

	var id1, id2, idSeed string
	for _, u := range users {
		switch u.Email {
		case "admin1-guard@test.com":
			id1 = u.ID
		case "admin2-guard@test.com":
			id2 = u.ID
		default:
			if u.Role == RoleAdmin {
				idSeed = u.ID // the admin newInvite seeded
			}
		}
	}
	if id1 == "" || id2 == "" || idSeed == "" {
		t.Fatalf("could not resolve admin ids: %+v", users)
	}

	// Ban the seed admin and admin2 — leaves admin1 as the last admin.
	for _, id := range []string{idSeed, id2} {
		r := e.agentReq(t, "POST", "/api/admin/users/"+id+"/ban", "", "", admin1)
		r.Body.Close()
		if r.StatusCode != http.StatusOK {
			t.Fatalf("ban admin %s: got %d, want 200", id, r.StatusCode)
		}
	}

	// admin2's session is now invalid, so use admin1 for the final attempt:
	// banning admin1 (the last admin) must be refused.
	r := e.agentReq(t, "POST", "/api/admin/users/"+id1+"/ban", "", "", admin1)
	r.Body.Close()
	if r.StatusCode != http.StatusForbidden {
		t.Fatalf("last-admin ban: got %d, want 403", r.StatusCode)
	}
	if u := e.store.GetUser(id1); u == nil || u.Disabled {
		t.Fatal("last admin was banned despite the guard")
	}
}

// hasAlert reports whether the alert list contains one of the given kind whose
// ticket id matches want (nil want = any/no ticket id; non-nil = exact match).
func hasAlert(alerts []*AdminAlert, kind AlertKind, want *string) bool {
	for _, a := range alerts {
		if a.Kind != kind {
			continue
		}
		if want == nil {
			return true
		}
		if a.TicketID != nil && *a.TicketID == *want {
			return true
		}
	}
	return false
}
