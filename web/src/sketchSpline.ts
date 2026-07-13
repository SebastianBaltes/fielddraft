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

// ── Split (ticket e5482a0a) ────────────────────────────────────────
//
// The Split tool cuts a curve at a click point. A `sketch_spline` stores
// ONLY its control points + mode + closed flag — the knot vector is
// implied (clamped-uniform / periodic), so an EXACT split (whose halves
// generally need non-uniform knots) is not representable in this format.
// Both halves therefore stay ordinary `sketch_spline`s:
//
//   - **interpolate** (Catmull-Rom): the click point is inserted into the
//     point list and the list is cut there. Every original point still
//     lies on its half — only the tangent AT THE SEAM is re-derived (each
//     half gets its own natural end tangent), so the shape can differ
//     slightly right at the cut.
//   - **control** (cubic B-spline): the control polygon is split exactly
//     (Boehm knot insertion to multiplicity `degree` at the split
//     parameter), so both halves start/end exactly at the click point and
//     inherit the true sub-curve's control points; only their implied
//     uniform re-parameterisation differs from the exact sub-curve.
//
// A CLOSED spline has no two halves — one click opens the loop AT the
// click (the same convention Split uses on a full circle: it plants a
// vertex and leaves one open curve).

/** Nearest point on a sampled polyline: its index into `poly` plus the
 *  parameter `s ∈ [0, 1]` of the whole polyline (arc-length-free, i.e.
 *  sample-index based — enough to locate the span). */
function nearestSample(
  poly: Vec2[],
  click: Vec2,
): { index: number; point: Vec2 } | null {
  if (poly.length < 2) return null;
  let bestD = Infinity;
  let bestI = 0;
  let bestP: Vec2 = poly[0];
  for (let i = 0; i + 1 < poly.length; i++) {
    const a = poly[i];
    const b = poly[i + 1];
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const ll = dx * dx + dy * dy;
    const t =
      ll < 1e-12
        ? 0
        : Math.max(
            0,
            Math.min(
              1,
              ((click[0] - a[0]) * dx + (click[1] - a[1]) * dy) / ll,
            ),
          );
    const p: Vec2 = [a[0] + t * dx, a[1] + t * dy];
    const d = Math.hypot(click[0] - p[0], click[1] - p[1]);
    if (d < bestD) {
      bestD = d;
      bestI = i + t; // fractional sample index
      bestP = p;
    }
  }
  return { index: bestI, point: bestP };
}

/** One Boehm knot insertion of `u` into (`poly`, `knots`, `degree`). */
function insertKnot(
  poly: Vec2[],
  knots: number[],
  degree: number,
  u: number,
): { poly: Vec2[]; knots: number[] } {
  const n = poly.length;
  // Span k: knots[k] <= u < knots[k+1] (same convention as deBoor).
  let k = degree;
  while (k + 1 <= n - 1 && knots[k + 1] <= u) k++;
  const out: Vec2[] = [];
  for (let i = 0; i <= k - degree; i++) out.push(poly[i]);
  for (let i = k - degree + 1; i <= k; i++) {
    const denom = knots[i + degree] - knots[i];
    const a = Math.abs(denom) < 1e-12 ? 0 : (u - knots[i]) / denom;
    out.push([
      (1 - a) * poly[i - 1][0] + a * poly[i][0],
      (1 - a) * poly[i - 1][1] + a * poly[i][1],
    ]);
  }
  for (let i = k; i < n; i++) out.push(poly[i]);
  const newKnots = [...knots.slice(0, k + 1), u, ...knots.slice(k + 1)];
  return { poly: out, knots: newKnots };
}

/** Clamped-uniform knot vector for `n` controls of degree `deg` — the
 *  scheme {@link bsplinePoints} implies for an OPEN spline. */
function clampedKnots(n: number, deg: number): number[] {
  const interior = n - deg - 1;
  const knots: number[] = [];
  for (let i = 0; i <= deg; i++) knots.push(0);
  for (let i = 1; i <= interior; i++) knots.push(i);
  const last = interior + 1;
  for (let i = 0; i <= deg; i++) knots.push(last);
  return knots;
}

