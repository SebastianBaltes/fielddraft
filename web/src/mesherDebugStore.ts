/**
 * Debug ▸ Mesher bridge.
 *
 * The "Debug ▸ Mesher" dialog is the scaffold for building a custom meshing
 * method step by step. Step 1 visualises a narrow-band adaptive octree: while
 * the dialog is open it publishes the octree parameters here, and the viewport
 * (`MesherOverlay`) samples the kernel octree and draws each leaf cell's centre
 * as a point colour-coded by its SDF value plus a short line along its gradient
 * (the surface normal). Closing the dialog clears it.
 *
 * Module-scoped singleton with a `useSyncExternalStore`-friendly
 * subscribe/getSnapshot pair (like `sdfPlaneStore`).
 */

export interface MesherDebug {
  /** Max adaptive octree depth (cells subdivide while in the surface band). */
  maxDepth: number;
  /** Band width factor: a cell subdivides while `|sdf| ≤ halfDiagonal · guard`. */
  bandGuard: number;
  /** Normal line length as a fraction of the cell size. */
  normalScale: number;
  /** Point size in pixels. */
  pointSize: number;
}

let params: MesherDebug | null = null;
const listeners = new Set<() => void>();

export function setMesherDebug(next: MesherDebug | null): void {
  params = next;
  for (const l of listeners) l();
}

export function getMesherDebug(): MesherDebug | null {
  return params;
}

export function subscribeMesherDebug(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// ── Stats readout ────────────────────────────────────────────────────
// The overlay publishes how many leaf cells the last octree build produced (and
// whether the cap was hit) so the dialog can show it.

export interface MesherStats {
  cells: number;
  capped: boolean;
}

let stats: MesherStats | null = null;
const statsListeners = new Set<() => void>();

export function setMesherStats(next: MesherStats | null): void {
  if (next === null && stats === null) return;
  stats = next;
  for (const l of statsListeners) l();
}

export function getMesherStats(): MesherStats | null {
  return stats;
}

export function subscribeMesherStats(listener: () => void): () => void {
  statsListeners.add(listener);
  return () => {
    statsListeners.delete(listener);
  };
}
