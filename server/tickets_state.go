// Ticket state machine (§7.2).
//
// One authoritative map of allowed transitions. The store validates every
// state change against it and rejects illegal moves with a 4xx; each accepted
// transition appends a `system` audit comment.
//
// Six states, matching IssueVault 0.4.x (the collapse from eleven — see
// TicketState in models.go for what merged where). Beyond the flow below, the
// AUTHOR may drive a safe subset (ticket 066230d9, see userMaySetState): close
// their own ticket, park it, un-park it, and reopen it.
//
//	  (user creates)
//	      │  PO: off-vision / spam / malicious
//	      ▼          ┌────────────▶ rejected ───▶ triage   (user objects)
//	  ┌────────┐─────┘              (reason in the PO verdict + thread)
//	  │ triage │
//	  └───┬────┘  PO accepts (work-order created)
//	      ▼
//	  ┌─────────────┐ dev needs clarification
//	  │ in_progress │◀──────────────▶ needs_info   (user reply revives it)
//	  └──────┬──────┘
//	         ▼  finished (fix `version` stamped)
//	  ┌──────────┐ reopen — "didn't stick"
//	  │  closed  │──────────────────▶ in_progress
//	  └──────────┘
//
//	  parked ◀── from triage / needs_info / in_progress   (team-parked; a user
//	     └──────▶ triage / in_progress / closed            comment does NOT revive)

package main

import "net/http"

// ticketTransitions is the set of legal state moves. A move is allowed iff the
// target appears in the source's slice.
var ticketTransitions = map[TicketState][]TicketState{
	StateTriage: {
		// accept → in_progress (with a work-order); ask; park; close; reject.
		StateInProgress, StateNeedsInfo, StateParked, StateClosed, StateRejected,
	},
	// Dialogic: PO re-triages / accepts after the user replies; may also reject.
	StateNeedsInfo: {
		StateTriage, StateInProgress, StateParked, StateClosed, StateRejected,
	},
	StateInProgress: {
		// back to the user; park mid-flow; finish (with a version); reject.
		StateNeedsInfo, StateParked, StateClosed, StateRejected,
	},
	// Parked: un-park back into the flow, or close outright.
	StateParked: {StateTriage, StateInProgress, StateClosed},
	// Reopen a closed ticket → re-triage, or straight back to work ("didn't stick").
	StateClosed: {StateTriage, StateInProgress},
	// A rejected ticket can be reopened on objection (inherits declined's escape
	// hatch); spam/malicious simply never come back, which is fine.
	StateRejected: {StateTriage},
}

// userMaySetState reports whether a non-admin ticket AUTHOR may drive from→to.
// Authors get a safe subset of the machine (§7.2, ticket 066230d9): close their
// own ticket, park it to `backlog`, un-park it, and reopen it. Everything else
// (accept, decline, deploy, reject, …) stays PO/dev/admin-driven. Every move
// permitted here is also a legal machine edge above, so validateTransition still
// gates the actual store write.
func userMaySetState(from, to TicketState) bool {
	switch to {
	case StateClosed:
		// Close a ticket the user owns — from anywhere that isn't already terminal.
		switch from {
		case StateTriage, StateNeedsInfo, StateInProgress, StateParked:
			return true
		}
	case StateParked:
		// Park a not-yet-finished ticket.
		return from == StateTriage || from == StateNeedsInfo
	case StateTriage:
		// Un-park (parked→triage) or reopen a closed ticket (closed→triage).
		return from == StateParked || from == StateClosed
	case StateInProgress:
		// Reopen a closed ticket — "the fix didn't stick" (closed→in_progress).
		return from == StateClosed
	}
	return false
}

// errInvalidTransition is returned for a disallowed state move.
var errInvalidTransition = &httpError{http.StatusBadRequest, "illegal state transition"}

// errUnknownState is returned for a state value outside the machine.
var errUnknownState = &httpError{http.StatusBadRequest, "unknown ticket state"}

// validState reports whether s is a known machine state.
func validState(s TicketState) bool {
	_, ok := ticketTransitions[s]
	return ok
}

// canTransition reports whether from→to is a legal move. A no-op (from==to) is
// rejected so callers don't emit a meaningless audit comment.
func canTransition(from, to TicketState) bool {
	if from == to {
		return false
	}
	for _, t := range ticketTransitions[from] {
		if t == to {
			return true
		}
	}
	return false
}

// validateTransition returns the matching *httpError for an illegal move, or nil.
func validateTransition(from, to TicketState) error {
	return validateStateChange(from, to, false)
}

// validateStateChange is validateTransition plus the admin override (ticket
// #345): with `direct`, only the TARGET is checked — any known state may be set
// from any state, the transition graph is skipped. A no-op (from == to) stays an
// error either way, so no meaningless audit comment is written.
func validateStateChange(from, to TicketState, direct bool) error {
	if !validState(to) {
		return errUnknownState
	}
	if direct {
		if from == to {
			return errInvalidTransition
		}
		return nil
	}
	if !canTransition(from, to) {
		return errInvalidTransition
	}
	return nil
}

// validCategory reports whether c is an allowed ticket category (empty = unset).
func validCategory(c string) bool {
	switch c {
	case "", "bug", "feature", "question", "other":
		return true
	}
	return false
}

// validVisionFit reports whether v is an allowed po_vision_fit value.
func validVisionFit(v string) bool {
	switch v {
	case "", "fit", "partial", "off":
		return true
	}
	return false
}

// isRejected reports whether a state hides a ticket from the public list (§7.3).
func isRejected(s TicketState) bool {
	return s == StateRejected
}
