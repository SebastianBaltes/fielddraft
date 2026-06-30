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
  type AccountOverlay,
  type AccountState,
} from "./accountState";

export interface UseAccount {
  state: AccountState;
  isAdmin: boolean;
  open(overlay: AccountOverlay): void;
  /** Open the tickets overlay, optionally deep-linked to a ticket id. */
  openTicket(ticketId?: string | null): void;
  close(): void;
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
      if (!signal?.cancelled) {
        dispatch({ type: "session-resolved", user });
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

  const open = useCallback((overlay: AccountOverlay) => {
    dispatch({ type: "open", overlay });
  }, []);

  const openTicket = useCallback((ticketId: string | null = null) => {
    dispatch({ type: "open-ticket", ticketId });
  }, []);

  const close = useCallback(() => {
    dispatch({ type: "close" });
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

  return {
    state,
    isAdmin: isAdmin(state),
    open,
    openTicket,
    close,
    login,
    register,
    logout,
    refresh,
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
