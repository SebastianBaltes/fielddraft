/**
 * F21 / jcb7: "Project" / "Use" external geometry into the 2D sketch.
 *
 * Existing bodies' B-Rep edges are projected onto the active sketch plane and
 * drawn as faint reference guides (see `projectEdgesToSketchPlane` in
 * SketchCanvas2d). The Project tool turns a clicked guide into an ASSOCIATIVE
 * `sketch_project` sub-call: it stores the *query* naming the source edge
 * (`edge: qEdge(hash)`), not a frozen coordinate snapshot. On every regen the
 * kernel re-resolves that edge against the live B-Rep, so the projected line
 * FOLLOWS the body when an upstream parameter changes (ticket jcb7's "Sofort
 * Assoziativ!"). Projected geometry is CONSTRUCTION by CAD convention: visible,
 * snappable, constrainable, but excluded from the extrude profile.
 *
 * Two pure, DOM-free concerns live here:
 *  1. The pick: given projected edge segments (sketch-plane mm) and a click
 *     point, the nearest segment index within tolerance (`nearestProjectedEdge`).
 *  2. The DISPLAY follow (`resolveProjectedSubCalls`): committed sketches render
 *     in JS from their FROZEN document sub-calls, which would freeze the
 *     projection visually even though the kernel follows. This resolver rewrites
 *     each `sketch_project` sub-call to a live `sketch_line` by re-projecting the
 *     CURRENT body edge onto the sketch frame — the same re-resolution the kernel
 *     does in `SketchFeature::execute`, mirrored so the overlay tracks the body.
 */

import type { PlaneFrame } from "./sketchPlaneFrame";
import type { Vec2 } from "./sketchTrim";
import type { SketchSubCall } from "./sketchOutline";

export interface ProjectedEdgeSeg {
  p0: Vec2;
  p1: Vec2;
}

/** Shortest distance from point `p` to the segment `a`–`b`. A degenerate
 *  (zero-length) segment collapses to the point-to-point distance. */
export function pointToSegmentDist(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 1e-12) return Math.hypot(p[0] - a[0], p[1] - a[1]);
  let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / lenSq;
  t = Math.max(0, Math.min(1, t));
  const cx = a[0] + t * dx;
  const cy = a[1] + t * dy;
  return Math.hypot(p[0] - cx, p[1] - cy);
}

/**
 * Index of the projected edge whose segment is closest to `click`, provided
 * that closest distance is within `tolMm`; otherwise null.
 *
 * Ties (two segments exactly equidistant) resolve deterministically to the
 * FIRST one in array order — a strict `<` comparison never replaces an
 * equal-distance incumbent.
 */
export function nearestProjectedEdge(
  edges: ProjectedEdgeSeg[],
  click: Vec2,
  tolMm: number,
): number | null {
  let bestIdx: number | null = null;
  let bestDist = Infinity;
  for (let i = 0; i < edges.length; i++) {
    const d = pointToSegmentDist(click, edges[i].p0, edges[i].p1);
    if (d < bestDist) {
      bestDist = d;
      bestIdx = i;
    }
  }
  if (bestIdx === null || bestDist > tolMm) return null;
  return bestIdx;
}

// ─── jcb7: associative display follow ──────────────────────────────────────

/** Extract the decimal id-hash from a `qEdge(<hash>)` query string, or null
 *  when the string isn't a single-edge query. Whitespace-tolerant. */
export function parseQEdgeIdHash(query: unknown): string | null {
  if (typeof query !== "string") return null;
  const m = query.trim().match(/^qEdge\(\s*(\d+)\s*\)$/);
  return m ? m[1] : null;
}

/** Orthogonally project a world point onto a plane frame's `(u, v)` plane —
 *  the JS mirror of the kernel's `SketchFrame::world_to_local_2d`. */
function worldToLocal2d(p: readonly number[], frame: PlaneFrame): Vec2 {
  const dx = p[0] - frame.origin[0];
  const dy = p[1] - frame.origin[1];
  const dz = p[2] - frame.origin[2];
  return [
    dx * frame.xAxis[0] + dy * frame.xAxis[1] + dz * frame.xAxis[2],
    dx * frame.yAxis[0] + dy * frame.yAxis[1] + dz * frame.yAxis[2],
  ];
}

