/**
 * Invite self-registration — the pure decision layer.
 *
 * A `?invite=<token>` link must land the visitor on a self-registration form
 * with the invitee's email/name prefilled, **regardless of whether they are
 * already logged in** (Ticket A). Today an authed visitor sees the app and the
 * invite link does nothing; an authed visitor who registers gets a fresh
 * session cookie and so BECOMES the new user.
 *
 * `decideInviteRegister` maps the (auth-status, fetched invite-info) inputs to a
 * pure view descriptor the {@link InviteRegister} component renders. It is
 * side-effect-free so the branching is trivially testable in isolation.
 */

import type { InviteInfo } from "../api/platform";
import type { SessionStatus } from "./accountState";

/** The three terminal states of the invite screen. */
export type InviteRegisterView =
  /** Still fetching the invite info / resolving the session. */
  | { kind: "loading" }
  /** Token missing / used / expired — no form, just an explanatory message. */
  | { kind: "invalid" }
  /** Show the register form with prefills, plus an optional authed-user note. */
  | {
      kind: "register";
      emailPrefill: string;
      namePrefill: string;
      /** Non-null when the visitor is already authenticated: a note telling them
       *  that registering creates a NEW account (a new session cookie). */
      note: string | null;
    };

export interface InviteRegisterInputs {
  /** The session lifecycle from the account reducer. */
  status: SessionStatus;
  /** The fetched invite info, or `null` while still loading. */
  info: InviteInfo | null;
  /** The already-authenticated user's email, if any (for the note). */
  authedEmail: string | null;
}

/** The note shown to an already-logged-in visitor following an invite link. */
export function authedInviteNote(email: string): string {
  return `Angemeldet als ${email} — mit der Registrierung erstellst du ein neues Konto.`;
}

/** Decide what the invite-registration screen renders from the current inputs.
 *  Pure: no fetches, no dispatch. */
export function decideInviteRegister(
  inputs: InviteRegisterInputs,
): InviteRegisterView {
  const { status, info, authedEmail } = inputs;
  // Wait for both the session boot and the invite-info fetch to resolve.
  if (status === "loading" || info === null) {
    return { kind: "loading" };
  }
  if (!info.valid) {
    return { kind: "invalid" };
  }
  const note =
    status === "authed" && authedEmail ? authedInviteNote(authedEmail) : null;
  return {
    kind: "register",
    emailPrefill: info.invitee_email ?? "",
    namePrefill: info.invitee_name ?? "",
    note,
  };
}
