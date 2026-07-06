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
  /** Screen-space stroke width (px) of the normal lines; the centre dot is 3×. */
  strokeWidth: number;
  /** |sdf| threshold (mm) at/below which a sample is coloured green (surface).
   *  Beyond it the colour ramps to blue (inside) / red (outside). */
  surfaceThreshold: number;
  /** Opacity of the rendered part (1 = opaque). Below 1 the solid turns
   *  see-through so the interior octree samples/normals are visible. */
  partOpacity: number;
  /** Only show leaf cells within `nearSurfaceDist` of the surface. Also
   *  processing filter A of the jq2t point pipeline. */
  nearSurfaceOnly: boolean;
  /** |sdf| threshold (mm) for `nearSurfaceOnly`. */
  nearSurfaceDist: number;

  // ── jq2t: iterative point optimisation ──
  /** Processing filter B: only cells whose field sign flips towards a direct
   *  neighbour. ANDed with filter A when both are on. */
  signChangeNeighboursOnly: boolean;
  /** Topological protection: clamp every point update to its leaf cell. */
  constrainToCell: boolean;
  /** Feature 1: damped Newton projection onto the SDF zero set. */
  fitOnSurface: boolean;
  fitMaxIterations: number;
  fitTargetEpsilon: number;
  /** Newton damping, 0.1–1.0. */
  fitStepMultiplier: number;
  /** Feature 2: tangential repulsion relaxation (with surface re-pinning). */
  repulsion: boolean;
  repulsionIterations: number;
  interactionRadius: number;
  repulsionStrength: number;
  weightingFunction: MesherWeighting;
}

export type MesherWeighting = "linear" | "inverse-square";

let params: MesherDebug | null = null;
const listeners = new Set<() => void>();

export function setMesherDebug(next: MesherDebug | null): void {
  params = next;
  for (const l of listeners) l();
}

export function getMesherDebug(): MesherDebug | null {
  return params;
}

/** Global part-opacity multiplier for the body renderers (SDF + WebGL): the
 *  Mesher dialog's `partOpacity` while it is open, else 1 (fully opaque). Read
 *  imperatively each frame by the material-apply path — no subscription. */
export function getMesherPartOpacity(): number {
  return params ? params.partOpacity : 1;
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
