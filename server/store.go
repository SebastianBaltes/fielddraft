// The Store interface and its shared errors.
//
// Two implementations back it:
//   - memStore   (store_mem.go) — in-memory maps; a fast, hermetic test
//     fixture. NewStore() returns it, so the handler tests run with no DB.
//   - sqlStore   (store_sql.go) — MariaDB via database/sql + content-addressed
//     blob files on disk. main() wires it up from DB_DSN.
//
// Documents/thumbnails cross this boundary as []byte / json.RawMessage exactly
// as before; blob storage is a private detail of sqlStore. So handlers and API
// shapes are unchanged regardless of which store is installed.

package main

import (
	"encoding/json"
	"net/http"
	"time"
)

// Store is everything the HTTP layer needs from persistence.
type Store interface {
	// Users & auth.
	CreateUser(email, displayName, password string, role Role) (*User, error)
	AuthenticateUser(email, password string) (*User, error) // rejects disabled users
	GetUser(id string) *User
	GetUserByEmail(email string) *User
	UpdatePassword(userID, newPassword string)
	HasAdmin() bool

	// User administration / ban (B3, §4.4).
	ListUsers() []*User // all users, newest first
	SetUserDisabled(userID string, disabled bool) *User
	AdminCount(excludeUserID string) int // # of admins not counting excludeUserID
	DeleteUserSessions(userID string)    // purge a user's sessions on ban

	// Sessions (DB-backed in prod so they survive restarts).
	CreateSession(userID string) *Session
	GetSession(token string) *Session
	DeleteSession(token string)

	// Password reset.
	CreateResetToken(userID string) string
	ConsumeResetToken(token string) (string, bool)

	// Invites (admin-gated registration).
	CreateInvite(createdBy, inviteeName, inviteeEmail, message string, expiresAt time.Time) *Invite
	GetInvite(token string) *Invite
	ListInvites() []*Invite
	ConsumeInvite(token, usedBy string) (*Invite, error)
	DeleteInvite(token string) bool

	// RegisterWithInvite validates + burns the invite and creates the user as
	// ONE atomic operation (a transaction in sqlStore, one lock section in
	// memStore) — two racing registrations with the same token can never both
	// create a user, and a duplicate-email failure never burns the token.
	// Errors: errInviteInvalid / errInviteUsed / errConflict.
	RegisterWithInvite(email, displayName, password, inviteToken string) (*User, error)

	// Projects.
	CreateProject(ownerID, name string, doc json.RawMessage) *Project
	ListProjects(ownerID string) []*Project
	GetProject(id, ownerID string) *Project
	UpdateProject(id, ownerID, name string, doc json.RawMessage, visibility string) *Project
	DeleteProject(id, ownerID string) bool
	ListPublicProjects() []*Project

	// Thumbnails (content-addressed blobs in sqlStore).
	SetThumbnail(projectID string, data []byte)
	GetThumbnail(projectID string) []byte

	// Content-addressed asset blobs (client cloud-save asset sync, plan §5.5).
	// A lean project document references large mesh/font payloads by sha256 hex;
	// those bytes live here, deduplicated and immutable. PutBlob stores data and
	// returns its ref (idempotent); GetBlob returns the bytes or nil when absent;
	// HasBlob reports presence without reading the bytes (batch-missing probe).
	PutBlob(data []byte) (string, error)
	GetBlob(ref string) ([]byte, error)
	HasBlob(ref string) bool

	// Garbage collection of orphaned asset blobs (plan §6, blobs_gc.go). The
	// content-addressed blob store is SHARED across project documents,
	// thumbnails, snapshots, mesh-job STLs, ticket artifacts, AND the mesh/font
	// asset blobs a lean project document references by hash. GC must protect
	// every one of those categories, so it unions two reference sources:
	//
	//   ListReferencedRefs   — every blob ref held DIRECTLY in a store column /
	//     field (document / thumbnail / snapshot / mesh-artifact / ticket-
	//     artifact). Deleting one would drop live data, so these are NEVER
	//     collectible.
	//   AllProjectDocuments  — every project's stored document, so GC can walk it
	//     for embedded mesh (`__asset`) + font (`fonts`) blob hashes.
	//
	// ListBlobRefs enumerates the physical blobs (ref + mtime + size); DeleteBlob
	// removes one. All three can fail (a DB / filesystem error); GC then ABORTS
	// and deletes NOTHING — an incomplete reference set must never delete a live
	// blob (fail closed). Only blobs absent from BOTH reference sources AND older
	// than the grace window are ever deleted (see gcOrphanBlobs).
	AllProjectDocuments() ([]json.RawMessage, error)
	ListReferencedRefs() ([]string, error)
	ListBlobRefs() ([]BlobRef, error)
	DeleteBlob(ref string) error

	// Snapshots (published, shareable copies).
	CreateSnapshot(projectID string, doc json.RawMessage) *Snapshot
	GetSnapshot(id string) *Snapshot

	// The ticket domain (tickets / comments / artifacts / work-orders / deploys)
	// is factored out into the pluggable TicketStore interface (ticketstore.go)
	// so it can be file-backed for local dev. Both memStore and sqlStore still
	// implement every TicketStore method, so a *Store also satisfies TicketStore
	// — the DB backend is simply the existing store's ticket methods (no behavior
	// change). See Server.tickets and TICKET_BACKEND selection in NewServer.

	// Mesh jobs (server-side CGAL meshing, plan §P4). The STL result is a
	// content-addressed blob; MeshJobArtifact is owner-gated like GetMeshJob.
	CreateMeshJob(ownerID, projectID, partID, paramsJSON string) *MeshJob
	GetMeshJob(id, ownerID string) *MeshJob
	ClaimNextMeshJob() *MeshJob // oldest queued → running; nil if none
	FinishMeshJob(id string, status MeshJobStatus, stl []byte, reportJSON, errMsg string)
	CancelMeshJob(id, ownerID string) bool // queued → error("canceled"); running/done untouched
	ActiveMeshJobs(ownerID string) int     // # of queued+running jobs of one user
	MeshJobArtifact(id, ownerID string) []byte
	ResetStaleMeshJobs() // boot recovery: running → queued

	// Admin alerts (B2, §4.4 admin-awareness): real notify records for malicious
	// tickets/comments, withheld outgoing comments, and user bans.
	CreateAdminAlert(kind AlertKind, ticketID *string, detail string) *AdminAlert
	ListAdminAlerts(unseenOnly bool) []*AdminAlert // newest first
	MarkAdminAlertSeen(id string) bool
}

var (
	errConflict      = &httpError{http.StatusConflict, "email already registered"}
	errNotFound      = &httpError{http.StatusNotFound, "not found"}
	errUnauthorized  = &httpError{http.StatusUnauthorized, "invalid credentials"}
	errUserDisabled  = &httpError{http.StatusForbidden, "account disabled"}
	errInviteInvalid = &httpError{http.StatusBadRequest, "invalid or expired invite"}
	errInviteUsed    = &httpError{http.StatusBadRequest, "invite already used"}
)

// The ticket-domain errors (errTicketMissing, errWorkOrder*, errDeployMissing)
// live next to their interface in ticketstore.go.

type httpError struct {
	Code    int
	Message string
}

func (e *httpError) Error() string { return e.Message }
