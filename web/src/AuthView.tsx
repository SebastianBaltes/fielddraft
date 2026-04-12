/**
 * M11: Login / Register / Logout views.
 *
 * A minimal auth UI that talks to the Go backend at /api/auth/*.
 * When the user is authenticated, the children (the main App
 * shell) render. When not, a login/register form is shown.
 */

import { useEffect, useState, type ReactNode } from "react";

interface AuthUser {
  id: string;
  email: string;
  display_name: string;
}

interface AuthContextValue {
  user: AuthUser | null;
  logout: () => Promise<void>;
}

const API_BASE = "/api";

async function fetchJson<T>(
  url: string,
  opts?: RequestInit,
): Promise<{ ok: boolean; status: number; data?: T; error?: string }> {
  try {
    const res = await fetch(API_BASE + url, {
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      ...opts,
    });
    const text = await res.text();
    let data: T | undefined;
    try {
      data = JSON.parse(text) as T;
    } catch {
      // not JSON
    }
    if (!res.ok) {
      return {
        ok: false,
        status: res.status,
        error: (data as { error?: string } | undefined)?.error ?? text,
      };
    }
    return { ok: true, status: res.status, data };
  } catch (err) {
    return { ok: false, status: 0, error: String(err) };
  }
}

export function AuthGate({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<AuthUser | null>(null);
  const [loading, setLoading] = useState(true);
  const [mode, setMode] = useState<"login" | "register">("login");
  const [error, setError] = useState<string | null>(null);

  // Check session on mount.
  useEffect(() => {
    fetchJson<AuthUser>("/auth/me", { method: "GET" }).then((res) => {
      if (res.ok && res.data) {
        setUser(res.data);
      }
      setLoading(false);
    });
  }, []);

  const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    setError(null);
    const form = new FormData(e.currentTarget);
    const email = form.get("email") as string;
    const password = form.get("password") as string;

    if (mode === "register") {
      const displayName = form.get("display_name") as string;
      const res = await fetchJson<AuthUser>("/auth/register", {
        method: "POST",
        body: JSON.stringify({ email, password, display_name: displayName }),
      });
      if (res.ok && res.data) {
        setUser(res.data);
      } else {
        setError(res.error ?? "Registration failed");
      }
    } else {
      const res = await fetchJson<AuthUser>("/auth/login", {
        method: "POST",
        body: JSON.stringify({ email, password }),
      });
      if (res.ok && res.data) {
        setUser(res.data);
      } else {
        setError(res.error ?? "Login failed");
      }
    }
  };

  const logout = async () => {
    await fetchJson("/auth/logout", { method: "POST" });
    setUser(null);
  };

  if (loading) {
    return <div className="fd-auth-loading">Loading…</div>;
  }

  // Not authenticated → show login/register form.
  if (!user) {
    return (
      <div className="fd-auth">
        <div className="fd-auth-panel">
          <h1>FieldDraft</h1>
          <div className="fd-auth-tabs">
            <button
              type="button"
              className={mode === "login" ? "active" : ""}
              onClick={() => { setMode("login"); setError(null); }}
            >
              Login
            </button>
            <button
              type="button"
              className={mode === "register" ? "active" : ""}
              onClick={() => { setMode("register"); setError(null); }}
            >
              Register
            </button>
          </div>
          <form onSubmit={handleSubmit}>
            {mode === "register" ? (
              <input
                name="display_name"
                placeholder="Display name"
                autoComplete="name"
              />
            ) : null}
            <input
              name="email"
              type="email"
              placeholder="Email"
              required
              autoComplete="email"
            />
            <input
              name="password"
              type="password"
              placeholder="Password"
              required
              autoComplete={mode === "register" ? "new-password" : "current-password"}
            />
            <button type="submit">
              {mode === "login" ? "Log in" : "Create account"}
            </button>
          </form>
          {error ? <div className="fd-auth-error">{error}</div> : null}
          <div className="fd-auth-skip">
            <button
              type="button"
              onClick={() => setUser({ id: "guest", email: "guest", display_name: "Guest" })}
            >
              Continue as guest (local only)
            </button>
          </div>
        </div>
      </div>
    );
  }

  // Authenticated → render children with auth context available.
  return (
    <>
      {children}
      <AuthBar user={user} onLogout={logout} />
    </>
  );
}

/** Tiny bar at the bottom showing the current user + logout. */
function AuthBar({ user, onLogout }: { user: AuthUser; onLogout: () => void }) {
  if (user.id === "guest") return null;
  return (
    <div className="fd-auth-bar">
      <span>{user.display_name || user.email}</span>
      <button type="button" onClick={onLogout}>
        Logout
      </button>
    </div>
  );
}

export type { AuthUser, AuthContextValue };
