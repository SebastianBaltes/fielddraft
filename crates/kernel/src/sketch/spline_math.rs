//! Reine Spline-Numerik: Catmull-Rom-/B-Spline-Verquadratisierung
//! (`catmull_rom_quad_beziers`, `bspline_quad_beziers`, De-Boor,
//! Knoteneinfügung, kubisch→quadratisch-Zerlegung).

use crate::sketch2d::{SketchSegment};
/// Lower a Catmull-Rom interpolating spline through `control_pts`
/// into a chain of [`SketchSegment::QuadBezier`] segments that stays
/// within `tol` (model units) of the exact spline.
///
/// Each Catmull-Rom span (`p1`→`p2`, neighbours `p0`/`p3` supplying
/// the tangents `(p2−p0)/2` and `(p3−p1)/2`) IS a cubic Bézier —
/// exactly:
///
///   b0 = p1,  b1 = p1 + (p2−p0)/6,  b2 = p2 − (p3−p1)/6,  b3 = p2
///
/// — so the lowering converts each span exactly and delegates to the
/// error-bounded [`cubic_to_quads`]. No whole-curve sampling: the
/// interpolated control points are hit exactly (span endpoints are
/// preserved through subdivision), the segment count adapts to
/// curvature (a straight-ish span is a single quad), and junctions
/// inherit the spline's C1 tangents exactly.
///
/// * Open splines (`closed == false`) duplicate the first/last
///   control point to synthesise the end tangents.
/// * Closed splines wrap the neighbour indices and append a final
///   span back to the first point.
///
/// Returns an empty vec for fewer than 2 control points.
pub fn catmull_rom_quad_beziers(
    control_pts: &[[f32; 2]],
    closed: bool,
    tol: f64,
) -> Vec<SketchSegment> {
    let n = control_pts.len();
    if n < 2 {
        return Vec::new();
    }
    let span_count = if closed { n } else { n - 1 };
    let mut segs = Vec::with_capacity(span_count * 2);
    for i in 0..span_count {
        let (i0, i1, i2, i3) = if closed {
            ((i + n - 1) % n, i % n, (i + 1) % n, (i + 2) % n)
        } else {
            (
                if i == 0 { 0 } else { i - 1 },
                i,
                i + 1,
                if i + 2 >= n { n - 1 } else { i + 2 },
            )
        };
        let p = |idx: usize| -> [f64; 2] {
            [control_pts[idx][0] as f64, control_pts[idx][1] as f64]
        };
        let p0 = p(i0);
        let p1 = p(i1);
        let p2 = p(i2);
        let p3 = p(i3);
        let b1 = [p1[0] + (p2[0] - p0[0]) / 6.0, p1[1] + (p2[1] - p0[1]) / 6.0];
        let b2 = [p2[0] - (p3[0] - p1[0]) / 6.0, p2[1] - (p3[1] - p1[1]) / 6.0];
        cubic_to_quads(p1, b1, b2, p2, tol, &mut segs);
    }
    segs
}

