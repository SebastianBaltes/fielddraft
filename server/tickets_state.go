// Ticket state machine (§7.2).
//
// One authoritative map of allowed transitions. The store validates every
// state change against it and rejects illegal moves with a 4xx; each accepted
// transition appends a `system` audit comment.
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
		StateRejectedSpam, StateRejectedMalicious,
	},
	// Dialogic: PO re-triages after the user replies; PO may also reject
	// info-requests that turn out to be spam/malicious.
	StateNeedsInfo: {
		StateTriage, StateAccepted, StateDeclined,
		StateRejectedSpam, StateRejectedMalicious,
	},
	// User can object to a decline → PO reconsiders (back to triage).
	StateDeclined: {StateTriage},
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
		StateClosed, StateInProgress, // reopen on regression
	},
	// Terminal states.
	StateClosed:            {},
	StateRejectedSpam:      {},
	StateRejectedMalicious: {},
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
	if !validState(to) {
		return errUnknownState
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
