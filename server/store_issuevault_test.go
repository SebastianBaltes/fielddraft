package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

// TestIssuevaultListTicketsResolvedProjectWide verifies the §-scope split:
// RESOLVED tickets are read project-wide (all installations), every other state
// (and the caller's own resolved) stays scope:"own". A fake IssueVault records
// the search predicates and returns canned tickets by scope.
func TestIssuevaultListTicketsResolvedProjectWide(t *testing.T) {
	type call struct {
		scope     string
		hasStatus bool
		status    string
	}
	var calls []call

	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/tickets/search" {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		var pred map[string]any
		_ = json.NewDecoder(r.Body).Decode(&pred)
		c := call{}
		c.scope, _ = pred["scope"].(string)
		if st, ok := pred["status"].([]any); ok && len(st) > 0 {
			c.hasStatus = true
			c.status, _ = st[0].(string)
		}
		calls = append(calls, c)

		// Canned tickets per query.
		var ivs []ivTicket
		switch {
		case c.scope == "own" && !c.hasStatus:
			ivs = []ivTicket{{ID: "own-triage", Status: "triage"}, {ID: "own-res", Status: "resolved"}}
		case c.scope == "own" && c.status == "triage":
			ivs = []ivTicket{{ID: "own-triage", Status: "triage"}}
		case c.scope == "project" && c.status == "resolved":
			// own-res overlaps with the own query above → must dedup by id.
			ivs = []ivTicket{{ID: "own-res", Status: "resolved"}, {ID: "peer-res", Status: "resolved"}}
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"tickets": ivs})
	}))
	defer srv.Close()

	s := &issuevaultStore{baseURL: srv.URL, token: "t", http: srv.Client()}

	ids := func(ts []*Ticket) map[string]bool {
		m := map[string]bool{}
		for _, t := range ts {
			m[t.ID] = true
		}
		return m
	}

	// (1) All states → own (every state) + project-wide resolved, deduped.
	calls = nil
	got := ids(s.ListTickets(TicketFilter{}))
	if len(got) != 3 || !got["own-triage"] || !got["own-res"] || !got["peer-res"] {
		t.Fatalf("all-states set = %v, want {own-triage, own-res, peer-res}", got)
	}
	if len(calls) != 2 || calls[0].scope != "own" || calls[0].hasStatus ||
		calls[1].scope != "project" || calls[1].status != "resolved" {
		t.Fatalf("all-states calls = %+v, want [own(no status), project(resolved)]", calls)
	}

	// (2) Resolved filter → ONLY the project-wide resolved query.
	calls = nil
	got = ids(s.ListTickets(TicketFilter{State: StateResolved}))
	if len(got) != 2 || !got["own-res"] || !got["peer-res"] {
		t.Fatalf("resolved set = %v, want {own-res, peer-res}", got)
	}
	if len(calls) != 1 || calls[0].scope != "project" || calls[0].status != "resolved" {
		t.Fatalf("resolved calls = %+v, want [project(resolved)]", calls)
	}

	// (3) A non-resolved state filter → own installation only.
	calls = nil
	got = ids(s.ListTickets(TicketFilter{State: StateTriage}))
	if len(got) != 1 || !got["own-triage"] {
		t.Fatalf("triage set = %v, want {own-triage}", got)
	}
	if len(calls) != 1 || calls[0].scope != "own" || calls[0].status != "triage" {
		t.Fatalf("triage calls = %+v, want [own(triage)]", calls)
	}
}
