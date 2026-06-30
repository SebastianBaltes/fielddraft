/**
 * Guest → account project import (WP9 B4).
 *
 * When a guest who has done real local work logs in (or registers) for the
 * first time, we offer — non-intrusively — to import their localStorage guest
 * project as a new server project. This module holds the *pure* decision logic
 * so it is trivially testable; the React surface ({@link GuestImportBanner})
 * and the API call live elsewhere.
 *
 * The rule: offer the import iff
 *   1. the user just became authenticated, AND
 *   2. the live workspace is NOT already bound to a server project
 *      (`boundProjectId === null` — a bound workspace is already on the server),
 *      AND
 *   3. the guest document carries real content (not a pristine empty project).
 *
 * Declining must NOT lose the guest project: we never clear localStorage here.
 * The banner simply dismisses, and guest mode keeps working when logged out.
 */

/** Inspect a serialized guest Project document and decide whether it holds any
 *  real user work worth importing. A pristine default project (one empty part
 *  tab, no features) returns `false`; anything with features, scripts,
 *  assemblies, or extra tabs returns `true`. Robust to malformed input — an
 *  unparseable document is treated as "no content" (nothing to import). */
export function guestDocumentHasContent(document: unknown): boolean {
  let doc: unknown = document;
  if (typeof doc === "string") {
    try {
      doc = JSON.parse(doc);
    } catch {
      return false;
    }
  }
  if (!doc || typeof doc !== "object") return false;
  const tabs = (doc as { tabs?: unknown }).tabs;
  if (!Array.isArray(tabs) || tabs.length === 0) return false;
  // More than one tab is itself a signal of real work.
  if (tabs.length > 1) return true;
  for (const tab of tabs) {
    if (tabHasContent(tab)) return true;
  }
  return false;
}

/** True when a single project tab carries user work. */
function tabHasContent(tab: unknown): boolean {
  if (!tab || typeof tab !== "object") return false;
  const t = tab as Record<string, unknown>;
  switch (t.kind) {
    case "part": {
      // The part's document JSON holds `feature_calls`; a non-empty list (or
      // any other unexpected payload) counts as content.
      const json = t.documentJson;
      if (typeof json !== "string") return false;
      try {
        const parsed = JSON.parse(json) as { feature_calls?: unknown };
        return (
          Array.isArray(parsed.feature_calls) && parsed.feature_calls.length > 0
        );
      } catch {
        return false;
      }
    }
    case "script":
      // A script tab is itself authored content.
      return true;
    case "assembly": {
      // An assembly with any part instances is real work.
      const document = t.document as { parts?: unknown } | undefined;
      return (
        !!document && Array.isArray(document.parts) && document.parts.length > 0
      );
    }
    default:
      return false;
  }
}

/** The full decision for offering the guest-import prompt. */
export function shouldOfferGuestImport(args: {
  /** True once the session has resolved to an authenticated user. */
  authed: boolean;
  /** The server project id the workspace is bound to, or null when unbound. */
  boundProjectId: string | null;
  /** The serialized guest document (string or parsed), or null when none. */
  guestDocument: unknown | null;
}): boolean {
  if (!args.authed) return false;
  if (args.boundProjectId !== null) return false;
  if (args.guestDocument == null) return false;
  return guestDocumentHasContent(args.guestDocument);
}
