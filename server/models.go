// Shared domain models for the FieldDraft backend.
//
// These are storage-agnostic: both the in-memory store (test fixture) and
// the MariaDB-backed store populate the same structs, and the HTTP handlers
// only ever see these — so API response shapes are identical regardless of
// which store backs the server.

package main

import (
	"encoding/json"
	"time"
)

// Role is a user's authorization level.
type Role string

const (
	RoleUser  Role = "user"
	RoleAdmin Role = "admin"
)

type User struct {
	ID           string    `json:"id"`
	Email        string    `json:"email"`
	DisplayName  string    `json:"display_name"`
	Role         Role      `json:"role"`
	Disabled     bool      `json:"disabled"` // banned: no login, sessions invalid (§4.4)
	PasswordHash []byte    `json:"-"`
	Salt         []byte    `json:"-"`
	CreatedAt    time.Time `json:"created_at"`
}

type Project struct {
	ID         string          `json:"id"`
	OwnerID    string          `json:"owner_id"`
	Name       string          `json:"name"`
	Visibility string          `json:"visibility"` // "private" | "public"
	Document   json.RawMessage `json:"document"`
	UpdatedAt  time.Time       `json:"updated_at"`
	CreatedAt  time.Time       `json:"created_at"`
}

type Session struct {
	Token   string
	UserID  string
	Expires time.Time
}

// ResetToken is a single-use password-reset token (1 h validity).
type ResetToken struct {
	Token   string
	UserID  string
	Expires time.Time
}

// Snapshot is a published, shareable copy of a project document.
type Snapshot struct {
	ID        string          `json:"id"`
	ProjectID string          `json:"project_id"`
	Document  json.RawMessage `json:"document"`
	CreatedAt time.Time       `json:"created_at"`
}

// Invite is a single-use, admin-issued registration token. It carries the
// intended recipient (name + email) and an optional invitation message.
type Invite struct {
	Token        string     `json:"token"`
	CreatedBy    string     `json:"created_by"`
	InviteeName  string     `json:"invitee_name"`
	InviteeEmail string     `json:"invitee_email"`
	Message      string     `json:"message"`
	ExpiresAt    time.Time  `json:"expires_at"`
	UsedBy       *string    `json:"used_by"` // nil while open
	UsedAt       *time.Time `json:"used_at"` // nil while open
	CreatedAt    time.Time  `json:"created_at"`
}

// Status reports an invite's lifecycle bucket for the admin list.
func (i *Invite) Status(now time.Time) string {
	switch {
	case i.UsedBy != nil:
		return "used"
	case now.After(i.ExpiresAt):
		return "expired"
	default:
		return "open"
	}
}

// ─── tickets ────────────────────────────────────────────────────

// TicketState is a ticket's position in the §7.2 state machine.
type TicketState string

const (
	StateTriage            TicketState = "triage" // initial
	StateNeedsInfo         TicketState = "needs_info"
	StateAccepted          TicketState = "accepted"
	StateInProgress        TicketState = "in_progress"
	StateDeploying         TicketState = "deploying"
	StateResolved          TicketState = "resolved"
	StateClosed            TicketState = "closed"
	StateDeclined          TicketState = "declined"
	StateRejectedSpam      TicketState = "rejected_spam"
	StateRejectedMalicious TicketState = "rejected_malicious"
)

// AuthorType is who wrote a ticket comment (§4 security context).
type AuthorType string

const (
	AuthorUser   AuthorType = "user"
	AuthorPO     AuthorType = "po"
	AuthorDev    AuthorType = "dev"
	AuthorSystem AuthorType = "system"
)

// ArtifactKind classifies a ticket's repro blob.
type ArtifactKind string

const (
	ArtifactScreenshot ArtifactKind = "screenshot"
	ArtifactFdraft     ArtifactKind = "fdraft"
	ArtifactActions    ArtifactKind = "actions"
	ArtifactSettings   ArtifactKind = "settings"
	ArtifactUIState    ArtifactKind = "ui_state"
	ArtifactAttachment ArtifactKind = "attachment"
)

// Ticket is a user-authored issue (bug / feature / question). Repro artifacts
// hang off it as TicketArtifact; the discussion is a TicketComment thread.
type Ticket struct {
	ID          string      `json:"id"`
	AuthorID    string      `json:"author_id"`
	Title       string      `json:"title"`
	Body        string      `json:"body"`
	State       TicketState `json:"state"`
	Category    *string     `json:"category"` // bug|feature|question|other, or nil
	POSpam      bool        `json:"po_spam"`
	POMalicious bool        `json:"po_malicious"`
	POVisionFit *string     `json:"po_vision_fit"` // fit|partial|off, or nil
	POReason    *string     `json:"po_reason"`
	POVerdictAt *time.Time  `json:"po_verdict_at"`
	CreatedAt   time.Time   `json:"created_at"`
	UpdatedAt   time.Time   `json:"updated_at"`

	// Hydrated on GetTicket; nil on list rows.
	Comments  []*TicketComment  `json:"comments,omitempty"`
	Artifacts []*TicketArtifact `json:"artifacts,omitempty"`
}

