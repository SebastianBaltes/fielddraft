/**
 * Typed client for the FieldDraft platform backend (WP3).
 *
 * A thin, dependency-free façade over the cookie-session JSON API served by
 * `server/` (see server/main.go + server/auth.go). Every call:
 *   - speaks JSON,
 *   - sends `credentials: "include"` so the HttpOnly `session` cookie rides
 *     along (the backend issues it on login/register),
 *   - throws an {@link ApiError} carrying the HTTP status + the server's
 *     `{"error": "..."}` message on a non-2xx response.
 *
 * The shapes mirror the Go structs verbatim (snake_case JSON tags):
 *   User      ← server/models.go  User
 *   Project   ← server/models.go  Project
 *   Invite    ← server/auth.go    inviteResponse (adds status + register_url)
 *
 * Guest mode never touches this module — it is only used once the user opens
 * the account UI. A failed `getMe()` (401) is the normal "logged out" signal
 * and surfaces as `null`, not a thrown error.
 */

/** A user's authorization level. Mirrors server `Role`. */
export type Role = "user" | "admin";

/** A project's sharing state. Mirrors server `Project.Visibility`. */
export type Visibility = "private" | "public";

/** Authenticated user, as returned by `/api/me`, login and register. */
export interface User {
  id: string;
  email: string;
  display_name: string;
  role: Role;
  created_at: string;
}

/** A server-side project. `document` is the raw `.fdraft` project JSON the
 *  kernel store serializes; it is omitted from list responses' payloads only
 *  if the backend trims it (today it is always present). */
export interface Project {
  id: string;
  owner_id: string;
  name: string;
  visibility: Visibility;
  document: unknown;
  updated_at: string;
  created_at: string;
}

/** An admin-issued, single-use registration invite. Mirrors `inviteResponse`.
 *  WP13: invites now carry the addressee (name + email) and a free-text
 *  invitation `message` shown to the recipient, replacing the old private
 *  `note`. */
export interface Invite {
  token: string;
  created_by: string;
  invitee_name: string;
  invitee_email: string;
  message: string;
  expires_at: string;
  used_by: string | null;
  used_at: string | null;
  created_at: string;
  /** "open" | "used" | "expired" — derived server-side. */
  status: string;
  /** `/register?invite=<token>` — the copyable link. */
  register_url: string;
}

/** Body for {@link PlatformApi.createInvite} (WP13). The addressee's name +
 *  email and the invitation message shown to them. */
export interface CreateInviteInput {
  invitee_name: string;
  invitee_email: string;
  message: string;
}

/** The PUBLIC (no-auth) invite lookup used to prefill the self-registration
 *  form from a `?invite=<token>` link. Mirrors server `inviteInfoResponse`:
 *  an OPEN invite carries the invitee's name + email with `valid: true`; a
 *  missing / used / expired token returns only `valid: false` (nothing else
 *  leaks). Name/email are omitted (undefined) when invalid. */
export interface InviteInfo {
  invitee_name?: string;
  invitee_email?: string;
  valid: boolean;
}

/** The result of publishing a project to a shareable snapshot. */
export interface PublishResult {
  snapshot_id: string;
  view_url: string;
}

/* ── tickets (WP4) ─────────────────────────────────────────────── */

/** A ticket's position in the §7.2 state machine. Mirrors server `TicketState`. */
export type TicketState =
  | "triage"
  | "needs_info"
  | "accepted"
  | "in_progress"
  | "deploying"
  | "resolved"
  | "closed"
  | "declined"
  | "rejected_spam"
  | "rejected_malicious";

/** Who wrote a ticket comment (§4 security context). Mirrors server `AuthorType`.
 *  `user` / `po` / `system` are user-visible; `dev` is admin-only. */
export type AuthorType = "user" | "po" | "dev" | "system";

/** A ticket's issue category (or `null` when unset). */
export type TicketCategory = "bug" | "feature" | "question" | "other";

/** A repro blob attached to a ticket. Mirrors server `ArtifactKind`. */
export type ArtifactKind =
  "screenshot" | "fdraft" | "actions" | "settings" | "ui_state" | "attachment";

/** A list row: no thread, no body blob (mirrors server `ticketSummary`). */
export interface TicketSummary {
  id: string;
  author_id: string;
  title: string;
  state: TicketState;
  category: string | null;
  created_at: string;
  updated_at: string;
}

/** One entry in a ticket's discussion thread (mirrors server `TicketComment`).
 *  The backend already drops `internal`/`withheld` comments for non-admins; the
 *  UI guards again as defense-in-depth. */
