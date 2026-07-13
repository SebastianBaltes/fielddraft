// Ticket state machine (§7.2).
//
// One authoritative map of allowed transitions. The store validates every
// state change against it and rejects illegal moves with a 4xx; each accepted
// transition appends a `system` audit comment.
//
// Beyond the PO/dev flow drawn below, the AUTHOR may drive a safe subset
// (ticket 066230d9, see userMaySetState): close their own ticket, park it to
// `backlog` (recorded, do-not-work), un-park it, and reopen it.
//
//	                 ┌──────────── rejected_spam       (PO: spam → hidden)
//	  (user creates) │
//	      │          ├──────────── rejected_malicious  (PO: malicious → hidden + notify)
//	      ▼          │
//	  ┌────────┐ PO  │
//	  │ triage │─────┼──────────── declined            (PO: off-vision; reason in thread)
//	  └────────┘     │                ▲   │ user objects → triage
//	      ▲          │                │   ▼
//	      │          ▼                │
//	  ┌──────────┐ ┌──────────┐       │
//	  │needs_info│◀│ accepted │───────┘   (PO: accepted, work-order created)
//	  └──────────┘ └────┬─────┘
//	                    ▼
//	             ┌─────────────┐ dev needs clarification
//	             │ in_progress │──────────────▶ needs_info (via PO)
//	             └──────┬──────┘
//	                    ▼
//	             ┌───────────┐
//	             │ deploying │   (built, tests green, rollout)
//	             └─────┬─────┘
//	                   ▼
//	             ┌──────────┐ confirmed / duplicate
//	             │ resolved │──────────────▶ closed
//	             └──────────┘
//	                   ▲ regression / reopen
//	                   └──────────── in_progress

package main

import "net/http"

// ticketTransitions is the set of legal state moves. A move is allowed iff the
// target appears in the source's slice.
var ticketTransitions = map[TicketState][]TicketState{
	StateTriage: {
		StateAccepted, StateDeclined, StateNeedsInfo,
		StateBacklog, StateClosed, // user: park / close (ticket 066230d9)
		StateRejectedSpam, StateRejectedMalicious,
	},
	// Dialogic: PO re-triages after the user replies; PO may also reject
	// info-requests that turn out to be spam/malicious.
	StateNeedsInfo: {
		StateTriage, StateAccepted, StateDeclined,
		StateBacklog, StateClosed, // user: park / close
		StateRejectedSpam, StateRejectedMalicious,
	},
	// User can object to a decline → PO reconsiders (back to triage), or accept
	// it and close the ticket.
	StateDeclined: {StateTriage, StateClosed},
	// Parked: the user can un-park it (→ triage, request work) or drop it.
	StateBacklog: {StateTriage, StateClosed},
	StateAccepted: {
		StateInProgress, StateNeedsInfo, StateDeclined,
	},
	StateInProgress: {
		StateDeploying, StateNeedsInfo,
	},
	StateDeploying: {
		StateResolved, StateInProgress,
	},
	StateResolved: {
		StateClosed, StateInProgress, // close, or reopen on regression
	},
	// Reopen a closed ticket → re-triage (ticket 066230d9).
	StateClosed: {StateTriage},
	// Terminal states.
	StateRejectedSpam:      {},
	StateRejectedMalicious: {},
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
		// Close from a pre-work or resolved state (not mid-flow, not terminal).
		switch from {
		case StateTriage, StateNeedsInfo, StateDeclined, StateBacklog, StateResolved:
			return true
		}
	case StateBacklog:
		// Park a not-yet-worked ticket.
		return from == StateTriage || from == StateNeedsInfo
	case StateTriage:
		// Un-park (backlog→triage) or reopen a closed ticket (closed→triage).
		return from == StateBacklog || from == StateClosed
	case StateInProgress:
		// Reopen a resolved ticket — "the fix didn't stick" (resolved→in_progress).
		return from == StateResolved
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
	return s == StateRejectedSpam || s == StateRejectedMalicious
}
