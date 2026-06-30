// Shared domain models for the FieldDraft backend.
//
// These are storage-agnostic: both the in-memory store (test fixture) and
// the MariaDB-backed store populate the same structs, and the HTTP handlers
// only ever see these — so API response shapes are identical regardless of
// which store backs the server.

package main

import (
	"encoding/json"
	"time"
)

// Role is a user's authorization level.
type Role string

const (
	RoleUser  Role = "user"
	RoleAdmin Role = "admin"
)

type User struct {
	ID           string    `json:"id"`
	Email        string    `json:"email"`
	DisplayName  string    `json:"display_name"`
	Role         Role      `json:"role"`
	PasswordHash []byte    `json:"-"`
	Salt         []byte    `json:"-"`
	CreatedAt    time.Time `json:"created_at"`
}

type Project struct {
	ID         string          `json:"id"`
	OwnerID    string          `json:"owner_id"`
	Name       string          `json:"name"`
	Visibility string          `json:"visibility"` // "private" | "public"
	Document   json.RawMessage `json:"document"`
	UpdatedAt  time.Time       `json:"updated_at"`
	CreatedAt  time.Time       `json:"created_at"`
}

type Session struct {
	Token   string
	UserID  string
	Expires time.Time
}

// ResetToken is a single-use password-reset token (1 h validity).
type ResetToken struct {
	Token   string
	UserID  string
	Expires time.Time
}

// Snapshot is a published, shareable copy of a project document.
type Snapshot struct {
	ID        string          `json:"id"`
	ProjectID string          `json:"project_id"`
	Document  json.RawMessage `json:"document"`
	CreatedAt time.Time       `json:"created_at"`
}

// Invite is a single-use, admin-issued registration token.
type Invite struct {
	Token     string     `json:"token"`
	CreatedBy string     `json:"created_by"`
	Note      string     `json:"note"`
	ExpiresAt time.Time  `json:"expires_at"`
	UsedBy    *string    `json:"used_by"` // nil while open
	UsedAt    *time.Time `json:"used_at"` // nil while open
	CreatedAt time.Time  `json:"created_at"`
}

// Status reports an invite's lifecycle bucket for the admin list.
func (i *Invite) Status(now time.Time) string {
	switch {
	case i.UsedBy != nil:
		return "used"
	case now.After(i.ExpiresAt):
		return "expired"
	default:
		return "open"
	}
}