export interface TicketComment {
  id: string;
  ticket_id: string;
  author_type: AuthorType;
  author_id: string | null;
  internal: boolean;
  body: string;
  created_at: string;
  withheld: boolean;
}

/* ── admin moderation (WP8/WP9) ────────────────────────────────── */

/** The classification of an admin alert. Mirrors server `AlertKind`. */
export type AlertKind =
  "malicious_ticket" | "malicious_comment" | "withheld_comment" | "user_banned";

/** A durable admin-awareness record (§4.4). Mirrors server `AdminAlert`.
 *  `ticket_id` is `null` for non-ticket-scoped alerts (e.g. a user ban). */
export interface AdminAlert {
  id: string;
  kind: AlertKind;
  ticket_id: string | null;
  detail: string;
  created_at: string;
  seen: boolean;
}

/** A row in the admin user-management list (§6.2). Mirrors server `adminUser`.
 *  `disabled` is the banned flag (no login, sessions invalidated). */
export interface AdminUser {
  id: string;
  email: string;
  display_name: string;
  role: Role;
  disabled: boolean;
  created_at: string;
}

/** A content-addressed repro blob (mirrors server `TicketArtifact`). */
export interface TicketArtifact {
  id: string;
  ticket_id: string;
  kind: ArtifactKind;
  blob_ref: string;
  mime: string;
  size: number;
  name: string;
  created_at: string;
}

/** The full ticket view: the ticket + a visibility-filtered thread + artifacts
 *  (mirrors server `ticketDetailResponse`, which embeds the `Ticket`). */
export interface TicketDetail {
  id: string;
  author_id: string;
  title: string;
  body: string;
  state: TicketState;
  category: string | null;
  po_spam: boolean;
  po_malicious: boolean;
  po_vision_fit: string | null;
  po_reason: string | null;
  po_verdict_at: string | null;
  created_at: string;
  updated_at: string;
  comments: TicketComment[];
  artifacts: TicketArtifact[];
}

/** Filters for {@link PlatformApi.listTickets} (§7.3). All optional; an empty
 *  field is omitted from the query string. `author: "me"` is the "only mine"
 *  toggle. */
export interface TicketListFilter {
  author?: "me" | string;
  state?: TicketState | "";
  category?: TicketCategory | "";
  q?: string;
}

/** Body for the plain (GitHub-style) ticket-creation path (§7.1). */
export interface CreateTicketInput {
  title: string;
  body: string;
  category?: TicketCategory | "";
}

/** Fields the ticket PATCH endpoint accepts. The author may edit title/body and
 *  close (state → "closed"); admins may drive any validated transition. */
export interface TicketPatch {
  title?: string;
  body?: string;
  state?: TicketState;
}

/** A non-2xx API response, carrying the HTTP status and server message. */
export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/** Pull the `{"error": "..."}` message out of a failed response body, falling
 *  back to the HTTP status text. Never throws. */
async function extractError(res: Response): Promise<string> {
  try {
    const text = await res.text();
    if (text) {
      try {
        const parsed = JSON.parse(text) as { error?: unknown };
        if (typeof parsed.error === "string" && parsed.error) {
          return parsed.error;
        }
      } catch {
        // Not JSON — fall through to the raw text.
      }
      return text;
    }
  } catch {
    // Body already consumed / network drop — fall through.
  }
  return res.statusText || `HTTP ${res.status}`;
}

/** Core fetch wrapper: JSON in/out, cookie session, typed errors. `T = void`
 *  for endpoints whose body we don't care about. `body` here is a plain JSON-
 *  serializable value (not the DOM `BodyInit`), so it is omitted from the base
 *  `RequestInit` and re-typed. */
async function request<T>(
  path: string,
  init: Omit<RequestInit, "body"> & { body?: unknown } = {},
): Promise<T> {
  const headers: Record<string, string> = {
    Accept: "application/json",
    ...(init.headers as Record<string, string> | undefined),
  };
  let body: BodyInit | undefined;
  if (init.body !== undefined) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(init.body);
  }
  const res = await fetch(path, {
    ...init,
    headers,
    body,
    credentials: "include",
  });
  if (!res.ok) {
    throw new ApiError(res.status, await extractError(res));
  }
  // 204 / empty body → undefined; otherwise parse JSON.
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/* ── auth ──────────────────────────────────────────────────────── */

export interface RegisterInput {
  email: string;
  display_name: string;
  password: string;
  invite: string;
}

export interface LoginInput {
  email: string;
  password: string;
}

