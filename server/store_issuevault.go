// issuevaultStore — a TicketStore backed by the external IssueVault service.
//
// The ONLY ticket backend (ticket #346) — there is no switch to select it and no
// fallback behind it. Instead of MariaDB, the whole ticket domain (tickets /
// comments / artifacts / work-orders / verdicts / deploys) is persisted in
// IssueVault over its REST API. The FieldDraft domain models are
// mapped onto IssueVault's generic ticket shape:
//
//   - FieldDraft TicketState  ⇄ IssueVault `status` (verbatim string); the
//     coarse open/closed `state` is DERIVED from it (derivedIVState).
//   - PO verdict fields + the one-per-ticket work-order live under ticket `meta`
//     (IssueVault applies RFC 7386 JSON-merge-patch to `meta`, so partial
//     updates merge in without clobbering siblings — the work-order lifecycle
//     patches rely on this).
//   - Comment provenance (author_type / internal / withheld / po_reviewed) lives
//     under comment `meta`; `author_type` is the source of truth, mirrored to a
//     coarse IssueVault `author_kind` (user→user, po/dev→agent, system→system).
//   - Artifacts map to IssueVault files (BlobRef ⇄ sha256).
//   - Deploys map to IssueVault project records of type "deploy".
//
// No user session is involved: the store authenticates with a single bearer
// token (ISSUEVAULT_TOKEN) scoped to the configured project/installation.

package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"strings"
	"time"
)

// issuevaultStore persists the ticket domain in IssueVault over REST.
type issuevaultStore struct {
	baseURL        string // e.g. https://…/api/v1 (no trailing slash)
	token          string // bearer token
	projectID      string // IssueVault project (for /projects/{id}/records)
	installationID string // IssueVault installation (stamped on created tickets)
	http           *http.Client
}

var _ TicketStore = (*issuevaultStore)(nil)

// newIssuevaultStore builds the store from the environment. ISSUEVAULT_URL and
// ISSUEVAULT_PROJECT carry defaults; ISSUEVAULT_TOKEN / ISSUEVAULT_INSTALLATION
// are read verbatim (an empty token simply makes every call fail 401 upstream).
func newIssuevaultStore() *issuevaultStore {
	base := os.Getenv("ISSUEVAULT_URL")
	if base == "" {
		base = "https://issuevault.203.0.113.10.sslip.io/api/v1"
	}
	project := os.Getenv("ISSUEVAULT_PROJECT")
	if project == "" {
		project = "744dc267-24dd-470d-8f06-2f56bd412349"
	}
	log.Printf("ticket store: issuevault backend at %s (project %s)", base, project)
	return &issuevaultStore{
		baseURL:        strings.TrimRight(base, "/"),
		token:          os.Getenv("ISSUEVAULT_TOKEN"),
		projectID:      project,
		installationID: os.Getenv("ISSUEVAULT_INSTALLATION"),
		http:           &http.Client{Timeout: 30 * time.Second},
	}
}

// ─── IssueVault wire types ──────────────────────────────────────

// ivBool is a bool that also accepts 0/1 and "true"/"false" on the wire.
//
// IssueVault does not validate the shape of `meta` — any writer (a peer
// installation, an import, an agent posting straight to the REST API) can put a
// SQL-flavoured 0/1 where a bool belongs. With a plain `bool` that one value
// fails the ticket's json.Unmarshal, GetTicket returns nil, and the whole ticket
// 404s: one sloppy comment hides an entire issue. It happened (ticket #345: a
// dev comment written with internal=0, po_reviewed=1 made #344 and #345
// unreachable in the app). Reading is therefore lenient; WRITING stays strict —
// every payload this store sends uses real booleans.
type ivBool bool

func (b *ivBool) UnmarshalJSON(data []byte) error {
	var v any
	if err := json.Unmarshal(data, &v); err != nil {
		return err
	}
	switch x := v.(type) {
	case bool:
		*b = ivBool(x)
	case float64:
		*b = x != 0
	case string:
		*b = x == "true" || x == "1"
	case nil:
		*b = false
	default:
		return fmt.Errorf("issuevault: cannot read %T as bool", v)
	}
	return nil
}

