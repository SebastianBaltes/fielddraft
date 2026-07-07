/**
 * Account session state — a small, pure reducer that drives the account UI.
 *
 * WP13 redesign: navigation is now two-level, not an overlay stack.
 *   1. the session lifecycle (`loading` until the boot `/api/me` resolves,
 *      then `guest` or `authed`),
 *   2. the current account user (when authed),
 *   3. the top-level `page` — the **Overview** (a tabbed browser: Projects /
 *      Issues / Invites / Users) or the **modeling** view (a single project
 *      loaded into the kernel). After login the default landing is the
 *      Overview; opening a project enters the modeling view; the topbar `◀`
 *      returns to the Overview.
 *   4. which Overview tab is active (`overviewTab`),
 *   5. a standalone `auth` screen (login / register / reset) shown when a guest
 *      needs to authenticate — orthogonal to the two pages,
 *   6. the admin `bell` panel (alerts) toggled from the topbar bell.
 *
 * The reducer is synchronous and side-effect-free so it is trivially testable;
 * the async API calls live in the `useAccount` hook, which dispatches here.
 */

import type { User } from "../api/platform";

/** Session lifecycle. `loading` is the pre-boot state before `/api/me`. */
export type SessionStatus = "loading" | "guest" | "authed";

/** The top-level page. `overview` = the tabbed browser (the main level);
 *  `project` = the modeling view (a project loaded into the kernel). */
export type AccountPage = "overview" | "project";

/** The Overview's active tab. Invites & Users render only for admins. */
export type OverviewTab = "projects" | "issues" | "invites" | "users";

/** A standalone auth screen, or `null` when none is open. Orthogonal to the
 *  page — a guest can open login/register over either page. */
export type AuthScreenMode =
  null | "login" | "register" | "reset-request" | "reset-confirm";

export interface AccountState {
  status: SessionStatus;
  /** The logged-in user, or `null` in guest/loading. */
  user: User | null;
  /** The top-level page (Overview vs. modeling). */
  page: AccountPage;
  /** The Overview's active tab. */
  overviewTab: OverviewTab;
  /** A standalone auth screen open over the current page, or null. */
  auth: AuthScreenMode;
  /** Whether the admin alerts (bell) panel is open. */
  bellOpen: boolean;
  /** Invite token prefilled into the register form (from `?invite=`). */
  invitePrefill: string;
  /** Reset token prefilled into the reset-confirm form (from `?reset=`). */
  resetPrefill: string;
  /** The Issues tab's open ticket (detail view), or null for the list.
   *  Drives BOTH the controlled TicketsView and the URL hash (#/issues/<id>,
   *  ticket 3032c29b) — cleared when leaving the Issues tab. */
  ticketDetail: string | null;
  /** Count of unseen admin alerts, surfaced as a badge on the topbar bell.
   *  Always 0 for non-admins (never fetched). */
  unseenAlertCount: number;
}

export type AccountAction =
  /** Boot resolved: either a user (authed) or null (guest). */
  | { type: "session-resolved"; user: User | null }
  /** Login / register succeeded. */
  | { type: "authed"; user: User }
  /** Logout succeeded. */
  | { type: "logged-out" }
  /** Go to the Overview, optionally selecting a tab (default: keep current). */
  | { type: "show-overview"; tab?: OverviewTab }
  /** Select an Overview tab (implicitly on the Overview page). */
  | { type: "select-tab"; tab: OverviewTab }
  /** Enter the modeling view (a project was opened). */
  | { type: "enter-project" }
  /** Open the Issues tab deep-linked to a specific ticket (or null for the
   *  list). */
  | { type: "open-ticket"; ticketId: string | null }
  /** Open a standalone auth screen (login/register/reset). */
  | { type: "open-auth"; mode: Exclude<AuthScreenMode, null> }
  /** Close the standalone auth screen. */
  | { type: "close-auth" }
  /** Toggle / set the admin alerts (bell) panel. */
  | { type: "set-bell"; open: boolean }
  /** Update the unseen admin-alert badge count. */
  | { type: "unseen-alerts"; count: number };

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
  // Pick the deep-link auth screen to open once the session resolves. A reset
  // link wins over an invite link (you're confirming a reset, not registering).
  let auth: AuthScreenMode = null;
  if (resetPrefill) auth = "reset-confirm";
  else if (invitePrefill) auth = "register";
  return {
    status: "loading",
    user: null,
    // Default to the modeling view: a guest (login gate off) boots straight
    // into the offline modeling app, and every ungated e2e / dev flow lands on
    // the toolbar as before. Authenticating flips the landing to the Overview
    // (see the `session-resolved` / `authed` cases).
    page: "project",
    overviewTab: "projects",
    auth,
    bellOpen: false,
    invitePrefill,
    resetPrefill,
    ticketDetail: null,
    unseenAlertCount: 0,
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
        // An authenticated boot lands on the Overview (the main level); a guest
        // stays in the modeling view (offline mode). Don't clobber an explicit
        // deep-link that already moved us (e.g. ?ticket= via open-ticket).
        page: action.user ? "overview" : state.page,
        // A logged-in user landing on a login/register screen has it dismissed;
        // otherwise keep any deep-link auth screen (invite/reset).
        auth:
          action.user && (state.auth === "login" || state.auth === "register")
            ? null
            : state.auth,
      };
    case "authed":
      return {
        ...state,
        status: "authed",
        user: action.user,
        // Land on the Overview after auth.
        page: "overview",
        overviewTab: "projects",
        auth: null,
        // Burn the one-shot deep-link prefills once consumed.
        invitePrefill: "",
        resetPrefill: "",
      };
    case "logged-out":
      return {
        ...state,
        status: "guest",
        user: null,
        page: "overview",
        overviewTab: "projects",
        auth: null,
        bellOpen: false,
        ticketDetail: null,
        unseenAlertCount: 0,
      };
    case "show-overview":
      return {
        ...state,
        page: "overview",
        overviewTab: action.tab ?? state.overviewTab,
        // Leaving for the Overview clears a stale ticket deep-link unless we're
        // explicitly (re)opening the Issues tab.
        ticketDetail:
          (action.tab ?? state.overviewTab) === "issues"
            ? state.ticketDetail
            : null,
      };
    case "select-tab":
      return {
        ...state,
        page: "overview",
        overviewTab: action.tab,
        ticketDetail: action.tab === "issues" ? state.ticketDetail : null,
      };
    case "enter-project":
      return { ...state, page: "project", ticketDetail: null };
    case "open-ticket":
      return {
        ...state,
        page: "overview",
        overviewTab: "issues",
        ticketDetail: action.ticketId,
      };
    case "open-auth":
      return { ...state, auth: action.mode };
    case "close-auth":
      return { ...state, auth: null };
    case "set-bell":
      return { ...state, bellOpen: action.open };
    case "unseen-alerts":
      return { ...state, unseenAlertCount: Math.max(0, action.count) };
    default:
      return state;
  }
}

/** True when the user holds the admin role (gates invites / users / alerts). */
export function isAdmin(state: AccountState): boolean {
  return state.user?.role === "admin";
}