/** The platform API surface. A single object so callers (and tests) can pass a
 *  fake implementation. The default instance is {@link platformApi}. */
export interface PlatformApi {
  // Auth.
  register(input: RegisterInput): Promise<User>;
  login(input: LoginInput): Promise<User>;
  logout(): Promise<void>;
  /** Current session's user, or `null` when not authenticated (401). */
  getMe(): Promise<User | null>;
  requestPasswordReset(email: string): Promise<void>;
  resetPassword(token: string, newPassword: string): Promise<void>;

  // Projects (owner-scoped, auth required).
  listProjects(): Promise<Project[]>;
  createProject(name: string, document: unknown): Promise<Project>;
  getProject(id: string): Promise<Project>;
  updateProject(id: string, patch: ProjectPatch): Promise<Project>;
  deleteProject(id: string): Promise<void>;
  setVisibility(id: string, visibility: Visibility): Promise<Project>;
  publishProject(id: string): Promise<PublishResult>;

  // Gallery (public, no auth).
  listGallery(): Promise<Project[]>;

  // Invites (admin only).
  listInvites(): Promise<Invite[]>;
  createInvite(input: CreateInviteInput): Promise<Invite>;
  deleteInvite(token: string): Promise<void>;
  /** Public (no-auth) invite lookup — reads the invitee name/email to prefill
   *  the self-registration form. Returns `{valid:false}` for a missing / used /
   *  expired token. */
  getInviteInfo(token: string): Promise<InviteInfo>;

  // Tickets (WP4 — GitHub-Issues-like). Listing/reading is auth-only on the
  // backend; creating/commenting requires a session too.
  listTickets(filter?: TicketListFilter): Promise<TicketSummary[]>;
  getTicket(id: string): Promise<TicketDetail>;
  createTicket(input: CreateTicketInput): Promise<TicketDetail>;
  updateTicket(id: string, patch: TicketPatch): Promise<TicketDetail>;
  deleteTicket(id: string): Promise<void>;
  listComments(id: string): Promise<TicketComment[]>;
  addComment(id: string, body: string): Promise<TicketComment>;
  /** Fetch an artifact blob's raw bytes (e.g. the `.fdraft` ZIP to load). */
  fetchArtifact(ticketId: string, artifactId: string): Promise<ArrayBuffer>;

  // Admin moderation (admin only — the backend re-checks the role on each).
  /** List admin alerts, newest-first. `unseenOnly` adds `?unseen=1`. */
  listAdminAlerts(unseenOnly?: boolean): Promise<AdminAlert[]>;
  /** Mark a single alert seen (idempotent on the backend). */
  markAdminAlertSeen(id: string): Promise<void>;
  /** List all users for moderation (id/email/name/role/disabled/created). */
  listAdminUsers(): Promise<AdminUser[]>;
  /** Ban (disable) a user — kills their sessions; returns the updated user. */
  banUser(id: string): Promise<AdminUser>;
  /** Lift a ban — returns the updated user. */
  unbanUser(id: string): Promise<AdminUser>;
}

/** Fields the project PATCH endpoint accepts. Any subset; the server treats an
 *  empty `name`/`visibility` as "unchanged". */
export interface ProjectPatch {
  name?: string;
  document?: unknown;
  visibility?: Visibility;
}