/// Lower a cubic (degree-3) B-spline with control polygon
/// `control_pts` into a chain of [`SketchSegment::QuadBezier`]
/// segments, EXACTLY (no whole-curve point sampling).
///
/// Knot-vector scheme:
/// * **Open** (`closed == false`): a clamped-uniform knot vector of
///   degree 3 — `[0,0,0,0, 1,2,…, m,m,m,m]`. The clamping makes the
///   curve interpolate the first and last control points, so those
///   are the curve endpoints (and the registered point entities), and
///   a Coincident constraint on them is a true endpoint constraint.
/// * **Closed** (`closed == true`): a periodic/uniform knot vector.
///   The control polygon is wrapped by repeating the first `degree`
///   control points at the end (`P0…P_{n-1}, P0,P1,P2`) over a uniform
///   knot vector, giving a smooth C² wrap-around loop (an extrudable
///   region).
///
/// Exact lowering pipeline:
/// 1. Build the (possibly wrapped) control points + knot vector.
/// 2. Convert each non-empty knot span into ONE cubic Bézier via
///    Boehm's knot-insertion algorithm (insert each interior knot up
///    to multiplicity `degree`, i.e. the Bézier control points of a
///    span are read straight off the fully-refined polygon).
/// 3. Subdivide each cubic Bézier into quadratic Béziers within the
///    deviation tolerance (the kernel `SketchSegment` has no cubic).
///
/// Edge cases (no panic, NaN-free):
/// * `< 2` control points → empty.
/// * exactly `2` → a single straight `Line`.
/// * exactly `3` → a degree-2 curve: one quad through the control
///   triangle (open) / a small wrapped loop (closed).
/// * collinear control points → the cubic collapses to a straight
///   curve (still emitted, no division by zero).
pub fn bspline_quad_beziers(
    control_pts: &[[f64; 2]],
    closed: bool,
    tol: f64,
) -> Vec<SketchSegment> {
    let n = control_pts.len();
    if n < 2 {
        return Vec::new();
    }
    // Two points: a straight line, regardless of mode.
    if n == 2 {
        return vec![SketchSegment::Line {
            start: [control_pts[0][0] as f32, control_pts[0][1] as f32],
            end: [control_pts[1][0] as f32, control_pts[1][1] as f32],
        }];
    }

    // Degree: cubic when we have enough control points, else drop to
    // n-1 so 3 control points give a degree-2 curve, 2 a line.
    let degree = 3usize.min(n - 1);

    // Build the working control polygon + clamped/periodic knots and
    // the valid loop / curve parameter domain [u_lo, u_hi].
    let (poly, knots, u_lo, u_hi): (Vec<[f64; 2]>, Vec<f64>, f64, f64) = if closed {
        // Periodic: wrap the first `degree` control points to the end
        // and use a uniform knot vector over the padded polygon. The
        // closed loop lives on the parameter sub-domain
        // [knots[degree], knots[degree + n]] — the part where all the
        // wrapped basis functions overlap into a seamless C² loop.
        let mut poly = control_pts.to_vec();
        for k in 0..degree {
            poly.push(control_pts[k % n]);
        }
        let m = poly.len() + degree + 1; // knot count
        let knots: Vec<f64> = (0..m).map(|i| i as f64).collect();
        let u_lo = knots[degree];
        let u_hi = knots[degree + n];
        (poly, knots, u_lo, u_hi)
    } else {
        // Clamped-uniform: degree+1 repeats at each end.
        let poly = control_pts.to_vec();
        let interior = poly.len() - degree - 1; // # interior knots
        let mut knots: Vec<f64> = Vec::with_capacity(poly.len() + degree + 1);
        for _ in 0..=degree {
            knots.push(0.0);
        }
        for i in 1..=interior {
            knots.push(i as f64);
        }
        let last = (interior + 1) as f64;
        for _ in 0..=degree {
            knots.push(last);
        }
        (poly, knots, 0.0, last)
    };

    // Decompose the B-spline into per-span Bézier control points via
    // repeated knot insertion (Boehm), restricted to the [u_lo, u_hi]
    // domain (clamping both ends to multiplicity `degree`). The refined
    // polygon then reads off as consecutive Bézier segments of
    // `degree+1` control points sharing endpoints.
    let beziers = bspline_to_beziers(&poly, &knots, degree, u_lo, u_hi);

    // Subdivide each Bézier (degree 2 or 3) into quads.
    let mut segs: Vec<SketchSegment> = Vec::new();
    for bez in &beziers {
        match bez.len() {
            4 => cubic_to_quads(bez[0], bez[1], bez[2], bez[3], tol, &mut segs),
            3 => {
                // Already a quad.
                segs.push(SketchSegment::QuadBezier {
                    p0: [bez[0][0] as f32, bez[0][1] as f32],
                    p1: [bez[1][0] as f32, bez[1][1] as f32],
                    p2: [bez[2][0] as f32, bez[2][1] as f32],
                });
            }
            2 => segs.push(SketchSegment::Line {
                start: [bez[0][0] as f32, bez[0][1] as f32],
                end: [bez[1][0] as f32, bez[1][1] as f32],
            }),
            _ => {}
        }
    }
    segs
}