// ivTicket is IssueVault's ticket representation.
type ivTicket struct {
	ID             string            `json:"id"`
	InstallationID string            `json:"installation_id"`
	Number         int               `json:"number"`
	Title          string            `json:"title"`
	Body           string            `json:"body"`
	State          string            `json:"state"`  // "open" | "closed" (derived)
	Status         string            `json:"status"` // FieldDraft TicketState verbatim
	Category       *string           `json:"category"`
	Author         string            `json:"author"`
	Assignee       *string           `json:"assignee"`
	CreatedAt      time.Time         `json:"created_at"`
	UpdatedAt      time.Time         `json:"updated_at"`
	ClosedAt       *time.Time        `json:"closed_at"`
	Meta           ivTicketMeta      `json:"meta"`
	Comments       []ivComment       `json:"comments"`
	Files          []ivFile          `json:"files"`
	Commits        []json.RawMessage `json:"commits"`
}

// ivTicketMeta is the FieldDraft-specific payload carried under ticket.meta.
type ivTicketMeta struct {
	POSpam      ivBool       `json:"po_spam"`
	POMalicious ivBool       `json:"po_malicious"`
	POVisionFit *string      `json:"po_vision_fit"`
	POReason    *string      `json:"po_reason"`
	POVerdictAt *time.Time   `json:"po_verdict_at"`
	WorkOrder   *ivWorkOrder `json:"work_order"`
}

// ivWorkOrder is the one-per-ticket PO→coding handoff, under meta.work_order.
type ivWorkOrder struct {
	Payload     json.RawMessage `json:"payload"`
	CreatedAt   *time.Time      `json:"created_at"`
	PickedUpAt  *time.Time      `json:"picked_up_at"`
	CompletedAt *time.Time      `json:"completed_at"`
	DeployID    *string         `json:"deploy_id"`
}

// ivComment is IssueVault's comment representation.
type ivComment struct {
	ID         string        `json:"id"`
	Author     string        `json:"author"`
	AuthorKind string        `json:"author_kind"` // "user" | "agent" | "system"
	Body       string        `json:"body"`
	CreatedAt  time.Time     `json:"created_at"`
	Meta       ivCommentMeta `json:"meta"`
}

// ivCommentMeta carries the FieldDraft comment provenance under comment.meta.
type ivCommentMeta struct {
	AuthorType string `json:"author_type"` // "user" | "po" | "dev" | "system"
	Internal   ivBool `json:"internal"`
	Withheld   ivBool `json:"withheld"`
	POReviewed ivBool `json:"po_reviewed"`
}

// ivFile is IssueVault's file (=FileMeta) representation.
type ivFile struct {
	ID        string    `json:"id"`
	SHA256    string    `json:"sha256"`
	Name      string    `json:"name"`
	Kind      string    `json:"kind"`
	Mime      string    `json:"mime"`
	Size      int64     `json:"size"`
	CreatedAt time.Time `json:"created_at"`
}

// ivRecord is a generic IssueVault project record. FieldDraft uses type "deploy".
type ivRecord struct {
	ID        string       `json:"id"`
	Type      string       `json:"type"`
	Data      ivDeployData `json:"data"`
	TicketIDs []string     `json:"ticket_ids"`
	CreatedAt time.Time    `json:"created_at"`
}

// ivDeployData is the payload of a "deploy" record.
type ivDeployData struct {
	Version string `json:"version"`
	GitSHA  string `json:"git_sha"`
	Status  string `json:"status"`
}

// ─── HTTP helpers ───────────────────────────────────────────────

