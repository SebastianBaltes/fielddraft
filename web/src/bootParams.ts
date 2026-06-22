// Zero-dependency reader for the ClaudeDraft "open ticket" deep-link.
//
// ClaudeDraft's file browser opens a `*.fdraft` by launching this webapp at
// `<webapp_url>/?project=<repo-root-relative-path>` (URL-encoded). This module
// extracts that param. It deliberately imports NOTHING so both `store.ts` (to
// pick an inert autosave for the repro tab) and `bootProject.ts` (to fetch +
// load) can read it without an import cycle.

/** Extract the `project` query param from a URL search string (decoded by
 *  URLSearchParams), or null if absent/malformed. */
export function parseProjectParam(search: string): string | null {
  try {
    return new URLSearchParams(search).get("project");
  } catch {
    return null;
  }
}

/** The `?project=` deep-link path from the current URL, or null when the app
 *  was opened normally. Repo-root-relative, e.g.
 *  "tickets/open/<id>/project.fdraft". */
export function bootProjectPath(): string | null {
  if (typeof window === "undefined") return null;
  return parseProjectParam(window.location.search);
}

/** Folder of a repo-relative file path ("a/b/c.fdraft" → "a/b"; no slash → ""). */
export function ticketDir(path: string): string {
  const i = path.lastIndexOf("/");
  return i > 0 ? path.slice(0, i) : "";
}

/** Same-origin dev-server read endpoint (web/vite.config.ts `fileReader`) for a
 *  repo-root-relative path; the value is URL-encoded so slashes survive. */
export function fileApiUrl(repoRelPath: string): string {
  return `/api/file?path=${encodeURIComponent(repoRelPath)}`;
}
