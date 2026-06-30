/**
 * Account session state — a small, pure reducer that drives the WP3 auth UI.
 *
 * It is deliberately decoupled from the kernel store: guest/localStorage mode
 * stays the app's default, and the account layer is an ADDITIVE overlay on top.
 * The reducer tracks three things:
 *   1. the session lifecycle (`loading` until the boot `/api/me` resolves,
 *      then `guest` or `authed`),
 *   2. the current account user (when authed), and
 *   3. which account screen, if any, is open (`overlay`).
 *
 * The reducer is synchronous and side-effect-free so it is trivially testable;
 * the async API calls live in the `useAccount` hook (accountHook.ts), which
 * dispatches into this reducer.
 */

import type { User } from "../api/platform";

/** Session lifecycle. `loading` is the pre-boot state before `/api/me`. */
export type SessionStatus = "loading" | "guest" | "authed";

/** Which account screen is open. `null` = the modeling app (no overlay). */
export type AccountOverlay =
  | null
  | "login"
  | "register"
  | "reset-request"
  | "reset-confirm"
  | "dashboard"
  | "gallery"
  | "invites"
  | "tickets";

export interface AccountState {
  status: SessionStatus;
  /** The logged-in user, or `null` in guest/loading. */
  user: User | null;
  overlay: AccountOverlay;
  /** Invite token prefilled into the register form (from `?invite=`). */
  invitePrefill: string;
  /** Reset token prefilled into the reset-confirm form (from `?reset=`). */
  resetPrefill: string;
  /** Ticket id to deep-link into when the tickets overlay opens (e.g. straight
   *  from a filed report), or null to open the list. One-shot. */
  ticketPrefill: string | null;
}

export type AccountAction =
  /** Boot resolved: either a user (authed) or null (guest). */
  | { type: "session-resolved"; user: User | null }
  /** Login / register succeeded. */
  | { type: "authed"; user: User }
  /** Logout succeeded. */
  | { type: "logged-out" }
  /** Open a specific account screen. */
  | { type: "open"; overlay: AccountOverlay }
  /** Open the tickets overlay deep-linked to a specific ticket (or null for
   *  the list). */
  | { type: "open-ticket"; ticketId: string | null }
  /** Close the current overlay (back to the modeling app). */
  | { type: "close" };

/** Build the initial state, reading any `?invite=` / `?reset=` deep-link from
 *  the URL search string. A `?invite=` link auto-opens the register screen; a
 *  `?reset=` link auto-opens the reset-confirm screen. */
export function initialAccountState(search = ""): AccountState {
  let invitePrefill = "";
  let resetPrefill = "";
  try {
    const params = new URLSearchParams(search);
    invitePrefill = params.get("invite") ?? "";
    resetPrefill = params.get("reset") ?? "";
  } catch {
    // Malformed search — ignore, no prefill.
  }
  // Pick the deep-link overlay to open once the session resolves. A reset link
  // wins over an invite link (you're confirming a reset, not registering).
  let overlay: AccountOverlay = null;
  if (resetPrefill) overlay = "reset-confirm";
  else if (invitePrefill) overlay = "register";
  return {
    status: "loading",
    user: null,
    overlay,
    invitePrefill,
    resetPrefill,
    ticketPrefill: null,
  };
}

export function accountReducer(
  state: AccountState,
  action: AccountAction,
): AccountState {
  switch (action.type) {
    case "session-resolved":
      return {
        ...state,
        status: action.user ? "authed" : "guest",
        user: action.user,
        // A logged-in user landing on the login/register screen is bounced to
        // the dashboard; otherwise keep any deep-link overlay (invite/reset).
        overlay:
          action.user &&
          (state.overlay === "login" || state.overlay === "register")
            ? "dashboard"
            : state.overlay,
      };
    case "authed":
      return {
        ...state,
        status: "authed",
        user: action.user,
        overlay: "dashboard",
        // Burn the one-shot deep-link prefills once consumed.
        invitePrefill: "",
        resetPrefill: "",
      };
    case "logged-out":
      return {
        ...state,
        status: "guest",
        user: null,
        overlay: null,
        ticketPrefill: null,
      };
    case "open":
      // Opening any non-tickets screen clears a stale ticket deep-link.
      return {
        ...state,
        overlay: action.overlay,
        ticketPrefill:
          action.overlay === "tickets" ? state.ticketPrefill : null,
      };
    case "open-ticket":
      return { ...state, overlay: "tickets", ticketPrefill: action.ticketId };
    case "close":
      return { ...state, overlay: null, ticketPrefill: null };
    default:
      return state;
  }
}

/** True when the user holds the admin role (gates the invite panel). */
export function isAdmin(state: AccountState): boolean {
  return state.user?.role === "admin";
}