// TicketComment is one entry in a ticket's discussion thread.
type TicketComment struct {
	ID         string     `json:"id"`
	TicketID   string     `json:"ticket_id"`
	AuthorType AuthorType `json:"author_type"`
	AuthorID   *string    `json:"author_id"` // nil for po/dev/system
	Internal   bool       `json:"internal"`  // dev notes: admin only, never user
	Body       string     `json:"body"`
	CreatedAt  time.Time  `json:"created_at"`
	Withheld   bool       `json:"withheld"`    // leak-filter hold (§4.2)
	POReviewed bool       `json:"po_reviewed"` // PO post-moderation done (§4.2/§8)
}

// TicketArtifact is a content-addressed repro blob attached to a ticket.
type TicketArtifact struct {
	ID        string       `json:"id"`
	TicketID  string       `json:"ticket_id"`
	Kind      ArtifactKind `json:"kind"`
	BlobRef   string       `json:"blob_ref"`
	Mime      string       `json:"mime"`
	Size      int64        `json:"size"`
	Name      string       `json:"name"`
	CreatedAt time.Time    `json:"created_at"`
}

// TicketFilter narrows a ticket listing (§7.3).
type TicketFilter struct {
	AuthorID string // exact author; "" = any
	State    TicketState
	Category string
	Query    string // free-text over title/body
	// IncludeRejected lets admins see rejected_spam/malicious in the list;
	// for non-admins the store still surfaces the caller's OWN rejected tickets
	// (handled by the visibility filter in the store).
	ViewerID      string
	ViewerIsAdmin bool
}

// POVerdict carries the PO's classification + reason for a ticket.
type POVerdict struct {
	Spam      bool
	Malicious bool
	VisionFit *string // fit|partial|off, or nil
	Reason    string
	State     TicketState // resulting state
}

// ─── agent / work-order layer (§9, §10) ─────────────────────────

// WorkOrder is the PO→Coding handoff for an accepted ticket (§9). One per
// ticket (TicketID is the primary key). Payload is the verbatim §9 Work-Order
// JSON (PO task + directly-forwarded untrusted ticket data); the server keeps
// it opaque. The lifecycle timestamps drive the coding agent's queue.
type WorkOrder struct {
	TicketID    string          `json:"ticket_id"`
	Payload     json.RawMessage `json:"payload"`
	CreatedAt   time.Time       `json:"created_at"`
	PickedUpAt  *time.Time      `json:"picked_up_at"` // nil while unclaimed
	CompletedAt *time.Time      `json:"completed_at"` // nil while in progress
	DeployID    *string         `json:"deploy_id"`    // nil until a deploy resolves it
}

// Open reports whether the work-order is still actionable for the coding agent:
// it exists and has not yet been completed (claimed-but-incomplete counts).
func (wo *WorkOrder) Open() bool {
	return wo != nil && wo.CompletedAt == nil
}

// DeployStatus is a deploy record's lifecycle bucket.
type DeployStatus string

const (
	DeployBuilding   DeployStatus = "building"
	DeployLive       DeployStatus = "live"
	DeployRolledBack DeployStatus = "rolled_back"
)

// validDeployStatus reports whether s is an allowed deploy status.
func validDeployStatus(s string) bool {
	switch DeployStatus(s) {
	case DeployBuilding, DeployLive, DeployRolledBack:
		return true
	}
	return false
}

// Deploy is a recorded rollout (§10.3) — a getaggtes Image plus the tickets it
// shipped. The audit trail for reversibility/rollback.
type Deploy struct {
	ID         string       `json:"id"`
	Version    string       `json:"version"`
	GitSHA     string       `json:"git_sha"`
	TicketIDs  []string     `json:"ticket_ids"`
	Status     DeployStatus `json:"status"`
	DeployedAt time.Time    `json:"deployed_at"`
}

// ─── admin alerts (§4.4 admin-awareness) ────────────────────────

// AlertKind classifies an admin alert.
type AlertKind string

const (
	AlertMaliciousTicket  AlertKind = "malicious_ticket"  // a verdict flagged a ticket malicious
	AlertMaliciousComment AlertKind = "malicious_comment" // a comment moderated spam/malicious
	AlertWithheldComment  AlertKind = "withheld_comment"  // leak filter held an outgoing po comment
	AlertUserBanned       AlertKind = "user_banned"       // an admin banned a user
)

// AdminAlert is a real admin-notify record (§4.4) — the durable side of the
// "Push/Notify bei malicious-Flag und zurückgehaltenen Kommentaren" net.
type AdminAlert struct {
	ID        string    `json:"id"`
	Kind      AlertKind `json:"kind"`
	TicketID  *string   `json:"ticket_id"` // nil when not ticket-scoped (e.g. a ban)
	Detail    string    `json:"detail"`
	CreatedAt time.Time `json:"created_at"`
	Seen      bool      `json:"seen"`
}
