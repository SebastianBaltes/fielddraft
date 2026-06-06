/**
 * Catmull-Rom interpolating spline sampling — the TS mirror of the
 * kernel's `catmull_rom_quad_beziers` (crates/kernel/src/sketch_features.rs).
 *
 * Used for the live preview while drawing a `sketch_spline` and for
 * unit testing. The kernel builds the extrudable region from the same
 * control points, so this only needs to be *visually* consistent — it
 * samples the spline into a polyline rather than reproducing the
 * quad-Bézier fit byte-for-byte.
 *
 * The control points are stored on the sub-call as the flat `points`
 * array `[x0, y0, …]`; the kernel registers one point entity per
 * control point (id-mirroring), and `buildDraftSketchState` mirrors
 * that exactly.
 */

export type Vec2 = [number, number];

/**
 * Sample a Catmull-Rom spline through `controlPts`.
 *
 * - `closed` wraps the curve into a loop (the neighbour indices wrap
 *   modulo N and a final span returns to the first point).
 * - Open splines duplicate the first/last control point to synthesise
 *   the end tangents, so the curve still interpolates the endpoints.
 * - Produces `stepsPerSpan · spans + 1` points for the open case
 *   (`spans = N − 1`), and `stepsPerSpan · N + 1` for the closed case.
 * - Fewer than 2 control points → `[]`.
 * - Exactly 2 control points behaves like a straight line.
 */
export function catmullRomPoints(
  controlPts: Vec2[],
  closed: boolean,
  stepsPerSpan: number,
): Vec2[] {
  const n = controlPts.length;
  if (n < 2) return [];
  const steps = Math.max(1, Math.floor(stepsPerSpan));

  const cr = (
    p0: Vec2,
    p1: Vec2,
    p2: Vec2,
    p3: Vec2,
    t: number,
  ): Vec2 => {
    const t2 = t * t;
    const t3 = t2 * t;
    const f = (a: number, b: number, c: number, d: number) =>
      0.5 *
      (2 * b +
        (-a + c) * t +
        (2 * a - 5 * b + 4 * c - d) * t2 +
        (-a + 3 * b - 3 * c + d) * t3);
    return [f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])];
  };

  const spanCount = closed ? n : n - 1;
  const out: Vec2[] = [];
  for (let i = 0; i < spanCount; i++) {
    let i0: number;
    let i1: number;
    let i2: number;
    let i3: number;
    if (closed) {
      i0 = (i + n - 1) % n;
      i1 = i % n;
      i2 = (i + 1) % n;
      i3 = (i + 2) % n;
    } else {
      i0 = i === 0 ? 0 : i - 1;
      i1 = i;
      i2 = i + 1;
      i3 = i + 2 >= n ? n - 1 : i + 2;
    }
    const p0 = controlPts[i0];
    const p1 = controlPts[i1];
    const p2 = controlPts[i2];
    const p3 = controlPts[i3];
    // Emit `steps` points per span; include the very first point once.
    for (let s = i === 0 ? 0 : 1; s <= steps; s++) {
      out.push(cr(p0, p1, p2, p3, s / steps));
    }
  }
  return out;
}