/** A lookup from a B-Rep edge id-hash to its CURRENT world-space polyline
 *  (flat `[x,y,z, …]`), or null when the edge no longer resolves. Backed by
 *  `kernel.bodyBrepEdges` at the call site. */
export type LiveEdgeLookup = (idHash: string) => number[] | null;

/**
 * Rewrite every `sketch_project` sub-call to a live `sketch_line` so the
 * committed/session overlays render the projection at the body's CURRENT
 * position (ticket jcb7). For each projection:
 *   - parse its `edge: qEdge(hash)` query,
 *   - look the edge up live and project its chord (first→last sample) onto
 *     `frame`,
 *   - emit a `sketch_line { start, end, construction }` carrying the live
 *     coordinates.
 * An edge that no longer resolves falls back to the sub-call's cached
 * `start`/`end` (so it stays put rather than vanishing); a projection with
 * neither a resolvable edge nor cached endpoints is dropped. Every other
 * sub-call passes through untouched, so the downstream collectors
 * (`collectSketchLineSegments`, `collectSketchSessionGeometry`, …) need no
 * knowledge of projection at all.
 */
export function resolveProjectedSubCalls(
  subCalls: readonly SketchSubCall[],
  frame: PlaneFrame,
  lookup: LiveEdgeLookup,
): SketchSubCall[] {
  const getVec2 = (v: unknown): Vec2 | null =>
    Array.isArray(v) &&
    v.length >= 2 &&
    typeof v[0] === "number" &&
    typeof v[1] === "number"
      ? [v[0], v[1]]
      : null;

  const out: SketchSubCall[] = [];
  for (const call of subCalls) {
    if (call.type !== "sketch_project") {
      out.push(call);
      continue;
    }
    const construction = call.params.construction !== false; // default true
    let start: Vec2 | null = null;
    let end: Vec2 | null = null;

    const idHash = parseQEdgeIdHash(call.params.edge);
    if (idHash !== null) {
      const poly = lookup(idHash);
      if (poly && poly.length >= 6) {
        const n = poly.length / 3;
        start = worldToLocal2d(
          [poly[0], poly[1], poly[2]],
          frame,
        );
        end = worldToLocal2d(
          [poly[(n - 1) * 3], poly[(n - 1) * 3 + 1], poly[(n - 1) * 3 + 2]],
          frame,
        );
      }
    }
    // Fallback to the cached endpoints when the live edge is gone.
    if (!start || !end) {
      start = getVec2(call.params.start);
      end = getVec2(call.params.end);
    }
    if (!start || !end) continue; // unresolvable → drop

    out.push({
      type: "sketch_line",
      id: call.id,
      params: { start, end, construction },
    });
  }
  return out;
}

/**
 * Build a {@link LiveEdgeLookup} over every body's current B-Rep edges. The
 * returned function maps an edge id-hash to its world-space polyline by
 * scanning `kernel.bodyBrepEdges` across all bodies (memoised on first use so a
 * sketch with several projections pays the scan once).
 */
export function makeLiveEdgeLookup(kernel: {
  bodyCount(): number;
  bodyBrepEdges(i: number): Array<{ points: number[]; idHash?: string }>;
}): LiveEdgeLookup {
  let table: Map<string, number[]> | null = null;
  const build = (): Map<string, number[]> => {
    const m = new Map<string, number[]>();
    let count = 0;
    try {
      count = kernel.bodyCount();
    } catch {
      count = 0;
    }
    for (let i = 0; i < count; i++) {
      let edges: Array<{ points: number[]; idHash?: string }>;
      try {
        edges = kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      for (const e of edges) {
        if (e.idHash) m.set(e.idHash, e.points);
      }
    }
    return m;
  };
  return (idHash: string) => {
    if (!table) table = build();
    return table.get(idHash) ?? null;
  };
}
