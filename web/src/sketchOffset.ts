/**
 * F15: Interactive offset for the 2D sketcher.
 *
 * Pure, DOM-free parallel copy of a single sketch curve at a given
 * distance. The user picks the Offset tool, sets a distance, and clicks a
 * curve ON THE SIDE they want the offset; the store appends an editable
 * `sketch_line` / `sketch_circle` / `sketch_arc` toward the clicked side
 * (see store.offsetSketchAt). No kernel offset tool is involved — the
 * result is just more of the primitives the kernel already understands
 * (like trim / fillet / mirror).
 *
 * Geometry per kind:
 *  - line (a→b): the offset is a parallel line. The unit normal is
 *    n = perp(unit(b − a)) = (−dy, dx)/len. The click's side is
 *    s = sign(dot(sidePoint − a, n)); a click on the +n side gives +1, on
 *    the −n side −1. (If the click sits exactly on the line, dot ≈ 0 and
 *    we deterministically pick +1 — the +n side.) Both endpoints shift by
 *    s·distance·n, so the copy is `distance` mm away on the clicked side.
 *  - circle: the offset is concentric. If the click is OUTSIDE the circle
 *    (|sidePoint − c| > r) the radius grows by `distance`; if INSIDE it
 *    shrinks by `distance`. A shrink that collapses the circle
 *    (r − distance ≤ 0) yields null. Centre is unchanged.
 *  - arc: same radius rule as circle (grow outside / shrink inside),
 *    keeping the same centre, start angle and sweep. r ≤ 0 → null.
 *
 * Returns null for unsupported kinds, a non-finite / non-positive
 * distance, or a degenerate (zero-length) line.
 */

import type { Curve, Vec2 } from "./sketchTrim";

/** Squared length below which a line is treated as degenerate. */
const LEN_EPS_SQ = 1e-18;

/**
 * Offset `cv` by `distance` mm toward the side the click `sidePoint`
 * lies on. Returns the offset curve, or null when no valid offset exists
 * (see module docstring for the per-kind rules and null cases).
 */
export function offsetCurve(
  cv: Curve,
  distance: number,
  sidePoint: Vec2,
): Curve | null {
  if (!Number.isFinite(distance) || distance <= 0) return null;

  if (cv.kind === "line") {
    const dx = cv.b[0] - cv.a[0];
    const dy = cv.b[1] - cv.a[1];
    const lenSq = dx * dx + dy * dy;
    if (lenSq < LEN_EPS_SQ) return null;
    const len = Math.sqrt(lenSq);
    // Unit normal n = perp(unit(b − a)) = (−dy, dx)/len.
    const nx = -dy / len;
    const ny = dx / len;
    // Which side of the line is the click on?
    const sideDot = (sidePoint[0] - cv.a[0]) * nx + (sidePoint[1] - cv.a[1]) * ny;
    // Exactly on the line (sideDot ≈ 0) → pick the +n side deterministically.
    const s = sideDot < 0 ? -1 : 1;
    const ox = s * distance * nx;
    const oy = s * distance * ny;
    return {
      kind: "line",
      a: [cv.a[0] + ox, cv.a[1] + oy],
      b: [cv.b[0] + ox, cv.b[1] + oy],
    };
  }

  if (cv.kind === "circle") {
    const distToCentre = Math.hypot(
      sidePoint[0] - cv.c[0],
      sidePoint[1] - cv.c[1],
    );
    const outside = distToCentre > cv.r;
    const r = outside ? cv.r + distance : cv.r - distance;
    if (r <= 0) return null;
    return { kind: "circle", c: cv.c, r };
  }

  // arc: same radius rule as circle, same centre / start / sweep.
  const distToCentre = Math.hypot(
    sidePoint[0] - cv.c[0],
    sidePoint[1] - cv.c[1],
  );
  const outside = distToCentre > cv.r;
  const r = outside ? cv.r + distance : cv.r - distance;
  if (r <= 0) return null;
  return { kind: "arc", c: cv.c, r, start: cv.start, sweep: cv.sweep };
}
