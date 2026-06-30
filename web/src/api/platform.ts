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

/** An admin-issued, single-use registration invite. Mirrors `inviteResponse`. */
export interface Invite {
  token: string;
  created_by: string;
  note: string;
  expires_at: string;
  used_by: string | null;
  used_at: string | null;
  created_at: string;
  /** "open" | "used" | "expired" — derived server-side. */
  status: string;
  /** `/register?invite=<token>` — the copyable link. */
  register_url: string;
}

/** The result of publishing a project to a shareable snapshot. */
export interface PublishResult {
  snapshot_id: string;
  view_url: string;
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
  createInvite(note?: string, expiresAt?: string): Promise<Invite>;
  deleteInvite(token: string): Promise<void>;
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

  createInvite: (note, expiresAt) =>
    request<Invite>("/api/invites", {
      method: "POST",
      body: { note: note ?? "", expires_at: expiresAt ?? "" },
    }),

  deleteInvite: (token) =>
    request<void>(`/api/invites/${encodeURIComponent(token)}`, {
      method: "DELETE",
    }),
};

/** The thumbnail URL for a project (served by `GET /api/projects/{id}/thumbnail`).
 *  Returns a 404 image when none exists — callers should handle `onError`. */
export function thumbnailUrl(projectId: string): string {
  return `/api/projects/${encodeURIComponent(projectId)}/thumbnail`;
}
