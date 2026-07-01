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
	"crypto/rand"
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
	"golang.org/x/crypto/argon2"
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
	salt := make([]byte, 16)
	rand.Read(salt)
	hash := argon2.IDKey([]byte(password), salt, 1, 64*1024, 4, 32)

	u := &User{
		ID:           newID(),
		Email:        email,
		DisplayName:  displayName,
		Role:         role,
		PasswordHash: hash,
		Salt:         salt,
		CreatedAt:    time.Now().UTC(),
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
	hash := argon2.IDKey([]byte(password), u.Salt, 1, 64*1024, 4, 32)
	if !bytesEqual(hash, u.PasswordHash) {
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
	salt := make([]byte, 16)
	rand.Read(salt)
	hash := argon2.IDKey([]byte(newPassword), salt, 1, 64*1024, 4, 32)
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
		Expires: time.Now().UTC().Add(7 * 24 * time.Hour),
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
	if time.Now().UTC().After(sess.Expires) {
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
		token, userID, time.Now().UTC().Add(1*time.Hour)); err != nil {
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
	if time.Now().UTC().After(expires) {
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
		CreatedAt:    time.Now().UTC(),
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
	if time.Now().UTC().After(inv.ExpiresAt) {
		return nil, errInviteInvalid
	}
	if dbUsedBy.Valid {
		return nil, errInviteUsed
	}
	now := time.Now().UTC()
	if _, err := tx.Exec(
		`UPDATE invites SET used_by = ?, used_at = ? WHERE token = ?`,
		usedBy, now, token); err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	inv.UsedBy = &usedBy
	inv.UsedAt = &now
	return inv, nil
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
	now := time.Now().UTC()
	p := &Project{
		ID:         newID(),
		OwnerID:    ownerID,
		Name:       name,
		Visibility: "private",
		Document:   doc,
		CreatedAt:  now,
		UpdatedAt:  now,
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
	p.UpdatedAt = time.Now().UTC()

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
		CreatedAt: time.Now().UTC(),
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

// ─── tickets ────────────────────────────────────────────────────

const ticketCols = `id, author_id, title, body, state, category, po_spam, po_malicious,
	po_vision_fit, po_reason, po_verdict_at, created_at, updated_at`

func (s *sqlStore) scanTicket(scan func(...any) error) *Ticket {
	t := &Ticket{}
	var state string
	var category, visionFit, reason sql.NullString
	var verdictAt sql.NullTime
	if err := scan(&t.ID, &t.AuthorID, &t.Title, &t.Body, &state, &category,
		&t.POSpam, &t.POMalicious, &visionFit, &reason, &verdictAt,
		&t.CreatedAt, &t.UpdatedAt); err != nil {
		return nil
	}
	t.State = TicketState(state)
	if category.Valid {
		t.Category = &category.String
	}
	if visionFit.Valid {
		t.POVisionFit = &visionFit.String
	}
	if reason.Valid {
		t.POReason = &reason.String
	}
	if verdictAt.Valid {
		v := verdictAt.Time
		t.POVerdictAt = &v
	}
	return t
}

func (s *sqlStore) CreateTicket(authorID, title, body, category string) *Ticket {
	now := time.Now().UTC()
	t := &Ticket{
		ID:        newID(),
		AuthorID:  authorID,
		Title:     title,
		Body:      body,
		State:     StateTriage,
		CreatedAt: now,
		UpdatedAt: now,
	}
	var cat any
	if category != "" {
		cat = category
		c := category
		t.Category = &c
	}
	if _, err := s.db.Exec(
		`INSERT INTO tickets (id, author_id, title, body, state, category, created_at, updated_at)
		 VALUES (?, ?, ?, ?, 'triage', ?, ?, ?)`,
		t.ID, t.AuthorID, t.Title, t.Body, cat, t.CreatedAt, t.UpdatedAt); err != nil {
		log.Printf("sqlStore: CreateTicket: %v", err)
		return nil
	}
	return t
}

func (s *sqlStore) GetTicket(id string) *Ticket {
	row := s.db.QueryRow(`SELECT `+ticketCols+` FROM tickets WHERE id = ?`, id)
	t := s.scanTicket(row.Scan)
	if t == nil {
		return nil
	}
	t.Comments = s.ListComments(id)
	t.Artifacts = s.listArtifacts(id)
	return t
}

func (s *sqlStore) ListTickets(f TicketFilter) []*Ticket {
	q := `SELECT ` + ticketCols + ` FROM tickets WHERE 1=1`
	var args []any
	if f.AuthorID != "" {
		q += ` AND author_id = ?`
		args = append(args, f.AuthorID)
	}
	if f.State != "" {
		q += ` AND state = ?`
		args = append(args, string(f.State))
	}
	if f.Category != "" {
		q += ` AND category = ?`
		args = append(args, f.Category)
	}
	if f.Query != "" {
		q += ` AND (title LIKE ? OR body LIKE ?)`
		like := "%" + f.Query + "%"
		args = append(args, like, like)
	}
	// Visibility (§7.3): rejected tickets are visible only to admins or the
	// ticket's own author.
	if !f.ViewerIsAdmin {
		q += ` AND (state NOT IN ('rejected_spam','rejected_malicious') OR author_id = ?)`
		args = append(args, f.ViewerID)
	}
	q += ` ORDER BY created_at DESC`

	rows, err := s.db.Query(q, args...)
	if err != nil {
		log.Printf("sqlStore: ListTickets: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Ticket
	for rows.Next() {
		if t := s.scanTicket(rows.Scan); t != nil {
			out = append(out, t)
		}
	}
	return out
}

func (s *sqlStore) UpdateTicket(id, title, body string) *Ticket {
	t := s.getTicketRow(id)
	if t == nil {
		return nil
	}
	if title != "" {
		t.Title = title
	}
	if body != "" {
		t.Body = body
	}
	t.UpdatedAt = time.Now().UTC()
	if _, err := s.db.Exec(
		`UPDATE tickets SET title = ?, body = ?, updated_at = ? WHERE id = ?`,
		t.Title, t.Body, t.UpdatedAt, id); err != nil {
		log.Printf("sqlStore: UpdateTicket: %v", err)
		return nil
	}
	return t
}

func (s *sqlStore) SetTicketCategory(id, category string) *Ticket {
	t := s.getTicketRow(id)
	if t == nil {
		return nil
	}
	now := time.Now().UTC()
	var cat any
	if category != "" {
		cat = category
		c := category
		t.Category = &c
	} else {
		t.Category = nil
	}
	if _, err := s.db.Exec(
		`UPDATE tickets SET category = ?, updated_at = ? WHERE id = ?`,
		cat, now, id); err != nil {
		log.Printf("sqlStore: SetTicketCategory: %v", err)
		return nil
	}
	t.UpdatedAt = now
	return t
}

// getTicketRow fetches a bare ticket (no comments/artifacts).
func (s *sqlStore) getTicketRow(id string) *Ticket {
	row := s.db.QueryRow(`SELECT `+ticketCols+` FROM tickets WHERE id = ?`, id)
	return s.scanTicket(row.Scan)
}

func (s *sqlStore) SetTicketState(id string, to TicketState) (*Ticket, error) {
	t := s.getTicketRow(id)
	if t == nil {
		return nil, errTicketMissing
	}
	if err := validateTransition(t.State, to); err != nil {
		return nil, err
	}
	from := t.State
	now := time.Now().UTC()
	if _, err := s.db.Exec(
		`UPDATE tickets SET state = ?, updated_at = ? WHERE id = ?`,
		string(to), now, id); err != nil {
		log.Printf("sqlStore: SetTicketState: %v", err)
		return nil, err
	}
	t.State = to
	t.UpdatedAt = now
	s.AddComment(id, AuthorSystem, nil, "state: "+string(from)+" → "+string(to), false)
	return t, nil
}

func (s *sqlStore) SetPOVerdict(id string, v POVerdict) (*Ticket, error) {
	t := s.getTicketRow(id)
	if t == nil {
		return nil, errTicketMissing
	}
	if err := validateTransition(t.State, v.State); err != nil {
		return nil, err
	}
	from := t.State
	now := time.Now().UTC()
	var visionFit any
	if v.VisionFit != nil {
		visionFit = *v.VisionFit
	}
	var reason any
	if v.Reason != "" {
		reason = v.Reason
	}
	if _, err := s.db.Exec(
		`UPDATE tickets SET po_spam = ?, po_malicious = ?, po_vision_fit = ?,
		 po_reason = ?, po_verdict_at = ?, state = ?, updated_at = ? WHERE id = ?`,
		v.Spam, v.Malicious, visionFit, reason, now, string(v.State), now, id); err != nil {
		log.Printf("sqlStore: SetPOVerdict: %v", err)
		return nil, err
	}
	t.POSpam = v.Spam
	t.POMalicious = v.Malicious
	t.POVisionFit = v.VisionFit
	if v.Reason != "" {
		r := v.Reason
		t.POReason = &r
	}
	t.POVerdictAt = &now
	t.State = v.State
	t.UpdatedAt = now
	s.AddComment(id, AuthorSystem, nil, "po verdict: "+string(from)+" → "+string(v.State), false)
	return t, nil
}

func (s *sqlStore) DeleteTicket(id string) bool {
	res, err := s.db.Exec(`DELETE FROM tickets WHERE id = ?`, id)
	if err != nil {
		log.Printf("sqlStore: DeleteTicket: %v", err)
		return false
	}
	n, _ := res.RowsAffected()
	return n > 0
}

// commentCols is the canonical comment projection (B1 adds `po_reviewed`).
const commentCols = `id, ticket_id, author_type, author_id, internal, body, created_at, withheld, po_reviewed`

func (s *sqlStore) scanComment(scan func(...any) error) *TicketComment {
	c := &TicketComment{}
	var at string
	var aid sql.NullString
	if err := scan(&c.ID, &c.TicketID, &at, &aid, &c.Internal, &c.Body, &c.CreatedAt, &c.Withheld, &c.POReviewed); err != nil {
		return nil
	}
	c.AuthorType = AuthorType(at)
	if aid.Valid {
		c.AuthorID = &aid.String
	}
	return c
}

func (s *sqlStore) AddComment(ticketID string, authorType AuthorType, authorID *string, body string, internal bool) *TicketComment {
	c := &TicketComment{
		ID:         newID(),
		TicketID:   ticketID,
		AuthorType: authorType,
		AuthorID:   authorID,
		Internal:   internal,
		Body:       body,
		CreatedAt:  time.Now().UTC(),
		// Only user comments need PO post-moderation; po/dev/system are inherently
		// reviewed (B1, §4.2).
		POReviewed: authorType != AuthorUser,
	}
	var aid any
	if authorID != nil {
		aid = *authorID
	}
	if _, err := s.db.Exec(
		`INSERT INTO ticket_comments (id, ticket_id, author_type, author_id, internal, body, created_at, withheld, po_reviewed)
		 VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)`,
		c.ID, c.TicketID, string(c.AuthorType), aid, c.Internal, c.Body, c.CreatedAt, c.POReviewed); err != nil {
		log.Printf("sqlStore: AddComment: %v", err)
		return nil
	}
	// Bump the ticket's activity timestamp.
	s.db.Exec(`UPDATE tickets SET updated_at = ? WHERE id = ?`, c.CreatedAt, ticketID)
	return c
}

func (s *sqlStore) ListComments(ticketID string) []*TicketComment {
	rows, err := s.db.Query(
		`SELECT `+commentCols+`
		 FROM ticket_comments WHERE ticket_id = ? ORDER BY created_at ASC`, ticketID)
	if err != nil {
		log.Printf("sqlStore: ListComments: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*TicketComment
	for rows.Next() {
		if c := s.scanComment(rows.Scan); c != nil {
			out = append(out, c)
		}
	}
	return out
}

func (s *sqlStore) GetComment(ticketID, commentID string) *TicketComment {
	row := s.db.QueryRow(
		`SELECT `+commentCols+` FROM ticket_comments WHERE id = ? AND ticket_id = ?`,
		commentID, ticketID)
	return s.scanComment(row.Scan)
}

// SetCommentReviewed marks a comment PO-reviewed and, when withheld, hides it
// (B1, §4.2). Withheld is sticky: it is set, never cleared, here.
func (s *sqlStore) SetCommentReviewed(commentID string, withheld bool) *TicketComment {
	var q string
	if withheld {
		q = `UPDATE ticket_comments SET po_reviewed = 1, withheld = 1 WHERE id = ?`
	} else {
		q = `UPDATE ticket_comments SET po_reviewed = 1 WHERE id = ?`
	}
	if _, err := s.db.Exec(q, commentID); err != nil {
		log.Printf("sqlStore: SetCommentReviewed: %v", err)
		return nil
	}
	row := s.db.QueryRow(`SELECT `+commentCols+` FROM ticket_comments WHERE id = ?`, commentID)
	return s.scanComment(row.Scan)
}

// ListTicketsWithUnreviewedComments returns every ticket (any state) with at
// least one unreviewed USER comment, hydrated with thread + artifacts (B1).
func (s *sqlStore) ListTicketsWithUnreviewedComments() []*Ticket {
	rows, err := s.db.Query(
		`SELECT ` + ticketCols + ` FROM tickets t
		 WHERE EXISTS (
		     SELECT 1 FROM ticket_comments c
		     WHERE c.ticket_id = t.id AND c.author_type = 'user' AND c.po_reviewed = 0)
		 ORDER BY t.created_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListTicketsWithUnreviewedComments: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Ticket
	for rows.Next() {
		if t := s.scanTicket(rows.Scan); t != nil {
			out = append(out, t)
		}
	}
	for _, t := range out {
		t.Comments = s.ListComments(t.ID)
		t.Artifacts = s.listArtifacts(t.ID)
	}
	return out
}

func (s *sqlStore) AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact {
	if len(data) == 0 {
		return nil
	}
	ref, err := s.putBlob(data)
	if err != nil {
		log.Printf("sqlStore: AddArtifact putBlob: %v", err)
		return nil
	}
	a := &TicketArtifact{
		ID:        newID(),
		TicketID:  ticketID,
		Kind:      kind,
		BlobRef:   ref,
		Mime:      mime,
		Size:      int64(len(data)),
		Name:      name,
		CreatedAt: time.Now().UTC(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO ticket_artifacts (id, ticket_id, kind, blob_ref, mime, size, name, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
		a.ID, a.TicketID, string(a.Kind), a.BlobRef, a.Mime, a.Size, a.Name, a.CreatedAt); err != nil {
		log.Printf("sqlStore: AddArtifact: %v", err)
		return nil
	}
	return a
}

func (s *sqlStore) listArtifacts(ticketID string) []*TicketArtifact {
	rows, err := s.db.Query(
		`SELECT id, ticket_id, kind, blob_ref, mime, size, name, created_at
		 FROM ticket_artifacts WHERE ticket_id = ? ORDER BY created_at ASC`, ticketID)
	if err != nil {
		log.Printf("sqlStore: listArtifacts: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*TicketArtifact
	for rows.Next() {
		a := &TicketArtifact{}
		var kind string
		if err := rows.Scan(&a.ID, &a.TicketID, &kind, &a.BlobRef, &a.Mime, &a.Size, &a.Name, &a.CreatedAt); err != nil {
			continue
		}
		a.Kind = ArtifactKind(kind)
		out = append(out, a)
	}
	return out
}

func (s *sqlStore) GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte) {
	a := &TicketArtifact{}
	var kind string
	err := s.db.QueryRow(
		`SELECT id, ticket_id, kind, blob_ref, mime, size, name, created_at
		 FROM ticket_artifacts WHERE id = ? AND ticket_id = ?`, artifactID, ticketID).
		Scan(&a.ID, &a.TicketID, &kind, &a.BlobRef, &a.Mime, &a.Size, &a.Name, &a.CreatedAt)
	if err != nil {
		return nil, nil
	}
	a.Kind = ArtifactKind(kind)
	data, err := s.getBlob(a.BlobRef)
	if err != nil {
		log.Printf("sqlStore: GetArtifactBlob getBlob: %v", err)
		return a, nil
	}
	return a, data
}

// ─── work-orders (§9) ───────────────────────────────────────────

const workOrderCols = `ticket_id, payload, created_at, picked_up_at, completed_at, deploy_id`

func (s *sqlStore) scanWorkOrder(scan func(...any) error) *WorkOrder {
	wo := &WorkOrder{}
	var payload []byte
	var pickedUp, completed sql.NullTime
	var deployID sql.NullString
	if err := scan(&wo.TicketID, &payload, &wo.CreatedAt, &pickedUp, &completed, &deployID); err != nil {
		return nil
	}
	wo.Payload = json.RawMessage(payload)
	if pickedUp.Valid {
		t := pickedUp.Time
		wo.PickedUpAt = &t
	}
	if completed.Valid {
		t := completed.Time
		wo.CompletedAt = &t
	}
	if deployID.Valid {
		wo.DeployID = &deployID.String
	}
	return wo
}

func (s *sqlStore) CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error) {
	// The work-order PK is the ticket id, and the FK enforces the ticket exists.
	if s.getTicketRow(ticketID) == nil {
		return nil, errTicketMissing
	}
	wo := &WorkOrder{
		TicketID:  ticketID,
		Payload:   payload,
		CreatedAt: time.Now().UTC(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO work_orders (ticket_id, payload, created_at) VALUES (?, ?, ?)`,
		wo.TicketID, []byte(payload), wo.CreatedAt); err != nil {
		if isDuplicate(err) {
			return nil, errWorkOrderExists
		}
		log.Printf("sqlStore: CreateWorkOrder: %v", err)
		return nil, err
	}
	return wo, nil
}

func (s *sqlStore) GetWorkOrder(ticketID string) *WorkOrder {
	row := s.db.QueryRow(`SELECT `+workOrderCols+` FROM work_orders WHERE ticket_id = ?`, ticketID)
	return s.scanWorkOrder(row.Scan)
}

func (s *sqlStore) ClaimWorkOrder(ticketID string) (*WorkOrder, error) {
	now := time.Now().UTC()
	// Atomic claim: only sets picked_up_at if still NULL.
	res, err := s.db.Exec(
		`UPDATE work_orders SET picked_up_at = ? WHERE ticket_id = ? AND picked_up_at IS NULL`,
		now, ticketID)
	if err != nil {
		log.Printf("sqlStore: ClaimWorkOrder: %v", err)
		return nil, err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		// Either it doesn't exist or it's already claimed — disambiguate.
		if s.GetWorkOrder(ticketID) == nil {
			return nil, errWorkOrderMissing
		}
		return nil, errWorkOrderClaimed
	}
	return s.GetWorkOrder(ticketID), nil
}

func (s *sqlStore) CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) {
	if s.GetWorkOrder(ticketID) == nil {
		return nil, errWorkOrderMissing
	}
	now := time.Now().UTC()
	var did any
	if deployID != "" {
		did = deployID
	}
	if _, err := s.db.Exec(
		`UPDATE work_orders SET completed_at = ?, deploy_id = COALESCE(?, deploy_id) WHERE ticket_id = ?`,
		now, did, ticketID); err != nil {
		log.Printf("sqlStore: CompleteWorkOrder: %v", err)
		return nil, err
	}
	return s.GetWorkOrder(ticketID), nil
}

// ─── deploys (§10.3) ────────────────────────────────────────────

const deployCols = `id, version, git_sha, ticket_ids, status, deployed_at`

func (s *sqlStore) scanDeploy(scan func(...any) error) *Deploy {
	d := &Deploy{}
	var ticketIDs []byte
	var status string
	if err := scan(&d.ID, &d.Version, &d.GitSHA, &ticketIDs, &status, &d.DeployedAt); err != nil {
		return nil
	}
	d.Status = DeployStatus(status)
	if len(ticketIDs) > 0 {
		json.Unmarshal(ticketIDs, &d.TicketIDs)
	}
	if d.TicketIDs == nil {
		d.TicketIDs = []string{}
	}
	return d
}

func (s *sqlStore) CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error) {
	if ticketIDs == nil {
		ticketIDs = []string{}
	}
	d := &Deploy{
		ID:         newID(),
		Version:    version,
		GitSHA:     gitSHA,
		TicketIDs:  ticketIDs,
		Status:     status,
		DeployedAt: time.Now().UTC(),
	}
	idsJSON, err := json.Marshal(ticketIDs)
	if err != nil {
		return nil, err
	}
	if _, err := s.db.Exec(
		`INSERT INTO deploys (id, version, git_sha, ticket_ids, status, deployed_at)
		 VALUES (?, ?, ?, ?, ?, ?)`,
		d.ID, d.Version, d.GitSHA, idsJSON, string(d.Status), d.DeployedAt); err != nil {
		log.Printf("sqlStore: CreateDeploy: %v", err)
		return nil, err
	}
	return d, nil
}

func (s *sqlStore) ListDeploys() []*Deploy {
	rows, err := s.db.Query(`SELECT ` + deployCols + ` FROM deploys ORDER BY deployed_at DESC, id DESC`)
	if err != nil {
		log.Printf("sqlStore: ListDeploys: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Deploy
	for rows.Next() {
		if d := s.scanDeploy(rows.Scan); d != nil {
			out = append(out, d)
		}
	}
	return out
}

// ─── agent comments ─────────────────────────────────────────────

func (s *sqlStore) AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment {
	c := &TicketComment{
		ID:         newID(),
		TicketID:   ticketID,
		AuthorType: authorType,
		AuthorID:   nil, // po/dev/system have no user id
		Internal:   internal,
		Body:       body,
		CreatedAt:  time.Now().UTC(),
		Withheld:   withheld,
		POReviewed: true, // po/dev are authored by the agents — inherently reviewed
	}
	if _, err := s.db.Exec(
		`INSERT INTO ticket_comments (id, ticket_id, author_type, author_id, internal, body, created_at, withheld, po_reviewed)
		 VALUES (?, ?, ?, NULL, ?, ?, ?, ?, 1)`,
		c.ID, c.TicketID, string(c.AuthorType), c.Internal, c.Body, c.CreatedAt, c.Withheld); err != nil {
		log.Printf("sqlStore: AddAgentComment: %v", err)
		return nil
	}
	s.db.Exec(`UPDATE tickets SET updated_at = ? WHERE id = ?`, c.CreatedAt, ticketID)
	return c
}

// ListAcceptedWithOpenWorkOrder returns accepted tickets that still carry an
// open (uncompleted) work-order — the coding agent's queue (§10.1).
func (s *sqlStore) ListAcceptedWithOpenWorkOrder() []*Ticket {
	rows, err := s.db.Query(
		`SELECT ` + ticketCols + ` FROM tickets t
		 WHERE t.state = 'accepted'
		   AND EXISTS (
		       SELECT 1 FROM work_orders w
		       WHERE w.ticket_id = t.id AND w.completed_at IS NULL)
		 ORDER BY t.created_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListAcceptedWithOpenWorkOrder: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Ticket
	for rows.Next() {
		if t := s.scanTicket(rows.Scan); t != nil {
			out = append(out, t)
		}
	}
	// Hydrate thread + artifacts so the agent has full repro context.
	for _, t := range out {
		t.Comments = s.ListComments(t.ID)
		t.Artifacts = s.listArtifacts(t.ID)
	}
	return out
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
		CreatedAt: time.Now().UTC(),
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
