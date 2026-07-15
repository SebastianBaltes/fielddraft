/**
 * Ticket presentation helpers — pure, framework-free mappings from the API
 * enums (server/models.go) to user-facing labels, badge classes and persona
 * styling. Kept separate from the React component so they are trivially unit-
 * testable and shared by the list + detail views.
 */

import type {
  AuthorType,
  TicketComment,
  TicketState,
  TicketSummary,
} from "../api/platform";

/** Human-readable label for a ticket state (the six-value enum). */
export function stateLabel(state: TicketState): string {
  switch (state) {
    case "triage":
      return "Triage";
    case "needs_info":
      return "Needs info";
    case "in_progress":
      return "In progress";
    case "parked":
      return "Parked";
    case "closed":
      return "Closed";
    case "rejected":
      return "Rejected";
  }
}

/** The short "#N" reference for a ticket (ticket 066230d9). */
export function ticketRef(number: number): string {
  return `#${number}`;
}

/** The list's Author column label (ticket #365): the resolved human name the
 *  server sends alongside each row, same resolution path as comment authors
 *  (#359 → commentAuthorLabel). Falls back to "Unknown" for a deleted user. */
export function ticketAuthorLabel(t: Pick<TicketSummary, "author_name">): string {
  return t.author_name?.trim() || "Unknown";
}

/** Every state of the machine, in flow order (the six-value enum). */
export const TICKET_STATES: TicketState[] = [
  "triage",
  "needs_info",
  "in_progress",
  "parked",
  "closed",
  "rejected",
];

/**
 * The states the viewer may move this ticket to — what the detail panel's state
 * picker offers (ticket #345). Mirrors the server (`userMaySetState` +
 * `handlePatchTicket`), which stays the authority:
 *
 *   · ADMIN — every state, directly, no matter where the ticket stands. The
 *     transition graph guards the automated user→PO→dev flow, not the operator
 *     driving it, and a direct set is the only way out of a dead-end state.
 *   · AUTHOR — the safe subset (ticket 066230d9): close, park, un-park, reopen.
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
  // close: from anywhere that isn't already terminal.
  if (
    current === "triage" ||
    current === "needs_info" ||
    current === "in_progress" ||
    current === "parked"
  ) {
    allowed.push("closed");
  }
  // park a not-yet-finished ticket.
  if (current === "triage" || current === "needs_info") allowed.push("parked");
  // un-park (parked→triage) or reopen a closed one → re-triage.
  if (current === "parked" || current === "closed") allowed.push("triage");
  // reopen a closed one — "the fix didn't stick".
  if (current === "closed") allowed.push("in_progress");
  return TICKET_STATES.filter((s) => allowed.includes(s));
}

/** Coarse status bucket driving the badge color: open work, parked, done, or
 *  rejected. Used as a `state-<bucket>` modifier class on the badge. */
export type StateBucket = "open" | "progress" | "backlog" | "done" | "rejected";

export function stateBucket(state: TicketState): StateBucket {
  switch (state) {
    case "triage":
    case "needs_info":
      return "open";
    case "in_progress":
      return "progress";
    case "parked":
      return "backlog";
    case "closed":
      return "done";
    case "rejected":
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

type AuthoredComment = Pick<TicketComment, "author_type" | "author_name">;

/** Initials for an avatar tag: "Sebastian Baltes" → "SB", "sorokan" → "SO". */
function initials(name: string): string {
  const parts = name.split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

/** The name shown for a comment's author (#359): the resolved human name when
 *  present, else the generic persona label ("User" / "System" / …). */
export function commentAuthorLabel(c: AuthoredComment): string {
  return c.author_name?.trim() || personaLabel(c.author_type);
}

/** The avatar tag for a comment (#359): initials of the human name when present,
 *  else the persona tag (U / PO / DEV / SYS). */
export function commentAuthorTag(c: AuthoredComment): string {
  const name = c.author_name?.trim();
  return name ? initials(name) : personaTag(c.author_type);
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
