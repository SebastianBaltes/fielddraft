//! Derive a 2D sketch profile from a **planar** B-Rep face's boundary.
//!
//! This is the kernel half of "extrude a face" (roadmap: B-Rep elements as an
//! extrude source). Given a planar face and a world-space [`SketchFrame`], it
//! walks the face's outer + inner loops, projects each bounding edge into the
//! frame's 2D `(u, v)` coordinates and rebuilds them as analytic
//! [`SketchSegment`]s — exact `Line` for straight edges, exact `Arc` for
//! circular edges, and a fine polyline of `Line`s as the fallback for a general
//! spline. The result is a [`Sketch2d`] region (outer `Path`, holes subtracted)
//! that the ordinary [`crate::Primitive::Extrude`] machinery sweeps — so a face
//! extrude reuses the whole sketch-extrude pipeline (SDF, frame composition,
//! boolean fold) for free.
//!
//! **Coordinate spaces.** Edge geometry is body-local (DL-56); the caller's
//! `placement` lifts it to world, and the `frame` (also world) projects it to
//! 2D. For a coplanar multi-face selection the caller passes ONE shared frame so
//! every face's region lands in the same `(u, v)` plane and can be unioned.
//!
//! **Curved faces are deliberately out of scope** (first throw): a non-planar
//! face has no single plane / no 2D parameter domain here, so [`face_region_2d`]
//! returns `None` for anything but [`HalfSpace::Plane`]. Thicken/offset of curved
//! faces is a separate future feature.

use fielddraft_protocol::{HalfSpace, Sketch2d, Sketch2dPrimitive, SketchSegment, Transform};

use crate::brep::{BRep, FaceId, Loop, SurfaceGeometry};
use crate::sketch_features::SketchFrame;

/// Project a planar face's boundary into `frame`'s 2D coordinates and build the
/// filled [`Sketch2d`] region (outer loop minus inner hole loops).
///
/// Returns `None` if the face is not an analytic plane (curved faces deferred)
/// or its outer loop is degenerate (no extrudable area).
pub fn face_region_2d(
    brep: &BRep,
    face_id: FaceId,
    frame: &SketchFrame,
    placement: &Transform,
) -> Option<Sketch2d> {
    // Planar faces only — a curved face has no single (u, v) plane here.
    match &brep.face(face_id).surface {
        SurfaceGeometry::Analytic { surface: HalfSpace::Plane { .. }, .. } => {}
        _ => return None,
    }

    let face = brep.face(face_id);
    let outer = loop_segments(brep, &face.outer, frame, placement);
    // One segment is legitimate (a full circular rim → a single Arc); only a
    // genuinely empty loop has no extrudable area.
    if outer.is_empty() {
        return None;
    }
    let mut region = Sketch2d::Prim(Sketch2dPrimitive::Path { segments: outer, closed: true });

    for hole in &face.inner {
        let hseg = loop_segments(brep, hole, frame, placement);
        if !hseg.is_empty() {
            region = Sketch2d::Difference(
                Box::new(region),
                Box::new(Sketch2d::Prim(Sketch2dPrimitive::Path { segments: hseg, closed: true })),
            );
        }
    }
    Some(region)
}

/// Walk one boundary loop into a chain of analytic 2D segments. Each B-Rep edge
/// is sampled (body-local), lifted to world by `placement`, projected to the
/// frame's `(u, v)`, then classified into `Line` / `Arc` / polyline.
fn loop_segments(
    brep: &BRep,
    lp: &Loop,
    frame: &SketchFrame,
    placement: &Transform,
) -> Vec<SketchSegment> {
    let mut segs: Vec<SketchSegment> = Vec::new();
    for &(edge_id, reversed) in &lp.edges {
        let mut pts = crate::brep::edge_sampling::sample_edge(&brep.edge(edge_id).geometry);
        if reversed {
            pts.reverse();
        }
        let uv: Vec<[f32; 2]> = pts
            .iter()
            .map(|p| project(place_point(placement, *p), frame))
            .collect();
        segs.extend(edge_to_segments(&uv));
    }
    segs
}

