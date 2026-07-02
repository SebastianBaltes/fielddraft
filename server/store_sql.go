// sqlStore — MariaDB implementation of Store.
//
// database/sql + the go-sql-driver/mysql driver. Large/binary payloads
// (project documents, snapshot documents, thumbnails) are NOT stored in the
// DB; they are written to content-addressed blob files (sha256 hex) under
// BLOB_DIR and the rows carry only the ref. This keeps MariaDB schlank and
// makes backups a DB dump plus an rsync of the blob dir (§5.2). Blob storage
// is a private detail here: the Store interface still passes documents as
// json.RawMessage/[]byte, so handlers and API shapes are unchanged.

package main

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"time"

	"github.com/go-sql-driver/mysql"
)

type sqlStore struct {
	db      *sql.DB
	blobDir string
}

// NewSQLStore connects to MariaDB, runs migrations, and prepares the
// content-addressed blob directory. Fails fast if the DB is unreachable.
func NewSQLStore(dsn string) (*sqlStore, error) {
	db, err := sql.Open("mysql", dsn)
	if err != nil {
		return nil, fmt.Errorf("open db: %w", err)
	}
	db.SetConnMaxLifetime(3 * time.Minute)
	db.SetMaxOpenConns(20)
	db.SetMaxIdleConns(10)
	if err := db.Ping(); err != nil {
		db.Close()
		return nil, fmt.Errorf("ping db: %w", err)
	}

	if err := runMigrations(db); err != nil {
		db.Close()
		return nil, fmt.Errorf("migrate: %w", err)
	}

	blobDir, err := resolveBlobDir()
	if err != nil {
		db.Close()
		return nil, err
	}
	log.Printf("sqlStore: blob dir = %s", blobDir)

	return &sqlStore{db: db, blobDir: blobDir}, nil
}

// Close releases the underlying database pool.
func (s *sqlStore) Close() error { return s.db.Close() }

// resolveBlobDir picks BLOB_DIR (default /data/blobs), falling back to
// ./data/blobs if /data isn't writable, so the server also runs locally.
func resolveBlobDir() (string, error) {
	dir := os.Getenv("BLOB_DIR")
	if dir == "" {
		dir = "/data/blobs"
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		fallback := filepath.Join(".", "data", "blobs")
		if dir == "/data/blobs" {
			if ferr := os.MkdirAll(fallback, 0o755); ferr == nil {
				log.Printf("sqlStore: %s not writable, using %s", dir, fallback)
				return fallback, nil
			}
		}
		return "", fmt.Errorf("blob dir %q: %w", dir, err)
	}
	return dir, nil
}

// ─── content-addressed blobs ────────────────────────────────────

// putBlob writes data to <blobDir>/<sha256> (idempotent, deduplicated) and
// returns the hex ref. Empty/nil data yields an empty ref (no file).
func (s *sqlStore) putBlob(data []byte) (string, error) {
	if len(data) == 0 {
		return "", nil
	}
	sum := sha256.Sum256(data)
	ref := hex.EncodeToString(sum[:])
	path := filepath.Join(s.blobDir, ref)
	if _, err := os.Stat(path); err == nil {
		return ref, nil // already present — dedup
	}
	// Atomic-ish write via a temp file then rename.
	tmp := path + ".tmp-" + newID()
	if err := os.WriteFile(tmp, data, 0o644); err != nil {
		return "", err
	}
	if err := os.Rename(tmp, path); err != nil {
		os.Remove(tmp)
		return "", err
	}
	return ref, nil
}

// getBlob loads a blob by ref. An empty ref yields nil with no error.
func (s *sqlStore) getBlob(ref string) ([]byte, error) {
	if ref == "" {
		return nil, nil
	}
	data, err := os.ReadFile(filepath.Join(s.blobDir, ref))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	return data, err
}

// ─── users & auth ───────────────────────────────────────────────