/**
 * Split an OPEN control-polygon B-spline at parameter `u`, returning the
 * two control polygons. Boehm-inserts `u` until its multiplicity equals
 * the degree; the shared control point is then the curve point at `u`
 * and the polygon splits there.
 */
function splitBSplineControls(
  controls: Vec2[],
  degree: number,
  u: number,
): [Vec2[], Vec2[]] | null {
  const n = controls.length;
  const deg = Math.min(Math.max(1, Math.floor(degree)), n - 1);
  let poly = controls.slice();
  let knots = clampedKnots(n, deg);
  for (let r = 0; r < deg; r++) {
    const ins = insertKnot(poly, knots, deg, u);
    poly = ins.poly;
    knots = ins.knots;
  }
  // With multiplicity == degree the curve point at `u` IS a control point:
  // the one whose Greville abscissa is u, i.e. the control whose knot
  // window `knots[j+1 .. j+deg]` is exactly the inserted group. With the
  // group starting at knot index i0 that is j = i0 − 1.
  let j = -1;
  for (let i = 0; i < knots.length; i++) {
    if (Math.abs(knots[i] - u) < 1e-9) {
      j = i - 1;
      break;
    }
  }
  if (j <= 0 || j >= poly.length - 1) return null;
  const left = poly.slice(0, j + 1);
  const right = poly.slice(j);
  if (left.length < 2 || right.length < 2) return null;
  return [left, right];
}

/**
 * Split a spline's control points at `click`.
 *
 * Returns the point lists of the two halves for an OPEN spline, or a
 * SINGLE list (the opened loop, starting and ending at the click) for a
 * closed one. `null` when the click lands on/near an endpoint (nothing
 * meaningful to cut) or the spline is degenerate.
 */
export function splitSplinePoints(
  controls: Vec2[],
  mode: "control" | "interpolate",
  closed: boolean,
  click: Vec2,
): Vec2[][] | null {
  const n = controls.length;
  if (n < 2) return null;
  const isControl = mode === "control";
  const SAMPLES_PER_SPAN = 32;
  const spans = closed ? n : n - 1;

  if (isControl && !closed) {
    // Exact control-polygon split via knot insertion. Locate the split
    // parameter by sampling the same clamped-uniform domain bsplinePoints
    // walks, then map the nearest sample back to `u`.
    const deg = Math.min(3, n - 1);
    const total = spans * SAMPLES_PER_SPAN;
    const poly = bsplinePoints(controls, 3, false, total);
    const hit = nearestSample(poly, click);
    if (!hit) return null;
    const knots = clampedKnots(n, deg);
    const uHi = knots[knots.length - 1];
    const u = (hit.index / total) * uHi;
    // Too close to either end → no meaningful split.
    if (u <= 1e-6 || u >= uHi - 1e-6) return null;
    return splitBSplineControls(controls, 3, u);
  }

  // Interpolate (and any closed spline): cut the POINT LIST at the click,
  // inserting the click point as the shared seam vertex.
  const total = spans * SAMPLES_PER_SPAN;
  const poly = isControl
    ? bsplinePoints(controls, 3, closed, total)
    : catmullRomPoints(controls, closed, SAMPLES_PER_SPAN);
  const hit = nearestSample(poly, click);
  if (!hit) return null;
  const span = Math.min(spans - 1, Math.floor(hit.index / SAMPLES_PER_SPAN));
  const p = hit.point;

  if (closed) {
    // Open the loop at the click: walk the points from the split forward
    // once around, with the split point at both ends.
    const out: Vec2[] = [p];
    for (let i = 0; i < n; i++) out.push(controls[(span + 1 + i) % n]);
    out.push(p);
    return [out];
  }

  // Reject a click that lands essentially on an endpoint (a zero-length
  // half is no split).
  const near = (a: Vec2, b: Vec2) => Math.hypot(a[0] - b[0], a[1] - b[1]) < 1e-6;
  if (near(p, controls[0]) || near(p, controls[n - 1])) return null;
  const left = [...controls.slice(0, span + 1), p];
  const right = [p, ...controls.slice(span + 1)];
  if (left.length < 2 || right.length < 2) return null;
  return [left, right];
}
