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

// TestIssuevaultUnknownStatusIsDeadNotOpen pins the trap that mis-imported 60
// finished tickets: IssueVault does not whitelist `status`, so a bad writer can
// leave a ticket in a state FieldDraft has never heard of. It then looks like a
// perfectly normal open ticket in every listing — but NO transition out of it is
// legal, so it can never be resolved. The value must survive verbatim (silently
// healing it would hide the corruption) and be recognisable as invalid.
func TestIssuevaultUnknownStatusIsDeadNotOpen(t *testing.T) {
	tk := mapTicket(&ivTicket{ID: "20260609-190836-sff3", Status: "open"})

	if tk.State != TicketState("open") {
		t.Fatalf("state = %q, want the raw %q (no silent healing)", tk.State, "open")
	}
	if validState(tk.State) {
		t.Fatal(`"open" must NOT be a valid TicketState — it is not in the state machine`)
	}
	// The dead end itself: the state a finished ticket belongs in is unreachable.
	if err := validateTransition(tk.State, StateResolved); err == nil {
		t.Fatal("open → resolved must be rejected; repair such tickets at the source")
	}
}

// TestIssuevaultLenientMetaBools — a comment whose meta carries SQL-flavoured
// 0/1 instead of booleans must still decode. IssueVault does not validate `meta`,
// so any writer can produce them; with a strict bool the ticket's Unmarshal fails
// and the WHOLE ticket 404s in the app (that is exactly what happened to #344 and
// #345). Reading is lenient, writing stays strict.
func TestIssuevaultLenientMetaBools(t *testing.T) {
	raw := `{
	  "id": "t1", "number": 7, "title": "T", "body": "B", "status": "triage",
	  "created_at": "2026-07-13T18:00:00Z", "updated_at": "2026-07-13T18:00:00Z",
	  "meta": {"po_spam": 0, "po_malicious": 1},
	  "comments": [
	    {"id": "c1", "author_kind": "agent", "body": "note",
	     "created_at": "2026-07-13T18:01:00Z",
	     "meta": {"author_type": "dev", "internal": 0, "po_reviewed": 1}},
	    {"id": "c2", "author_kind": "user", "body": "hi",
	     "created_at": "2026-07-13T18:02:00Z",
	     "meta": {"author_type": "user", "internal": true, "withheld": "true"}}
	  ]
	}`
	var iv ivTicket
	if err := json.Unmarshal([]byte(raw), &iv); err != nil {
		t.Fatalf("lenient decode: %v", err)
	}
	tk := mapTicketFull(&iv)
	if tk.POSpam || !tk.POMalicious {
		t.Fatalf("ticket meta: po_spam=%v po_malicious=%v, want false/true", tk.POSpam, tk.POMalicious)
	}
	if len(tk.Comments) != 2 {
		t.Fatalf("comments: got %d, want 2", len(tk.Comments))
	}
	if tk.Comments[0].Internal || !tk.Comments[0].POReviewed {
		t.Fatalf("c1: internal=%v po_reviewed=%v, want false/true",
			tk.Comments[0].Internal, tk.Comments[0].POReviewed)
	}
	if !tk.Comments[1].Internal || !tk.Comments[1].Withheld {
		t.Fatalf("c2: internal=%v withheld=%v, want true/true",
			tk.Comments[1].Internal, tk.Comments[1].Withheld)
	}
}
