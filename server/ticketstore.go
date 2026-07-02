// The TicketStore interface — the pluggable ticket domain (WP15).
//
// The ticket / comment / artifact / work-order / verdict / agent-queue methods
// were factored off Store into their own interface so the ticket domain can be
// backed independently of users/projects/sessions. Two impls satisfy it:
//
//   - the existing Store (memStore + sqlStore) — its ticket methods ARE the DB
//     backend. Selecting TICKET_BACKEND=db (the default, and the only prod path)
//     wires s.tickets = s.store, so the behavior is byte-identical to before
//     this split.
//   - fileTicketStore (ticketstore_file.go) — a file-backed store under a base
//     dir, for local dev (TICKET_BACKEND=files). The materialized layout doubles
//     as the on-disk tickets the desktop agents harvest (§3.1).
//
// The ticket + agent HTTP handlers call s.tickets.X for these methods; only
// users/projects/sessions/invites/admin_alerts stay on s.store.

package main

import (
	"encoding/json"
)

// TicketStore is the pluggable ticket domain (§7–§11). Both memStore/sqlStore
// (DB backend) and fileTicketStore (local file backend) implement it.
type TicketStore interface {
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
	SetCommentReviewed(ticketID, commentID string, withheld bool) *TicketComment
	ListTicketsWithUnreviewedComments() []*Ticket // hydrated; any state

	// Agent-authored comments: leak-filtered `po` comment (withheld iff it
	// trips the filter §4.2) or an internal `dev` note (admin-only).
	AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment

	// Ticket artifacts (content-addressed repro blobs).
	AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact
	GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte)

	// Work-orders (PO→Coding handoff, §9). One per ticket.
	CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error)
	GetWorkOrder(ticketID string) *WorkOrder
	ClaimWorkOrder(ticketID string) (*WorkOrder, error)              // sets picked_up_at
	CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) // sets completed_at + deploy link

	// Queue helper: tickets in `accepted` that still have an open work-order
	// (for the coding agent's queue, §10.1).
	ListAcceptedWithOpenWorkOrder() []*Ticket

	// Deploys (rollout audit / rollback, §10.3).
	CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error)
	ListDeploys() []*Deploy
}

// Compile-time assertions: both DB-backed stores satisfy TicketStore, so
// s.tickets = s.store is legal for the default (db) backend.
var (
	_ TicketStore = (*memStore)(nil)
	_ TicketStore = (*sqlStore)(nil)
)
