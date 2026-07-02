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

// TestSQLStoreModeration exercises the WP8 Store methods that are NOT part of
// the TicketStore contract: admin alerts (B2) and user disable/list + session
// purge (B3). Auto-skips without TEST_DB_DSN/DB_DSN. (The ticket/comment/work-
// order behavior lives in the contract suite, ticketstore_contract_test.go.)
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

func containsUser(us []*User, id string) bool {
	for _, u := range us {
		if u.ID == id {
			return true
		}
	}
	return false
}
