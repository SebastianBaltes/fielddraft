/**
 * Ticket presentation helpers — pure, framework-free mappings from the API
 * enums (server/models.go) to user-facing labels, badge classes and persona
 * styling. Kept separate from the React component so they are trivially unit-
 * testable and shared by the list + detail views.
 */

import type { AuthorType, TicketComment, TicketState } from "../api/platform";

/** Human-readable label for a ticket state (§7.2). */
export function stateLabel(state: TicketState): string {
  switch (state) {
    case "triage":
      return "Triage";
    case "needs_info":
      return "Needs info";
    case "accepted":
      return "Accepted";
    case "in_progress":
      return "In progress";
    case "deploying":
      return "Deploying";
    case "resolved":
      return "Resolved";
    case "closed":
      return "Closed";
    case "declined":
      return "Declined";
    case "backlog":
      return "Backlog";
    case "rejected_spam":
      return "Rejected (spam)";
    case "rejected_malicious":
      return "Rejected (malicious)";
  }
}

/** The short "#N" reference for a ticket (ticket 066230d9). */
export function ticketRef(number: number): string {
  return `#${number}`;
}

/** Every state of the §7.2 machine, in flow order. */
export const TICKET_STATES: TicketState[] = [
  "triage",
  "needs_info",
  "accepted",
  "in_progress",
  "deploying",
  "resolved",
  "closed",
  "declined",
  "backlog",
  "rejected_spam",
  "rejected_malicious",
];

/**
 * The states the viewer may move this ticket to — what the detail panel's state
 * picker offers (ticket #345). Mirrors the server (`userMaySetState` +
 * `handlePatchTicket`), which stays the authority:
 *
 *   · ADMIN — every state, directly, no matter where the ticket stands. The
 *     transition graph guards the automated user→PO→dev flow, not the operator
 *     driving it, and a direct set is the only way out of a dead-end state.
 *   · AUTHOR — the safe subset (ticket 066230d9): close, park to backlog,
 *     un-park, reopen a resolved one.
 *   · anyone else — nothing.
 *
 * The current state is never offered (a no-op the server rejects).
 */
export function settableStates(
  current: TicketState,
  opts: { isAdmin: boolean; isAuthor: boolean },
): TicketState[] {
  if (opts.isAdmin) return TICKET_STATES.filter((s) => s !== current);
  if (!opts.isAuthor) return [];
  const allowed: TicketState[] = [];
  // close: from a pre-work or resolved state (not mid-flow, not terminal).
  if (
    current === "triage" ||
    current === "needs_info" ||
    current === "declined" ||
    current === "backlog" ||
    current === "resolved"
  ) {
    allowed.push("closed");
  }
  // park a not-yet-worked ticket.
  if (current === "triage" || current === "needs_info") allowed.push("backlog");
  // un-park, or reopen a closed one → re-triage.
  if (current === "backlog" || current === "closed") allowed.push("triage");
  // reopen a resolved one — "the fix didn't stick".
  if (current === "resolved") allowed.push("in_progress");
  return TICKET_STATES.filter((s) => allowed.includes(s));
}

/** Coarse status bucket driving the badge color: open work, parked, done, or
 *  rejected. Used as a `state-<bucket>` modifier class on the badge. */
export type StateBucket = "open" | "progress" | "backlog" | "done" | "rejected";

export function stateBucket(state: TicketState): StateBucket {
  switch (state) {
    case "triage":
    case "needs_info":
    case "accepted":
    case "declined":
      return "open";
    case "in_progress":
    case "deploying":
      return "progress";
    case "backlog":
      return "backlog";
    case "resolved":
    case "closed":
      return "done";
    case "rejected_spam":
    case "rejected_malicious":
      return "rejected";
  }
}

/** Display label for a comment author persona (§7.4). */
export function personaLabel(author: AuthorType): string {
  switch (author) {
    case "user":
      return "User";
    case "po":
      return "Product Owner";
    case "dev":
      return "Developer";
    case "system":
      return "System";
  }
}

/** A short tag shown next to the avatar in the thread. */
export function personaTag(author: AuthorType): string {
  switch (author) {
    case "user":
      return "U";
    case "po":
      return "PO";
    case "dev":
      return "DEV";
    case "system":
      return "SYS";
  }
}

/** Whether a comment may render at all for the given viewer. The backend
 *  already filters internal `dev` notes + withheld comments for non-admins, but
 *  we guard again in the UI as defense-in-depth (§7.4). */
export function commentVisible(c: TicketComment, isAdmin: boolean): boolean {
  if (isAdmin) return true;
  return !c.internal && !c.withheld;
}

/** A user's display label for a category (or "" → "Uncategorized"). */
export function categoryLabel(category: string | null): string {
  switch (category) {
    case "bug":
      return "Bug";
    case "feature":
      return "Feature";
    case "question":
      return "Question";
    case "other":
      return "Other";
    default:
      return "Uncategorized";
  }
}

/** A short relative-ish date for list/thread meta. Falls back to "" on a bad
 *  ISO string. */
export function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** Date + time-of-day for list/thread meta ("11. Juli 2026, 20:34"). Falls
 *  back to "" on a bad ISO string. */
export function formatDateTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
