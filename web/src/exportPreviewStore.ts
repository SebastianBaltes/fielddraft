/**
 * Export-preview bridge.
 *
 * While the (non-modal) export dialog is open and a mesh has been
 * built, it publishes the merged export triangles here. The viewport
 * then suppresses the normal SDF/WebGL body pass and renders this exact
 * mesh as a flat-shaded solid instead — so the user sees precisely the
 * geometry that will land in the STL/3MF, and can orbit around it with
 * the dialog still open. Closing the dialog clears the preview and the
 * original renderer takes over again.
 *
 * Like the query-picker bridge, this is a plain module-scoped singleton
 * with a `useSyncExternalStore`-friendly subscribe/getSnapshot pair —
 * there is only ever one export preview at a time, and both the dialog
 * (a plain React component) and the renderer components (inside R3F's
 * Canvas boundary) need cheap synchronous access without threading
 * context across that boundary.
 */

export interface ExportPreview {
  /** xyz-interleaved vertex positions of the merged export mesh. */
  positions: Float32Array;
  /** Triangle indices into `positions`. */
  indices: Uint32Array;
}

let preview: ExportPreview | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** Publish the export preview (dialog meshed a body). Replaces any
 *  previous preview. */
export function setExportPreview(next: ExportPreview | null): void {
  preview = next;
  notify();
}

/** Current export preview, or `null` when the renderer should draw the
 *  normal body. */
export function getExportPreview(): ExportPreview | null {
  return preview;
}

/** Subscribe to preview changes. Returns unsubscribe. */
export function subscribeExportPreview(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
