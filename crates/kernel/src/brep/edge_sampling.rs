//! Sample a B-Rep [`EdgeGeometry`] into a flat polyline for visualisation.
//!
//! The viewport draws edges as `LineSegments`, so it needs a point chain, not
//! the analytic curve. Woven edges are [`EdgeGeometry::Nurbs`] (a clamped
//! Bézier chain produced by [`crate::brep::weaving::hermite_to_nurbs`]); the
//! other variants are defensive fallbacks for hand-built / degenerate edges.
//!
//! All points come out in the **body-local** frame — the same frame the B-Rep
//! is woven in (DL-56). The caller folds the body `placement` back to world.

use crate::brep::{EdgeGeometry, NurbsCurve};

/// How many straight segments approximate one Bézier span. A cubic span is
/// visually smooth well below this; straight edges collapse to two collinear
/// points either way, so the cost is bounded.
const SAMPLES_PER_SPAN: usize = 16;

/// The **structural** points of an edge for the debug overlay, in the
/// body-local frame: the raw control points plus the curve degree. Unlike
/// [`sample_edge`] (a dense tessellation of ~16 points per span) this exposes
/// the *few* points that actually define the curve — so a NURBS edge built
/// from a handful of control points reads as a handful of markers instead of a
/// solid bead chain.
///
/// Returns `(control_points, degree)`. For a clamped Bézier-chain NURBS the
/// on-curve span joins sit at every `degree`-th control point (indices
/// `0, degree, 2·degree, …`); the points in between are the off-curve Bézier
/// handles. Polyline / Hermite edges have no separate handles, so every point
/// is a node and `degree` is reported as `1`.
pub fn edge_control_structure(geometry: &EdgeGeometry) -> (Vec<[f64; 3]>, usize) {
    match geometry {
        EdgeGeometry::Polyline(pts) => (pts.clone(), 1),
        EdgeGeometry::Hermite(samples) => (samples.iter().map(|s| s.point).collect(), 1),
        EdgeGeometry::Nurbs(curve) => (curve.control_points.clone(), curve.degree.max(1)),
    }
}

/// Sample `geometry` into a body-local polyline (≥ 2 points for any non-empty
/// edge). Returns the raw point chain; consecutive duplicate join points
/// between Bézier spans are dropped.
pub fn sample_edge(geometry: &EdgeGeometry) -> Vec<[f64; 3]> {
    sample_edge_dense(geometry, SAMPLES_PER_SPAN)
}

/// Like [`sample_edge`], but with a caller-chosen per-span sample count.
/// The export tessellator (Meshing Phase 2) oversamples (then decimates by
/// chord tolerance) so large arcs stay inside the export tolerance — the
/// viewport default of 16/span is a *visual* density, not a metric one.
pub fn sample_edge_dense(geometry: &EdgeGeometry, samples_per_span: usize) -> Vec<[f64; 3]> {
    match geometry {
        EdgeGeometry::Polyline(pts) => pts.clone(),
        // Hermite must not survive weaving (it is converted to NURBS), but a
        // hand-built or pre-weave edge may carry it — use the sample points.
        EdgeGeometry::Hermite(samples) => samples.iter().map(|s| s.point).collect(),
        EdgeGeometry::Nurbs(curve) => sample_nurbs(curve, samples_per_span),
    }
}

/// Sample a clamped Bézier-chain NURBS (the only kind the weaver produces) by
/// de Casteljau per span. Falls back to the control polygon if the knot/control
/// structure is not a clean `degree`-multiplicity Bézier chain.
fn sample_nurbs(curve: &NurbsCurve, samples_per_span: usize) -> Vec<[f64; 3]> {
    let d = curve.degree;
    let n = curve.control_points.len();
    let per_span = samples_per_span.max(1);
    // A clamped Bézier chain has `d * n_seg + 1` control points; bail to the
    // control polygon for anything that doesn't fit that shape.
    if d == 0 || n < d + 1 || !(n - 1).is_multiple_of(d) || curve.weights.len() != n {
        return curve.control_points.clone();
    }
    let n_seg = (n - 1) / d;
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(n_seg * per_span + 1);
    for seg in 0..n_seg {
        let lo = d * seg;
        let cps = &curve.control_points[lo..=lo + d];
        let ws = &curve.weights[lo..=lo + d];
        for i in 0..=per_span {
            // Skip the first point of every span after the first: it equals the
            // previous span's last point (C⁰ join), so emit it only once.
            if seg > 0 && i == 0 {
                continue;
            }
            let t = i as f64 / per_span as f64;
            out.push(de_casteljau(cps, ws, t));
        }
    }
    out
}

