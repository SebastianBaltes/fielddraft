/**
 * Debug ▸ Mesher Lichen bridge.
 *
 * The "Debug ▸ Mesher: Lichen" dialog drives the new Lichen debug mesher.
 * While the dialog is open it publishes its parameters here, and the viewport
 * (`LichenOverlay`) asks the kernel (`lichenMesh`) for the same flat overlay
 * format the PASM mesher emits and draws the surface particles / edge chains /
 * corners.
 *
 * A SEPARATE store from `mesherDebugStore` (its own singleton + overlay) so the
 * Lichen dialog never fights the PASM dialog over the shared mesher store.
 *
 * Module-scoped singleton with a `useSyncExternalStore`-friendly
 * subscribe/getSnapshot pair (like `mesherDebugStore`).
 */

export interface LichenDebug {
  // ── Lichen kernel params (camelCase — read by `Kernel::lichenMesh`) ──
  /** Target edge length h (mm) — THE resolution parameter. */
  targetEdgeLength: number;
  /** Waben-Wachstum: grow the particle set outward instead of seeding it all. */
  grow: boolean;
  /** Max. Normalen-Knickwinkel pro Kante in Grad — kleiner = dichter auf
   *  gekrümmten Flächen/Kanten. Kernel rechnet intern in Radiant um. */
  curvAngleDeg: number;
  /** Use the local reach (outward footpoint tracking) to bound U(p). */
  useReach: boolean;
  /** Footpoint departure threshold (mm) for the reach march. */
  reachTol: number;
  /** Untere Grenze des Punktabstands auf Kanten (mm) — deckelt die
   *  winkelgetriebene Verdichtung auf sehr kleinen Kreisen; geschlossene
   *  Kanten behalten kernel-seitig ≥4 Punkte. */
  minEdgeSpacing: number;
  /** Hard cap on the particle count (safety valve). */
  maxPoints: number;
  /** ALLGEMEINE (chartlose) Triangulierung über Stern-Konsistenz statt
   *  Chart-CDT — ein Code für plan + gekrümmt; Diagnose-first. */
  generalTri: boolean;
  /** Akzeptanz-Bedingung des General-Pfads: Stern-Konsens ("vote"),
   *  Umkugel/Gabriel mit deterministischem Tie-Break ("gabriel"), oder
   *  beides ("hybrid"). */
  accept: "vote" | "gabriel" | "hybrid";

  // ── Display knobs (overlay only, ignored by the kernel) ──
  /** Opacity of the rendered part (1 = opaque). Below 1 the solid turns
   *  see-through so the interior particles are visible. */
  partOpacity: number;
  /** Screen-space stroke width (px) of the wireframe; the centre dot is 3×. */
  strokeWidth: number;
  /** |sdf| threshold (mm) at/below which a sample is coloured green (surface). */
  surfaceThreshold: number;
  /** Normal line length as a fraction of the particle size (0 = no needles). */
  normalScale: number;
  /** Draw the U(p) reach circle (radius = per-point U) in the tangent plane. */
  showCircles: boolean;
  /** Draw the point markers (corner/edge/surface, colour-coded). */
  showPoints: boolean;
  /** Draw the triangle mesh wireframe (once the triangulation stage emits it). */
  showTriangles: boolean;
  /** Edge length (mm) of the point-marker boxes. */
  pointSize: number;
  /** Draw a needle along each point's ∇f normal (to inspect orientation). */
  showNormals: boolean;
  /** Length (mm) of the normal needles. */
  normalLength: number;
}

let params: LichenDebug | null = null;
const listeners = new Set<() => void>();

export function setLichenDebug(next: LichenDebug | null): void {
  params = next;
  for (const l of listeners) l();
}

export function getLichenDebug(): LichenDebug | null {
  return params;
}

/** Global part-opacity multiplier while the Lichen dialog is open, else 1
 *  (fully opaque). Read imperatively each frame by the material-apply path. */
export function getLichenPartOpacity(): number {
  return params ? params.partOpacity : 1;
}

export function subscribeLichenDebug(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// ── Stats readout ────────────────────────────────────────────────────
// The overlay publishes how many particles / triangles the last Lichen build
// produced (and whether the cap was hit) so the dialog can show it.

export interface LichenStats {
  cells: number;
  capped: boolean;
  /** Triangles of the debug mesh (0 while meshing is not emitted yet). */
  tris: number;
  /** Of `tris`, how many came from the fallback path — drawn orange. */
  fallbackTris: number;
  /** General-Pfad-Diagnose: offene Loch-Kanten (rot gezeichnet). */
  holeEdges: number;
  /** General-Pfad-Diagnose: abgelehnte Fast-Konsens-Kandidaten (orange). */
  weakTris: number;
  /** Qualitäts-Block (beide Pfade, kernel-berechnet über das finale Netz):
   *  Kanten mit genau einem Dreieck (offener Rand). */
  boundaryEdges: number;
  /** Kanten mit >2 Dreiecken (0 = manifold). */
  nonmanifoldEdges: number;
  /** Dreiecke mit ~Null-Fläche oder doppelten Ecken. */
  degenerateTris: number;
  /** Zusammenhangskomponenten. */
  components: number;
  /** Euler-Charakteristik V−E+F (2 je geschlossener Kugel-Komponente). */
  euler: number;
  /** Mehrfach gleichgerichtete Kanten (0 = konsistent orientierbar). */
  orientationViolations: number;
}

let stats: LichenStats | null = null;
const statsListeners = new Set<() => void>();

export function setLichenStats(next: LichenStats | null): void {
  if (next === null && stats === null) return;
  stats = next;
  for (const l of statsListeners) l();
}

export function getLichenStats(): LichenStats | null {
  return stats;
}

export function subscribeLichenStats(listener: () => void): () => void {
  statsListeners.add(listener);
  return () => {
    statsListeners.delete(listener);
  };
}
