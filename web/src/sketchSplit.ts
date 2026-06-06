/**
 * Interactive split for the 2D sketcher (F13).
 *
 * "Split" is the keep-both sibling of Trim ([`./sketchTrim`]): the user
 * clicks a curve at a point and the curve is replaced by TWO sub-curves
 * that meet at the click point — nothing is deleted. Like Trim it's a
 * pure, DOM-free sub-call rewrite that emits ordinary `sketch_line` /
 * `sketch_arc` primitives the kernel already understands; the store
 * splices the replacement in via [`replaceSketchSubCallAt`] (rewriting
 * downstream constraint refs) and re-solves. The kernel needs no split
 * tool.
 *
 * Scope (v1): simple `sketch_line` and `sketch_arc` only. A full
 * `sketch_circle` can't be split at a single click — one point on a
 * closed loop leaves a single 360° arc, which is no split at all — so we
 * return null for circles and document it here. Composites and every
 * other kind return null too (the caller treats null as "do nothing").
 */

import type { Curve, Vec2 } from "./sketchTrim";

/** Parametric tolerance: a click within this of an endpoint isn't a
 *  meaningful split (it would leave a zero-length piece). Matches the
 *  endpoint tolerance used throughout [`./sketchTrim`]. */
const EPS = 1e-7;
const TAU = Math.PI * 2;

const sub = (p: Vec2, q: Vec2): Vec2 => [p[0] - q[0], p[1] - q[1]];
const dot = (p: Vec2, q: Vec2): number => p[0] * q[0] + p[1] * q[1];
const len2 = (p: Vec2): number => p[0] * p[0] + p[1] * p[1];

function pointOnLine(a: Vec2, b: Vec2, t: number): Vec2 {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
}

/** Wrap a value into [0, TAU). */
function norm2pi(a: number): number {
  let x = a % TAU;
  if (x < 0) x += TAU;
  return x;
}

/**
 * Normalized parameter s ∈ [0, 1] of `theta` along an arc's sweep (0 =
 * start, 1 = end), or null when `theta` falls outside the span. Handles
 * both sweep directions and the ±π seam — same convention as
 * `arcParam` in [`./sketchTrim`].
 */
function arcParam(start: number, sweep: number, theta: number): number | null {
  const span = Math.abs(sweep);
  const travelled = sweep >= 0 ? norm2pi(theta - start) : norm2pi(start - theta);
  if (travelled <= span + EPS) return Math.min(1, travelled / span);
  return null;
}

/**
 * Split a curve into two sub-curves meeting at `clickPoint`, or null when
 * no meaningful split exists.
 *
 * - **line**: project `clickPoint` onto the segment to get t ∈ (0, 1),
 *   the closest point on the segment. A click within `EPS` of either
 *   endpoint (t≈0 or t≈1) → null. Split point P = a + t·(b−a). Returns
 *   `[ {line a→P}, {line P→b} ]`. A click off the line still splits at
 *   the projection (the closest point on the segment).
 * - **arc**: find the angle θ of `clickPoint` about the arc centre and
 *   its param s ∈ (0, 1) along the sweep. s within `EPS` of 0/1 → null.
 *   Returns two arcs sharing centre/radius: arc1 = start..θ (sweep =
 *   s·sweep), arc2 = θ..end (start = start + s·sweep, sweep =
 *   (1−s)·sweep). The two sub-sweeps sum to the original and share its
 *   sign by construction.
 * - **circle** / anything else: null (can't single-click split a closed
 *   loop; see module docstring).
 */
export function splitCurveAt(cv: Curve, clickPoint: Vec2): [Curve, Curve] | null {
  if (cv.kind === "line") {
    const d = sub(cv.b, cv.a);
    const ll = len2(d);
    if (ll < EPS) return null; // degenerate zero-length line
    const t = dot(sub(clickPoint, cv.a), d) / ll;
    if (t <= EPS || t >= 1 - EPS) return null; // at / past an endpoint
    const p = pointOnLine(cv.a, cv.b, t);
    return [
      { kind: "line", a: cv.a, b: p },
      { kind: "line", a: p, b: cv.b },
    ];
  }

  if (cv.kind === "arc") {
    const theta = Math.atan2(clickPoint[1] - cv.c[1], clickPoint[0] - cv.c[0]);
    const s = arcParam(cv.start, cv.sweep, theta);
    if (s === null || s <= EPS || s >= 1 - EPS) return null;
    const splitAngle = cv.start + s * cv.sweep;
    return [
      { kind: "arc", c: cv.c, r: cv.r, start: cv.start, sweep: s * cv.sweep },
      {
        kind: "arc",
        c: cv.c,
        r: cv.r,
        start: splitAngle,
        sweep: (1 - s) * cv.sweep,
      },
    ];
  }

  // Full circle (or any unsupported kind): no single-click split.
  return null;
}