// request issues a raw HTTP call with the bearer token attached and returns the
// response body on 2xx, or an error (status + body) otherwise. It doubles as the
// raw-bytes helper for file upload/download.
func (s *issuevaultStore) request(method, path, contentType string, body []byte, extra map[string]string) ([]byte, error) {
	var r io.Reader
	if body != nil {
		r = bytes.NewReader(body)
	}
	req, err := http.NewRequest(method, s.baseURL+path, r)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+s.token)
	req.Header.Set("Accept", "application/json")
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	for k, v := range extra {
		req.Header.Set(k, v)
	}
	resp, err := s.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	data, _ := io.ReadAll(resp.Body)
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("issuevault %s %s: %d %s", method, path, resp.StatusCode, strings.TrimSpace(string(data)))
	}
	return data, nil
}

// doJSON marshals body (if any) as JSON, issues the request, and unmarshals the
// response into out (if any). Non-2xx becomes an error.
func (s *issuevaultStore) doJSON(method, path string, body, out any) error {
	var raw []byte
	ct := ""
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return err
		}
		raw = b
		ct = "application/json"
	}
	data, err := s.request(method, path, ct, raw, nil)
	if err != nil {
		return err
	}
	if out != nil && len(data) > 0 {
		return json.Unmarshal(data, out)
	}
	return nil
}

// search runs a POST /tickets/search with the given predicate and returns the
// matched tickets. The response envelope is assumed to be {"tickets":[…]}.
func (s *issuevaultStore) search(predicate map[string]any) ([]ivTicket, error) {
	var out struct {
		Tickets []ivTicket `json:"tickets"`
	}
	if err := s.doJSON(http.MethodPost, "/tickets/search", predicate, &out); err != nil {
		return nil, err
	}
	return out.Tickets, nil
}

// getTicketRaw fetches a hydrated ticket in its wire form (keeps meta.work_order,
// which the FieldDraft *Ticket mapping drops). Returns nil on any error.
func (s *issuevaultStore) getTicketRaw(id string) *ivTicket {
	var iv ivTicket
	if err := s.doJSON(http.MethodGet, "/tickets/"+id, nil, &iv); err != nil {
		return nil
	}
	return &iv
}

// postSystemComment appends a `system` audit comment (state/verdict trail).
func (s *issuevaultStore) postSystemComment(ticketID, body string) {
	payload := map[string]any{
		"author_kind": "system",
		"body":        body,
		"meta":        map[string]any{"author_type": string(AuthorSystem)},
	}
	if err := s.doJSON(http.MethodPost, "/tickets/"+ticketID+"/comments", payload, nil); err != nil {
		log.Printf("issuevault: system comment on %s: %v", ticketID, err)
	}
}

// ─── mapping helpers (IssueVault → FieldDraft) ──────────────────

// mapIVStatus adopts IssueVault's `status` as the FieldDraft TicketState.
//
// IssueVault does NOT whitelist that field, so any writer — an import, a peer
// installation — can park a ticket in a state this server has never heard of.
// Such a ticket is DEAD: validateTransition rejects every move out of it, so it
// can never be resolved, only deleted. It happened: a one-shot import wrote
// status "open" (see agents/migrate-issuevault.mjs) and 60 long-finished
// tickets came back looking open forever.
//
// The value is kept VERBATIM — silently rewriting it to a legal state would
// hide the corruption instead of showing it. The log line is the alarm.
func mapIVStatus(id, status string) TicketState {
	s := TicketState(status)
	if !validState(s) {
		log.Printf("issuevault: ticket %s has unknown status %q — no legal transition out of it", id, status)
	}
	return s
}

// derivedIVState maps a FieldDraft state to IssueVault's coarse open/closed.
func derivedIVState(s TicketState) string {
	switch s {
	case StateResolved, StateClosed, StateDeclined, StateRejectedSpam, StateRejectedMalicious:
		return "closed"
	default:
		return "open"
	}
}

// ivAuthorKind maps a FieldDraft author type to IssueVault's coarse author_kind.
func ivAuthorKind(at AuthorType) string {
	switch at {
	case AuthorUser:
		return "user"
	case AuthorSystem:
		return "system"
	default: // po, dev
		return "agent"
	}
}

