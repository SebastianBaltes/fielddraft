/**
 * F14: Interactive mirror for the 2D sketcher.
 *
 * Pure, DOM-free reflection of sketch geometry across an infinite axis
 * line. The user selects source entities, activates the Mirror tool and
 * clicks an axis line; the store reflects every selected simple-geometry
 * sub-call (line / circle / arc) and appends ordinary
 * `sketch_line` / `sketch_circle` / `sketch_arc` sub-calls — exactly the
 * primitives the kernel already understands. No kernel mirror tool is
 * involved: the result is just more geometry (like trim / fillet).
 *
 * Reflection of a point P across the infinite line through A with unit
 * direction d:
 *
 *   P' = 2·(A + ((P − A)·d) d) − P
 *
 * (project P onto the axis, then reflect P about that foot.)
 *
 * Lines reflect both endpoints. Circles reflect the centre, radius
 * unchanged. Arcs reflect the centre AND the start/end POINTS, then
 * recompute the start angle from the reflected start point and FLIP THE
 * SWEEP SIGN — a reflection reverses orientation, so a CCW arc becomes CW
 * and vice versa. The reflected arc therefore passes through the
 * reflections of the original arc's start and end points.
 */

import type { Curve, Vec2 } from "./sketchTrim";

/** Squared length of the axis below which it's treated as degenerate. */
const AXIS_EPS_SQ = 1e-18;

/**
 * Reflect a point across the infinite line through `axisA` and `axisB`.
 *
 * Throws on a degenerate axis (A == B) — there's no line to mirror
 * across, so a silent no-op would hide a caller bug. Callers that pick an
 * axis line from solved geometry never hit this (a zero-length line isn't
 * a valid axis); the unit test pins the behaviour.
 */
export function reflectPoint(p: Vec2, axisA: Vec2, axisB: Vec2): Vec2 {
  const dx = axisB[0] - axisA[0];
  const dy = axisB[1] - axisA[1];
  const lenSq = dx * dx + dy * dy;
  if (lenSq < AXIS_EPS_SQ) {
    throw new Error("reflectPoint: degenerate axis (A == B)");
  }
  // Projection of (P − A) onto the axis direction, as a fraction of the
  // axis vector — works with the un-normalised direction directly.
  const t = ((p[0] - axisA[0]) * dx + (p[1] - axisA[1]) * dy) / lenSq;
  const footX = axisA[0] + t * dx;
  const footY = axisA[1] + t * dy;
  // Reflection: P' = 2·foot − P.
  return [2 * footX - p[0], 2 * footY - p[1]];
}

/** Point on a circle/arc at parametric angle `theta`. */
function pointAt(c: Vec2, r: number, theta: number): Vec2 {
  return [c[0] + r * Math.cos(theta), c[1] + r * Math.sin(theta)];
}

/**
 * Reflect a curve across the infinite axis line through `axisA`/`axisB`.
 *
 * Geometry per kind:
 *  - line:   reflect both endpoints.
 *  - circle: reflect the centre, keep the radius.
 *  - arc:    reflect the centre and the start/end POINTS; the new start
 *            angle is atan2 of (reflectedStart − reflectedCentre) and the
 *            sweep sign flips (reflection reverses orientation), so the
 *            reflected arc spans between the reflected start and end.
 *
 * The radius is invariant under reflection (an isometry), so it carries
 * across unchanged for circles and arcs.
 */
export function mirrorCurve(cv: Curve, axisA: Vec2, axisB: Vec2): Curve {
  if (cv.kind === "line") {
    return {
      kind: "line",
      a: reflectPoint(cv.a, axisA, axisB),
      b: reflectPoint(cv.b, axisA, axisB),
    };
  }
  if (cv.kind === "circle") {
    return {
      kind: "circle",
      c: reflectPoint(cv.c, axisA, axisB),
      r: cv.r,
    };
  }
  // arc: reflect the centre and the start point, recompute the start
  // angle from the reflected start, and flip the sweep sign. With the
  // sweep magnitude preserved, the arc's END then lands exactly on the
  // reflection of the original end (a reflection is an isometry); the
  // unit test pins that by sampling both endpoints.
  const startPt = pointAt(cv.c, cv.r, cv.start);
  const rc = reflectPoint(cv.c, axisA, axisB);
  const rStart = reflectPoint(startPt, axisA, axisB);
  const newStart = Math.atan2(rStart[1] - rc[1], rStart[0] - rc[0]);
  return {
    kind: "arc",
    c: rc,
    r: cv.r,
    start: newStart,
    // Orientation reverses under reflection: a +sweep becomes −sweep.
    sweep: -cv.sweep,
  };
}
