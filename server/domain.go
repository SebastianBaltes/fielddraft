// Shared domain logic — the single home for business rules that every store
// implementation (memStore, sqlStore, issuevaultStore) must apply identically.
//
// Everything here is storage-agnostic: password hashing parameters, TTLs,
// comment construction (incl. the PO-review rule), and the ticket state-machine
// application (transition validation + audit-comment body + verdict fields).
// The stores only persist what these helpers compute, so the three backends
// cannot drift apart on business rules.

package main

import (
	"crypto/rand"
	"strings"
	"time"

	"golang.org/x/crypto/argon2"
)

// now is the canonical clock: all persisted timestamps are UTC, in every store.
func now() time.Time { return time.Now().UTC() }

// ─── lifetimes ──────────────────────────────────────────────────

// sessionTTL is how long a login session stays valid.
const sessionTTL = 7 * 24 * time.Hour

// resetTokenTTL is how long a password-reset token stays valid.
const resetTokenTTL = 1 * time.Hour

// minPasswordLen is the password policy (docs/AUTH-CONTRACT.md) — declared
// exactly once, enforced at every entry point (register, reset, change).
const minPasswordLen = 8

// ─── password hashing (Argon2id) ────────────────────────────────

// The Argon2id parameters — defined exactly once. Changing them here changes
// every store; per-store copies are forbidden.
const (
	argonTime    = 1
	argonMemory  = 64 * 1024 // 64 MB
	argonThreads = 4
	argonKeyLen  = 32
	argonSaltLen = 16
)

// hashPassword derives a fresh salt + Argon2id hash for a password.
func hashPassword(password string) (hash, salt []byte) {
	salt = make([]byte, argonSaltLen)
	rand.Read(salt)
	hash = argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return hash, salt
}

// verifyPassword reports whether password matches the stored salt+hash
// (constant-time comparison).
func verifyPassword(password string, salt, hash []byte) bool {
	got := argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return bytesEqual(got, hash)
}

// ─── ticket comments ────────────────────────────────────────────

// newComment builds a ticket comment with the shared business rules applied:
// a fresh ID, a UTC timestamp, and the PO-review rule — only USER comments
// need PO post-moderation; po/dev/system comments are inherently reviewed
// (B1, §4.2).
func newComment(ticketID string, authorType AuthorType, authorID *string, body string, internal, withheld bool) *TicketComment {
	return &TicketComment{
		ID:         newID(),
		TicketID:   ticketID,
		AuthorType: authorType,
		AuthorID:   authorID,
		Internal:   internal,
		Body:       body,
		CreatedAt:  now(),
		Withheld:   withheld,
		POReviewed: authorType != AuthorUser,
	}
}

// ─── state machine application ──────────────────────────────────

// applyStateChange validates the from→to move against the §7.2 state machine
// and, if legal, applies it to t (state + updated_at). It returns the audit
// `system` comment body the store must append. t is only mutated on success.
//
// `direct` is the ADMIN OVERRIDE (ticket #345): it skips the transition graph
// so an operator can set any state from any state — the graph exists to keep the
// automated user→PO→dev flow honest, not to handcuff the operator who runs it,
// and it is the only way to rescue a ticket parked in a state with no legal way
// out. The state itself is still validated: an unknown value is never written.
func applyStateChange(t *Ticket, to TicketState, at time.Time, direct bool) (audit string, err error) {
	if err := validateStateChange(t.State, to, direct); err != nil {
		return "", err
	}
	from := t.State
	t.State = to
	t.UpdatedAt = at
	return "state: " + string(from) + " → " + string(to), nil
}

// applyPOVerdict validates the verdict's state move and, if legal, applies the
// full verdict to t (po_* fields + state + updated_at + verdict timestamp). It
// returns the audit `system` comment body. t is only mutated on success.
func applyPOVerdict(t *Ticket, v POVerdict, at time.Time) (audit string, err error) {
	if err := validateTransition(t.State, v.State); err != nil {
		return "", err
	}
	from := t.State
	t.POSpam = v.Spam
	t.POMalicious = v.Malicious
	t.POVisionFit = v.VisionFit
	// A verdict replaces the whole verdict block: an empty reason clears any
	// previous one (sqlStore's behavior; memStore used to keep the old value).
	t.POReason = nil
	if v.Reason != "" {
		r := v.Reason
		t.POReason = &r
	}
	verdictAt := at
	t.POVerdictAt = &verdictAt
	t.State = v.State
	t.UpdatedAt = at
	return "po verdict: " + string(from) + " → " + string(v.State), nil
}

// ─── search semantics ───────────────────────────────────────────

// escapeLike escapes the SQL LIKE metacharacters (% _ \) so a user query is
// matched as a literal substring — the same semantics memStore/issuevaultStore
// implement with strings.Contains.
func escapeLike(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `%`, `\%`)
	s = strings.ReplaceAll(s, `_`, `\_`)
	return s
}