// emptyToNil returns nil for "", else a pointer to s (for ""→nil JSON fields).
func emptyToNil(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

func mapTicket(iv *ivTicket) *Ticket {
	return &Ticket{
		ID: iv.ID,
		// IssueVault assigns `number` per-project at creation, stable for life
		// (ticket 066230d9). Surfaced as the short "#N" reference.
		Number:      iv.Number,
		AuthorID:    iv.Author,
		Title:       iv.Title,
		Body:        iv.Body,
		State:       mapIVStatus(iv.ID, iv.Status),
		Category:    iv.Category,
		POSpam:      bool(iv.Meta.POSpam),
		POMalicious: bool(iv.Meta.POMalicious),
		POVisionFit: iv.Meta.POVisionFit,
		POReason:    iv.Meta.POReason,
		POVerdictAt: iv.Meta.POVerdictAt,
		CreatedAt:   iv.CreatedAt,
		UpdatedAt:   iv.UpdatedAt,
	}
}

// mapTicketFull maps a ticket and hydrates its comments + artifacts.
func mapTicketFull(iv *ivTicket) *Ticket {
	t := mapTicket(iv)
	for i := range iv.Comments {
		t.Comments = append(t.Comments, mapComment(iv.ID, &iv.Comments[i]))
	}
	for i := range iv.Files {
		t.Artifacts = append(t.Artifacts, mapFile(iv.ID, &iv.Files[i]))
	}
	return t
}

func mapComment(ticketID string, c *ivComment) *TicketComment {
	tc := &TicketComment{
		ID:         c.ID,
		TicketID:   ticketID,
		AuthorType: AuthorType(c.Meta.AuthorType),
		Internal:   bool(c.Meta.Internal),
		Body:       c.Body,
		CreatedAt:  c.CreatedAt,
		Withheld:   bool(c.Meta.Withheld),
		POReviewed: bool(c.Meta.POReviewed),
	}
	// AuthorID is meaningful only for user comments; po/dev/system carry none.
	if tc.AuthorType == AuthorUser {
		tc.AuthorID = emptyToNil(c.Author)
	}
	return tc
}

func mapFile(ticketID string, f *ivFile) *TicketArtifact {
	return &TicketArtifact{
		ID:        f.ID,
		TicketID:  ticketID,
		Kind:      ArtifactKind(f.Kind),
		BlobRef:   f.SHA256,
		Mime:      f.Mime,
		Size:      f.Size,
		Name:      f.Name,
		CreatedAt: f.CreatedAt,
	}
}

func mapWorkOrder(ticketID string, wo *ivWorkOrder) *WorkOrder {
	if wo == nil {
		return nil
	}
	out := &WorkOrder{
		TicketID:    ticketID,
		Payload:     wo.Payload,
		PickedUpAt:  wo.PickedUpAt,
		CompletedAt: wo.CompletedAt,
		DeployID:    wo.DeployID,
	}
	if wo.CreatedAt != nil {
		out.CreatedAt = *wo.CreatedAt
	}
	return out
}

func mapDeploy(r *ivRecord) *Deploy {
	return &Deploy{
		ID:         r.ID,
		Version:    r.Data.Version,
		GitSHA:     r.Data.GitSHA,
		TicketIDs:  r.TicketIDs,
		Status:     DeployStatus(r.Data.Status),
		DeployedAt: r.CreatedAt,
	}
}

// ─── tickets ────────────────────────────────────────────────────

func (s *issuevaultStore) CreateTicket(authorID, title, body, category string) *Ticket {
	payload := map[string]any{
		"installation_id": s.installationID,
		"title":           title,
		"body":            body,
		"category":        emptyToNil(category),
		"status":          string(StateTriage), // §7.2 initial state
		"author":          authorID,
		// NB: IssueVault's create endpoint manages `state` itself (defaults open);
		// it rejects a client-supplied `state`. State is set on later PATCHes.
		"meta":            map[string]any{},
	}
	var iv ivTicket
	if err := s.doJSON(http.MethodPost, "/tickets", payload, &iv); err != nil {
		log.Printf("issuevault: CreateTicket: %v", err)
		return nil
	}
	return mapTicketFull(&iv)
}

func (s *issuevaultStore) GetTicket(id string) *Ticket {
	iv := s.getTicketRaw(id)
	if iv == nil {
		return nil
	}
	return mapTicketFull(iv)
}

func (s *issuevaultStore) ListTickets(f TicketFilter) []*Ticket {
	// RESOLVED tickets are shared across ALL installations of the project (they
	// migrate to main / are inherently shareable), so they are read PROJECT-WIDE
	// (scope:"project"). Every OTHER state stays isolated to the viewer's own
	// installation (scope:"own"). The installation-bound token IS allowed to read
	// scope:"project" (verified with IssueVault) and GET any ticket of the project
	// by id — the per-state isolation is this adapter's choice, not a token
	// boundary. So a peer installation's resolved ticket also lists AND opens.
	mk := func(scope string, status []string) map[string]any {
		// limit:500 is IssueVault's max page size (the default is 100, and >500
		// silently clamps back to 100). Without it the list truncates at 100 —
		// material now that the shared project holds 300+ tickets. TODO: paginate
		// (via number/updated_since cursors) if a scope ever exceeds 500.
		pred := map[string]any{"scope": scope, "limit": 500}
		if status != nil {
			pred["status"] = status
		}
		if f.Category != "" {
			pred["category"] = f.Category
		}
		if f.AuthorID != "" {
			// f.AuthorID is already resolved by the caller ("me" is not handled here).
			pred["author"] = f.AuthorID
		}
		if f.Query != "" {
			pred["q"] = f.Query
		}
		return pred
	}

	seen := map[string]bool{}
	var out []*Ticket
	add := func(ivs []ivTicket) {
		for i := range ivs {
			t := mapTicket(&ivs[i])
			if seen[t.ID] {
				continue
			}
			// Visibility (§7.3): hide rejected tickets unless the viewer is admin
			// or the ticket's own author. (Resolved is never rejected, so the
			// project-wide resolved set always passes.)
			if isRejected(t.State) && !f.ViewerIsAdmin && t.AuthorID != f.ViewerID {
				continue
			}
			seen[t.ID] = true
			out = append(out, t)
		}
	}
	run := func(pred map[string]any) bool {
		ivs, err := s.search(pred)
		if err != nil {
			log.Printf("issuevault: ListTickets: %v", err)
			return false
		}
		add(ivs)
		return true
	}

	resolved := []string{string(StateResolved)}
	switch f.State {
	case StateResolved:
		// Only resolved requested → all installations' resolved.
		if !run(mk("project", resolved)) {
			return nil
		}
	case "":
		// All states → the viewer's OWN (every state) PLUS every installation's
		// resolved, merged and de-duplicated by id (own resolved appears first,
		// peer resolved is added after).
		run(mk("own", nil))
		run(mk("project", resolved))
	default:
		// A specific non-resolved state → the viewer's own installation only.
		if !run(mk("own", []string{string(f.State)})) {
			return nil
		}
	}
	sortTicketsDesc(out)
	return out
}

func (s *issuevaultStore) UpdateTicket(id, title, body string) *Ticket {
	// Author edit: only overwrite non-empty fields (mirrors the reference store).
	patch := map[string]any{}
	if title != "" {
		patch["title"] = title
	}
	if body != "" {
		patch["body"] = body
	}
	var iv ivTicket
	if err := s.doJSON(http.MethodPatch, "/tickets/"+id, patch, &iv); err != nil {
		log.Printf("issuevault: UpdateTicket: %v", err)
		return nil
	}
	return mapTicket(&iv)
}

func (s *issuevaultStore) SetTicketState(id string, to TicketState, direct bool) (*Ticket, error) {
	cur := s.GetTicket(id)
	if cur == nil {
		return nil, errTicketMissing
	}
	if err := validateStateChange(cur.State, to, direct); err != nil {
		return nil, err
	}
	patch := map[string]any{
		"status": string(to),
		"state":  derivedIVState(to),
	}
	if err := s.doJSON(http.MethodPatch, "/tickets/"+id, patch, nil); err != nil {
		return nil, err
	}
	s.postSystemComment(id, "state: "+string(cur.State)+" → "+string(to))
	return s.GetTicket(id), nil
}

func (s *issuevaultStore) SetTicketCategory(id, category string) *Ticket {
	patch := map[string]any{"category": emptyToNil(category)}
	var iv ivTicket
	if err := s.doJSON(http.MethodPatch, "/tickets/"+id, patch, &iv); err != nil {
		log.Printf("issuevault: SetTicketCategory: %v", err)
		return nil
	}
	return mapTicket(&iv)
}

func (s *issuevaultStore) SetPOVerdict(id string, v POVerdict) (*Ticket, error) {
	cur := s.GetTicket(id)
	if cur == nil {
		return nil, errTicketMissing
	}
	if err := validateTransition(cur.State, v.State); err != nil {
		return nil, err
	}
	now := time.Now().UTC()
	meta := map[string]any{
		"po_spam":       v.Spam,
		"po_malicious":  v.Malicious,
		"po_vision_fit": v.VisionFit,          // nil clears it via merge-patch
		"po_reason":     emptyToNil(v.Reason), // ""→nil (clears), else set
		"po_verdict_at": now,
	}
	patch := map[string]any{
		"status": string(v.State),
		"state":  derivedIVState(v.State),
		"meta":   meta,
	}
	if err := s.doJSON(http.MethodPatch, "/tickets/"+id, patch, nil); err != nil {
		return nil, err
	}
	s.postSystemComment(id, "po verdict: "+string(cur.State)+" → "+string(v.State))
	return s.GetTicket(id), nil
}

func (s *issuevaultStore) DeleteTicket(id string) bool {
	if err := s.doJSON(http.MethodDelete, "/tickets/"+id, nil, nil); err != nil {
		log.Printf("issuevault: DeleteTicket: %v", err)
		return false
	}
	return true
}

// ─── comments ───────────────────────────────────────────────────

func (s *issuevaultStore) AddComment(ticketID string, authorType AuthorType, authorID *string, body string, internal bool) *TicketComment {
	author := ""
	if authorID != nil {
		author = *authorID
	}
	payload := map[string]any{
		"author":      author,
		"author_kind": ivAuthorKind(authorType),
		"body":        body,
		"meta": map[string]any{
			"author_type": string(authorType),
			"internal":    internal,
			// Only user comments need PO post-moderation; the rest are reviewed.
			"po_reviewed": authorType != AuthorUser,
			"withheld":    false,
		},
	}
	var c ivComment
	if err := s.doJSON(http.MethodPost, "/tickets/"+ticketID+"/comments", payload, &c); err != nil {
		log.Printf("issuevault: AddComment: %v", err)
		return nil
	}
	return mapComment(ticketID, &c)
}

func (s *issuevaultStore) ListComments(ticketID string) []*TicketComment {
	iv := s.getTicketRaw(ticketID)
	if iv == nil {
		return nil
	}
	var out []*TicketComment
	for i := range iv.Comments {
		out = append(out, mapComment(ticketID, &iv.Comments[i]))
	}
	return out
}

func (s *issuevaultStore) GetComment(ticketID, commentID string) *TicketComment {
	iv := s.getTicketRaw(ticketID)
	if iv == nil {
		return nil
	}
	for i := range iv.Comments {
		if iv.Comments[i].ID == commentID {
			return mapComment(ticketID, &iv.Comments[i])
		}
	}
	return nil
}

// SetCommentReviewed marks a comment PO-reviewed and, when withheld, hides it
// (B1, §4.2). Withheld is sticky: it is set here, never cleared — so a false
// `withheld` is not sent (which would otherwise clear a prior hold via
// merge-patch), matching the mem/sql reference stores.
func (s *issuevaultStore) SetCommentReviewed(ticketID, commentID string, withheld bool) *TicketComment {
	meta := map[string]any{"po_reviewed": true}
	if withheld {
		meta["withheld"] = true
	}
	patch := map[string]any{"meta": meta}
	var c ivComment
	if err := s.doJSON(http.MethodPatch, "/tickets/"+ticketID+"/comments/"+commentID, patch, &c); err != nil {
		log.Printf("issuevault: SetCommentReviewed: %v", err)
		return nil
	}
	return mapComment(ticketID, &c)
}

func (s *issuevaultStore) ListTicketsWithUnreviewedComments() []*Ticket {
	pred := map[string]any{
		"scope": "own",
		"has_comment": map[string]any{
			"author_kind": "user",
			"meta": []map[string]any{
				{"path": "po_reviewed", "op": "eq", "value": false},
			},
		},
	}
	ivs, err := s.search(pred)
	if err != nil {
		log.Printf("issuevault: ListTicketsWithUnreviewedComments: %v", err)
		return nil
	}
	// Hydrate each (so comments are present, matching the reference impl).
	var out []*Ticket
	for i := range ivs {
		if full := s.GetTicket(ivs[i].ID); full != nil {
			out = append(out, full)
		}
	}
	sortTicketsDesc(out)
	return out
}

func (s *issuevaultStore) AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment {
	payload := map[string]any{
		"author_kind": "agent",
		"body":        body,
		"meta": map[string]any{
			"author_type": string(authorType),
			"internal":    internal,
			"withheld":    withheld,
			"po_reviewed": true, // agent-authored → inherently reviewed
		},
	}
	var c ivComment
	if err := s.doJSON(http.MethodPost, "/tickets/"+ticketID+"/comments", payload, &c); err != nil {
		log.Printf("issuevault: AddAgentComment: %v", err)
		return nil
	}
	return mapComment(ticketID, &c)
}

// ─── artifacts ──────────────────────────────────────────────────

func (s *issuevaultStore) AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact {
	if len(data) == 0 {
		return nil
	}
	ct := mime
	if ct == "" {
		ct = "application/octet-stream"
	}
	extra := map[string]string{
		"X-Filename": name,
		"X-Kind":     string(kind),
	}
	raw, err := s.request(http.MethodPost, "/tickets/"+ticketID+"/files", ct, data, extra)
	if err != nil {
		log.Printf("issuevault: AddArtifact: %v", err)
		return nil
	}
	var f ivFile
	if err := json.Unmarshal(raw, &f); err != nil {
		log.Printf("issuevault: AddArtifact decode: %v", err)
		return nil
	}
	return mapFile(ticketID, &f)
}

func (s *issuevaultStore) GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte) {
	iv := s.getTicketRaw(ticketID)
	if iv == nil {
		return nil, nil
	}
	var meta *TicketArtifact
	for i := range iv.Files {
		if iv.Files[i].ID == artifactID {
			meta = mapFile(ticketID, &iv.Files[i])
			break
		}
	}
	if meta == nil {
		return nil, nil
	}
	data, err := s.request(http.MethodGet, "/tickets/"+ticketID+"/files/"+artifactID, "", nil, nil)
	if err != nil {
		log.Printf("issuevault: GetArtifactBlob: %v", err)
		return nil, nil
	}
	return meta, data
}

