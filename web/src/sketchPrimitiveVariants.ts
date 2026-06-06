/**
 * Pure geometry for the primitive-construction variant tools (F7):
 *
 *   - **Rectangle (center)** — drag from the centre to a corner; the
 *     result is an axis-aligned rectangle expressed as two opposite
 *     corners. Emitted as a single `sketch_rectangle` sub-call.
 *   - **Circle (3-point)** — three clicks on the circumference; the
 *     circumcircle through them gives centre + radius. Emitted as a
 *     single `sketch_circle` sub-call. Collinear points have no finite
 *     circumcircle, so the commit is aborted.
 *   - **Rectangle (3-point)** — three clicks: p1→p2 sets one edge and
 *     its direction; the 3rd click sets the perpendicular width. The
 *     result is a (possibly rotated) rectangle, expressed as its four
 *     corners. Emitted as four `sketch_line` sub-calls plus
 *     perpendicular constraints, since `sketch_rectangle` is AABB-only
 *     and cannot represent rotation.
 *
 * Kept free of React/DOM so the math is unit-testable in isolation,
 * matching the `sketchArc.ts` template.
 */

export type Vec2 = [number, number];

/** Below this (mm / mm²) an edge, width or radius counts as degenerate. */
export const MIN_VARIANT_EPS = 1e-9;

/**
 * Circumcircle through three points: the intersection of the
 * perpendicular bisectors of p1p2 and p2p3. Returns `null` when the
 * points are collinear (zero denominator → no finite centre).
 */
export function circleThrough3(
  p1: Vec2,
  p2: Vec2,
  p3: Vec2,
): { center: Vec2; radius: number } | null {
  const ax = p1[0];
  const ay = p1[1];
  const bx = p2[0];
  const by = p2[1];
  const cx = p3[0];
  const cy = p3[1];
  // d = 2 · (cross of the two edge vectors). Zero ⇒ collinear.
  const d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
  if (Math.abs(d) < MIN_VARIANT_EPS) return null;
  const a2 = ax * ax + ay * ay;
  const b2 = bx * bx + by * by;
  const c2 = cx * cx + cy * cy;
  const ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
  const uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
  const center: Vec2 = [ux, uy];
  const radius = Math.hypot(ax - ux, ay - uy);
  if (radius < MIN_VARIANT_EPS) return null;
  return { center, radius };
}

/**
 * Axis-aligned rectangle from a centre and a corner: the two opposite
 * corners are center ∓ |delta|, so dragging into any quadrant yields the
 * same min/max corner pair. Returns `null` for a zero-area rectangle.
 */
export function rectCenterCorners(
  center: Vec2,
  corner: Vec2,
): { cornerA: Vec2; cornerB: Vec2 } | null {
  const dx = Math.abs(corner[0] - center[0]);
  const dy = Math.abs(corner[1] - center[1]);
  if (dx < MIN_VARIANT_EPS || dy < MIN_VARIANT_EPS) return null;
  return {
    cornerA: [center[0] - dx, center[1] - dy],
    cornerB: [center[0] + dx, center[1] + dy],
  };
}

/**
 * Four corners of a (possibly rotated) rectangle from three points:
 *   - p1→p2 is one full edge (length and direction),
 *   - the signed perpendicular distance of p3 from that edge is the
 *     rectangle's width.
 * The loop is p1 → p2 → p2+w·n → p1+w·n where n is the unit normal of
 * the p1p2 edge. Returns `null` when the edge or the width is
 * degenerate (so the rectangle would collapse to a line or a point).
 */
export function rect3pCorners(
  p1: Vec2,
  p2: Vec2,
  p3: Vec2,
): [Vec2, Vec2, Vec2, Vec2] | null {
  const ex = p2[0] - p1[0];
  const ey = p2[1] - p1[1];
  const len = Math.hypot(ex, ey);
  if (len < MIN_VARIANT_EPS) return null;
  // Unit normal (perp of the edge, rotated +90°).
  const nx = -ey / len;
  const ny = ex / len;
  // Signed perpendicular distance of p3 from the edge.
  const w = (p3[0] - p2[0]) * nx + (p3[1] - p2[1]) * ny;
  if (Math.abs(w) < MIN_VARIANT_EPS) return null;
  const c3: Vec2 = [p2[0] + w * nx, p2[1] + w * ny];
  const c4: Vec2 = [p1[0] + w * nx, p1[1] + w * ny];
  return [[p1[0], p1[1]], [p2[0], p2[1]], c3, c4];
}
