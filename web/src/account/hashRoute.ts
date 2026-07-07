/**
 * URL-hash routing (ticket 3032c29b) — Issues and Projects have URLs and the
 * browser Back button works.
 *
 * The route is a pure projection of the account state (page / overview tab /
 * open ticket) plus the bound server-project id:
 *
 *   #/edit           modeling view, unbound (local) project
 *   #/p/<id>         modeling view, bound server project <id> (deep-linkable)
 *   #/projects       Overview → Projects
 *   #/issues         Overview → Issues (list)
 *   #/issues/<id>    Overview → Issues (ticket detail, deep-linkable)
 *   #/invites        Overview → Invites (admin)
 *   #/users          Overview → Users (admin)
 *
 * App.tsx owns the two-way sync: state changes write the hash (history
 * entries → Back works), hashchange/boot parses and dispatches. Both
 * directions are idempotent, so the write→hashchange→dispatch round trip
 * settles immediately.
 */

import type { OverviewTab } from "./accountState";

export type Route =
  | { page: "project"; projectId: string | null }
  | { page: "overview"; tab: OverviewTab; ticketId: string | null };

export function routeToHash(r: Route): string {
  if (r.page === "project") {
    return r.projectId
      ? `#/p/${encodeURIComponent(r.projectId)}`
      : "#/edit";
  }
  switch (r.tab) {
    case "projects":
      return "#/projects";
    case "issues":
      return r.ticketId
        ? `#/issues/${encodeURIComponent(r.ticketId)}`
        : "#/issues";
    case "invites":
      return "#/invites";
    case "users":
      return "#/users";
  }
}

/** Parse a location.hash; `null` = no/unknown route (leave state alone). */
export function parseHash(hash: string): Route | null {
  const h = hash.replace(/^#?\/?/, "").replace(/\/+$/, "");
  if (h === "") return null;
  const seg = h.split("/").map(decodeURIComponent);
  switch (seg[0]) {
    case "edit":
      return { page: "project", projectId: null };
    case "p":
      return seg[1] ? { page: "project", projectId: seg[1] } : null;
    case "projects":
      return { page: "overview", tab: "projects", ticketId: null };
    case "issues":
      return { page: "overview", tab: "issues", ticketId: seg[1] || null };
    case "invites":
      return { page: "overview", tab: "invites", ticketId: null };
    case "users":
      return { page: "overview", tab: "users", ticketId: null };
    default:
      return null;
  }
}