/// Rational de Casteljau evaluation of one Bézier span at parameter `t`.
/// Weights are carried in homogeneous coordinates and divided out at the end,
/// so uniform weights reduce to the ordinary polynomial Bézier.
fn de_casteljau(cps: &[[f64; 3]], ws: &[f64], t: f64) -> [f64; 3] {
    // Homogeneous control points: (w·x, w·y, w·z, w).
    let mut h: Vec<[f64; 4]> = cps
        .iter()
        .zip(ws)
        .map(|(p, &w)| [p[0] * w, p[1] * w, p[2] * w, w])
        .collect();
    let n = h.len();
    for r in 1..n {
        for i in 0..n - r {
            let (lo, hi) = (h[i], h[i + 1]);
            for (out, (&a, &b)) in h[i].iter_mut().zip(lo.iter().zip(hi.iter())) {
                *out = (1.0 - t) * a + t * b;
            }
        }
    }
    let w = if h[0][3].abs() > f64::EPSILON { h[0][3] } else { 1.0 };
    [h[0][0] / w, h[0][1] / w, h[0][2] / w]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brep::weaving::hermite_to_nurbs;
    use crate::brep::HermiteSample;

    fn close(a: [f64; 3], b: [f64; 3], eps: f64) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps && (a[2] - b[2]).abs() < eps
    }

    #[test]
    fn polyline_passthrough() {
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 2.0, 3.0]];
        assert_eq!(sample_edge(&EdgeGeometry::Polyline(pts.clone())), pts);
    }

    #[test]
    fn straight_nurbs_endpoints_match_and_stay_collinear() {
        // A straight segment → a single cubic span; its samples must start/end
        // at the Hermite endpoints and lie on the line between them.
        let samples = vec![
            HermiteSample { point: [0.0, 0.0, 0.0], tangent: [1.0, 0.0, 0.0] },
            HermiteSample { point: [3.0, 0.0, 0.0], tangent: [1.0, 0.0, 0.0] },
        ];
        let curve = hermite_to_nurbs(&samples);
        let poly = sample_edge(&EdgeGeometry::Nurbs(curve));
        assert!(poly.len() >= 2);
        assert!(close(*poly.first().unwrap(), [0.0, 0.0, 0.0], 1e-9));
        assert!(close(*poly.last().unwrap(), [3.0, 0.0, 0.0], 1e-9));
        for p in &poly {
            assert!(p[1].abs() < 1e-9 && p[2].abs() < 1e-9, "off-axis: {p:?}");
        }
    }

    #[test]
    fn two_span_chain_has_no_duplicate_join() {
        let samples = vec![
            HermiteSample { point: [0.0, 0.0, 0.0], tangent: [1.0, 0.0, 0.0] },
            HermiteSample { point: [1.0, 0.0, 0.0], tangent: [1.0, 0.0, 0.0] },
            HermiteSample { point: [2.0, 0.0, 0.0], tangent: [1.0, 0.0, 0.0] },
        ];
        let curve = hermite_to_nurbs(&samples);
        let poly = sample_edge(&EdgeGeometry::Nurbs(curve));
        // No two consecutive points coincide (the C⁰ join is emitted once).
        for w in poly.windows(2) {
            assert!(!close(w[0], w[1], 1e-12), "duplicate join: {:?}", w[0]);
        }
        assert!(close(*poly.last().unwrap(), [2.0, 0.0, 0.0], 1e-9));
    }
}
