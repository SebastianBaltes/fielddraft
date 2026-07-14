// The TicketStore interface — the ticket domain (WP15).
//
// The ticket / comment / artifact / work-order / verdict / agent-queue methods
// were factored off Store into their own interface so the ticket domain can be
// backed independently of users/projects/sessions.
//
// **IssueVault is the ONLY backend** (ticket #346): there is no TICKET_BACKEND
// switch, no SQL ticket path (migrations 0008/0009 dropped those tables) and no
// file-backed store — `fileTicketStore` is gone. Two impls satisfy the interface
// today:
//
//   - issuevaultStore (store_issuevault.go) — production, and the only path a
//     running server ever takes.
//   - memStore (ticketstore_mem.go) — in-memory, for TESTS only. It exists so the
//     contract tests can run without a network; nothing wires it in production.
//
// The ticket + agent HTTP handlers call s.tickets.X for these methods; only
// users/projects/sessions/invites/admin_alerts stay on s.store.

package main

import (
	"encoding/json"
	"net/http"
)

// The ticket-domain errors every TicketStore implementation returns (mapped to
// HTTP responses by writeStoreError).
var (
	errTicketMissing = &httpError{http.StatusNotFound, "ticket not found"}

	errWorkOrderExists    = &httpError{http.StatusConflict, "work-order already exists"}
	errWorkOrderMissing   = &httpError{http.StatusNotFound, "work-order not found"}
	errWorkOrderClaimed   = &httpError{http.StatusConflict, "work-order already claimed"}
	errWorkOrderCompleted = &httpError{http.StatusConflict, "work-order already completed"}
	errDeployMissing      = &httpError{http.StatusNotFound, "deploy not found"}
)

// TicketStore is the ticket domain (§7–§11). Implemented by issuevaultStore
// (production) and memStore (tests) — see the file header.
type TicketStore interface {
	// Tickets (GitHub-Issues-like, §7).
	CreateTicket(authorID, title, body, category string) *Ticket
	GetTicket(id string) *Ticket // hydrated with comments + artifacts
	ListTickets(f TicketFilter) []*Ticket
	UpdateTicket(id, title, body string) *Ticket // author edit of title/body
	// SetTicketState moves the ticket. `direct` is the admin override: it skips
	// the §7.2 transition graph (any known state from any state, ticket #345).
	SetTicketState(id string, to TicketState, direct bool) (*Ticket, error)
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

// Compile-time assertion for the in-memory test double (ticketstore_mem.go).
// The one PRODUCTION implementation is issuevaultStore (store_issuevault.go).
var _ TicketStore = (*memStore)(nil)
