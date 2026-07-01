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
// A banned (disabled) user's session is treated as invalid (B3, §4.4).
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
	if user == nil || user.Disabled {
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

// ─── public invite lookup (no auth) ─────────────────────────────

// inviteInfoResponse is the PUBLIC, minimal view of an invite used to prefill
// the self-registration form. For an open (unused, unexpired) invite it carries
// the addressee's name + email and Valid=true; for a missing / used / expired
// token it carries ONLY Valid=false — nothing else leaks about the invite.
type inviteInfoResponse struct {
	InviteeName  string `json:"invitee_name,omitempty"`
	InviteeEmail string `json:"invitee_email,omitempty"`
	Valid        bool   `json:"valid"`
}

// handleInviteInfo serves GET /api/invites/{token}/info. Public (no session):
// the register link is followed by someone who isn't logged in yet, so the form
// must be able to read the prefill fields. Only an OPEN invite exposes the
// name/email; every other case (unknown token, already used, expired) returns
// a bare {"valid": false} so the endpoint can't be used to probe invite state.
func (s *Server) handleInviteInfo(w http.ResponseWriter, r *http.Request) {
	token := r.PathValue("token")
	inv := s.store.GetInvite(token)
	if inv == nil || inv.Status(time.Now().UTC()) != "open" {
		json.NewEncoder(w).Encode(inviteInfoResponse{Valid: false})
		return
	}
	json.NewEncoder(w).Encode(inviteInfoResponse{
		InviteeName:  inv.InviteeName,
		InviteeEmail: inv.InviteeEmail,
		Valid:        true,
	})
}

// ─── invite endpoints (admin-only) ──────────────────────────────

// inviteResponse is the shape returned by GET/POST /api/invites.
type inviteResponse struct {
	Token        string     `json:"token"`
	CreatedBy    string     `json:"created_by"`
	InviteeName  string     `json:"invitee_name"`
	InviteeEmail string     `json:"invitee_email"`
	Message      string     `json:"message"`
	ExpiresAt    time.Time  `json:"expires_at"`
	UsedBy       *string    `json:"used_by"`
	UsedAt       *time.Time `json:"used_at"`
	CreatedAt    time.Time  `json:"created_at"`
	Status       string     `json:"status"`       // open | used | expired
	RegisterURL  string     `json:"register_url"` // …/register?invite=<token>
}

func toInviteResponse(inv *Invite, now time.Time) inviteResponse {
	return inviteResponse{
		Token:        inv.Token,
		CreatedBy:    inv.CreatedBy,
		InviteeName:  inv.InviteeName,
		InviteeEmail: inv.InviteeEmail,
		Message:      inv.Message,
		ExpiresAt:    inv.ExpiresAt,
		UsedBy:       inv.UsedBy,
		UsedAt:       inv.UsedAt,
		CreatedAt:    inv.CreatedAt,
		Status:       inv.Status(now),
		RegisterURL:  "/register?invite=" + inv.Token,
	}
}

func (s *Server) handleCreateInvite(w http.ResponseWriter, r *http.Request) {
	admin := s.requireAdmin(r)
	if admin == nil {
		http.Error(w, `{"error":"admin only"}`, http.StatusForbidden)
		return
	}
	var body struct {
		InviteeName  string `json:"invitee_name"`
		InviteeEmail string `json:"invitee_email"`
		Message      string `json:"message"`
		ExpiresAt    string `json:"expires_at"` // optional RFC3339; default +14d
	}
	// Body is optional — ignore decode errors on an empty body.
	json.NewDecoder(r.Body).Decode(&body)

	expires := time.Now().UTC().Add(defaultInviteTTL)
	if body.ExpiresAt != "" {
		if t, err := time.Parse(time.RFC3339, body.ExpiresAt); err == nil {
			expires = t.UTC()
		}
	}
	inv := s.store.CreateInvite(admin.ID, body.InviteeName, body.InviteeEmail, body.Message, expires)
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

// requireAdminOr401 resolves an authenticated admin, writing the correct error
// for the failure: 401 when there is no session at all, 403 when the session is
// a non-admin user. Returns nil (and has written the response) on failure.
func (s *Server) requireAdminOr401(w http.ResponseWriter, r *http.Request) *User {
	_, user := s.requireAuth(r)
	if user == nil {
		http.Error(w, `{"error":"not authenticated"}`, http.StatusUnauthorized)
		return nil
	}
	if user.Role != RoleAdmin {
		http.Error(w, `{"error":"admin only"}`, http.StatusForbidden)
		return nil
	}
	return user
}

// ─── admin alerts (B2, §4.4 admin-awareness) ────────────────────

func (s *Server) handleListAdminAlerts(w http.ResponseWriter, r *http.Request) {
	if s.requireAdminOr401(w, r) == nil {
		return
	}
	unseenOnly := r.URL.Query().Get("unseen") == "1"
	alerts := s.store.ListAdminAlerts(unseenOnly)
	if alerts == nil {
		alerts = []*AdminAlert{}
	}
	json.NewEncoder(w).Encode(alerts)
}

func (s *Server) handleMarkAdminAlertSeen(w http.ResponseWriter, r *http.Request) {
	if s.requireAdminOr401(w, r) == nil {
		return
	}
	if !s.store.MarkAdminAlertSeen(r.PathValue("id")) {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	w.Write([]byte(`{"ok":true}`))
}

// ─── user ban / disable (B3, §4.4 post-hoc admin controls) ──────

// adminUser is the §6.2 admin user-list row.
type adminUser struct {
	ID          string    `json:"id"`
	Email       string    `json:"email"`
	DisplayName string    `json:"display_name"`
	Role        Role      `json:"role"`
	Disabled    bool      `json:"disabled"`
	CreatedAt   time.Time `json:"created_at"`
}

func (s *Server) handleListUsers(w http.ResponseWriter, r *http.Request) {
	if s.requireAdminOr401(w, r) == nil {
		return
	}
	users := s.store.ListUsers()
	out := make([]adminUser, 0, len(users))
	for _, u := range users {
		out = append(out, adminUser{
			ID:          u.ID,
			Email:       u.Email,
			DisplayName: u.DisplayName,
			Role:        u.Role,
			Disabled:    u.Disabled,
			CreatedAt:   u.CreatedAt,
		})
	}
	json.NewEncoder(w).Encode(out)
}

func (s *Server) handleBanUser(w http.ResponseWriter, r *http.Request) {
	admin := s.requireAdminOr401(w, r)
	if admin == nil {
		return
	}
	id := r.PathValue("id")
	target := s.store.GetUser(id)
	if target == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	// Never ban the last remaining admin (§4.4: admin keeps control). The guard
	// counts admins OTHER than the target; banning is refused if none remain.
	if target.Role == RoleAdmin && s.store.AdminCount(target.ID) == 0 {
		http.Error(w, `{"error":"cannot ban the last admin"}`, http.StatusForbidden)
		return
	}
	updated := s.store.SetUserDisabled(id, true)
	if updated == nil {
		http.Error(w, `{"error":"could not ban user"}`, http.StatusInternalServerError)
		return
	}
	// Invalidate every session the banned user holds (§4.4).
	s.store.DeleteUserSessions(id)
	s.store.CreateAdminAlert(AlertUserBanned, nil, "user banned: "+updated.Email)
	json.NewEncoder(w).Encode(updated)
}

func (s *Server) handleUnbanUser(w http.ResponseWriter, r *http.Request) {
	if s.requireAdminOr401(w, r) == nil {
		return
	}
	id := r.PathValue("id")
	if s.store.GetUser(id) == nil {
		http.Error(w, `{"error":"not found"}`, http.StatusNotFound)
		return
	}
	updated := s.store.SetUserDisabled(id, false)
	if updated == nil {
		http.Error(w, `{"error":"could not unban user"}`, http.StatusInternalServerError)
		return
	}
	json.NewEncoder(w).Encode(updated)
}
