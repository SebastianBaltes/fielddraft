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
  /** Iterations of the single relaxation loop (and the final fit snap's max
   *  budget). One loop now — not two separate counts. */
  iterations: number;
  /** Fit on Surface: damped Newton projection onto the SDF zero set. */
  fitOnSurface: boolean;
  /** Drop points that did not actually REACH the surface after the fit
   *  (clamped at their cell bounds): removed from the point set, the
   *  repulsion and the mesh. */
  onlyOnSurface: boolean;
  fitTargetEpsilon: number;
  /** Newton damping, 0.1–1.0. */
  fitStepMultiplier: number;
  /** Repulsion inside surface: tangential relaxation, annealed. */
  repulsion: boolean;
  interactionRadius: number;
  /** Repulsion strength at the first iteration; the pairwise force follows an
   *  inverse-square law (∝ 1/d², like equally-charged monopoles). */
  repulsionStrength: number;
  /** Annealing: fraction of repulsion strength lost per iteration
   *  (0 = constant, 1 = gone after one linear step). */
  repulsionDamping: number;
  /** Annealing curve of the repulsion strength over the iterations. */
  dampingCurve: MesherDampingCurve;
  /** Hybrid-PASM P2: PASM Gaussian pair kernel (σ = interactionRadius/5,
   *  paper-treue 5σ-Trunkierung) statt 1/d²-Monopol. */
  pasmKernel: boolean;
  /** Hybrid-PASM P3+P4: Faces per Constrained Delaunay im 2D-Chart
   *  vernetzen — planar in der Face-Ebene, Zylinder/Kegel/Kugel-Bänder
   *  abgewickelt (Naht über geteilte Steiner-Punkte verschweißt);
   *  Skelett-Loops als Constraints, freie Partikel als Innenpunkte;
   *  wasserdicht über geteilte Ketten-Slots. */
  cdtFaces: boolean;
  /** Hybrid-PASM P1: sample the woven B-Rep edges into FIXED skeleton
   *  particles (corners pinned, chains at the interaction radius) that repel
   *  the free particles — sharp features as boundary conditions. */
  brepSkeleton: boolean;
  /** Feature snapping: detect edge/corner creases via normal probes, snap
   *  points onto them and lock them there (edge = 1D slide, corner = pinned)
   *  — isotropic repulsion alone never lands points on 0D/1D features. */
  featureSnap: boolean;
  /** Normals differing by more than this angle (°) count as distinct sheets. */
  featureAngleDeg: number;
  /** Mesh the shown (filtered) points: connect them along the octree's edge
   *  relationships into triangles (one vertex per leaf, a quad per minimal
   *  octree edge whose 4 cells are all shown). NOT dual contouring — no
   *  sign-change test; drawn as a wireframe. */
  meshPoints: boolean;
  /** 2:1-balance the octree before meshing (bounded neighbour depth
   *  contrast → more even triangle sizes). */
  balanceOctree: boolean;
  /** Mesher-PASM single resolution parameter: target edge length h (mm).
   *  When > 0 the KERNEL derives `maxDepth` (seed leaf size just below h)
   *  and overrides `interactionRadius = h` (= 5σ kernel truncation). Only
   *  published by the dedicated Debug ▸ Mesher PASM dialog. */
  targetEdgeLength?: number;
  /** Hybrid-PASM §6 F3: mesh chartless faces (Torus/Gyroid carriers,
   *  tessellated sheets, fold-guard failures) as signed-DC octree patches
   *  zipped to the exact skeleton rim chains. Needs brepSkeleton+cdtFaces. */
  fallbackPatches?: boolean;
  /** Hybrid-PASM P4d-2: curvature-anisotropic relaxation (metric-warped
   *  Gaussian force, paper Gl. 21). Denser where the surface curves, coarse
   *  where flat — fewer triangles at equal sag, at a higher mesh cost. */
  anisotropy?: boolean;
  /** Sag tolerance ε (mm) driving the anisotropic refinement. */
  anisoSag?: number;
  /** Max anisotropic stretch factor s_max. */
  anisoSmax?: number;
}

export type MesherDampingCurve = "linear" | "exponential";

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
  /** Triangles of the debug dual-contour mesh (0 when meshing is off). */
  tris: number;
  /** Of `tris`, how many came from the §6-F3 fallback (DC patch + zipper
   *  on chartless faces) — drawn orange by the overlay. */
  fallbackTris: number;
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