// ─── work-orders ────────────────────────────────────────────────

func (s *issuevaultStore) CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error) {
	iv := s.getTicketRaw(ticketID)
	if iv == nil {
		return nil, errTicketMissing
	}
	if iv.Meta.WorkOrder != nil {
		return nil, errWorkOrderExists
	}
	now := time.Now().UTC()
	patch := map[string]any{"meta": map[string]any{"work_order": map[string]any{
		"payload":      payload,
		"created_at":   now,
		"picked_up_at": nil,
		"completed_at": nil,
		"deploy_id":    nil,
	}}}
	if err := s.doJSON(http.MethodPatch, "/tickets/"+ticketID, patch, nil); err != nil {
		return nil, err
	}
	return &WorkOrder{TicketID: ticketID, Payload: payload, CreatedAt: now}, nil
}

func (s *issuevaultStore) GetWorkOrder(ticketID string) *WorkOrder {
	iv := s.getTicketRaw(ticketID)
	if iv == nil {
		return nil
	}
	return mapWorkOrder(ticketID, iv.Meta.WorkOrder)
}

func (s *issuevaultStore) ClaimWorkOrder(ticketID string) (*WorkOrder, error) {
	iv := s.getTicketRaw(ticketID)
	if iv == nil || iv.Meta.WorkOrder == nil {
		return nil, errWorkOrderMissing
	}
	if iv.Meta.WorkOrder.PickedUpAt != nil {
		return nil, errWorkOrderClaimed
	}
	now := time.Now().UTC()
	// merge-patch: sets picked_up_at without touching payload/created_at.
	patch := map[string]any{"meta": map[string]any{"work_order": map[string]any{"picked_up_at": now}}}
	if err := s.doJSON(http.MethodPatch, "/tickets/"+ticketID, patch, nil); err != nil {
		return nil, err
	}
	return s.GetWorkOrder(ticketID), nil
}

