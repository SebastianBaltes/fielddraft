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

/**
 * de Boor evaluation of a B-spline (`poly`, `knots`, `degree`) at
 * parameter `u`. Numerically stable (no basis-function powers).
 */
function deBoor(
  poly: Vec2[],
  knots: number[],
  degree: number,
  u: number,
): Vec2 {
  const n = poly.length;
  // Knot span k with knots[k] <= u < knots[k+1], clamped to n-1.
  let k = degree;
  while (k + 1 <= n - 1 && knots[k + 1] <= u) k++;
  const d: Vec2[] = [];
  for (let j = 0; j <= degree; j++) {
    d.push(poly[Math.min(k - degree + j, n - 1)]);
  }
  for (let r = 1; r <= degree; r++) {
    for (let j = degree; j >= r; j--) {
      const i = k - degree + j;
      const denom = knots[i + degree - r + 1] - knots[i];
      const alpha = Math.abs(denom) < 1e-12 ? 0 : (u - knots[i]) / denom;
      d[j] = [
        (1 - alpha) * d[j - 1][0] + alpha * d[j][0],
        (1 - alpha) * d[j - 1][1] + alpha * d[j][1],
      ];
    }
  }
  return d[degree];
}

/**
 * Sample a REAL cubic (degree-3) B-spline whose `controls` are the
 * control polygon — the TS mirror of the kernel's `bspline_quad_beziers`
 * (de Boor over the same clamped-uniform / periodic knot vectors). Used
 * for the live preview + canvas rendering of a CONTROL-mode spline and
 * for the grip re-eval.
 *
 * Knot-vector scheme (matches the kernel):
 * - **Open** (`closed === false`): clamped-uniform — the curve passes
 *   through the first and last control points.
 * - **Closed** (`closed === true`): periodic/uniform — the curve wraps
 *   into a smooth loop (the first `degree` controls are repeated and
 *   the loop sub-domain `[knots[d], knots[d+n]]` is sampled).
 *
 * Edge cases:
 * - `< 2` controls → `[]`.
 * - exactly `2` → a straight line (`samples + 1` points).
 * - `3` → degree-2 curve.
 * - collinear controls → a straight curve.
 *
 * `degree` clamps to `n - 1` so small control counts degrade sensibly.
 * Returns `samples + 1` points spanning the parameter domain.
 */
export function bsplinePoints(
  controls: Vec2[],
  degree: number,
  closed: boolean,
  samples: number,
): Vec2[] {
  const n = controls.length;
  if (n < 2) return [];
  const m = Math.max(1, Math.floor(samples));
  if (n === 2) {
    // Straight line A→B.
    const out: Vec2[] = [];
    for (let s = 0; s <= m; s++) {
      const t = s / m;
      out.push([
        controls[0][0] + t * (controls[1][0] - controls[0][0]),
        controls[0][1] + t * (controls[1][1] - controls[0][1]),
      ]);
    }
    return out;
  }
  const deg = Math.min(Math.max(1, Math.floor(degree)), n - 1);

  let poly: Vec2[];
  let knots: number[];
  let uLo: number;
  let uHi: number;
  if (closed) {
    poly = controls.slice();
    for (let i = 0; i < deg; i++) poly.push(controls[i % n]);
    const knotCount = poly.length + deg + 1;
    knots = [];
    for (let i = 0; i < knotCount; i++) knots.push(i);
    uLo = knots[deg];
    uHi = knots[deg + n];
  } else {
    poly = controls.slice();
    const interior = poly.length - deg - 1;
    knots = [];
    for (let i = 0; i <= deg; i++) knots.push(0);
    for (let i = 1; i <= interior; i++) knots.push(i);
    const last = interior + 1;
    for (let i = 0; i <= deg; i++) knots.push(last);
    uLo = 0;
    uHi = last;
  }

  const out: Vec2[] = [];
  for (let s = 0; s <= m; s++) {
    const u = uLo + ((uHi - uLo) * s) / m;
    out.push(deBoor(poly, knots, deg, Math.min(u, uHi - 1e-9 * (uHi - uLo))));
  }
  // Snap exact endpoints for the open (clamped) case so the curve
  // visibly touches the first/last control points.
  if (!closed) {
    out[0] = [controls[0][0], controls[0][1]];
    out[out.length - 1] = [controls[n - 1][0], controls[n - 1][1]];
  }
  return out;
}
