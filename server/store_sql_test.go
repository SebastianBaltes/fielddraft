// Integration test for the MariaDB-backed store.
//
// Skips automatically when no test database is reachable, so `go test ./...`
// stays green on a machine with no MariaDB. To run it, point TEST_DB_DSN (or
// DB_DSN) at a throwaway MySQL/MariaDB and a writable BLOB_DIR:
//
//	TEST_DB_DSN='fielddraft:fielddraft@tcp(127.0.0.1:3306)/fielddraft?parseTime=true' \
//	BLOB_DIR=$(mktemp -d) go test ./server -run TestSQLStore

package main

import (
	"encoding/json"
	"os"
	"testing"
	"time"
)

func testDSN() string {
	if d := os.Getenv("TEST_DB_DSN"); d != "" {
		return d
	}
	return os.Getenv("DB_DSN")
}

func newTestSQLStore(t *testing.T) *sqlStore {
	t.Helper()
	dsn := testDSN()
	if dsn == "" {
		t.Skip("no TEST_DB_DSN/DB_DSN set — skipping MariaDB integration test")
	}
	// Use a fresh temp blob dir so the test is self-contained.
	t.Setenv("BLOB_DIR", t.TempDir())
	store, err := NewSQLStore(dsn)
	if err != nil {
		t.Skipf("DB not reachable (%v) — skipping MariaDB integration test", err)
	}
	t.Cleanup(func() { store.Close() })
	return store
}

func TestSQLStoreRoundTrip(t *testing.T) {
	store := newTestSQLStore(t)

	email := "sqltest-" + newID() + "@test.com"
	u, err := store.CreateUser(email, "SQL Tester", "pass12345", RoleUser)
	if err != nil {
		t.Fatalf("CreateUser: %v", err)
	}
	if store.GetUser(u.ID) == nil {
		t.Fatal("GetUser returned nil")
	}

	// Auth round-trip.
	if _, err := store.AuthenticateUser(email, "pass12345"); err != nil {
		t.Fatalf("AuthenticateUser: %v", err)
	}
	if _, err := store.AuthenticateUser(email, "wrong"); err == nil {
		t.Fatal("AuthenticateUser accepted a wrong password")
	}

	// Sessions survive a fresh store handle (DB-backed).
	sess := store.CreateSession(u.ID)
	if sess == nil || store.GetSession(sess.Token) == nil {
		t.Fatal("session not persisted")
	}
	store.DeleteSession(sess.Token)
	if store.GetSession(sess.Token) != nil {
		t.Fatal("session not deleted")
	}

	// Invite single-use.
	admin, err := store.CreateUser("sqladmin-"+newID()+"@test.com", "A", "adminpass1", RoleAdmin)
	if err != nil {
		t.Fatalf("CreateUser admin: %v", err)
	}
	if !store.HasAdmin() {
		t.Fatal("HasAdmin false after creating an admin")
	}
	inv := store.CreateInvite(admin.ID, "Charlie", "charlie@test.com", "welcome", time.Now().Add(time.Hour))
	if inv == nil {
		t.Fatal("CreateInvite nil")
	}
	if got := store.GetInvite(inv.Token); got == nil ||
		got.InviteeName != "Charlie" || got.InviteeEmail != "charlie@test.com" || got.Message != "welcome" {
		t.Fatalf("GetInvite recipient fields: %+v", got)
	}
	if _, err := store.ConsumeInvite(inv.Token, u.ID); err != nil {
		t.Fatalf("ConsumeInvite: %v", err)
	}
	if _, err := store.ConsumeInvite(inv.Token, u.ID); err == nil {
		t.Fatal("ConsumeInvite reused a single-use token")
	}

	// Project + blob-backed document round-trip.
	doc := json.RawMessage(`{"schema_version":1,"hello":"world"}`)
	p := store.CreateProject(u.ID, "My Part", doc)
	if p == nil {
		t.Fatal("CreateProject nil")
	}
	got := store.GetProject(p.ID, u.ID)
	if got == nil || string(got.Document) != string(doc) {
		t.Fatalf("project document round-trip: %v", got)
	}

	// Update document and visibility.
	doc2 := json.RawMessage(`{"schema_version":2}`)
	upd := store.UpdateProject(p.ID, u.ID, "Renamed", doc2, "public")
	if upd == nil || upd.Name != "Renamed" || upd.Visibility != "public" || string(upd.Document) != string(doc2) {
		t.Fatalf("UpdateProject: %v", upd)
	}
	pub := store.ListPublicProjects()
	if len(pub) == 0 {
		t.Fatal("ListPublicProjects empty after publishing")
	}

	// Thumbnail blob.
	store.SetThumbnail(p.ID, []byte{0x89, 0x50, 0x4E, 0x47})
	if tn := store.GetThumbnail(p.ID); len(tn) != 4 {
		t.Fatalf("thumbnail round-trip: %v", tn)
	}

	// Snapshot blob.
	snap := store.CreateSnapshot(p.ID, doc2)
	if snap == nil || store.GetSnapshot(snap.ID) == nil {
		t.Fatal("snapshot round-trip failed")
	}

	if !store.DeleteProject(p.ID, u.ID) {
		t.Fatal("DeleteProject returned false")
	}
}