func (s *issuevaultStore) CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) {
	iv := s.getTicketRaw(ticketID)
	if iv == nil || iv.Meta.WorkOrder == nil {
		return nil, errWorkOrderMissing
	}
	now := time.Now().UTC()
	wo := map[string]any{"completed_at": now}
	if deployID != "" {
		wo["deploy_id"] = deployID
	}
	patch := map[string]any{"meta": map[string]any{"work_order": wo}}
	if err := s.doJSON(http.MethodPatch, "/tickets/"+ticketID, patch, nil); err != nil {
		return nil, err
	}
	return s.GetWorkOrder(ticketID), nil
}

func (s *issuevaultStore) ListAcceptedWithOpenWorkOrder() []*Ticket {
	pred := map[string]any{
		"scope":  "own",
		"status": []string{string(StateAccepted)},
		"meta": []map[string]any{
			{"path": "work_order", "op": "notnull"},
			{"path": "work_order.completed_at", "op": "isnull"},
		},
	}
	ivs, err := s.search(pred)
	if err != nil {
		log.Printf("issuevault: ListAcceptedWithOpenWorkOrder: %v", err)
		return nil
	}
	var out []*Ticket
	for i := range ivs {
		if full := s.GetTicket(ivs[i].ID); full != nil {
			out = append(out, full)
		}
	}
	sortTicketsDesc(out)
	return out
}

// ─── deploys ────────────────────────────────────────────────────

func (s *issuevaultStore) CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error) {
	payload := map[string]any{
		"type": "deploy",
		"data": map[string]any{
			"version": version,
			"git_sha": gitSHA,
			"status":  string(status),
		},
		"ticket_ids": ticketIDs,
	}
	var rec ivRecord
	if err := s.doJSON(http.MethodPost, "/projects/"+s.projectID+"/records", payload, &rec); err != nil {
		return nil, err
	}
	return mapDeploy(&rec), nil
}

func (s *issuevaultStore) ListDeploys() []*Deploy {
	var out struct {
		Records []ivRecord `json:"records"`
	}
	if err := s.doJSON(http.MethodGet, "/projects/"+s.projectID+"/records?type=deploy", nil, &out); err != nil {
		log.Printf("issuevault: ListDeploys: %v", err)
		return nil
	}
	var ds []*Deploy
	for i := range out.Records {
		ds = append(ds, mapDeploy(&out.Records[i]))
	}
	return ds
}
