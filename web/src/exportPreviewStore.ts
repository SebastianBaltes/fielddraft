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
/** Crossfade: mesh-solid opacity (1 = mesh only, 0 = SDF body only). */
let opacity = 1;
/** Overlay the mesh triangle wireframe. */
let wireframe = false;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

/** Publish the export preview (dialog meshed a body). Replaces any
 *  previous preview; passing `null` clears it and resets the display
 *  controls to their defaults. */
export function setExportPreview(next: ExportPreview | null): void {
  preview = next;
  if (next === null) {
    opacity = 1;
    wireframe = false;
  }
  notify();
}

/** Set the crossfade opacity of the mesh solid over the SDF body. */
export function setExportPreviewOpacity(value: number): void {
  opacity = Math.min(1, Math.max(0, value));
  notify();
}

/** Toggle the mesh wireframe overlay. */
export function setExportPreviewWireframe(value: boolean): void {
  wireframe = value;
  notify();
}

/** Current export preview, or `null` when the renderer should draw the
 *  normal body. */
export function getExportPreview(): ExportPreview | null {
  return preview;
}

export function getExportPreviewOpacity(): number {
  return opacity;
}

export function getExportPreviewWireframe(): boolean {
  return wireframe;
}

/** Subscribe to any preview change (mesh, opacity or wireframe). */
export function subscribeExportPreview(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