// TestSQLStoreTickets exercises the ticket methods against a live MariaDB.
// Auto-skips when no TEST_DB_DSN/DB_DSN is reachable.
func TestSQLStoreTickets(t *testing.T) {
	store := newTestSQLStore(t)

	author, err := store.CreateUser("tkauthor-"+newID()+"@test.com", "Author", "pass12345", RoleUser)
	if err != nil {
		t.Fatalf("CreateUser: %v", err)
	}

	// Create + read back with category.
	tk := store.CreateTicket(author.ID, "Crash", "it crashes", "bug")
	if tk == nil || tk.State != StateTriage {
		t.Fatalf("CreateTicket: %v", tk)
	}
	got := store.GetTicket(tk.ID)
	if got == nil || got.Category == nil || *got.Category != "bug" {
		t.Fatalf("GetTicket category: %v", got)
	}

	// Artifact blob round-trip.
	art := store.AddArtifact(tk.ID, ArtifactScreenshot, "screenshot.png", "image/png", []byte{0x89, 0x50, 0x4E, 0x47})
	if art == nil {
		t.Fatal("AddArtifact nil")
	}
	gotArt, data := store.GetArtifactBlob(tk.ID, art.ID)
	if gotArt == nil || len(data) != 4 {
		t.Fatalf("GetArtifactBlob: %v / %d bytes", gotArt, len(data))
	}

	// Comment round-trip.
	uid := author.ID
	if store.AddComment(tk.ID, AuthorUser, &uid, "a reply", false) == nil {
		t.Fatal("AddComment nil")
	}
	if cs := store.ListComments(tk.ID); len(cs) != 1 {
		t.Fatalf("ListComments: %d, want 1", len(cs))
	}

	// Legal transition appends a system audit comment.
	if _, err := store.SetTicketState(tk.ID, StateAccepted); err != nil {
		t.Fatalf("SetTicketState legal: %v", err)
	}
	// Illegal transition is rejected.
	if _, err := store.SetTicketState(tk.ID, StateResolved); err == nil {
		t.Fatal("SetTicketState accepted an illegal accepted→resolved move")
	}
	full := store.GetTicket(tk.ID)
	if full.State != StateAccepted {
		t.Fatalf("state = %q, want accepted", full.State)
	}
	// reply + audit system comment.
	if len(full.Comments) != 2 {
		t.Fatalf("comments after transition: %d, want 2", len(full.Comments))
	}

	// Filtered list (author + state) and rejected visibility.
	mine := store.ListTickets(TicketFilter{AuthorID: author.ID, State: StateAccepted, ViewerID: author.ID})
	if len(mine) != 1 {
		t.Fatalf("filtered list: %d, want 1", len(mine))
	}

	if !store.DeleteTicket(tk.ID) {
		t.Fatal("DeleteTicket returned false")
	}
	if store.GetTicket(tk.ID) != nil {
		t.Fatal("ticket still present after delete")
	}
}

