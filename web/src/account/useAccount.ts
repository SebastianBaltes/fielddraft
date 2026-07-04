/**
 * useAccount — the React hook that owns the account session.
 *
 * On mount it calls `GET /api/me` once to resolve the session, then exposes
 * the reducer state plus async actions (login / register / logout / open
 * screens). Every async action funnels through {@link platformApi}; the
 * resulting user (or error) is dispatched into {@link accountReducer}.
 *
 * The hook is intentionally store-agnostic: it never touches the kernel store.
 * App.tsx wires the two together (e.g. "open project" loads a server document
 * into the existing modeling store), keeping guest mode fully intact.
 */

import { useCallback, useEffect, useReducer, useState } from "react";
import {
  ApiError,
  platformApi,
  type PlatformApi,
  type User,
} from "../api/platform";
import {
  accountReducer,
  initialAccountState,
  isAdmin,
  type AccountState,
  type AuthScreenMode,
  type OverviewTab,
} from "./accountState";

export interface UseAccount {
  state: AccountState;
  isAdmin: boolean;
  /** Go to the Overview, optionally selecting a tab. */
  showOverview(tab?: OverviewTab): void;
  /** Select an Overview tab. */
  selectTab(tab: OverviewTab): void;
  /** Enter the modeling view (call after a project is loaded into the kernel). */
  enterProject(): void;
  /** Open the Issues tab, optionally deep-linked to a ticket id. */
  openTicket(ticketId?: string | null): void;
  /** Open a standalone auth screen (login / register / reset). */
  openAuth(mode: Exclude<AuthScreenMode, null>): void;
  /** Close the standalone auth screen. */
  closeAuth(): void;
  /** Open / close the admin alerts (bell) panel. */
  setBell(open: boolean): void;
  login(email: string, password: string): Promise<void>;
  register(input: {
    email: string;
    displayName: string;
    password: string;
    invite: string;
  }): Promise<void>;
  logout(): Promise<void>;
  /** Re-fetch `/api/me` (e.g. after an external session change). */
  refresh(): Promise<void>;
  /** Set the unseen-admin-alert badge count (driven by the alerts view). */
  setUnseenAlerts(count: number): void;
  /** Re-fetch the unseen-alert count for admins (no-op for non-admins). */
  refreshAlertCount(): Promise<void>;
}

export interface UseAccountOptions {
  /** API implementation. Defaults to the real {@link platformApi}; tests pass
   *  a fake. */
  api?: PlatformApi;
  /** URL search string for invite/reset deep-link prefills. Defaults to
   *  `window.location.search`. */
  search?: string;
}