/// Boehm knot-insertion decomposition restricted to the parameter
/// domain `[u_lo, u_hi]`: turn a B-spline (`poly`, `knots`, `degree`)
/// into a list of per-span Bézier control-point arrays (each of length
/// `degree+1`) covering exactly that domain.
///
/// Method: clamp both domain ends to multiplicity `degree` (so the
/// refined polygon has a control point landing exactly on the curve at
/// `u_lo` and `u_hi`), raise every interior knot inside `(u_lo, u_hi)`
/// to multiplicity `degree`, then read off consecutive Bézier segments
/// starting at the polygon index that corresponds to `u_lo`.
fn bspline_to_beziers(
    poly: &[[f64; 2]],
    knots: &[f64],
    degree: usize,
    u_lo: f64,
    u_hi: f64,
) -> Vec<Vec<[f64; 2]>> {
    let mut poly: Vec<[f64; 2]> = poly.to_vec();
    let mut knots: Vec<f64> = knots.to_vec();

    // Distinct knot values within [u_lo, u_hi] (inclusive of the ends)
    // that must reach multiplicity `degree`.
    let mut vals: Vec<f64> = Vec::new();
    let push_val = |v: f64, vals: &mut Vec<f64>| {
        if !vals.iter().any(|&p| (p - v).abs() < 1e-9) {
            vals.push(v);
        }
    };
    push_val(u_lo, &mut vals);
    for &k in &knots {
        if k > u_lo + 1e-9 && k < u_hi - 1e-9 {
            push_val(k, &mut vals);
        }
    }
    push_val(u_hi, &mut vals);
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());

    for &val in &vals {
        let mult = knots.iter().filter(|&&k| (k - val).abs() < 1e-9).count();
        for _ in mult..degree {
            insert_knot(&mut poly, &mut knots, degree, val);
        }
    }

    // Find the polygon index whose corresponding knot reaches u_lo:
    // after clamping, the control point at the start of the first
    // in-domain span is the one whose left knot equals u_lo. Count the
    // control points whose knot index is < the first knot == u_lo at
    // full multiplicity. The standard relation: the i-th control point
    // is associated with knots[i+1..=i+degree]; the first span in the
    // domain begins at the control point index = (index of first knot
    // == u_lo) — for a clamped end that is `degree` repeats. We locate
    // the first knot index k0 with knots[k0] == u_lo and knots reaching
    // full multiplicity there; the span start control index is k0.
    let k0 = knots
        .iter()
        .position(|&k| (k - u_lo).abs() < 1e-9)
        .unwrap_or(0);

    // Number of in-domain spans = distinct interior values + 1.
    let span_count = vals.len() - 1;
    let mut out: Vec<Vec<[f64; 2]>> = Vec::with_capacity(span_count);
    for s in 0..span_count {
        let start = k0 + s * degree;
        let mut seg: Vec<[f64; 2]> = Vec::with_capacity(degree + 1);
        for k in 0..=degree {
            let idx = (start + k).min(poly.len() - 1);
            seg.push(poly[idx]);
        }
        out.push(seg);
    }
    out
}

/// Insert knot value `u` once into (`poly`, `knots`) for a B-spline of
/// the given `degree` (Boehm's algorithm). Mutates both in place.
fn insert_knot(poly: &mut Vec<[f64; 2]>, knots: &mut Vec<f64>, degree: usize, u: f64) {
    // Find the span index k such that knots[k] <= u < knots[k+1].
    // Clamp at poly.len()-1 so the affected control indices
    // (k-degree+1)..=k stay in range (Boehm requires k ≤ n-1).
    let mut k = degree;
    while k + 1 < knots.len() && knots[k + 1] <= u && k + 1 <= poly.len() - 1 {
        k += 1;
    }
    // New control points P'_i for i in (k-degree+1)..=k are blended.
    let mut new_poly: Vec<[f64; 2]> = Vec::with_capacity(poly.len() + 1);
    for i in 0..=(k - degree) {
        new_poly.push(poly[i]);
    }
    for i in (k - degree + 1)..=k {
        let denom = knots[i + degree] - knots[i];
        let a = if denom.abs() < 1e-12 {
            0.0
        } else {
            (u - knots[i]) / denom
        };
        let prev = poly[i - 1];
        let cur = poly[i];
        new_poly.push([
            (1.0 - a) * prev[0] + a * cur[0],
            (1.0 - a) * prev[1] + a * cur[1],
        ]);
    }
    for i in k..poly.len() {
        new_poly.push(poly[i]);
    }
    *poly = new_poly;
    knots.insert(k + 1, u);
}

