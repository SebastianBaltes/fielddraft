/**
 * Onshape document URL parsing (Onshape-Import Schritt 4, UI).
 *
 * An Onshape browser URL carries the three ids the REST proxy needs:
 *
 *   https://cad.onshape.com/documents/<did>/w/<wid>/e/<eid>
 *   https://cad.onshape.com/d/<did>/w/<wid>/e/<eid>          (share links)
 *
 * All ids are 24-hex-digit strings. The element id (`/e/…`) is optional —
 * the import dialog falls back to the document's first Part Studio when it
 * is missing.
 */

export interface OnshapeDocRef {
  documentId: string;
  workspaceId: string;
  /** `null` when the URL stops after the workspace — caller picks the
   *  first PARTSTUDIO element via the elements endpoint. */
  elementId: string | null;
}

const URL_RE =
  /\/(?:documents|d)\/([0-9a-f]{24})\/w\/([0-9a-f]{24})(?:\/e\/([0-9a-f]{24}))?/i;

/** Extract document / workspace / element ids from an Onshape URL (or any
 *  string containing the path). Returns `null` when no match. */
export function parseOnshapeUrl(input: string): OnshapeDocRef | null {
  const m = URL_RE.exec(input.trim());
  if (!m) return null;
  return {
    documentId: m[1],
    workspaceId: m[2],
    elementId: m[3] ?? null,
  };
}

/**
 * What an imported project remembers about its Onshape origin (ticket #343).
 * It rides in the project document envelope (`Project.onshape`), so it survives
 * save/load, export and cloud sync without a schema change — and it is what the
 * project card's "Open in Onshape" and "Reimport" act on. The element id is
 * always resolved here (the import picks the first Part Studio when the pasted
 * URL had none), so a reimport hits the same Part Studio the user imported.
 */
export interface OnshapeSource {
  documentId: string;
  workspaceId: string;
  elementId: string;
  /** The document name at import time — the project title (e.g. "Lötkolbenhalter"). */
  documentName?: string;
  /** When the project was last (re)imported, ISO-8601. */
  importedAt?: string;
}

/** The canonical Onshape URL of a source — what "Open in Onshape" navigates to. */
export function onshapeSourceUrl(src: OnshapeSource): string {
  return `https://cad.onshape.com/documents/${src.documentId}/w/${src.workspaceId}/e/${src.elementId}`;
}