func (s *sqlStore) CreateUser(email, displayName, password string, role Role) (*User, error) {
	hash, salt := hashPassword(password)
	u := &User{
		ID:           newID(),
		Email:        email,
		DisplayName:  displayName,
		Role:         role,
		PasswordHash: hash,
		Salt:         salt,
		CreatedAt:    now(),
	}
	_, err := s.db.Exec(
		`INSERT INTO users (id, email, display_name, password_hash, salt, role, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		u.ID, u.Email, u.DisplayName, u.PasswordHash, u.Salt, string(u.Role), u.CreatedAt)
	if err != nil {
		if isDuplicate(err) {
			return nil, errConflict
		}
		return nil, err
	}
	return u, nil
}

// userCols is the canonical user projection (B3 adds `disabled`).
const userCols = `id, email, display_name, password_hash, salt, role, disabled, created_at`

func (s *sqlStore) scanUserRow(scan func(...any) error) *User {
	u := &User{}
	var role string
	if err := scan(&u.ID, &u.Email, &u.DisplayName, &u.PasswordHash, &u.Salt, &role, &u.Disabled, &u.CreatedAt); err != nil {
		return nil
	}
	u.Role = Role(role)
	return u
}

func (s *sqlStore) scanUser(row *sql.Row) *User {
	return s.scanUserRow(row.Scan)
}

func (s *sqlStore) AuthenticateUser(email, password string) (*User, error) {
	u := s.scanUser(s.db.QueryRow(
		`SELECT `+userCols+` FROM users WHERE email = ?`, email))
	if u == nil {
		return nil, errNotFound
	}
	if !verifyPassword(password, u.Salt, u.PasswordHash) {
		return nil, errUnauthorized
	}
	// A banned user cannot authenticate (B3, §4.4).
	if u.Disabled {
		return nil, errUserDisabled
	}
	return u, nil
}

func (s *sqlStore) GetUser(id string) *User {
	return s.scanUser(s.db.QueryRow(
		`SELECT `+userCols+` FROM users WHERE id = ?`, id))
}

func (s *sqlStore) GetUserByEmail(email string) *User {
	return s.scanUser(s.db.QueryRow(
		`SELECT `+userCols+` FROM users WHERE email = ?`, email))
}

func (s *sqlStore) UpdatePassword(userID, newPassword string) {
	hash, salt := hashPassword(newPassword)
	if _, err := s.db.Exec(
		`UPDATE users SET password_hash = ?, salt = ? WHERE id = ?`,
		hash, salt, userID); err != nil {
		log.Printf("sqlStore: UpdatePassword: %v", err)
	}
}

func (s *sqlStore) HasAdmin() bool {
	var n int
	if err := s.db.QueryRow(`SELECT COUNT(*) FROM users WHERE role = 'admin'`).Scan(&n); err != nil {
		log.Printf("sqlStore: HasAdmin: %v", err)
		return false
	}
	return n > 0
}

// ─── user administration / ban (B3) ─────────────────────────────

func (s *sqlStore) ListUsers() []*User {
	rows, err := s.db.Query(`SELECT ` + userCols + ` FROM users ORDER BY created_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListUsers: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*User
	for rows.Next() {
		if u := s.scanUserRow(rows.Scan); u != nil {
			out = append(out, u)
		}
	}
	return out
}

func (s *sqlStore) SetUserDisabled(userID string, disabled bool) *User {
	if _, err := s.db.Exec(
		`UPDATE users SET disabled = ? WHERE id = ?`, disabled, userID); err != nil {
		log.Printf("sqlStore: SetUserDisabled: %v", err)
		return nil
	}
	return s.GetUser(userID)
}

// AdminCount returns the number of enabled admins not counting excludeUserID
// (the last-admin-ban guard, B3).
func (s *sqlStore) AdminCount(excludeUserID string) int {
	var n int
	if err := s.db.QueryRow(
		`SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0 AND id <> ?`,
		excludeUserID).Scan(&n); err != nil {
		log.Printf("sqlStore: AdminCount: %v", err)
		return 0
	}
	return n
}

func (s *sqlStore) DeleteUserSessions(userID string) {
	if _, err := s.db.Exec(`DELETE FROM sessions WHERE user_id = ?`, userID); err != nil {
		log.Printf("sqlStore: DeleteUserSessions: %v", err)
	}
}

// ─── sessions ───────────────────────────────────────────────────

func (s *sqlStore) CreateSession(userID string) *Session {
	sess := &Session{
		Token:   newID(),
		UserID:  userID,
		Expires: now().Add(sessionTTL),
	}
	if _, err := s.db.Exec(
		`INSERT INTO sessions (token, user_id, expires_at) VALUES (?, ?, ?)`,
		sess.Token, sess.UserID, sess.Expires); err != nil {
		log.Printf("sqlStore: CreateSession: %v", err)
		return nil
	}
	return sess
}

func (s *sqlStore) GetSession(token string) *Session {
	sess := &Session{}
	err := s.db.QueryRow(
		`SELECT token, user_id, expires_at FROM sessions WHERE token = ?`, token).
		Scan(&sess.Token, &sess.UserID, &sess.Expires)
	if err != nil {
		return nil
	}
	if now().After(sess.Expires) {
		s.DeleteSession(token)
		return nil
	}
	return sess
}

func (s *sqlStore) DeleteSession(token string) {
	if _, err := s.db.Exec(`DELETE FROM sessions WHERE token = ?`, token); err != nil {
		log.Printf("sqlStore: DeleteSession: %v", err)
	}
}

// ─── password reset ─────────────────────────────────────────────

func (s *sqlStore) CreateResetToken(userID string) string {
	token := newID()
	if _, err := s.db.Exec(
		`INSERT INTO reset_tokens (token, user_id, expires_at) VALUES (?, ?, ?)`,
		token, userID, now().Add(resetTokenTTL)); err != nil {
		log.Printf("sqlStore: CreateResetToken: %v", err)
		return ""
	}
	return token
}

func (s *sqlStore) ConsumeResetToken(token string) (string, bool) {
	var userID string
	var expires time.Time
	err := s.db.QueryRow(
		`SELECT user_id, expires_at FROM reset_tokens WHERE token = ?`, token).
		Scan(&userID, &expires)
	if err != nil {
		return "", false
	}
	s.db.Exec(`DELETE FROM reset_tokens WHERE token = ?`, token)
	if now().After(expires) {
		return "", false
	}
	return userID, true
}

// ─── invites ────────────────────────────────────────────────────

func (s *sqlStore) CreateInvite(createdBy, inviteeName, inviteeEmail, message string, expiresAt time.Time) *Invite {
	inv := &Invite{
		Token:        newID(),
		CreatedBy:    createdBy,
		InviteeName:  inviteeName,
		InviteeEmail: inviteeEmail,
		Message:      message,
		ExpiresAt:    expiresAt.UTC(),
		CreatedAt:    now(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO invites (token, created_by, invitee_name, invitee_email, message, expires_at, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		inv.Token, inv.CreatedBy, inv.InviteeName, inv.InviteeEmail, inv.Message, inv.ExpiresAt, inv.CreatedAt); err != nil {
		log.Printf("sqlStore: CreateInvite: %v", err)
		return nil
	}
	return inv
}

func (s *sqlStore) scanInvite(scan func(...any) error) *Invite {
	inv := &Invite{}
	var usedBy sql.NullString
	var usedAt sql.NullTime
	var message sql.NullString
	if err := scan(&inv.Token, &inv.CreatedBy, &inv.InviteeName, &inv.InviteeEmail, &message, &inv.ExpiresAt, &usedBy, &usedAt, &inv.CreatedAt); err != nil {
		return nil
	}
	inv.Message = message.String
	if usedBy.Valid {
		inv.UsedBy = &usedBy.String
	}
	if usedAt.Valid {
		t := usedAt.Time
		inv.UsedAt = &t
	}
	return inv
}

func (s *sqlStore) GetInvite(token string) *Invite {
	row := s.db.QueryRow(
		`SELECT token, created_by, invitee_name, invitee_email, message, expires_at, used_by, used_at, created_at
		 FROM invites WHERE token = ?`, token)
	return s.scanInvite(row.Scan)
}

func (s *sqlStore) ListInvites() []*Invite {
	rows, err := s.db.Query(
		`SELECT token, created_by, invitee_name, invitee_email, message, expires_at, used_by, used_at, created_at
		 FROM invites ORDER BY created_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListInvites: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Invite
	for rows.Next() {
		if inv := s.scanInvite(rows.Scan); inv != nil {
			out = append(out, inv)
		}
	}
	return out
}

func (s *sqlStore) ConsumeInvite(token, usedBy string) (*Invite, error) {
	tx, err := s.db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()

	inv := &Invite{}
	var dbUsedBy sql.NullString
	var dbUsedAt sql.NullTime
	var message sql.NullString
	err = tx.QueryRow(
		`SELECT token, created_by, invitee_name, invitee_email, message, expires_at, used_by, used_at, created_at
		 FROM invites WHERE token = ? FOR UPDATE`, token).
		Scan(&inv.Token, &inv.CreatedBy, &inv.InviteeName, &inv.InviteeEmail, &message, &inv.ExpiresAt, &dbUsedBy, &dbUsedAt, &inv.CreatedAt)
	if err != nil {
		return nil, errInviteInvalid
	}
	inv.Message = message.String
	if now().After(inv.ExpiresAt) {
		return nil, errInviteInvalid
	}
	if dbUsedBy.Valid {
		return nil, errInviteUsed
	}
	usedAt := now()
	if _, err := tx.Exec(
		`UPDATE invites SET used_by = ?, used_at = ? WHERE token = ?`,
		usedBy, usedAt, token); err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	inv.UsedBy = &usedBy
	inv.UsedAt = &usedAt
	return inv, nil
}

// RegisterWithInvite validates + burns the invite and creates the user in ONE
// transaction. The invite row is locked FOR UPDATE, so two racing registrations
// with the same token serialize: the loser sees used_by set and no user row is
// left behind. A duplicate email rolls the whole Tx back — the token survives.
func (s *sqlStore) RegisterWithInvite(email, displayName, password, token string) (*User, error) {
	tx, err := s.db.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()

	var expiresAt time.Time
	var usedBy sql.NullString
	if err := tx.QueryRow(
		`SELECT expires_at, used_by FROM invites WHERE token = ? FOR UPDATE`, token).
		Scan(&expiresAt, &usedBy); err != nil {
		return nil, errInviteInvalid
	}
	if now().After(expiresAt) {
		return nil, errInviteInvalid
	}
	if usedBy.Valid {
		return nil, errInviteUsed
	}

	hash, salt := hashPassword(password)
	u := &User{
		ID:           newID(),
		Email:        email,
		DisplayName:  displayName,
		Role:         RoleUser,
		PasswordHash: hash,
		Salt:         salt,
		CreatedAt:    now(),
	}
	if _, err := tx.Exec(
		`INSERT INTO users (id, email, display_name, password_hash, salt, role, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?)`,
		u.ID, u.Email, u.DisplayName, u.PasswordHash, u.Salt, string(u.Role), u.CreatedAt); err != nil {
		if isDuplicate(err) {
			return nil, errConflict
		}
		return nil, err
	}
	if _, err := tx.Exec(
		`UPDATE invites SET used_by = ?, used_at = ? WHERE token = ?`,
		u.ID, now(), token); err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	return u, nil
}

func (s *sqlStore) DeleteInvite(token string) bool {
	res, err := s.db.Exec(`DELETE FROM invites WHERE token = ?`, token)
	if err != nil {
		log.Printf("sqlStore: DeleteInvite: %v", err)
		return false
	}
	n, _ := res.RowsAffected()
	return n > 0
}

// ─── projects ───────────────────────────────────────────────────

func (s *sqlStore) CreateProject(ownerID, name string, doc json.RawMessage) *Project {
	ref, err := s.putBlob(doc)
	if err != nil {
		log.Printf("sqlStore: CreateProject putBlob: %v", err)
		return nil
	}
	at := now()
	p := &Project{
		ID:         newID(),
		OwnerID:    ownerID,
		Name:       name,
		Visibility: "private",
		Document:   doc,
		CreatedAt:  at,
		UpdatedAt:  at,
	}
	if _, err := s.db.Exec(
		`INSERT INTO projects (id, owner_id, name, visibility, document_ref, created_at, updated_at)
		 VALUES (?, ?, ?, 'private', ?, ?, ?)`,
		p.ID, p.OwnerID, p.Name, nullIfEmpty(ref), p.CreatedAt, p.UpdatedAt); err != nil {
		log.Printf("sqlStore: CreateProject: %v", err)
		return nil
	}
	return p
}

// scanProject reads a project row and inlines its document blob.
func (s *sqlStore) scanProject(scan func(...any) error) *Project {
	p := &Project{}
	var docRef sql.NullString
	if err := scan(&p.ID, &p.OwnerID, &p.Name, &p.Visibility, &docRef, &p.CreatedAt, &p.UpdatedAt); err != nil {
		return nil
	}
	if docRef.Valid {
		data, err := s.getBlob(docRef.String)
		if err != nil {
			log.Printf("sqlStore: scanProject getBlob: %v", err)
		}
		p.Document = json.RawMessage(data)
	}
	return p
}

func (s *sqlStore) ListProjects(ownerID string) []*Project {
	rows, err := s.db.Query(
		`SELECT id, owner_id, name, visibility, document_ref, created_at, updated_at
		 FROM projects WHERE owner_id = ? ORDER BY updated_at DESC`, ownerID)
	if err != nil {
		log.Printf("sqlStore: ListProjects: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Project
	for rows.Next() {
		if p := s.scanProject(rows.Scan); p != nil {
			out = append(out, p)
		}
	}
	return out
}

func (s *sqlStore) GetProject(id, ownerID string) *Project {
	row := s.db.QueryRow(
		`SELECT id, owner_id, name, visibility, document_ref, created_at, updated_at
		 FROM projects WHERE id = ? AND owner_id = ?`, id, ownerID)
	return s.scanProject(row.Scan)
}

func (s *sqlStore) UpdateProject(id, ownerID, name string, doc json.RawMessage, visibility string) *Project {
	// Verify ownership first.
	p := s.GetProject(id, ownerID)
	if p == nil {
		return nil
	}
	if name != "" {
		p.Name = name
	}
	if visibility == "public" || visibility == "private" {
		p.Visibility = visibility
	}
	var docRef any = nil
	if doc != nil {
		ref, err := s.putBlob(doc)
		if err != nil {
			log.Printf("sqlStore: UpdateProject putBlob: %v", err)
			return nil
		}
		docRef = nullIfEmpty(ref)
		p.Document = doc
	}
	p.UpdatedAt = now()

	if doc != nil {
		_, err := s.db.Exec(
			`UPDATE projects SET name = ?, visibility = ?, document_ref = ?, updated_at = ?
			 WHERE id = ? AND owner_id = ?`,
			p.Name, p.Visibility, docRef, p.UpdatedAt, id, ownerID)
		if err != nil {
			log.Printf("sqlStore: UpdateProject: %v", err)
			return nil
		}
	} else {
		_, err := s.db.Exec(
			`UPDATE projects SET name = ?, visibility = ?, updated_at = ?
			 WHERE id = ? AND owner_id = ?`,
			p.Name, p.Visibility, p.UpdatedAt, id, ownerID)
		if err != nil {
			log.Printf("sqlStore: UpdateProject: %v", err)
			return nil
		}
	}
	return p
}

func (s *sqlStore) ListPublicProjects() []*Project {
	rows, err := s.db.Query(
		`SELECT id, owner_id, name, visibility, document_ref, created_at, updated_at
		 FROM projects WHERE visibility = 'public' ORDER BY updated_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListPublicProjects: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Project
	for rows.Next() {
		if p := s.scanProject(rows.Scan); p != nil {
			out = append(out, p)
		}
	}
	return out
}

func (s *sqlStore) DeleteProject(id, ownerID string) bool {
	res, err := s.db.Exec(`DELETE FROM projects WHERE id = ? AND owner_id = ?`, id, ownerID)
	if err != nil {
		log.Printf("sqlStore: DeleteProject: %v", err)
		return false
	}
	n, _ := res.RowsAffected()
	return n > 0
}

// ─── thumbnails ─────────────────────────────────────────────────

func (s *sqlStore) SetThumbnail(projectID string, data []byte) {
	ref, err := s.putBlob(data)
	if err != nil {
		log.Printf("sqlStore: SetThumbnail putBlob: %v", err)
		return
	}
	if _, err := s.db.Exec(
		`UPDATE projects SET thumbnail_ref = ? WHERE id = ?`,
		nullIfEmpty(ref), projectID); err != nil {
		log.Printf("sqlStore: SetThumbnail: %v", err)
	}
}

func (s *sqlStore) GetThumbnail(projectID string) []byte {
	var ref sql.NullString
	if err := s.db.QueryRow(
		`SELECT thumbnail_ref FROM projects WHERE id = ?`, projectID).Scan(&ref); err != nil {
		return nil
	}
	if !ref.Valid {
		return nil
	}
	data, err := s.getBlob(ref.String)
	if err != nil {
		log.Printf("sqlStore: GetThumbnail getBlob: %v", err)
		return nil
	}
	return data
}

// ─── snapshots ──────────────────────────────────────────────────

func (s *sqlStore) CreateSnapshot(projectID string, doc json.RawMessage) *Snapshot {
	ref, err := s.putBlob(doc)
	if err != nil {
		log.Printf("sqlStore: CreateSnapshot putBlob: %v", err)
		return nil
	}
	snap := &Snapshot{
		ID:        newID(),
		ProjectID: projectID,
		Document:  doc,
		CreatedAt: now(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO snapshots (id, project_id, document_ref, created_at) VALUES (?, ?, ?, ?)`,
		snap.ID, snap.ProjectID, nullIfEmpty(ref), snap.CreatedAt); err != nil {
		log.Printf("sqlStore: CreateSnapshot: %v", err)
		return nil
	}
	return snap
}

func (s *sqlStore) GetSnapshot(id string) *Snapshot {
	snap := &Snapshot{}
	var ref sql.NullString
	err := s.db.QueryRow(
		`SELECT id, project_id, document_ref, created_at FROM snapshots WHERE id = ?`, id).
		Scan(&snap.ID, &snap.ProjectID, &ref, &snap.CreatedAt)
	if err != nil {
		return nil
	}
	if ref.Valid {
		data, err := s.getBlob(ref.String)
		if err != nil {
			log.Printf("sqlStore: GetSnapshot getBlob: %v", err)
		}
		snap.Document = json.RawMessage(data)
	}
	return snap
}

// ─── admin alerts (B2, §4.4) ────────────────────────────────────

const adminAlertCols = `id, kind, ticket_id, detail, created_at, seen`

func (s *sqlStore) scanAdminAlert(scan func(...any) error) *AdminAlert {
	a := &AdminAlert{}
	var kind string
	var ticketID sql.NullString
	if err := scan(&a.ID, &kind, &ticketID, &a.Detail, &a.CreatedAt, &a.Seen); err != nil {
		return nil
	}
	a.Kind = AlertKind(kind)
	if ticketID.Valid {
		a.TicketID = &ticketID.String
	}
	return a
}

func (s *sqlStore) CreateAdminAlert(kind AlertKind, ticketID *string, detail string) *AdminAlert {
	a := &AdminAlert{
		ID:        newID(),
		Kind:      kind,
		Detail:    detail,
		CreatedAt: now(),
	}
	var tid any
	if ticketID != nil {
		tid = *ticketID
		t := *ticketID
		a.TicketID = &t
	}
	if _, err := s.db.Exec(
		`INSERT INTO admin_alerts (id, kind, ticket_id, detail, created_at, seen)
		 VALUES (?, ?, ?, ?, ?, 0)`,
		a.ID, string(a.Kind), tid, a.Detail, a.CreatedAt); err != nil {
		log.Printf("sqlStore: CreateAdminAlert: %v", err)
		return nil
	}
	return a
}

func (s *sqlStore) ListAdminAlerts(unseenOnly bool) []*AdminAlert {
	q := `SELECT ` + adminAlertCols + ` FROM admin_alerts`
	if unseenOnly {
		q += ` WHERE seen = 0`
	}
	q += ` ORDER BY created_at DESC, id DESC`
	rows, err := s.db.Query(q)
	if err != nil {
		log.Printf("sqlStore: ListAdminAlerts: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*AdminAlert
	for rows.Next() {
		if a := s.scanAdminAlert(rows.Scan); a != nil {
			out = append(out, a)
		}
	}
	return out
}

func (s *sqlStore) MarkAdminAlertSeen(id string) bool {
	res, err := s.db.Exec(`UPDATE admin_alerts SET seen = 1 WHERE id = ?`, id)
	if err != nil {
		log.Printf("sqlStore: MarkAdminAlertSeen: %v", err)
		return false
	}
	n, _ := res.RowsAffected()
	return n > 0
}

// ─── helpers ────────────────────────────────────────────────────

// nullIfEmpty returns nil for an empty ref so the DB column is NULL, else the ref.
func nullIfEmpty(ref string) any {
	if ref == "" {
		return nil
	}
	return ref
}

// isDuplicate reports whether err is a MySQL duplicate-key (1062) error.
func isDuplicate(err error) bool {
	var me *mysql.MySQLError
	return errors.As(err, &me) && me.Number == 1062
}