export function useAccount(options: UseAccountOptions = {}): UseAccount {
  const api = options.api ?? platformApi;
  const search =
    options.search ??
    (typeof window !== "undefined" ? window.location.search : "");

  const [state, dispatch] = useReducer(
    accountReducer,
    search,
    initialAccountState,
  );

  const resolveSession = useCallback(
    async (signal?: { cancelled: boolean }) => {
      let user: User | null = null;
      try {
        user = await api.getMe();
      } catch {
        // A non-401 failure (server down, network) → treat as guest; the
        // modeling app keeps working offline.
        user = null;
      }
      if (signal?.cancelled) return;
      // Ticket oyx8: dev auto-login. When the `vite dev` boot resolved to no
      // session, sign in as the seeded dev admin so the account features
      // (Overview / Projects / Issues) are reachable without a manual login.
      // Gated hard so it can only ever fire on a developer's own `vite dev`
      // browser:
      //   · `MODE === "development"` excludes the production bundle
      //     (MODE "production", where it's dead-code-eliminated) AND the vitest
      //     suite (MODE "test"),
      //   · `!navigator.webdriver` excludes the Playwright e2e (which also runs
      //     `vite dev`, but every automation driver sets navigator.webdriver).
      // Best-effort: a failure (backend down, wrong creds) leaves guest mode and
      // the offline modeling app intact. Defaults match the `just dev-full` seed.
      if (
        !user &&
        import.meta.env.MODE === "development" &&
        typeof navigator !== "undefined" &&
        !navigator.webdriver
      ) {
        try {
          user = await api.login({
            email: import.meta.env.VITE_DEV_AUTOLOGIN_EMAIL || "admin@local",
            password: import.meta.env.VITE_DEV_AUTOLOGIN_PASSWORD || "admin",
          });
        } catch {
          user = null; // keep guest — no dev backend, or wrong creds
        }
        if (signal?.cancelled) return;
      }
      dispatch({ type: "session-resolved", user });
      // Prime the unseen-alert badge for admins; failures are silently ignored
      // (the badge is a convenience, never a gate).
      if (user?.role === "admin") {
        try {
          const unseen = await api.listAdminAlerts(true);
          if (!signal?.cancelled) {
            dispatch({ type: "unseen-alerts", count: unseen.length });
          }
        } catch {
          // ignore — leave the badge at 0
        }
      }
    },
    [api],
  );

  // Boot: resolve the session exactly once.
  useEffect(() => {
    const signal = { cancelled: false };
    void resolveSession(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [resolveSession]);

  const showOverview = useCallback((tab?: OverviewTab) => {
    dispatch({ type: "show-overview", tab });
  }, []);

  const selectTab = useCallback((tab: OverviewTab) => {
    dispatch({ type: "select-tab", tab });
  }, []);

  const enterProject = useCallback(() => {
    dispatch({ type: "enter-project" });
  }, []);

  const openTicket = useCallback((ticketId: string | null = null) => {
    dispatch({ type: "open-ticket", ticketId });
  }, []);

  const openAuth = useCallback((mode: Exclude<AuthScreenMode, null>) => {
    dispatch({ type: "open-auth", mode });
  }, []);

  const closeAuth = useCallback(() => {
    dispatch({ type: "close-auth" });
  }, []);

  const setBell = useCallback((open: boolean) => {
    dispatch({ type: "set-bell", open });
  }, []);

  const login = useCallback(
    async (email: string, password: string) => {
      const user = await api.login({ email, password });
      dispatch({ type: "authed", user });
    },
    [api],
  );

  const register = useCallback(
    async (input: {
      email: string;
      displayName: string;
      password: string;
      invite: string;
    }) => {
      const user = await api.register({
        email: input.email,
        display_name: input.displayName,
        password: input.password,
        invite: input.invite,
      });
      dispatch({ type: "authed", user });
    },
    [api],
  );

  const logout = useCallback(async () => {
    try {
      await api.logout();
    } catch {
      // A failed server logout (already-expired session, offline) is harmless —
      // we still drop the local session below. Swallow rather than rethrow so
      // callers (e.g. the account menu) don't see an unhandled rejection.
    }
    dispatch({ type: "logged-out" });
  }, [api]);

  const refresh = useCallback(async () => {
    await resolveSession();
  }, [resolveSession]);

  const setUnseenAlerts = useCallback((count: number) => {
    dispatch({ type: "unseen-alerts", count });
  }, []);

  const refreshAlertCount = useCallback(async () => {
    if (!isAdmin(state)) return;
    try {
      const unseen = await api.listAdminAlerts(true);
      dispatch({ type: "unseen-alerts", count: unseen.length });
    } catch {
      // ignore — keep the previous badge value
    }
  }, [api, state]);

  return {
    state,
    isAdmin: isAdmin(state),
    showOverview,
    selectTab,
    enterProject,
    openTicket,
    openAuth,
    closeAuth,
    setBell,
    login,
    register,
    logout,
    refresh,
    setUnseenAlerts,
    refreshAlertCount,
  };
}

/** Narrow an unknown thrown value to a user-facing message. Used by the auth
 *  forms to surface API errors inline. */
export function accountErrorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return err.message;
  if (err instanceof Error && err.message) return err.message;
  return fallback;
}

/** Hook state for an async form submit: pending flag + inline error. Tiny
 *  helper so each auth screen doesn't re-implement it. */
export function useAsyncSubmit(): {
  pending: boolean;
  error: string | null;
  run(fn: () => Promise<void>, fallback: string): Promise<void>;
  setError(msg: string | null): void;
} {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async (fn: () => Promise<void>, fallback: string) => {
    setPending(true);
    setError(null);
    try {
      await fn();
    } catch (err) {
      setError(accountErrorMessage(err, fallback));
    } finally {
      setPending(false);
    }
  }, []);
  return { pending, error, run, setError };
}
