// Auth: session helpers, role gating, invites, and the admin invite endpoints.
//
// Registration is invite-gated; the first seeded account is admin; sessions
// are issued through the store (DB-backed in prod). Cookie attributes are
// centralized here so HttpOnly / SameSite=Lax / Secure-ready stay consistent.

package main

import (
	"encoding/json"
	"net/http"
	"time"
)

// defaultInviteTTL is how long a freshly created invite stays valid.
const defaultInviteTTL = 14 * 24 * time.Hour

// cookieSecure is set when SECURE_COOKIES=1 (i.e. served behind TLS/Caddy).
// Off by default so local HTTP and the httptest harness keep working.
var cookieSecure = false

// setSessionCookie writes the session cookie with the standard attributes.
func setSessionCookie(w http.ResponseWriter, sess *Session) {
	http.SetCookie(w, &http.Cookie{
		Name:     "session",
		Value:    sess.Token,
		Path:     "/",
		HttpOnly: true,
		Secure:   cookieSecure,
		SameSite: http.SameSiteLaxMode,
		Expires:  sess.Expires,
	})
}

// clearSessionCookie expires the session cookie.
func clearSessionCookie(w http.ResponseWriter) {
	http.SetCookie(w, &http.Cookie{
		Name:     "session",
		Value:    "",
		Path:     "/",
		HttpOnly: true,
		Secure:   cookieSecure,
		SameSite: http.SameSiteLaxMode,
		MaxAge:   -1,
	})
}

// requireAuth resolves the session + user from the request cookie, or (nil, nil).
func (s *Server) requireAuth(r *http.Request) (*Session, *User) {
	cookie, err := r.Cookie("session")
	if err != nil {
		return nil, nil
	}
	sess := s.store.GetSession(cookie.Value)
	if sess == nil {
		return nil, nil
	}
	user := s.store.GetUser(sess.UserID)
	if user == nil {
		return nil, nil
	}
	return sess, user
}

// requireAdmin resolves an authenticated admin user, or nil.
func (s *Server) requireAdmin(r *http.Request) *User {
	_, user := s.requireAuth(r)
	if user == nil || user.Role != RoleAdmin {
		return nil
	}
	return user
}

// ─── invite endpoints (admin-only) ──────────────────────────────

// inviteResponse is the shape returned by GET/POST /api/invites.
type inviteResponse struct {
	Token       string     `json:"token"`
	CreatedBy   string     `json:"created_by"`
	Note        string     `json:"note"`
	ExpiresAt   time.Time  `json:"expires_at"`
	UsedBy      *string    `json:"used_by"`
	UsedAt      *time.Time `json:"used_at"`
	CreatedAt   time.Time  `json:"created_at"`
	Status      string     `json:"status"`       // open | used | expired
	RegisterURL string     `json:"register_url"` // …/register?invite=<token>
}

func toInviteResponse(inv *Invite, now time.Time) inviteResponse {
	return inviteResponse{
		Token:       inv.Token,
		CreatedBy:   inv.CreatedBy,
		Note:        inv.Note,
		ExpiresAt:   inv.ExpiresAt,
		UsedBy:      inv.UsedBy,
		UsedAt:      inv.UsedAt,
		CreatedAt:   inv.CreatedAt,
		Status:      inv.Status(now),
		RegisterURL: "/register?invite=" + inv.Token,
	}
}

func (s *Server) handleCreateInvite(w http.ResponseWriter, r *http.Request) {
	admin := s.requireAdmin(r)
	if admin == nil {
		http.Error(w, `{"error":"admin only"}`, http.StatusForbidden)
		return
	}
	var body struct {
		Note      string `json:"note"`
		ExpiresAt string `json:"expires_at"` // optional RFC3339; default +14d
	}
	// Body is optional — ignore decode errors on an empty body.
	json.NewDecoder(r.Body).Decode(&body)

	expires := time.Now().UTC().Add(defaultInviteTTL)
	if body.ExpiresAt != "" {
		if t, err := time.Parse(time.RFC3339, body.ExpiresAt); err == nil {
			expires = t.UTC()
		}
	}
	inv := s.store.CreateInvite(admin.ID, body.Note, expires)
	if inv == nil {
		http.Error(w, `{"error":"could not create invite"}`, http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(toInviteResponse(inv, time.Now().UTC()))
}

func (s *Server) handleListInvites(w http.ResponseWriter, r *http.Request) {
	if s.requireAdmin(r) == nil {
		http.Error(w, `{"error":"admin only"}`, http.StatusForbidden)
		return
	}
	now := time.Now().UTC()
	invites := s.store.ListInvites()
	out := make([]inviteResponse, 0, len(invites))
	for _, inv := range invites {
		out = append(out, toInviteResponse(inv, now))
	}
	json.NewEncoder(w).Encode(out)
}

func (s *Server) handleDeleteInvite(w http.ResponseWriter, r *http.Request) {
	if s.requireAdmin(r) == nil {
		http.Error(w, `{"error":"admin only"}`, http.StatusForbidden)
		return
	}
	token := r.PathValue("token")
	if !s.store.DeleteInvite(token) {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	w.Write([]byte(`{"ok":true}`))
}
