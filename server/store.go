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

	// Snapshots (published, shareable copies).
	CreateSnapshot(projectID string, doc json.RawMessage) *Snapshot
	GetSnapshot(id string) *Snapshot

	// Tickets (GitHub-Issues-like, §7).
	CreateTicket(authorID, title, body, category string) *Ticket
	GetTicket(id string) *Ticket // hydrated with comments + artifacts
	ListTickets(f TicketFilter) []*Ticket
	UpdateTicket(id, title, body string) *Ticket // author edit of title/body
	SetTicketState(id string, to TicketState) (*Ticket, error)
	SetTicketCategory(id, category string) *Ticket // PO verdict classification (§8.3)
	SetPOVerdict(id string, v POVerdict) (*Ticket, error)
	DeleteTicket(id string) bool

	// Ticket comments.
	AddComment(ticketID string, authorType AuthorType, authorID *string, body string, internal bool) *TicketComment
	ListComments(ticketID string) []*TicketComment

	// Comment moderation (B1, §4.2/§8): the PO post-moderates user comments.
	GetComment(ticketID, commentID string) *TicketComment // nil if not on that ticket
	SetCommentReviewed(commentID string, withheld bool) *TicketComment
	ListTicketsWithUnreviewedComments() []*Ticket // hydrated; any state

	// Ticket artifacts (content-addressed repro blobs).
	AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact
	GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte)

	// Work-orders (PO→Coding handoff, §9). One per ticket.
	CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error)
	GetWorkOrder(ticketID string) *WorkOrder
	ClaimWorkOrder(ticketID string) (*WorkOrder, error)              // sets picked_up_at
	CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) // sets completed_at + deploy link

	// Deploys (rollout audit / rollback, §10.3).
	CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error)
	ListDeploys() []*Deploy

	// Agent-authored comments: leak-filtered `po` comment (withheld iff it
	// trips the filter §4.2) or an internal `dev` note (admin-only).
	AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment

	// Queue helper: tickets in `accepted` that still have an open work-order
	// (for the coding agent's queue, §10.1).
	ListAcceptedWithOpenWorkOrder() []*Ticket

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
	errTicketMissing = &httpError{http.StatusNotFound, "ticket not found"}

	errWorkOrderExists  = &httpError{http.StatusConflict, "work-order already exists"}
	errWorkOrderMissing = &httpError{http.StatusNotFound, "work-order not found"}
	errWorkOrderClaimed = &httpError{http.StatusConflict, "work-order already claimed"}
	errDeployMissing    = &httpError{http.StatusNotFound, "deploy not found"}
)

type httpError struct {
	Code    int
	Message string
}

func (e *httpError) Error() string { return e.Message }