// TestSQLStoreAgentLayer exercises the WP6 work-order / deploy / agent-comment
// methods against a live MariaDB. Auto-skips without TEST_DB_DSN/DB_DSN.
func TestSQLStoreAgentLayer(t *testing.T) {
	store := newTestSQLStore(t)

	author, err := store.CreateUser("agentsql-"+newID()+"@test.com", "A", "pass12345", RoleUser)
	if err != nil {
		t.Fatalf("CreateUser: %v", err)
	}
	tk := store.CreateTicket(author.ID, "WO test", "body", "bug")
	if tk == nil {
		t.Fatal("CreateTicket nil")
	}

	// Category verdict-classification path.
	if c := store.SetTicketCategory(tk.ID, "feature"); c == nil || c.Category == nil || *c.Category != "feature" {
		t.Fatalf("SetTicketCategory: %v", c)
	}

	// Move to accepted so the queue picks it up.
	if _, err := store.SetTicketState(tk.ID, StateAccepted); err != nil {
		t.Fatalf("to accepted: %v", err)
	}

	// Work-order create (single per ticket) + duplicate rejection.
	payload := json.RawMessage(`{"summary":"do the thing","acceptance_criteria":["works"]}`)
	wo, err := store.CreateWorkOrder(tk.ID, payload)
	if err != nil || wo == nil {
		t.Fatalf("CreateWorkOrder: %v", err)
	}
	if _, err := store.CreateWorkOrder(tk.ID, payload); err == nil {
		t.Fatal("CreateWorkOrder allowed a duplicate")
	}
	if got := store.GetWorkOrder(tk.ID); got == nil || string(got.Payload) == "" {
		t.Fatalf("GetWorkOrder: %v", got)
	}

	// Accepted-with-open-work-order queue surfaces it (hydrated).
	q := store.ListAcceptedWithOpenWorkOrder()
	if len(q) != 1 || q[0].ID != tk.ID {
		t.Fatalf("ListAcceptedWithOpenWorkOrder: %d items", len(q))
	}

	// Claim (idempotent guard) then double-claim rejected.
	if _, err := store.ClaimWorkOrder(tk.ID); err != nil {
		t.Fatalf("ClaimWorkOrder: %v", err)
	}
	if _, err := store.ClaimWorkOrder(tk.ID); err == nil {
		t.Fatal("ClaimWorkOrder allowed a double claim")
	}
	if got := store.GetWorkOrder(tk.ID); got == nil || got.PickedUpAt == nil {
		t.Fatalf("picked_up_at not set: %v", got)
	}

	// Deploy round-trip.
	dep, err := store.CreateDeploy("1.2", "abc123", []string{tk.ID}, DeployLive)
	if err != nil || dep == nil {
		t.Fatalf("CreateDeploy: %v", err)
	}
	deploys := store.ListDeploys()
	if len(deploys) == 0 {
		t.Fatal("ListDeploys empty")
	}
	if len(deploys[0].TicketIDs) != 1 || deploys[0].TicketIDs[0] != tk.ID {
		t.Fatalf("deploy ticket_ids round-trip: %+v", deploys[0].TicketIDs)
	}

	// Complete with deploy link.
	done, err := store.CompleteWorkOrder(tk.ID, dep.ID)
	if err != nil || done.CompletedAt == nil || done.DeployID == nil || *done.DeployID != dep.ID {
		t.Fatalf("CompleteWorkOrder: %v / %+v", err, done)
	}
	// No longer in the open-work-order queue.
	if len(store.ListAcceptedWithOpenWorkOrder()) != 0 {
		t.Fatal("completed work-order still in queue")
	}

	// Agent comments: po (visible) and dev (internal), both author-id NULL.
	po := store.AddAgentComment(tk.ID, AuthorPO, "public ok", false, false)
	if po == nil || po.AuthorType != AuthorPO || po.AuthorID != nil {
		t.Fatalf("AddAgentComment po: %+v", po)
	}
	dev := store.AddAgentComment(tk.ID, AuthorDev, "internal note", true, false)
	if dev == nil || !dev.Internal {
		t.Fatalf("AddAgentComment dev: %+v", dev)
	}
	withheld := store.AddAgentComment(tk.ID, AuthorPO, "server/main.go leak", false, true)
	if withheld == nil || !withheld.Withheld {
		t.Fatalf("AddAgentComment withheld: %+v", withheld)
	}

	// Deleting the ticket cascades the work-order away.
	if !store.DeleteTicket(tk.ID) {
		t.Fatal("DeleteTicket false")
	}
	if store.GetWorkOrder(tk.ID) != nil {
		t.Fatal("work-order survived ticket delete")
	}
}