/// Body-local → world: `placement` applies scale, then rotation, then
/// translation (mirrors `face_plane_frame`'s centroid mapping).
fn place_point(placement: &Transform, p: [f64; 3]) -> [f32; 3] {
    let scaled = [
        p[0] as f32 * placement.scale[0],
        p[1] as f32 * placement.scale[1],
        p[2] as f32 * placement.scale[2],
    ];
    let r = crate::quat_rotate(placement.rotation, scaled);
    [
        r[0] + placement.translate[0],
        r[1] + placement.translate[1],
        r[2] + placement.translate[2],
    ]
}

/// World point → frame 2D: `(u, v) = ((p − origin)·x_axis, (p − origin)·y_axis)`.
/// The component along the normal is dropped (≈ 0 for an in-plane point).
fn project(p: [f32; 3], frame: &SketchFrame) -> [f32; 2] {
    let d = [p[0] - frame.origin[0], p[1] - frame.origin[1], p[2] - frame.origin[2]];
    [dot(d, frame.x_axis), dot(d, frame.y_axis)]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

/// Classify one edge's projected point chain into analytic segments.
///
/// - collinear → a single exact `Line`,
/// - on a common circle → a single exact `Arc` (full `±2π` when the chain
///   closes, e.g. a cylinder cap rim),
/// - otherwise → a polyline of `Line`s (the general-spline fallback).
fn edge_to_segments(uv_in: &[[f32; 2]]) -> Vec<SketchSegment> {
    // Drop consecutive duplicates (Bézier-span C⁰ joins project onto the same
    // point); remember whether the *original* chain closed on itself.
    let mut uv: Vec<[f32; 2]> = Vec::with_capacity(uv_in.len());
    for &p in uv_in {
        if uv.last().is_none_or(|q| dist2(*q, p) > 1e-12) {
            uv.push(p);
        }
    }
    let n = uv.len();
    if n < 2 {
        return Vec::new();
    }
    if n == 2 {
        return vec![SketchSegment::Line { start: uv[0], end: uv[1] }];
    }

    // Tolerance scaled to the edge's own extent.
    let (mut mn, mut mx) = (uv[0], uv[0]);
    for p in &uv {
        mn = [mn[0].min(p[0]), mn[1].min(p[1])];
        mx = [mx[0].max(p[0]), mx[1].max(p[1])];
    }
    let diag = ((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2)).sqrt();
    let tol = (diag * 1e-4).max(1e-6);

    // Collinear → one exact line spanning the whole edge.
    if max_perp_dist(&uv) <= tol {
        return vec![SketchSegment::Line { start: uv[0], end: uv[n - 1] }];
    }

    // Circular → one exact arc.
    let closed_loop = dist2(uv_in[0], uv_in[uv_in.len() - 1]).sqrt() <= tol;
    if let Some((center, radius)) = fit_circle(&uv, tol) {
        let start_angle = (uv[0][1] - center[1]).atan2(uv[0][0] - center[0]);
        let walked = total_sweep(&uv, center);
        let sweep_angle = if closed_loop {
            std::f32::consts::TAU * walked.signum()
        } else {
            walked
        };
        return vec![SketchSegment::Arc { center, radius, start_angle, sweep_angle }];
    }

    // General spline → polyline fallback (still exact distance per Line span).
    uv.windows(2)
        .map(|w| SketchSegment::Line { start: w[0], end: w[1] })
        .collect()
}

/// Largest perpendicular distance of any point from the chord `first→last`.
fn max_perp_dist(uv: &[[f32; 2]]) -> f32 {
    let a = uv[0];
    let b = uv[uv.len() - 1];
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len = (ab[0] * ab[0] + ab[1] * ab[1]).sqrt();
    if len < 1e-9 {
        // Degenerate chord (closed edge): fall back to spread about `a`.
        return uv
            .iter()
            .map(|p| (dist2(*p, a)).sqrt())
            .fold(0.0, f32::max);
    }
    let (nx, ny) = (-ab[1] / len, ab[0] / len);
    uv.iter()
        .map(|p| ((p[0] - a[0]) * nx + (p[1] - a[1]) * ny).abs())
        .fold(0.0, f32::max)
}

/// Fit a circle through three well-spread points and accept it only if **every**
/// sample lies within `tol` of that circle. Returns `(center, radius)`.
fn fit_circle(uv: &[[f32; 2]], tol: f32) -> Option<([f32; 2], f32)> {
    let n = uv.len();
    // Three well-spread samples (≈120° apart on a circle). Avoid index `n-1`:
    // a closed rim's last sample coincides with the first, which would collapse
    // the circumcircle to two distinct points.
    let (a, b, c) = (uv[0], uv[n / 3], uv[(2 * n) / 3]);
    let (center, radius) = circumcircle(a, b, c)?;
    // Radius must be sane and every point on the circle within tolerance.
    let band = tol.max(radius * 1e-3);
    for p in uv {
        if ((dist2(*p, center)).sqrt() - radius).abs() > band {
            return None;
        }
    }
    Some((center, radius))
}

/// Circumcircle of three 2D points; `None` if (near-)collinear.
fn circumcircle(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> Option<([f32; 2], f32)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a[0] * a[0] + a[1] * a[1];
    let b2 = b[0] * b[0] + b[1] * b[1];
    let c2 = c[0] * c[0] + c[1] * c[1];
    let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
    let center = [ux, uy];
    Some((center, (dist2(center, a)).sqrt()))
}

/// Signed angle swept walking the chain around `center` (Σ wrapped deltas).
fn total_sweep(uv: &[[f32; 2]], center: [f32; 2]) -> f32 {
    let ang = |p: [f32; 2]| (p[1] - center[1]).atan2(p[0] - center[0]);
    let mut sum = 0.0_f32;
    for w in uv.windows(2) {
        let mut d = ang(w[1]) - ang(w[0]);
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        sum += d;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_chain_collapses_to_one_line() {
        let uv = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]];
        let segs = edge_to_segments(&uv);
        assert_eq!(segs.len(), 1);
        match segs[0] {
            SketchSegment::Line { start, end } => {
                assert_eq!(start, [0.0, 0.0]);
                assert_eq!(end, [3.0, 0.0]);
            }
            other => panic!("expected Line, got {other:?}"),
        }
    }

    #[test]
    fn quarter_circle_becomes_one_arc() {
        // Unit circle, first quadrant sampled densely.
        let n = 33;
        let uv: Vec<[f32; 2]> = (0..=n)
            .map(|i| {
                let t = (i as f32 / n as f32) * std::f32::consts::FRAC_PI_2;
                [t.cos(), t.sin()]
            })
            .collect();
        let segs = edge_to_segments(&uv);
        assert_eq!(segs.len(), 1);
        match segs[0] {
            SketchSegment::Arc { center, radius, start_angle, sweep_angle } => {
                assert!((center[0]).abs() < 1e-3 && (center[1]).abs() < 1e-3, "center {center:?}");
                assert!((radius - 1.0).abs() < 1e-3, "radius {radius}");
                assert!(start_angle.abs() < 1e-3, "start {start_angle}");
                assert!(
                    (sweep_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-2,
                    "sweep {sweep_angle}"
                );
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }

    #[test]
    fn full_circle_sweeps_tau() {
        let n = 64;
        let mut uv: Vec<[f32; 2]> = (0..=n)
            .map(|i| {
                let t = (i as f32 / n as f32) * std::f32::consts::TAU;
                [2.0 * t.cos(), 2.0 * t.sin()]
            })
            .collect();
        // Make the closing sample exactly coincide with the first.
        *uv.last_mut().unwrap() = uv[0];
        let segs = edge_to_segments(&uv);
        assert_eq!(segs.len(), 1);
        match segs[0] {
            SketchSegment::Arc { radius, sweep_angle, .. } => {
                assert!((radius - 2.0).abs() < 1e-3, "radius {radius}");
                assert!(
                    (sweep_angle.abs() - std::f32::consts::TAU).abs() < 1e-4,
                    "sweep {sweep_angle}"
                );
            }
            other => panic!("expected Arc, got {other:?}"),
        }
    }
}
