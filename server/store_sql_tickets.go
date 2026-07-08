// sqlStore — ticket domain (TicketStore implementation over MariaDB).
//
// Split out of store_sql.go purely mechanically: tickets, comments, artifacts,
// work-orders, deploys, and the agent queue queries. Users/projects/sessions/
// invites/admin-alerts stay in store_sql.go. Shared plumbing (blobs, scan
// helpers for non-ticket rows, nullIfEmpty, isDuplicate) also lives there.

package main

import (
	"database/sql"
	"encoding/json"
	"log"
)

// ─── tickets ────────────────────────────────────────────────────

const ticketCols = `id, author_id, title, body, state, category, po_spam, po_malicious,
	po_vision_fit, po_reason, po_verdict_at, created_at, updated_at, number`

func (s *sqlStore) scanTicket(scan func(...any) error) *Ticket {
	t := &Ticket{}
	var state string
	var category, visionFit, reason sql.NullString
	var verdictAt sql.NullTime
	if err := scan(&t.ID, &t.AuthorID, &t.Title, &t.Body, &state, &category,
		&t.POSpam, &t.POMalicious, &visionFit, &reason, &verdictAt,
		&t.CreatedAt, &t.UpdatedAt, &t.Number); err != nil {
		return nil
	}
	t.State = TicketState(state)
	if category.Valid {
		t.Category = &category.String
	}
	if visionFit.Valid {
		t.POVisionFit = &visionFit.String
	}
	if reason.Valid {
		t.POReason = &reason.String
	}
	if verdictAt.Valid {
		v := verdictAt.Time
		t.POVerdictAt = &v
	}
	return t
}

func (s *sqlStore) CreateTicket(authorID, title, body, category string) *Ticket {
	at := now()
	t := &Ticket{
		ID:        newID(),
		AuthorID:  authorID,
		Title:     title,
		Body:      body,
		State:     StateTriage,
		CreatedAt: at,
		UpdatedAt: at,
	}
	var cat any
	if category != "" {
		cat = category
		c := category
		t.Category = &c
	}
	res, err := s.db.Exec(
		`INSERT INTO tickets (id, author_id, title, body, state, category, created_at, updated_at)
		 VALUES (?, ?, ?, ?, 'triage', ?, ?, ?)`,
		t.ID, t.AuthorID, t.Title, t.Body, cat, t.CreatedAt, t.UpdatedAt)
	if err != nil {
		log.Printf("sqlStore: CreateTicket: %v", err)
		return nil
	}
	// `number` is the table's sole AUTO_INCREMENT column, so LastInsertId is the
	// assigned short reference (#N) — no extra round-trip needed.
	if n, e := res.LastInsertId(); e == nil {
		t.Number = int(n)
	}
	return t
}

func (s *sqlStore) GetTicket(id string) *Ticket {
	row := s.db.QueryRow(`SELECT `+ticketCols+` FROM tickets WHERE id = ?`, id)
	t := s.scanTicket(row.Scan)
	if t == nil {
		return nil
	}
	t.Comments = s.ListComments(id)
	t.Artifacts = s.listArtifacts(id)
	return t
}