// TestSQLStoreModeration exercises the WP8 methods (comment po_reviewed,
// admin alerts, user disable/list + session purge) against a live MariaDB.
// Auto-skips without TEST_DB_DSN/DB_DSN.
func TestSQLStoreModeration(t *testing.T) {
	store := newTestSQLStore(t)

	author, err := store.CreateUser("modsql-"+newID()+"@test.com", "A", "pass12345", RoleUser)
	if err != nil {
		t.Fatalf("CreateUser: %v", err)
	}
	tk := store.CreateTicket(author.ID, "Mod test", "body", "bug")
	if tk == nil {
		t.Fatal("CreateTicket nil")
	}

	// B1 — a user comment starts unreviewed; po/dev comments are reviewed.
	uid := author.ID
	uc := store.AddComment(tk.ID, AuthorUser, &uid, "user reply", false)
	if uc == nil || uc.POReviewed {
		t.Fatalf("user comment should start po_reviewed=0: %+v", uc)
	}
	poc := store.AddAgentComment(tk.ID, AuthorPO, "po reply", false, false)
	if poc == nil || !poc.POReviewed {
		t.Fatalf("po comment should be po_reviewed=1: %+v", poc)
	}

	// The ticket surfaces in the unreviewed-comment query while uc is unreviewed.
	un := store.ListTicketsWithUnreviewedComments()
	if !containsTicket(un, tk.ID) {
		t.Fatal("ticket with unreviewed comment not surfaced")
	}

	// GetComment is ticket-scoped.
	if store.GetComment(tk.ID, uc.ID) == nil {
		t.Fatal("GetComment missed an existing comment")
	}
	if store.GetComment("nope", uc.ID) != nil {
		t.Fatal("GetComment matched a comment on the wrong ticket")
	}

	// Moderate the comment spam → reviewed + withheld.
	mod := store.SetCommentReviewed(tk.ID, uc.ID, true)
	if mod == nil || !mod.POReviewed || !mod.Withheld {
		t.Fatalf("SetCommentReviewed: %+v", mod)
	}
	if containsTicket(store.ListTicketsWithUnreviewedComments(), tk.ID) {
		t.Fatal("ticket still unreviewed after moderation")
	}

	// B2 — admin alerts CRUD.
	tid := tk.ID
	a1 := store.CreateAdminAlert(AlertMaliciousComment, &tid, "spam")
	a2 := store.CreateAdminAlert(AlertUserBanned, nil, "banned x")
	if a1 == nil || a2 == nil {
		t.Fatal("CreateAdminAlert nil")
	}
	all := store.ListAdminAlerts(false)
	if len(all) < 2 {
		t.Fatalf("ListAdminAlerts: %d, want >=2", len(all))
	}
	if !store.MarkAdminAlertSeen(a2.ID) {
		t.Fatal("MarkAdminAlertSeen returned false")
	}
	for _, a := range store.ListAdminAlerts(true) {
		if a.ID == a2.ID {
			t.Fatal("seen alert still in unseen list")
		}
	}

	// B3 — user disable + session purge + last-admin count.
	admin, err := store.CreateUser("modadmin-"+newID()+"@test.com", "Adm", "adminpass1", RoleAdmin)
	if err != nil {
		t.Fatalf("CreateUser admin: %v", err)
	}
	sess := store.CreateSession(author.ID)
	if sess == nil {
		t.Fatal("CreateSession nil")
	}
	if store.SetUserDisabled(author.ID, true) == nil {
		t.Fatal("SetUserDisabled nil")
	}
	// A disabled user can't authenticate.
	if _, err := store.AuthenticateUser(author.Email, "pass12345"); err == nil {
		t.Fatal("AuthenticateUser accepted a disabled user")
	}
	store.DeleteUserSessions(author.ID)
	if store.GetSession(sess.Token) != nil {
		t.Fatal("session survived DeleteUserSessions")
	}
	// AdminCount excludes the named admin (last-admin guard).
	if n := store.AdminCount(admin.ID); n < 0 {
		t.Fatalf("AdminCount negative: %d", n)
	}
	if !containsUser(store.ListUsers(), admin.ID) {
		t.Fatal("ListUsers missing the admin")
	}

	store.DeleteTicket(tk.ID)
}

func containsTicket(ts []*Ticket, id string) bool {
	for _, t := range ts {
		if t.ID == id {
			return true
		}
	}
	return false
}

func containsUser(us []*User, id string) bool {
	for _, u := range us {
		if u.ID == id {
			return true
		}
	}
	return false
}
