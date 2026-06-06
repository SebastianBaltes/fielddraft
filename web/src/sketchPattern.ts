/**
 * F16: Pattern (linear / circular) for the 2D sketcher.
 *
 * Pure, DOM-free repetition of sketch geometry. The user selects source
 * entities, sets a count (and a spacing for linear / a total angle for
 * circular) and clicks one of the Pattern buttons; the store maps the
 * entity-id selection back to its simple-geometry sub-calls (line /
 * circle / arc), produces the copies here and appends ordinary
 * `sketch_line` / `sketch_circle` / `sketch_arc` sub-calls — exactly the
 * primitives the kernel already understands. No kernel pattern tool is
 * involved: the result is just more geometry (like trim / mirror /
 * offset).
 *
 * Transforms (both rigid isometries, so the radius of a circle/arc is
 * invariant):
 *  - translate: line shifts both endpoints; circle/arc shift the centre,
 *    angles unchanged.
 *  - rotate about a centre: line rotates both endpoints; circle rotates
 *    the centre, radius same; arc rotates the centre AND adds the rotation
 *    angle to its start angle (the sweep is unchanged — rotation preserves
 *    orientation), so the rotated arc spans the rotated start/end points.
 *
 * Both `patternLinear` / `patternCircular` return the COPIES ONLY
 * (k = 1 .. count − 1); the caller leaves the original in place. A
 * `count < 2` yields no copies.
 */

import type { Curve, Vec2 } from "./sketchTrim";

const TAU = Math.PI * 2;
/** |totalAngle − 2π| below this is treated as a full revolution. */
const FULL_CIRCLE_EPS = 1e-6;

/**
 * Translate a curve by `(dx, dy)`.
 *  - line: shift both endpoints.
 *  - circle: shift the centre (radius unchanged).
 *  - arc: shift the centre (radius / start / sweep unchanged — a pure
 *    translation doesn't touch the parametric angles).
 */
export function translateCurve(cv: Curve, dx: number, dy: number): Curve {
  if (cv.kind === "line") {
    return {
      kind: "line",
      a: [cv.a[0] + dx, cv.a[1] + dy],
      b: [cv.b[0] + dx, cv.b[1] + dy],
    };
  }
  if (cv.kind === "circle") {
    return { kind: "circle", c: [cv.c[0] + dx, cv.c[1] + dy], r: cv.r };
  }
  return {
    kind: "arc",
    c: [cv.c[0] + dx, cv.c[1] + dy],
    r: cv.r,
    start: cv.start,
    sweep: cv.sweep,
  };
}

/** Rotate a point about `center` by `angleRad` (CCW). */
function rotatePoint(p: Vec2, center: Vec2, angleRad: number): Vec2 {
  const cos = Math.cos(angleRad);
  const sin = Math.sin(angleRad);
  const px = p[0] - center[0];
  const py = p[1] - center[1];
  return [
    center[0] + px * cos - py * sin,
    center[1] + px * sin + py * cos,
  ];
}

/**
 * Rotate a curve about `center` by `angleRad` (CCW).
 *  - line: rotate both endpoints.
 *  - circle: rotate the centre, radius unchanged.
 *  - arc: rotate the centre AND add `angleRad` to the start angle; the
 *    sweep is unchanged (rotation preserves orientation), so the rotated
 *    arc passes through the rotated start/end points.
 */
export function rotateCurve(cv: Curve, center: Vec2, angleRad: number): Curve {
  if (cv.kind === "line") {
    return {
      kind: "line",
      a: rotatePoint(cv.a, center, angleRad),
      b: rotatePoint(cv.b, center, angleRad),
    };
  }
  if (cv.kind === "circle") {
    return { kind: "circle", c: rotatePoint(cv.c, center, angleRad), r: cv.r };
  }
  return {
    kind: "arc",
    c: rotatePoint(cv.c, center, angleRad),
    r: cv.r,
    start: cv.start + angleRad,
    sweep: cv.sweep,
  };
}

/**
 * Linear pattern: repeat every curve in `curves` along the per-step
 * spacing vector `(dx, dy)`. Returns the COPIES ONLY — for each curve,
 * one translated copy per step k = 1 .. count − 1, shifted by
 * k·(dx, dy). The original (k = 0) is intentionally omitted.
 *
 * `count < 2` → `[]` (no copies). Output order: all copies of the first
 * curve (k ascending), then the second curve, etc.
 */
export function patternLinear(
  curves: ReadonlyArray<Curve>,
  count: number,
  dx: number,
  dy: number,
): Curve[] {
  if (!(count >= 2)) return [];
  const out: Curve[] = [];
  for (const cv of curves) {
    for (let k = 1; k < count; k++) {
      out.push(translateCurve(cv, k * dx, k * dy));
    }
  }
  return out;
}

/**
 * Circular pattern: repeat every curve in `curves` around `center`,
 * spanning `totalAngleRad`. Returns the COPIES ONLY — for each curve, one
 * rotated copy per step k = 1 .. count − 1, rotated by k·step.
 *
 * Step rule:
 *  - FULL revolution (|totalAngle − 2π| < 1e-6): step = 2π / count, so the
 *    `count` instances (original + count−1 copies) divide the circle
 *    evenly and the last copy does NOT land back on the original.
 *  - PARTIAL span (anything else): step = totalAngle / (count − 1), so the
 *    span is inclusive — the original sits at angle 0 and the last copy at
 *    exactly `totalAngle`.
 *
 * `count < 2` → `[]` (no copies). Output order: all copies of the first
 * curve (k ascending), then the second curve, etc.
 */
export function patternCircular(
  curves: ReadonlyArray<Curve>,
  count: number,
  center: Vec2,
  totalAngleRad: number,
): Curve[] {
  if (!(count >= 2)) return [];
  const isFull = Math.abs(Math.abs(totalAngleRad) - TAU) < FULL_CIRCLE_EPS;
  const step = isFull
    ? (Math.sign(totalAngleRad) || 1) * (TAU / count)
    : totalAngleRad / (count - 1);
  const out: Curve[] = [];
  for (const cv of curves) {
    for (let k = 1; k < count; k++) {
      out.push(rotateCurve(cv, center, k * step));
    }
  }
  return out;
}