/// Subdivide a cubic Bézier (`p0,p1,p2,p3`) into the SMALLEST uniform
/// chain of [`SketchSegment::QuadBezier`] segments that provably
/// deviates from the cubic by at most `tol` (cu2qu-style: try
/// n = 1, 2, 3, … equal-parameter pieces and take the first n whose
/// pieces all pass). Pushes onto `out`.
///
/// Each emitted quad keeps the cubic's exact end-tangent DIRECTIONS
/// (its control point is the intersection of the piece's end
/// tangents), so the chain is exactly G1 at every junction — the path
/// G1-grouping (and with it branch/face identity in the BRep) does
/// not depend on `tol`. Near-parallel tangents (straight or
/// inflection-carrying pieces) fall back to the "midpoint" control
/// `q = (3(p1+p2) − p0 − p3)/4`.
///
/// The per-piece error bound is analytic, no sampling: elevate the
/// candidate quad to a cubic and diff the control polygons — the
/// difference curve is `e1·B1(t) + e2·B2(t)` with
/// `e1 = p1 − (p0+2c)/3`, `e2 = p2 − (p3+2c)/3`, and since
/// `B1(t)+B2(t) = 3t(1−t) ≤ 3/4`, the same-parameter deviation is
/// ≤ `¾·max(‖e1‖,‖e2‖)`. It is zero iff the piece IS the quad, and
/// shrinks with the third-order term (~n⁻³), so n stays small.
fn cubic_to_quads(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    tol: f64,
    out: &mut Vec<SketchSegment>,
) {
    // Split at inflection points first: each sub-piece is then convex,
    // so the G1-preserving tangent-intersection control exists for
    // every piece, and the cubic's own tangent at the inflection is
    // shared exactly by both sides — the chain stays exactly G1 even
    // across inflections. Inflections of a Bézier cubic are the roots
    // of cross(B'(t), B''(t)) = cross(b,c)·t² + cross(a,c)·t +
    // cross(a,b) with a = p1−p0, b = p2−2p1+p0, c = p3−3p2+3p1−p0.
    let a = [p1[0] - p0[0], p1[1] - p0[1]];
    let b = [p2[0] - 2.0 * p1[0] + p0[0], p2[1] - 2.0 * p1[1] + p0[1]];
    let c = [
        p3[0] - 3.0 * p2[0] + 3.0 * p1[0] - p0[0],
        p3[1] - 3.0 * p2[1] + 3.0 * p1[1] - p0[1],
    ];
    let cross = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
    let (qa, qb, qc) = (cross(b, c), cross(a, c), cross(a, b));
    let mut cuts = [0.0f64, 1.0, 1.0, 1.0];
    let mut n_cuts = 1;
    let push_root = |t: f64, cuts: &mut [f64; 4], n_cuts: &mut usize| {
        if t > 1e-6 && t < 1.0 - 1e-6 && t.is_finite() {
            cuts[*n_cuts] = t;
            *n_cuts += 1;
        }
    };
    let curve_scale = (a[0].abs() + b[0].abs() + c[0].abs())
        .max(a[1].abs() + b[1].abs() + c[1].abs())
        .max(1e-300);
    if qa.abs() > 1e-12 * curve_scale * curve_scale {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc > 0.0 {
            let r = -0.5 * (qb + qb.signum() * disc.sqrt());
            let r = if r == 0.0 { f64::MIN_POSITIVE } else { r };
            let (t1, t2) = (r / qa, qc / r);
            push_root(t1.min(t2), &mut cuts, &mut n_cuts);
            push_root(t1.max(t2), &mut cuts, &mut n_cuts);
        }
    } else if qb.abs() > 1e-12 * curve_scale * curve_scale {
        push_root(-qc / qb, &mut cuts, &mut n_cuts);
    }
    cuts[n_cuts] = 1.0;
    n_cuts += 1;
    for w in 0..n_cuts - 1 {
        let (s0, s1, s2, s3) = cubic_piece(p0, p1, p2, p3, cuts[w], cuts[w + 1]);
        convex_cubic_to_quads(s0, s1, s2, s3, tol, 24, out);
    }
}

