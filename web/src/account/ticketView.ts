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