export const platformApi: PlatformApi = {
  register: (input) =>
    request<User>("/api/auth/register", { method: "POST", body: input }),

  login: (input) =>
    request<User>("/api/auth/login", { method: "POST", body: input }),

  logout: () => request<void>("/api/auth/logout", { method: "POST" }),

  async getMe() {
    try {
      return await request<User>("/api/me", { method: "GET" });
    } catch (err) {
      // 401 is the expected "logged out" answer — not an error to surface.
      if (err instanceof ApiError && err.status === 401) return null;
      throw err;
    }
  },

  requestPasswordReset: (email) =>
    request<void>("/api/auth/password-reset-request", {
      method: "POST",
      body: { email },
    }),

  resetPassword: (token, newPassword) =>
    request<void>("/api/auth/password-reset", {
      method: "POST",
      body: { token, new_password: newPassword },
    }),

  listProjects: () => request<Project[]>("/api/projects", { method: "GET" }),

  createProject: (name, document) =>
    request<Project>("/api/projects", {
      method: "POST",
      body: { name, document },
    }),

  getProject: (id) =>
    request<Project>(`/api/projects/${encodeURIComponent(id)}`, {
      method: "GET",
    }),

  updateProject: (id, patch) =>
    request<Project>(`/api/projects/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: patch,
    }),

  deleteProject: (id) =>
    request<void>(`/api/projects/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),

  setVisibility: (id, visibility) =>
    request<Project>(`/api/projects/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: { visibility },
    }),

  publishProject: (id) =>
    request<PublishResult>(`/api/projects/${encodeURIComponent(id)}/publish`, {
      method: "POST",
    }),

  listGallery: () => request<Project[]>("/api/gallery", { method: "GET" }),

  listInvites: () => request<Invite[]>("/api/invites", { method: "GET" }),

  createInvite: (input) =>
    request<Invite>("/api/invites", {
      method: "POST",
      body: {
        invitee_name: input.invitee_name,
        invitee_email: input.invitee_email,
        message: input.message,
      },
    }),

  deleteInvite: (token) =>
    request<void>(`/api/invites/${encodeURIComponent(token)}`, {
      method: "DELETE",
    }),

  getInviteInfo: (token) =>
    request<InviteInfo>(`/api/invites/${encodeURIComponent(token)}/info`, {
      method: "GET",
    }),

  listTickets: (filter) =>
    request<TicketSummary[]>(`/api/tickets${ticketQuery(filter)}`, {
      method: "GET",
    }),

  getTicket: (id) =>
    request<TicketDetail>(`/api/tickets/${encodeURIComponent(id)}`, {
      method: "GET",
    }),

  createTicket: (input) =>
    request<TicketDetail>("/api/tickets", {
      method: "POST",
      body: {
        title: input.title,
        body: input.body,
        category: input.category ?? "",
      },
    }),

  updateTicket: (id, patch) =>
    request<TicketDetail>(`/api/tickets/${encodeURIComponent(id)}`, {
      method: "PATCH",
      body: patch,
    }),

  deleteTicket: (id) =>
    request<void>(`/api/tickets/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),

  listComments: (id) =>
    request<TicketComment[]>(
      `/api/tickets/${encodeURIComponent(id)}/comments`,
      { method: "GET" },
    ),

  addComment: (id, body) =>
    request<TicketComment>(`/api/tickets/${encodeURIComponent(id)}/comments`, {
      method: "POST",
      body: { body },
    }),

  async fetchArtifact(ticketId, artifactId) {
    const res = await fetch(artifactUrl(ticketId, artifactId), {
      credentials: "include",
    });
    if (!res.ok) {
      throw new ApiError(res.status, await extractError(res));
    }
    return res.arrayBuffer();
  },

  listAdminAlerts: (unseenOnly) =>
    request<AdminAlert[]>(`/api/admin/alerts${unseenOnly ? "?unseen=1" : ""}`, {
      method: "GET",
    }),

  markAdminAlertSeen: (id) =>
    request<void>(`/api/admin/alerts/${encodeURIComponent(id)}/seen`, {
      method: "POST",
    }),

  listAdminUsers: () =>
    request<AdminUser[]>("/api/admin/users", { method: "GET" }),

  banUser: (id) =>
    request<AdminUser>(`/api/admin/users/${encodeURIComponent(id)}/ban`, {
      method: "POST",
    }),

  unbanUser: (id) =>
    request<AdminUser>(`/api/admin/users/${encodeURIComponent(id)}/unban`, {
      method: "POST",
    }),
};

/** Build the `?author=&state=&category=&q=` query string, omitting empty
 *  fields so the backend treats them as "any". Returns "" for no filter. */
function ticketQuery(filter?: TicketListFilter): string {
  if (!filter) return "";
  const params = new URLSearchParams();
  if (filter.author) params.set("author", filter.author);
  if (filter.state) params.set("state", filter.state);
  if (filter.category) params.set("category", filter.category);
  if (filter.q && filter.q.trim()) params.set("q", filter.q.trim());
  const qs = params.toString();
  return qs ? `?${qs}` : "";
}

/** The download URL for a ticket artifact blob (served by
 *  `GET /api/tickets/{id}/artifacts/{artifactId}`). Use directly as an `<img>`
 *  src for screenshots, or via {@link PlatformApi.fetchArtifact} for bytes. */
export function artifactUrl(ticketId: string, artifactId: string): string {
  return `/api/tickets/${encodeURIComponent(ticketId)}/artifacts/${encodeURIComponent(
    artifactId,
  )}`;
}

/** The thumbnail URL for a project (served by `GET /api/projects/{id}/thumbnail`).
 *  Returns a 404 image when none exists — callers should handle `onError`. */
export function thumbnailUrl(projectId: string): string {
  return `/api/projects/${encodeURIComponent(projectId)}/thumbnail`;
}