/// Adaptive quad subdivision of an inflection-free cubic piece (see
/// [`cubic_to_quads`]): accept a single quad when [`quad_for_cubic`]
/// finds one within `tol`, otherwise bisect — refinement stays local
/// to the curvy / near-inflection parts.
fn convex_cubic_to_quads(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    tol: f64,
    depth: u32,
    out: &mut Vec<SketchSegment>,
) {
    if let Some(ctrl) = quad_for_cubic(p0, p1, p2, p3, tol) {
        out.push(SketchSegment::QuadBezier {
            p0: [p0[0] as f32, p0[1] as f32],
            p1: [ctrl[0] as f32, ctrl[1] as f32],
            p2: [p3[0] as f32, p3[1] as f32],
        });
        return;
    }
    if depth == 0 {
        // Pathological cusp: emit the midpoint-control quad, whose
        // deviation err_mid is microscopic at this depth.
        let q = [
            (3.0 * (p1[0] + p2[0]) - p0[0] - p3[0]) / 4.0,
            (3.0 * (p1[1] + p2[1]) - p0[1] - p3[1]) / 4.0,
        ];
        out.push(SketchSegment::QuadBezier {
            p0: [p0[0] as f32, p0[1] as f32],
            p1: [q[0] as f32, q[1] as f32],
            p2: [p3[0] as f32, p3[1] as f32],
        });
        return;
    }
    let mid = |a: [f64; 2], b: [f64; 2]| [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
    let p01 = mid(p0, p1);
    let p12 = mid(p1, p2);
    let p23 = mid(p2, p3);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let p0123 = mid(p012, p123);
    convex_cubic_to_quads(p0, p01, p012, p0123, tol, depth - 1, out);
    convex_cubic_to_quads(p0123, p123, p23, p3, tol, depth - 1, out);
}

/// Extract the sub-cubic of (`p0..p3`) on the parameter interval
/// `[ta, tb]` (two de Casteljau splits).
fn cubic_piece(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    ta: f64,
    tb: f64,
) -> ([f64; 2], [f64; 2], [f64; 2], [f64; 2]) {
    let lerp = |a: [f64; 2], b: [f64; 2], t: f64| {
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    };
    // Split off [ta, 1], then take the leading fraction of it.
    let split_right = |p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64| {
        let p01 = lerp(p0, p1, t);
        let p12 = lerp(p1, p2, t);
        let p23 = lerp(p2, p3, t);
        let p012 = lerp(p01, p12, t);
        let p123 = lerp(p12, p23, t);
        let pm = lerp(p012, p123, t);
        (pm, p123, p23, p3)
    };
    let split_left = |p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64| {
        let p01 = lerp(p0, p1, t);
        let p12 = lerp(p1, p2, t);
        let p23 = lerp(p2, p3, t);
        let p012 = lerp(p01, p12, t);
        let p123 = lerp(p12, p23, t);
        let pm = lerp(p012, p123, t);
        (p0, p01, p012, pm)
    };
    let (a0, a1, a2, a3) = split_right(p0, p1, p2, p3, ta);
    let local = if (1.0 - ta).abs() < 1e-12 {
        0.0
    } else {
        (tb - ta) / (1.0 - ta)
    };
    split_left(a0, a1, a2, a3, local)
}

/// Junction-angle budget for the midpoint-control fallback (radians,
/// per piece end). Capping the end-tangent rotation caps every
/// lowering-induced junction break at ≤ 2·QUAD_JUNCTION_ANGLE_MAX
/// ≈ 1.15° — comfortably below the G1-grouping threshold
/// (`PATH_G1_COS_THRESHOLD` 0.999 ≙ 2.56°), which therefore keeps
/// separating REAL sketch corners only. Without this cap, a piece at
/// the deviation tolerance can rotate its end tangent by ~3.5·tol/
/// handle — enough to cross the grouping threshold and split faces.
const QUAD_JUNCTION_ANGLE_MAX: f64 = 0.01;

/// Try to approximate a convex (inflection-free) cubic piece by ONE
/// quad within `tol`; returns the quad control point on success.
///
/// Preference order:
/// 1. Tangent-intersection control — preserves both end-tangent
///    directions EXACTLY (G1-exact junctions). Used when the piece
///    turns enough for the intersection to be well-conditioned and
///    the analytic deviation bound `¾·max(‖e1‖,‖e2‖)` (difference of
///    the degree-elevated control polygons, see [`cubic_to_quads`])
///    is within `tol`.
/// 2. Midpoint control `q = (3(p1+p2) − p0 − p3)/4` — exact deviation
///    bound `√3/36·‖d3‖` (cu2qu's bound), accepted when that is
///    within `tol` AND the end-tangent direction error stays inside
///    [`QUAD_JUNCTION_ANGLE_MAX`]. This is the only choice for
///    near-straight pieces, where the tangent intersection is
///    ill-conditioned (it can land arbitrarily far out).
fn quad_for_cubic(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    p3: [f64; 2],
    tol: f64,
) -> Option<[f64; 2]> {
    let d3 = [
        p3[0] - 3.0 * p2[0] + 3.0 * p1[0] - p0[0],
        p3[1] - 3.0 * p2[1] + 3.0 * p1[1] - p0[1],
    ];
    let d3_len = (d3[0] * d3[0] + d3[1] * d3[1]).sqrt();
    let err_mid = (3.0f64.sqrt() / 36.0) * d3_len;
    let t0 = [p1[0] - p0[0], p1[1] - p0[1]];
    let t1 = [p2[0] - p3[0], p2[1] - p3[1]];
    let l0 = (t0[0] * t0[0] + t0[1] * t0[1]).sqrt();
    let l1 = (t1[0] * t1[0] + t1[1] * t1[1]).sqrt();
    let denom = t0[0] * t1[1] - t0[1] * t1[0];
    // 1) Tangent intersection, when the turn is well-conditioned
    //    (|sin| of the angle between the end tangents ≥ ~1.1°).
    if denom.abs() > 0.02 * (l0 * l1).max(1e-300) {
        let s = ((p3[0] - p0[0]) * t1[1] - (p3[1] - p0[1]) * t1[0]) / denom;
        let c = [p0[0] + s * t0[0], p0[1] + s * t0[1]];
        if c[0].is_finite() && c[1].is_finite() {
            let e1 = [
                p1[0] - (p0[0] + 2.0 * c[0]) / 3.0,
                p1[1] - (p0[1] + 2.0 * c[1]) / 3.0,
            ];
            let e2 = [
                p2[0] - (p3[0] + 2.0 * c[0]) / 3.0,
                p2[1] - (p3[1] + 2.0 * c[1]) / 3.0,
            ];
            let m = (e1[0] * e1[0] + e1[1] * e1[1])
                .max(e2[0] * e2[0] + e2[1] * e2[1])
                .sqrt();
            if 0.75 * m <= tol {
                return Some(c);
            }
        }
    }
    // 2) Midpoint control with deviation + junction-angle budget.
    //    The elevated-control differences are e1 = d3/6, e2 = −d3/6
    //    EXACTLY, so the end-tangent rotation is the perpendicular
    //    component: δθ_i ≈ |cross(d3, t_i)| / (6·‖t_i‖²).
    let rot = |t: [f64; 2], l: f64| -> f64 {
        (d3[0] * t[1] - d3[1] * t[0]).abs() / (6.0 * (l * l).max(1e-300))
    };
    let angle_ok =
        rot(t0, l0) <= QUAD_JUNCTION_ANGLE_MAX && rot(t1, l1) <= QUAD_JUNCTION_ANGLE_MAX;
    if err_mid <= tol && angle_ok {
        return Some([
            (3.0 * (p1[0] + p2[0]) - p0[0] - p3[0]) / 4.0,
            (3.0 * (p1[1] + p2[1]) - p0[1] - p3[1]) / 4.0,
        ]);
    }
    None
}
