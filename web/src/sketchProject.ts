/**
 * F21: "Project" / "Use" external geometry into the 2D sketch.
 *
 * Existing bodies' B-Rep edges are already projected onto the active sketch
 * plane and drawn as faint reference guides (see
 * `projectEdgesToSketchPlane` in SketchCanvas2d). They are inert, though —
 * the constraint solver can't reference a guide that has no sub-call behind
 * it. The Project tool turns a clicked guide into a real CONSTRUCTION
 * `sketch_line`, matching the CAD convention where "projected reference"
 * geometry is construction by default (visible, snappable, constrainable,
 * but excluded from the extrude profile).
 *
 * This module is the pure, DOM-free pick: given the projected edge segments
 * (already in sketch-plane mm) and a click point, return the index of the
 * nearest segment within tolerance, or null. The canvas layer turns that
 * index into the emitted `sketch_line` sub-call.
 */

import type { Vec2 } from "./sketchTrim";

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