func (s *sqlStore) ListTickets(f TicketFilter) []*Ticket {
	q := `SELECT ` + ticketCols + ` FROM tickets WHERE 1=1`
	var args []any
	if f.AuthorID != "" {
		q += ` AND author_id = ?`
		args = append(args, f.AuthorID)
	}
	if f.State != "" {
		q += ` AND state = ?`
		args = append(args, string(f.State))
	}
	if f.Category != "" {
		q += ` AND category = ?`
		args = append(args, f.Category)
	}
	if f.Query != "" {
		// escapeLike keeps the query a literal substring match — parity with the
		// strings.Contains semantics of memStore/fileTicketStore.
		q += ` AND (title LIKE ? OR body LIKE ?)`
		like := "%" + escapeLike(f.Query) + "%"
		args = append(args, like, like)
	}
	// Visibility (§7.3): rejected tickets are visible only to admins or the
	// ticket's own author.
	if !f.ViewerIsAdmin {
		q += ` AND (state NOT IN ('rejected_spam','rejected_malicious') OR author_id = ?)`
		args = append(args, f.ViewerID)
	}
	q += ` ORDER BY created_at DESC`

	rows, err := s.db.Query(q, args...)
	if err != nil {
		log.Printf("sqlStore: ListTickets: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Ticket
	for rows.Next() {
		if t := s.scanTicket(rows.Scan); t != nil {
			out = append(out, t)
		}
	}
	return out
}

func (s *sqlStore) UpdateTicket(id, title, body string) *Ticket {
	t := s.getTicketRow(id)
	if t == nil {
		return nil
	}
	if title != "" {
		t.Title = title
	}
	if body != "" {
		t.Body = body
	}
	t.UpdatedAt = now()
	if _, err := s.db.Exec(
		`UPDATE tickets SET title = ?, body = ?, updated_at = ? WHERE id = ?`,
		t.Title, t.Body, t.UpdatedAt, id); err != nil {
		log.Printf("sqlStore: UpdateTicket: %v", err)
		return nil
	}
	return t
}

func (s *sqlStore) SetTicketCategory(id, category string) *Ticket {
	t := s.getTicketRow(id)
	if t == nil {
		return nil
	}
	at := now()
	var cat any
	if category != "" {
		cat = category
		c := category
		t.Category = &c
	} else {
		t.Category = nil
	}
	if _, err := s.db.Exec(
		`UPDATE tickets SET category = ?, updated_at = ? WHERE id = ?`,
		cat, at, id); err != nil {
		log.Printf("sqlStore: SetTicketCategory: %v", err)
		return nil
	}
	t.UpdatedAt = at
	return t
}

// getTicketRow fetches a bare ticket (no comments/artifacts).
func (s *sqlStore) getTicketRow(id string) *Ticket {
	row := s.db.QueryRow(`SELECT `+ticketCols+` FROM tickets WHERE id = ?`, id)
	return s.scanTicket(row.Scan)
}

func (s *sqlStore) SetTicketState(id string, to TicketState) (*Ticket, error) {
	t := s.getTicketRow(id)
	if t == nil {
		return nil, errTicketMissing
	}
	audit, err := applyStateChange(t, to, now())
	if err != nil {
		return nil, err
	}
	if _, err := s.db.Exec(
		`UPDATE tickets SET state = ?, updated_at = ? WHERE id = ?`,
		string(t.State), t.UpdatedAt, id); err != nil {
		log.Printf("sqlStore: SetTicketState: %v", err)
		return nil, err
	}
	s.AddComment(id, AuthorSystem, nil, audit, false)
	return t, nil
}

func (s *sqlStore) SetPOVerdict(id string, v POVerdict) (*Ticket, error) {
	t := s.getTicketRow(id)
	if t == nil {
		return nil, errTicketMissing
	}
	audit, err := applyPOVerdict(t, v, now())
	if err != nil {
		return nil, err
	}
	var visionFit any
	if t.POVisionFit != nil {
		visionFit = *t.POVisionFit
	}
	var reason any
	if t.POReason != nil {
		reason = *t.POReason
	}
	if _, err := s.db.Exec(
		`UPDATE tickets SET po_spam = ?, po_malicious = ?, po_vision_fit = ?,
		 po_reason = ?, po_verdict_at = ?, state = ?, updated_at = ? WHERE id = ?`,
		t.POSpam, t.POMalicious, visionFit, reason, t.POVerdictAt, string(t.State), t.UpdatedAt, id); err != nil {
		log.Printf("sqlStore: SetPOVerdict: %v", err)
		return nil, err
	}
	s.AddComment(id, AuthorSystem, nil, audit, false)
	return t, nil
}

func (s *sqlStore) DeleteTicket(id string) bool {
	res, err := s.db.Exec(`DELETE FROM tickets WHERE id = ?`, id)
	if err != nil {
		log.Printf("sqlStore: DeleteTicket: %v", err)
		return false
	}
	n, _ := res.RowsAffected()
	return n > 0
}

// commentCols is the canonical comment projection (B1 adds `po_reviewed`).
const commentCols = `id, ticket_id, author_type, author_id, internal, body, created_at, withheld, po_reviewed`

func (s *sqlStore) scanComment(scan func(...any) error) *TicketComment {
	c := &TicketComment{}
	var at string
	var aid sql.NullString
	if err := scan(&c.ID, &c.TicketID, &at, &aid, &c.Internal, &c.Body, &c.CreatedAt, &c.Withheld, &c.POReviewed); err != nil {
		return nil
	}
	c.AuthorType = AuthorType(at)
	if aid.Valid {
		c.AuthorID = &aid.String
	}
	return c
}

func (s *sqlStore) AddComment(ticketID string, authorType AuthorType, authorID *string, body string, internal bool) *TicketComment {
	// The PO-review rule (only user comments need moderation) lives in newComment.
	c := newComment(ticketID, authorType, authorID, body, internal, false)
	return s.insertComment(c)
}

// insertComment persists a prepared comment row and bumps the ticket's
// activity timestamp. Shared by AddComment and AddAgentComment.
func (s *sqlStore) insertComment(c *TicketComment) *TicketComment {
	var aid any
	if c.AuthorID != nil {
		aid = *c.AuthorID
	}
	if _, err := s.db.Exec(
		`INSERT INTO ticket_comments (id, ticket_id, author_type, author_id, internal, body, created_at, withheld, po_reviewed)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		c.ID, c.TicketID, string(c.AuthorType), aid, c.Internal, c.Body, c.CreatedAt, c.Withheld, c.POReviewed); err != nil {
		log.Printf("sqlStore: insertComment: %v", err)
		return nil
	}
	// Bump the ticket's activity timestamp.
	s.db.Exec(`UPDATE tickets SET updated_at = ? WHERE id = ?`, c.CreatedAt, c.TicketID)
	return c
}

func (s *sqlStore) ListComments(ticketID string) []*TicketComment {
	rows, err := s.db.Query(
		`SELECT `+commentCols+`
		 FROM ticket_comments WHERE ticket_id = ? ORDER BY created_at ASC`, ticketID)
	if err != nil {
		log.Printf("sqlStore: ListComments: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*TicketComment
	for rows.Next() {
		if c := s.scanComment(rows.Scan); c != nil {
			out = append(out, c)
		}
	}
	return out
}

func (s *sqlStore) GetComment(ticketID, commentID string) *TicketComment {
	row := s.db.QueryRow(
		`SELECT `+commentCols+` FROM ticket_comments WHERE id = ? AND ticket_id = ?`,
		commentID, ticketID)
	return s.scanComment(row.Scan)
}

// SetCommentReviewed marks a comment PO-reviewed and, when withheld, hides it
// (B1, §4.2). Withheld is sticky: it is set, never cleared, here.
func (s *sqlStore) SetCommentReviewed(ticketID, commentID string, withheld bool) *TicketComment {
	// Scope to the named ticket: a commentID from another ticket must not be
	// moderated here (mirrors GetComment's ticket-scoped lookup).
	var q string
	if withheld {
		q = `UPDATE ticket_comments SET po_reviewed = 1, withheld = 1 WHERE id = ? AND ticket_id = ?`
	} else {
		q = `UPDATE ticket_comments SET po_reviewed = 1 WHERE id = ? AND ticket_id = ?`
	}
	if _, err := s.db.Exec(q, commentID, ticketID); err != nil {
		log.Printf("sqlStore: SetCommentReviewed: %v", err)
		return nil
	}
	row := s.db.QueryRow(`SELECT `+commentCols+` FROM ticket_comments WHERE id = ? AND ticket_id = ?`, commentID, ticketID)
	return s.scanComment(row.Scan)
}

// ListTicketsWithUnreviewedComments returns every ticket (any state) with at
// least one unreviewed USER comment, hydrated with thread + artifacts (B1).
func (s *sqlStore) ListTicketsWithUnreviewedComments() []*Ticket {
	rows, err := s.db.Query(
		`SELECT ` + ticketCols + ` FROM tickets t
		 WHERE EXISTS (
		     SELECT 1 FROM ticket_comments c
		     WHERE c.ticket_id = t.id AND c.author_type = 'user' AND c.po_reviewed = 0)
		 ORDER BY t.created_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListTicketsWithUnreviewedComments: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Ticket
	for rows.Next() {
		if t := s.scanTicket(rows.Scan); t != nil {
			out = append(out, t)
		}
	}
	for _, t := range out {
		t.Comments = s.ListComments(t.ID)
		t.Artifacts = s.listArtifacts(t.ID)
	}
	return out
}

func (s *sqlStore) AddArtifact(ticketID string, kind ArtifactKind, name, mime string, data []byte) *TicketArtifact {
	if len(data) == 0 {
		return nil
	}
	ref, err := s.putBlob(data)
	if err != nil {
		log.Printf("sqlStore: AddArtifact putBlob: %v", err)
		return nil
	}
	a := &TicketArtifact{
		ID:        newID(),
		TicketID:  ticketID,
		Kind:      kind,
		BlobRef:   ref,
		Mime:      mime,
		Size:      int64(len(data)),
		Name:      name,
		CreatedAt: now(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO ticket_artifacts (id, ticket_id, kind, blob_ref, mime, size, name, created_at)
		 VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
		a.ID, a.TicketID, string(a.Kind), a.BlobRef, a.Mime, a.Size, a.Name, a.CreatedAt); err != nil {
		log.Printf("sqlStore: AddArtifact: %v", err)
		return nil
	}
	return a
}

func (s *sqlStore) listArtifacts(ticketID string) []*TicketArtifact {
	rows, err := s.db.Query(
		`SELECT id, ticket_id, kind, blob_ref, mime, size, name, created_at
		 FROM ticket_artifacts WHERE ticket_id = ? ORDER BY created_at ASC`, ticketID)
	if err != nil {
		log.Printf("sqlStore: listArtifacts: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*TicketArtifact
	for rows.Next() {
		a := &TicketArtifact{}
		var kind string
		if err := rows.Scan(&a.ID, &a.TicketID, &kind, &a.BlobRef, &a.Mime, &a.Size, &a.Name, &a.CreatedAt); err != nil {
			continue
		}
		a.Kind = ArtifactKind(kind)
		out = append(out, a)
	}
	return out
}

func (s *sqlStore) GetArtifactBlob(ticketID, artifactID string) (*TicketArtifact, []byte) {
	a := &TicketArtifact{}
	var kind string
	err := s.db.QueryRow(
		`SELECT id, ticket_id, kind, blob_ref, mime, size, name, created_at
		 FROM ticket_artifacts WHERE id = ? AND ticket_id = ?`, artifactID, ticketID).
		Scan(&a.ID, &a.TicketID, &kind, &a.BlobRef, &a.Mime, &a.Size, &a.Name, &a.CreatedAt)
	if err != nil {
		return nil, nil
	}
	a.Kind = ArtifactKind(kind)
	data, err := s.getBlob(a.BlobRef)
	if err != nil {
		log.Printf("sqlStore: GetArtifactBlob getBlob: %v", err)
		return a, nil
	}
	return a, data
}

// ─── work-orders (§9) ───────────────────────────────────────────

const workOrderCols = `ticket_id, payload, created_at, picked_up_at, completed_at, deploy_id`

func (s *sqlStore) scanWorkOrder(scan func(...any) error) *WorkOrder {
	wo := &WorkOrder{}
	var payload []byte
	var pickedUp, completed sql.NullTime
	var deployID sql.NullString
	if err := scan(&wo.TicketID, &payload, &wo.CreatedAt, &pickedUp, &completed, &deployID); err != nil {
		return nil
	}
	wo.Payload = json.RawMessage(payload)
	if pickedUp.Valid {
		t := pickedUp.Time
		wo.PickedUpAt = &t
	}
	if completed.Valid {
		t := completed.Time
		wo.CompletedAt = &t
	}
	if deployID.Valid {
		wo.DeployID = &deployID.String
	}
	return wo
}

func (s *sqlStore) CreateWorkOrder(ticketID string, payload json.RawMessage) (*WorkOrder, error) {
	// The work-order PK is the ticket id, and the FK enforces the ticket exists.
	if s.getTicketRow(ticketID) == nil {
		return nil, errTicketMissing
	}
	wo := &WorkOrder{
		TicketID:  ticketID,
		Payload:   payload,
		CreatedAt: now(),
	}
	if _, err := s.db.Exec(
		`INSERT INTO work_orders (ticket_id, payload, created_at) VALUES (?, ?, ?)`,
		wo.TicketID, []byte(payload), wo.CreatedAt); err != nil {
		if isDuplicate(err) {
			return nil, errWorkOrderExists
		}
		log.Printf("sqlStore: CreateWorkOrder: %v", err)
		return nil, err
	}
	return wo, nil
}

func (s *sqlStore) GetWorkOrder(ticketID string) *WorkOrder {
	row := s.db.QueryRow(`SELECT `+workOrderCols+` FROM work_orders WHERE ticket_id = ?`, ticketID)
	return s.scanWorkOrder(row.Scan)
}

func (s *sqlStore) ClaimWorkOrder(ticketID string) (*WorkOrder, error) {
	// Atomic claim: only sets picked_up_at if still NULL.
	res, err := s.db.Exec(
		`UPDATE work_orders SET picked_up_at = ? WHERE ticket_id = ? AND picked_up_at IS NULL`,
		now(), ticketID)
	if err != nil {
		log.Printf("sqlStore: ClaimWorkOrder: %v", err)
		return nil, err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		// Either it doesn't exist or it's already claimed — disambiguate.
		if s.GetWorkOrder(ticketID) == nil {
			return nil, errWorkOrderMissing
		}
		return nil, errWorkOrderClaimed
	}
	return s.GetWorkOrder(ticketID), nil
}

func (s *sqlStore) CompleteWorkOrder(ticketID, deployID string) (*WorkOrder, error) {
	wo := s.GetWorkOrder(ticketID)
	if wo == nil {
		return nil, errWorkOrderMissing
	}
	// A second complete must not silently overwrite completed_at/deploy link.
	if wo.CompletedAt != nil {
		return nil, errWorkOrderCompleted
	}
	// An unknown deploy link is a clean 404 — parity with memStore/fileTicketStore
	// (previously the FK violation surfaced as a 500 here).
	if deployID != "" && !s.hasDeploy(deployID) {
		return nil, errDeployMissing
	}
	var did any
	if deployID != "" {
		did = deployID
	}
	if _, err := s.db.Exec(
		`UPDATE work_orders SET completed_at = ?, deploy_id = COALESCE(?, deploy_id) WHERE ticket_id = ?`,
		now(), did, ticketID); err != nil {
		log.Printf("sqlStore: CompleteWorkOrder: %v", err)
		return nil, err
	}
	return s.GetWorkOrder(ticketID), nil
}

// hasDeploy reports whether a deploy row with the given id exists.
func (s *sqlStore) hasDeploy(id string) bool {
	var one int
	return s.db.QueryRow(`SELECT 1 FROM deploys WHERE id = ?`, id).Scan(&one) == nil
}

// ─── deploys (§10.3) ────────────────────────────────────────────

const deployCols = `id, version, git_sha, ticket_ids, status, deployed_at`

func (s *sqlStore) scanDeploy(scan func(...any) error) *Deploy {
	d := &Deploy{}
	var ticketIDs []byte
	var status string
	if err := scan(&d.ID, &d.Version, &d.GitSHA, &ticketIDs, &status, &d.DeployedAt); err != nil {
		return nil
	}
	d.Status = DeployStatus(status)
	if len(ticketIDs) > 0 {
		json.Unmarshal(ticketIDs, &d.TicketIDs)
	}
	if d.TicketIDs == nil {
		d.TicketIDs = []string{}
	}
	return d
}

func (s *sqlStore) CreateDeploy(version, gitSHA string, ticketIDs []string, status DeployStatus) (*Deploy, error) {
	if ticketIDs == nil {
		ticketIDs = []string{}
	}
	d := &Deploy{
		ID:         newID(),
		Version:    version,
		GitSHA:     gitSHA,
		TicketIDs:  ticketIDs,
		Status:     status,
		DeployedAt: now(),
	}
	idsJSON, err := json.Marshal(ticketIDs)
	if err != nil {
		return nil, err
	}
	if _, err := s.db.Exec(
		`INSERT INTO deploys (id, version, git_sha, ticket_ids, status, deployed_at)
		 VALUES (?, ?, ?, ?, ?, ?)`,
		d.ID, d.Version, d.GitSHA, idsJSON, string(d.Status), d.DeployedAt); err != nil {
		log.Printf("sqlStore: CreateDeploy: %v", err)
		return nil, err
	}
	return d, nil
}

func (s *sqlStore) ListDeploys() []*Deploy {
	rows, err := s.db.Query(`SELECT ` + deployCols + ` FROM deploys ORDER BY deployed_at DESC, id DESC`)
	if err != nil {
		log.Printf("sqlStore: ListDeploys: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Deploy
	for rows.Next() {
		if d := s.scanDeploy(rows.Scan); d != nil {
			out = append(out, d)
		}
	}
	return out
}

// ─── agent comments ─────────────────────────────────────────────

func (s *sqlStore) AddAgentComment(ticketID string, authorType AuthorType, body string, internal, withheld bool) *TicketComment {
	// po/dev/system have no user id and are inherently reviewed (newComment rule).
	c := newComment(ticketID, authorType, nil, body, internal, withheld)
	return s.insertComment(c)
}

// ListAcceptedWithOpenWorkOrder returns accepted tickets that still carry an
// open (uncompleted) work-order — the coding agent's queue (§10.1).
func (s *sqlStore) ListAcceptedWithOpenWorkOrder() []*Ticket {
	rows, err := s.db.Query(
		`SELECT ` + ticketCols + ` FROM tickets t
		 WHERE t.state = 'accepted'
		   AND EXISTS (
		       SELECT 1 FROM work_orders w
		       WHERE w.ticket_id = t.id AND w.completed_at IS NULL)
		 ORDER BY t.created_at DESC`)
	if err != nil {
		log.Printf("sqlStore: ListAcceptedWithOpenWorkOrder: %v", err)
		return nil
	}
	defer rows.Close()
	var out []*Ticket
	for rows.Next() {
		if t := s.scanTicket(rows.Scan); t != nil {
			out = append(out, t)
		}
	}
	// Hydrate thread + artifacts so the agent has full repro context.
	for _, t := range out {
		t.Comments = s.ListComments(t.ID)
		t.Artifacts = s.listArtifacts(t.ID)
	}
	return out
}
