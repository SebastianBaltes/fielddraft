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

// TicketState is a ticket's position in the state machine — the SAME six-value
// enum IssueVault 0.4.x enforces (there is no separate open/closed field and no
// free-form status anymore). FieldDraft used to carry eleven states in
// IssueVault's opaque `status` string; the store now reduced to one validated
// `state`, so this collapsed to match (§7.2).
//
// The three merges and where their old distinctions went:
//   - accepted/deploying → in_progress. "Accepted" is no longer a state: the
//     truth for the PO→coding hand-off is the WORK-ORDER's existence (the agent
//     queue already keys on that, not on the state).
//   - resolved → closed. "Fixed, deploy-verification pending" vs. "done" is now
//     carried by the fix `version` stamped on close, not by a separate state.
//   - declined/rejected_spam/rejected_malicious → rejected. The reason lives in
//     the PO verdict + a comment, not in the state.
type TicketState string

const (
	StateTriage     TicketState = "triage" // initial
	StateNeedsInfo  TicketState = "needs_info"
	StateInProgress TicketState = "in_progress"
	// StateParked: team-parked (external dependency / "later"). Non-terminal, but
	// UNLIKE needs_info a user comment does NOT pull it back into a queue — the
	// active queues exclude it and un-parking is an explicit state change. This is
	// the one old distinction (backlog vs. needs_info) that survives the collapse,
	// as a first-class state (IssueVault 0.4.2).
	StateParked TicketState = "parked"
	// StateClosed / StateRejected are terminal. IssueVault sets closed_at itself
	// on entry and clears it on leave — this adapter never writes closed_at.
	StateClosed   TicketState = "closed"
	StateRejected TicketState = "rejected"
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
	ID string `json:"id"`
	// Number is the short, human/agent-friendly reference shown as "#N". The
	// opaque `id` stays the storage/routing key; number is assigned per-DB
	// (auto-increment) or, in prod, by IssueVault's per-project counter.
	Number      int         `json:"number"`
	AuthorID    string      `json:"author_id"`
	Title       string      `json:"title"`
	Body        string      `json:"body"`
	State       TicketState `json:"state"`
	// Version is the fix marker stamped when the ticket was closed (empty while
	// open, and empty on tickets closed before this existed). Opaque — set by the
	// closer to the release the fix shipped in (IssueVault 0.4.1).
	Version     string      `json:"version"`
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
	AuthorID   *string    `json:"author_id"` // nil for po/dev/automated-system
	// AuthorName is the human author's display name, resolved from AuthorID at
	// serialization (#359). Present for user comments and for state-change audits
	// a human triggered — so the thread shows "Sebastian" instead of "SYS". Empty
	// for automated/system events (no human actor). Never persisted.
	AuthorName string    `json:"author_name,omitempty"`
	Internal   bool      `json:"internal"` // dev notes: admin only, never user
	Body       string    `json:"body"`
	CreatedAt  time.Time `json:"created_at"`
	Withheld   bool      `json:"withheld"`    // leak-filter hold (§4.2)
	POReviewed bool      `json:"po_reviewed"` // PO post-moderation done (§4.2/§8)
	// AgentModel/AgentEffort name the subagent that actually did the analysis or
	// implementation behind an agent comment (#387) — deliberately NOT the
	// orchestrating main agent when that one only summarizes someone else's work.
	// Carried verbatim from the comment's IssueVault `meta`; empty when unstamped
	// (every comment predating this), in which case the UI shows nothing.
	AgentModel  string `json:"agent_model,omitempty"`
	AgentEffort string `json:"agent_effort,omitempty"`
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

// ─── mesh jobs (server-side CGAL meshing, plan §P4) ─────────────

// MeshJobStatus is a mesh job's lifecycle state.
type MeshJobStatus string

const (
	MeshQueued  MeshJobStatus = "queued"
	MeshRunning MeshJobStatus = "running"
	MeshDone    MeshJobStatus = "done"
	MeshError   MeshJobStatus = "error"
)

// MeshJob is one server-side meshing order: a part of a saved project is
// prepared (fd-prepare) and meshed (fd-cgal-mesh) by the mesher service; the
// resulting STL lands as a blob. Jobs survive server restarts.
type MeshJob struct {
	ID         string        `json:"id"`
	OwnerID    string        `json:"owner_id"`
	ProjectID  string        `json:"project_id"`
	PartID     string        `json:"part_id"`
	ParamsJSON string        `json:"params_json"`
	Status     MeshJobStatus `json:"status"`
	BlobRef    string        `json:"-"`           // content-addressed STL blob (done only)
	ReportJSON string        `json:"report_json"` // mesher report (done only)
	Error      string        `json:"error"`       // failure detail (error only)
	CreatedAt  time.Time     `json:"created_at"`
	StartedAt  *time.Time    `json:"started_at"`
	FinishedAt *time.Time    `json:"finished_at"`
}
