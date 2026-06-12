//! Dual-number SDF evaluation for exact analytical gradients.
//!
//! This module provides three layers:
//!   A) Generic primitive SDF functions over `T: DualNum<f64>`
//!   B) Concrete BVH tree traversal using `DualSVec64<3>`
//!   C) Public API: `sdf_gradient`

use nalgebra::SVector;
use num_dual::{Derivative, DualNum, DualSVec64};

use fielddraft_protocol::{HalfSpace, Primitive, Transform};

use crate::bvh::{distance_point_to_aabb, BoundedCsg};
use crate::csg::{BlendMode, EvalMode};
use crate::sketch2d::{distance_point_to_aabb2, Sketch2dPrimitive};

// ─── Type alias ──────────────────────────────────────────────────────
type D3 = DualSVec64<3>;

/// `sqrt` that avoids NaN derivatives at zero. Many SDF primitives
/// use `sqrt(x² + z²)` for radial distance, which has an undefined
/// gradient on the axis (x=z=0). Adding a tiny epsilon keeps the
/// derivative well-defined without measurably changing the SDF value.
#[inline]
fn safe_sqrt<T: DualNum<f64> + Copy>(x: T) -> T {
    (x + T::from(1e-20)).sqrt()
}

// ═══════════════════════════════════════════════════════════════════════
// Layer A: Generic primitive SDF functions
// ═══════════════════════════════════════════════════════════════════════

/// Generic SDF of a single primitive at point `p`.
/// Mirrors `crate::sdf_primitive` but works with any `T: DualNum<f64>`.
///
/// Extrude, Revolve, and ImportedMesh return a zero-derivative fallback
/// (`T::from(f32_result as f64)`) **only on this bare-primitive path**, which
/// real bodies never take: an extrude/revolve always reaches the kernel as a
/// `BoundedCsg::{Extrude,Revolve}` carrying its `BoundedSketch2d`, and that is
/// evaluated at the tree level (Layer B, `eval_bounded_d3`) with an EXACT dual
/// gradient — the sketch SDF is fully `DualNum`-generic (Layer A.5/A.6), with
/// the closest-point parameter and the inside/outside sign frozen on the real
/// parts (exact a.e. by the envelope theorem). Only `ImportedMesh` truly has no
/// analytic gradient and falls back to finite differences (`fd_fallback_3d`).
fn sdf_primitive_generic<T: DualNum<f64> + Copy + PartialOrd>(
    prim: &Primitive,
    p: [T; 3],
) -> T {
    match prim {
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0] - T::from(center[0] as f64)).abs() - T::from(half_extents[0] as f64),
                (p[1] - T::from(center[1] as f64)).abs() - T::from(half_extents[1] as f64),
                (p[2] - T::from(center[2] as f64)).abs() - T::from(half_extents[2] as f64),
            ];
            let zero = T::from(0.0);
            let q0 = if q[0] > zero { q[0] } else { zero };
            let q1 = if q[1] > zero { q[1] } else { zero };
            let q2 = if q[2] > zero { q[2] } else { zero };
            let outside = safe_sqrt(q0 * q0 + q1 * q1 + q2 * q2);
            // inside = min(max(q[0], q[1], q[2]), 0)
            let qmax = if q[0] > q[1] { q[0] } else { q[1] };
            let qmax = if qmax > q[2] { qmax } else { q[2] };
            let inside = if qmax < zero { qmax } else { zero };
            outside + inside
        }
        Primitive::Sphere { center, radius } => {
            let dx = p[0] - T::from(center[0] as f64);
            let dy = p[1] - T::from(center[1] as f64);
            let dz = p[2] - T::from(center[2] as f64);
            safe_sqrt(dx * dx + dy * dy + dz * dz) - T::from(*radius as f64)
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = p[0] - T::from(center[0] as f64);
            let dy = p[1] - T::from(center[1] as f64);
            let dz = p[2] - T::from(center[2] as f64);
            let radial = safe_sqrt(dx * dx + dz * dz) - T::from(*radius as f64);
            let axial = dy.abs() - T::from(*half_height as f64);
            let zero = T::from(0.0);
            let r_clamped = if radial > zero { radial } else { zero };
            let a_clamped = if axial > zero { axial } else { zero };
            let outside = safe_sqrt(r_clamped * r_clamped + a_clamped * a_clamped);
            let inside_max = if radial > axial { radial } else { axial };
            let inside = if inside_max < zero { inside_max } else { zero };
            outside + inside
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            let dx = p[0] - T::from(center[0] as f64);
            let dy = p[1] - T::from(center[1] as f64);
            let dz = p[2] - T::from(center[2] as f64);
            let q0 = safe_sqrt(dx * dx + dz * dz) - T::from(*major_radius as f64);
            let q1 = dy;
            safe_sqrt(q0 * q0 + q1 * q1) - T::from(*minor_radius as f64)
        }
        Primitive::Gyroid { cell_size, thickness } => {
            let s = T::from(*cell_size as f64);
            let x = p[0] / s;
            let y = p[1] / s;
            let z = p[2] / s;
            let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            (f.abs() - T::from(*thickness as f64)) * s * T::from(0.2887)
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            let r = safe_sqrt(p[0] * p[0] + p[2] * p[2]);
            // Axis guard: ∂θ/∂(x,z) = (-z, x)/r² diverges as r→0 and produces NaN
            // dual gradients on the helix axis. The axis is deep interior
            // (r ≪ nominal_radius), so the surface never depends on θ there —
            // freeze θ's derivative below eps_axis to keep the gradient finite.
            let theta = if r.re() > 1e-4 {
                p[2].atan2(p[0])
            } else {
                T::from(p[2].re().atan2(p[0].re()))
            };
            let tau = T::from(std::f64::consts::TAU);
            let y_unwrapped = p[1] - T::from(*pitch as f64) * theta / tau;
            let phase = (y_unwrapped / T::from(*pitch as f64)) * tau;
            let r_profile = T::from(*nominal_radius as f64)
                - T::from(*depth as f64) * T::from(0.5) * (T::from(1.0) - phase.cos());
            let d_radial = r - r_profile;
            let d_axial = p[1].abs() - T::from(*half_height as f64);
            let d = if d_radial > d_axial { d_radial } else { d_axial };
            // Lipschitz normalization (constant in p → preserves gradient
            // direction, scales magnitude to ≤1): the helix folds y into
            // r_profile's phase, so ∂r_profile/∂y = π·depth/pitch and |∇f| peaks
            // at sqrt(1 + (π·depth/pitch)²) ≈ 1.86 for the default thread. The raw
            // Chebyshev max overestimates distance; dividing by the exact bound
            // makes it a conservative distance so the sphere-march stops
            // overshooting the groove walls (cf. Gyroid 0.2887, Cone wall_scale).
            let pitch64 = (*pitch as f64).max(1e-6);
            let lip = (1.0 + (std::f64::consts::PI * (*depth as f64) / pitch64).powi(2)).sqrt();
            d * T::from(1.0 / lip)
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = safe_sqrt(p[0] * p[0] + p[2] * p[2]);
            let hh = T::from(*half_height as f64);
            let two_hh = hh * T::from(2.0);
            // t = clamp((p[1] + hh) / (2*hh), 0, 1) — use re() for branch
            let t_raw = (p[1] + hh) / two_hh;
            let zero = T::from(0.0);
            let one = T::from(1.0);
            let t = if t_raw.re() < 0.0 {
                zero
            } else if t_raw.re() > 1.0 {
                one
            } else {
                t_raw
            };
            let r_at_y = T::from(*r_bottom as f64) * (one - t) + T::from(*r_top as f64) * t;
            // Project the radial gap onto the inclined wall normal (constant
            // scalar factor) so the field is the true perpendicular distance
            // and stays Lipschitz-1. For a cylinder (dr == 0) the factor is 1.
            let dr = (*r_bottom - *r_top) as f64;
            let slant_len = (dr * dr + (2.0 * *half_height as f64) * (2.0 * *half_height as f64)).sqrt();
            let wall_scale = if slant_len > 1e-9 { (2.0 * *half_height as f64) / slant_len } else { 1.0 };
            let d_wall = (r - r_at_y) * T::from(wall_scale);
            let d_axial = p[1].abs() - hh;
            // Branch on real parts
            if d_wall.re() > 0.0 && d_axial.re() > 0.0 {
                safe_sqrt(d_wall * d_wall + d_axial * d_axial)
            } else {
                if d_wall > d_axial { d_wall } else { d_axial }
            }
        }
        Primitive::Capsule { radius, half_height } => {
            let r = safe_sqrt(p[0] * p[0] + p[2] * p[2]);
            let hh = T::from(*half_height as f64);
            let neg_hh = T::from(-*half_height as f64);
            let y_clamped = if p[1].re() < (-*half_height as f64) {
                neg_hh
            } else if p[1].re() > (*half_height as f64) {
                hh
            } else {
                p[1]
            };
            let dy = p[1] - y_clamped;
            (r * r + dy * dy).sqrt() - T::from(*radius as f64)
        }
        Primitive::HexPrism { apothem, half_height } => {
            let qx = p[0].abs();
            let qz = p[2].abs();
            let hex_term = qx * T::from(0.866_025_4_f64) + qz * T::from(0.5);
            let d_hex = (if hex_term > qz { hex_term } else { qz })
                - T::from(*apothem as f64);
            let d_axial = p[1].abs() - T::from(*half_height as f64);
            if d_hex.re() > 0.0 && d_axial.re() > 0.0 {
                safe_sqrt(d_hex * d_hex + d_axial * d_axial)
            } else {
                if d_hex > d_axial { d_hex } else { d_axial }
            }
        }
        Primitive::Ellipsoid { radii } => {
            let r_min = radii[0].min(radii[1]).min(radii[2]);
            let q = [
                p[0] / T::from(radii[0] as f64),
                p[1] / T::from(radii[1] as f64),
                p[2] / T::from(radii[2] as f64),
            ];
            let len = safe_sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2]);
            (len - T::from(1.0)) * T::from(r_min as f64)
        }
        // Fallback for Extrude/Revolve/ImportedMesh: evaluate with f32 and
        // lift to T without derivative. Proper dual-number handling for
        // these happens at the tree level (Layer B).
        Primitive::Extrude { .. }
        | Primitive::Revolve { .. }
        | Primitive::ImportedMesh { .. } => {
            let p_f32 = [p[0].re() as f32, p[1].re() as f32, p[2].re() as f32];
            let d = crate::sdf_primitive(prim, p_f32);
            T::from(d as f64)
        }
    }
}

pub(crate) fn sdf_halfspace_generic<T: DualNum<f64> + Copy + PartialOrd>(
    hs: &HalfSpace,
    p: [T; 3],
) -> T {
    match hs {
        HalfSpace::Plane { normal, offset } => {
            p[0] * T::from(normal[0] as f64)
                + p[1] * T::from(normal[1] as f64)
                + p[2] * T::from(normal[2] as f64)
                - T::from(*offset as f64)
        }
        HalfSpace::InfiniteCylinder { radius } => {
            safe_sqrt(p[0] * p[0] + p[2] * p[2]) - T::from(*radius as f64)
        }
        HalfSpace::Sphere { radius } => {
            safe_sqrt(p[0] * p[0] + p[1] * p[1] + p[2] * p[2]) - T::from(*radius as f64)
        }
        HalfSpace::Cone { half_angle } => {
            let r = safe_sqrt(p[0] * p[0] + p[2] * p[2]);
            let sin_a = T::from((*half_angle as f64).sin());
            let cos_a = T::from((*half_angle as f64).cos());
            r * cos_a - p[1].abs() * sin_a
        }
        HalfSpace::Torus { major_radius, minor_radius } => {
            let r_xz = safe_sqrt(p[0] * p[0] + p[2] * p[2]);
            let q0 = r_xz - T::from(*major_radius as f64);
            safe_sqrt(q0 * q0 + p[1] * p[1]) - T::from(*minor_radius as f64)
        }
        HalfSpace::Gyroid { cell_size, thickness } => {
            let s = T::from(*cell_size as f64);
            let x = p[0] / s;
            let y = p[1] / s;
            let z = p[2] / s;
            let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            (f.abs() - T::from(*thickness as f64)) * s * T::from(0.2887)
        }
        HalfSpace::RevolvedProfile { segments, ccw, offset } => {
            // First-order lift: the profile's signed distance and (r,y)-normal
            // are evaluated at the PRIMAL point (the closest-point solve has no
            // generic dual form), then linearised as d ≈ d0 + n·((r,y)−(r0,y0)).
            // A distance field is exactly first-order along its normal, which is
            // all the dual consumers read (value + gradient); curvature terms
            // beyond that are dropped.
            let r_t = safe_sqrt(p[0] * p[0] + p[2] * p[2]);
            let r0 = r_t.re();
            let y0 = p[1].re();
            let (d0, n2) = fielddraft_protocol::sketch2d::group_signed_distance_and_normal(
                segments,
                *ccw,
                [r0 as f32, y0 as f32],
            );
            T::from(d0 as f64 + *offset as f64)
                + (r_t - T::from(r0)) * T::from(n2[0] as f64)
                + (p[1] - T::from(y0)) * T::from(n2[1] as f64)
        }
    }
}

/// Rotate a dual vector `v` by quaternion `q = [x, y, z, w]` (constant), via the
/// standard `v + 2w(q×v) + 2 q×(q×v)` form. Used by the dual open-shell gate.
fn quat_rotate_d3(q: [f32; 4], v: [D3; 3]) -> [D3; 3] {
    let (qx, qy, qz, qw) = (
        D3::from(q[0] as f64),
        D3::from(q[1] as f64),
        D3::from(q[2] as f64),
        D3::from(q[3] as f64),
    );
    let two = D3::from(2.0);
    let tx = (qy * v[2] - qz * v[1]) * two;
    let ty = (qz * v[0] - qx * v[2]) * two;
    let tz = (qx * v[1] - qy * v[0]) * two;
    [
        v[0] + qw * tx + (qy * tz - qz * ty),
        v[1] + qw * ty + (qz * tx - qx * tz),
        v[2] + qw * tz + (qx * ty - qy * tx),
    ]
}

/// Dual-number mirror of [`crate::csg::OpenSurface::eval`]: the outward,
/// world-distance gate of one open face (DL-75). Maps the dual point world→local
/// (rigid inverse undoing the placement scale), evaluates the face's analytic
/// [`HalfSpace`], then Eikonal-normalises by the real world-gradient magnitude (a
/// scalar, so it scales the dual cleanly).
fn open_surface_eval_d3(f: &crate::csg::OpenSurface, p: [D3; 3]) -> D3 {
    let t = &f.placement;
    let d = [
        p[0] - D3::from(t.translate[0] as f64),
        p[1] - D3::from(t.translate[1] as f64),
        p[2] - D3::from(t.translate[2] as f64),
    ];
    let conj = [-t.rotation[0], -t.rotation[1], -t.rotation[2], t.rotation[3]];
    let r = quat_rotate_d3(conj, d);
    let lp = [
        r[0] / D3::from(t.scale[0] as f64),
        r[1] / D3::from(t.scale[1] as f64),
        r[2] / D3::from(t.scale[2] as f64),
    ];
    let fval = sdf_halfspace_generic(&f.surface, lp);
    let lpr = [lp[0].re as f32, lp[1].re as f32, lp[2].re as f32];
    let grad = crate::csg::grad_halfspace(&f.surface, lpr);
    let gn = ((grad[0] / t.scale[0]).powi(2)
        + (grad[1] / t.scale[1]).powi(2)
        + (grad[2] / t.scale[2]).powi(2))
    .sqrt();
    if gn > 1e-9 {
        fval / D3::from(gn as f64)
    } else {
        fval
    }
}

// ─── Generic helpers ─────────────────────────────────────────────────

/// Generic extrude composition. Mirrors `crate::csg::sdf_extrude`.
fn sdf_extrude_generic<T: DualNum<f64> + Copy + PartialOrd>(d2: T, z: T, half_h: T) -> T {
    let wz = z.abs() - half_h;
    let zero = T::from(0.0);
    let d2c = if d2 > zero { d2 } else { zero };
    let wzc = if wz > zero { wz } else { zero };
    let outside = safe_sqrt(d2c * d2c + wzc * wzc);
    let dmax = if d2 > wz { d2 } else { wz };
    let inside = if dmax < zero { dmax } else { zero };
    outside + inside
}

/// Generic smin polynomial. Mirrors `crate::csg::smin_poly`.
fn smin_poly_generic<T: DualNum<f64> + Copy + PartialOrd>(da: T, db: T, r: T) -> T {
    if r.re() <= 0.0 {
        return if da < db { da } else { db };
    }
    let diff = da - db;
    let abs_diff = diff.abs();
    let h_num = r - abs_diff;
    let zero = T::from(0.0);
    let h_clamped = if h_num > zero { h_num } else { zero };
    let h = h_clamped / r;
    let min_ab = if da < db { da } else { db };
    min_ab - h * h * h * r / T::from(6.0)
}

/// Generic chamfer union. Mirrors `crate::csg::chamfer_union`.
fn chamfer_union_generic<T: DualNum<f64> + Copy + PartialOrd>(da: T, db: T, r: T) -> T {
    let sqrt_half = T::from(std::f64::consts::FRAC_1_SQRT_2);
    let min_ab = if da < db { da } else { db };
    let chamfer = (da + db - r) * sqrt_half;
    if min_ab < chamfer { min_ab } else { chamfer }
}

/// Dispatch smooth-min by blend mode. Mirrors `crate::csg::blend_min`.
fn blend_min_generic<T: DualNum<f64> + Copy + PartialOrd>(
    da: T,
    db: T,
    r: T,
    mode: BlendMode,
) -> T {
    match mode {
        BlendMode::Fillet => smin_poly_generic(da, db, r),
        BlendMode::Chamfer => chamfer_union_generic(da, db, r),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Layer B: Concrete tree evaluation with D3 = DualSVec64<3>
// ═══════════════════════════════════════════════════════════════════════

/// Quaternion inverse-rotation for D3 vectors. Mirrors
/// `crate::quat_inverse_rotate` — quaternion components are f32
/// constants lifted to D3 via `D3::from(val as f64)`.
fn quat_inverse_rotate_d3(q: [f32; 4], v: [D3; 3]) -> [D3; 3] {
    let qx = D3::from(-q[0] as f64);
    let qy = D3::from(-q[1] as f64);
    let qz = D3::from(-q[2] as f64);
    let qw = D3::from(q[3] as f64);
    let two = D3::from(2.0);
    let t = [
        two * (qy * v[2] - qz * v[1]),
        two * (qz * v[0] - qx * v[2]),
        two * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Transform a D3 world-space point into a feature's local frame.
/// Mirrors `crate::world_to_local`.
fn world_to_local_d3(t: &Transform, p: [D3; 3]) -> [D3; 3] {
    let p = [
        p[0] - D3::from(t.translate[0] as f64),
        p[1] - D3::from(t.translate[1] as f64),
        p[2] - D3::from(t.translate[2] as f64),
    ];
    let p = quat_inverse_rotate_d3(t.rotation, p);
    [
        p[0] / D3::from(t.scale[0] as f64),
        p[1] / D3::from(t.scale[1] as f64),
        p[2] / D3::from(t.scale[2] as f64),
    ]
}

/// Construct a D3 seeded with partial derivative w.r.t. axis `i`.
fn seed_d3(val: f64, axis: usize) -> D3 {
    let mut eps = SVector::<f64, 3>::zeros();
    eps[axis] = 1.0;
    D3::new(val, Derivative::some(eps))
}

/// Finite-difference fallback for 3D functions (ImportedMesh).
fn fd_fallback_3d(f: &dyn Fn([f32; 3]) -> f32, p: [D3; 3]) -> D3 {
    let pr = [p[0].re as f32, p[1].re as f32, p[2].re as f32];
    let val = f(pr) as f64;
    let eps = 1e-3_f32;
    let ddx = ((f([pr[0] + eps, pr[1], pr[2]]) - f([pr[0] - eps, pr[1], pr[2]])) as f64)
        / (2.0 * eps as f64);
    let ddy = ((f([pr[0], pr[1] + eps, pr[2]]) - f([pr[0], pr[1] - eps, pr[2]])) as f64)
        / (2.0 * eps as f64);
    let ddz = ((f([pr[0], pr[1], pr[2] + eps]) - f([pr[0], pr[1], pr[2] - eps])) as f64)
        / (2.0 * eps as f64);
    let p0_eps = p[0].eps.unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
    let p1_eps = p[1].eps.unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
    let p2_eps = p[2].eps.unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
    let result_eps = p0_eps * ddx + p1_eps * ddy + p2_eps * ddz;
    D3::new(val, Derivative::some(result_eps))
}

/// Concrete wrapper around `sdf_primitive_generic` with FD fallback
/// for ImportedMesh.
fn eval_primitive_d3(prim: &Primitive, p: [D3; 3]) -> D3 {
    match prim {
        Primitive::ImportedMesh { mesh } => fd_fallback_3d(&|q| mesh.sdf(q), p),
        _ => sdf_primitive_generic(prim, p),
    }
}

/// Full BVH tree traversal with dual numbers. Mirrors `BoundedCsg::sdf`
/// in `bvh.rs`, using D3 for exact gradient propagation.
fn eval_bounded_d3(node: &BoundedCsg, p: [D3; 3], mode: EvalMode) -> D3 {
    match node {
        BoundedCsg::Empty => D3::from(f64::INFINITY),
        BoundedCsg::Leaf {
            primitive,
            world_transform,
            ..
        } => {
            let local = world_to_local_d3(world_transform, p);
            eval_primitive_d3(primitive, local) * D3::from(world_transform.min_scale() as f64)
        }
        BoundedCsg::Union { left, right, .. } => {
            let p_real = [p[0].re as f32, p[1].re as f32, p[2].re as f32];
            let dist_left = distance_point_to_aabb(p_real, left.aabb());
            let dist_right = distance_point_to_aabb(p_real, right.aabb());
            let (first_node, second_node, second_bound) = if dist_left <= dist_right {
                (&**left, &**right, dist_right)
            } else {
                (&**right, &**left, dist_left)
            };
            let d_first = eval_bounded_d3(first_node, p, mode);
            if second_bound > 0.0 && (d_first.re as f32) < second_bound {
                d_first
            } else {
                let d_second = eval_bounded_d3(second_node, p, mode);
                if d_first < d_second { d_first } else { d_second }
            }
        }
        BoundedCsg::Difference { left, right, .. } => {
            let p_real = [p[0].re as f32, p[1].re as f32, p[2].re as f32];
            let d_left = eval_bounded_d3(left, p, mode);
            let dist_right = distance_point_to_aabb(p_real, right.aabb());
            if dist_right > 0.0 && (d_left.re as f32) >= -dist_right {
                d_left
            } else {
                let d_right = eval_bounded_d3(right, p, mode);
                let neg_right = -d_right;
                if d_left > neg_right { d_left } else { neg_right }
            }
        }
        BoundedCsg::Intersection { left, right, .. } => {
            let da = eval_bounded_d3(left, p, mode);
            let db = eval_bounded_d3(right, p, mode);
            if da > db { da } else { db }
        }
        BoundedCsg::SmoothUnion {
            left,
            right,
            radius,
            mode: blend_mode,
            ..
        } => {
            let da = eval_bounded_d3(left, p, mode);
            let db = eval_bounded_d3(right, p, mode);
            match mode {
                EvalMode::Visual => {
                    blend_min_generic(da, db, D3::from(*radius as f64), *blend_mode)
                }
                EvalMode::Logical => {
                    if da < db { da } else { db }
                }
            }
        }
        BoundedCsg::SmoothDifference {
            left,
            right,
            radius,
            mode: blend_mode,
            ..
        } => {
            let da = eval_bounded_d3(left, p, mode);
            let db = eval_bounded_d3(right, p, mode);
            match mode {
                EvalMode::Visual => {
                    -blend_min_generic(-da, db, D3::from(*radius as f64), *blend_mode)
                }
                EvalMode::Logical => {
                    let neg_db = -db;
                    if da > neg_db { da } else { neg_db }
                }
            }
        }
        BoundedCsg::Twist { inner, rate, .. } => {
            let angle = D3::from(*rate as f64) * p[1];
            let (s, c) = angle.sin_cos();
            let twisted = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
            eval_bounded_d3(inner, twisted, mode)
        }
        BoundedCsg::Bend { inner, rate, .. } => {
            let angle = D3::from(*rate as f64) * p[0];
            let (s, c) = angle.sin_cos();
            let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
            eval_bounded_d3(inner, bent, mode)
        }
        BoundedCsg::Displace {
            inner,
            depth,
            freq,
            ..
        } => {
            let d = eval_bounded_d3(inner, p, mode);
            let freq_d = D3::from(*freq as f64);
            let disp = D3::from(*depth as f64)
                * (p[0] * freq_d).sin()
                * (p[1] * freq_d * D3::from(1.3)).sin()
                * (p[2] * freq_d * D3::from(0.7)).sin();
            d - disp
        }
        BoundedCsg::Offset { inner, delta, .. } => {
            eval_bounded_d3(inner, p, mode) - D3::from(*delta as f64)
        }
        BoundedCsg::Shell { inner, thickness, open_faces, .. } => {
            // Mirror `crate::csg::open_shell_sdf` exactly (DL-75): the inward wall
            // `max(d, −(d + e))` with local erosion `e = clamp(d − g, 0, t)`,
            // `g = max` over the open-face gates. The OLD centred `|d| − t` here
            // was stale (it pre-dated the open-shell rework) and disagreed with
            // the renderer / `evaluate`, so once the weave stopped skipping shells
            // the B-Rep's surface_gradient read the wrong sheet.
            let d = eval_bounded_d3(inner, p, mode);
            let t = D3::from(*thickness as f64);
            let e = if open_faces.is_empty() {
                t
            } else {
                let mut g = open_surface_eval_d3(&open_faces[0], p);
                for f in &open_faces[1..] {
                    let gate = open_surface_eval_d3(f, p);
                    if gate.re > g.re {
                        g = gate;
                    }
                }
                // e = clamp(d − g, 0, t).
                let de = d - g;
                let lo = if de.re > 0.0 { de } else { D3::from(0.0) };
                if lo.re < t.re {
                    lo
                } else {
                    t
                }
            };
            // shell = max(d, −(d + e)).
            let neg = -(d + e);
            if d.re >= neg.re {
                d
            } else {
                neg
            }
        }
        BoundedCsg::Morph {
            left, right, t, ..
        } => {
            let da = eval_bounded_d3(left, p, mode);
            let db = eval_bounded_d3(right, p, mode);
            let t_d = D3::from(*t as f64);
            (D3::from(1.0) - t_d) * da + t_d * db
        }
        BoundedCsg::MaskedBlend {
            left,
            right,
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
            ..
        } => {
            let da = eval_bounded_d3(left, p, mode);
            let db_raw = eval_bounded_d3(right, p, mode);
            match mode {
                EvalMode::Logical => {
                    if *is_difference {
                        let neg_db = -db_raw;
                        if da > neg_db { da } else { neg_db }
                    } else {
                        if da < db_raw { da } else { db_raw }
                    }
                }
                EvalMode::Visual => {
                    // Use f32 for the mask check (real parts only)
                    let p_real = [p[0].re as f32, p[1].re as f32, p[2].re as f32];
                    let mask_d =
                        crate::csg::sdf_polyline_tube(p_real, mask_polyline, *mask_radius);
                    if mask_d < 0.0 {
                        let r = D3::from(*blend_radius as f64);
                        if *is_difference {
                            -blend_min_generic(-da, db_raw, r, *blend_mode)
                        } else {
                            blend_min_generic(da, db_raw, r, *blend_mode)
                        }
                    } else {
                        if *is_difference {
                            let neg_db = -db_raw;
                            if da > neg_db { da } else { neg_db }
                        } else {
                            if da < db_raw { da } else { db_raw }
                        }
                    }
                }
            }
        }
        BoundedCsg::Extrude {
            sketch,
            half_height,
            world_transform,
            ..
        } => {
            let local = world_to_local_d3(world_transform, p);
            let sketch_ref: &crate::sketch2d::BoundedSketch2d = sketch;
            let d_sketch = eval_sketch_generic(sketch_ref, [local[0], local[1]]);
            let result =
                sdf_extrude_generic(d_sketch, local[2], D3::from(*half_height as f64));
            result * D3::from(world_transform.min_scale() as f64)
        }
        BoundedCsg::Revolve {
            sketch,
            world_transform,
            ..
        } => {
            let local = world_to_local_d3(world_transform, p);
            let r = safe_sqrt(local[0] * local[0] + local[2] * local[2]);
            let sketch_ref: &crate::sketch2d::BoundedSketch2d = sketch;
            let d = eval_sketch_generic(sketch_ref, [r, local[1]]);
            d * D3::from(world_transform.min_scale() as f64)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Layer A.5: Generic 2D sketch segment distance
// ═══════════════════════════════════════════════════════════════════════

/// Unsigned distance from a 2D point to a sketch segment, with exact
/// gradients via dual numbers. Finds the closest point using real
/// parts (branching), then computes the distance using dual arithmetic
/// so the gradient flows through correctly. At the closest point
/// d/dt|P−B(t)|² = 0, so treating t* as a constant is exact.
pub(crate) fn sketch_segment_distance_generic<T: DualNum<f64> + Copy + PartialOrd>(
    seg: &fielddraft_protocol::sketch2d::SketchSegment,
    p: [T; 2],
) -> T {
    let p_real = [p[0].re() as f32, p[1].re() as f32];
    let closest = seg.closest_point(p_real);
    let dx = p[0] - T::from(closest[0] as f64);
    let dy = p[1] - T::from(closest[1] as f64);
    safe_sqrt(dx * dx + dy * dy)
}

// ═══════════════════════════════════════════════════════════════════════
// Layer A.6: Generic 2D sketch primitive + tree evaluation
// ═══════════════════════════════════════════════════════════════════════

/// Generic SDF of a single 2D sketch primitive at point `p`. Mirrors
/// `fielddraft_protocol::sketch2d::Sketch2dPrimitive::sdf` exactly, but
/// works with any `T: DualNum<f64>` so the gradient flows through.
///
/// For Circle/Rectangle the closed forms are differentiated directly.
/// For Sector/Polygon/Path the inside/outside *sign* is piecewise
/// constant: it is computed on the REAL parts (branching) and multiplied
/// into the dual unsigned distance. This is exact a.e. (the sign only
/// flips across the boundary, where the SDF itself is non-smooth), the
/// same justification as freezing the closest point.
fn sdf_primitive2d_generic<T: DualNum<f64> + Copy + PartialOrd>(
    prim: &Sketch2dPrimitive,
    p: [T; 2],
) -> T {
    match prim {
        Sketch2dPrimitive::Circle { center, radius } => {
            let dx = p[0] - T::from(center[0] as f64);
            let dy = p[1] - T::from(center[1] as f64);
            safe_sqrt(dx * dx + dy * dy) - T::from(*radius as f64)
        }
        Sketch2dPrimitive::Rectangle {
            center,
            half_extents,
        } => {
            let qx = (p[0] - T::from(center[0] as f64)).abs() - T::from(half_extents[0] as f64);
            let qy = (p[1] - T::from(center[1] as f64)).abs() - T::from(half_extents[1] as f64);
            let zero = T::from(0.0);
            let qx0 = if qx > zero { qx } else { zero };
            let qy0 = if qy > zero { qy } else { zero };
            let outside = safe_sqrt(qx0 * qx0 + qy0 * qy0);
            let qmax = if qx > qy { qx } else { qy };
            let inside = if qmax < zero { qmax } else { zero };
            outside + inside
        }
        Sketch2dPrimitive::Sector {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => sdf_sector2d_generic(p, *center, *radius, *start_angle, *sweep_angle),
        Sketch2dPrimitive::Polygon { vertices } => sdf_polygon2d_generic(p, vertices),
        Sketch2dPrimitive::Path { segments, closed } => {
            sdf_path2d_generic(p, segments, *closed)
        }
    }
}

/// Generic mirror of `sketch2d::sdf_sector`. The angular-range branch is
/// taken on real parts; inside the range the value is the radial circle
/// distance (smooth in dual), outside it is the dual distance to the
/// nearer radial edge, max'd with the (real-frozen) exterior circle
/// distance.
fn sdf_sector2d_generic<T: DualNum<f64> + Copy + PartialOrd>(
    p: [T; 2],
    center: [f32; 2],
    radius: f32,
    start: f32,
    sweep: f32,
) -> T {
    let dx = p[0] - T::from(center[0] as f64);
    let dy = p[1] - T::from(center[1] as f64);
    let dist = safe_sqrt(dx * dx + dy * dy);
    let d_circle = dist - T::from(radius as f64);

    // Angular test on real parts (mirrors the f32 branching).
    let angle = (dy.re() as f32).atan2(dx.re() as f32);
    let half = sweep * 0.5;
    let mid = start + half;
    let mut rel = angle - mid;
    while rel > std::f32::consts::PI {
        rel -= std::f32::consts::TAU;
    }
    while rel < -std::f32::consts::PI {
        rel += std::f32::consts::TAU;
    }

    if rel.abs() <= half {
        d_circle
    } else {
        let edge_angle = if rel > 0.0 { start + sweep } else { start };
        let ex = edge_angle.cos();
        let ey = edge_angle.sin();
        // Projection scalar on real parts to clamp deterministically.
        let proj = (dx.re() as f32) * ex + (dy.re() as f32) * ey;
        let proj_clamped = proj.clamp(0.0, radius);
        let closest = [
            T::from((center[0] + ex * proj_clamped) as f64),
            T::from((center[1] + ey * proj_clamped) as f64),
        ];
        let ddx = p[0] - closest[0];
        let ddy = p[1] - closest[1];
        let d_edge = safe_sqrt(ddx * ddx + ddy * ddy);
        // d_edge.max(d_circle.max(0.0)). The inner `d_circle.max(0.0)`
        // selects on the real part; the outer max likewise.
        let zero = T::from(0.0);
        let dc0 = if d_circle > zero { d_circle } else { zero };
        if d_edge > dc0 { d_edge } else { dc0 }
    }
}

/// Generic mirror of `sketch2d::sdf_polygon`. The unsigned distance to
/// the nearest edge is computed in dual arithmetic (closest parameter
/// frozen on real parts); the even-odd sign is computed on real parts
/// and multiplied in.
fn sdf_polygon2d_generic<T: DualNum<f64> + Copy + PartialOrd>(p: [T; 2], v: &[[f32; 2]]) -> T {
    let n = v.len();
    if n < 3 {
        return T::from(f64::INFINITY);
    }
    let pr = [p[0].re() as f32, p[1].re() as f32];

    // Find the argmin edge on real parts, and the even-odd sign.
    let mut best_d2 = {
        let dx = pr[0] - v[0][0];
        let dy = pr[1] - v[0][1];
        dx * dx + dy * dy
    };
    let mut best_closest = v[0];
    let mut s = 1.0_f32;
    let mut j = n - 1;
    for i in 0..n {
        let ex = v[j][0] - v[i][0];
        let ey = v[j][1] - v[i][1];
        let wx = pr[0] - v[i][0];
        let wy = pr[1] - v[i][1];
        let ee = ex * ex + ey * ey;
        let t = if ee > 0.0 {
            ((wx * ex + wy * ey) / ee).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cx = v[i][0] + ex * t;
        let cy = v[i][1] + ey * t;
        let bx = pr[0] - cx;
        let by = pr[1] - cy;
        let d_candidate = bx * bx + by * by;
        if d_candidate < best_d2 {
            best_d2 = d_candidate;
            best_closest = [cx, cy];
        }
        let c1 = pr[1] >= v[i][1];
        let c2 = pr[1] < v[j][1];
        let c3 = ex * wy > ey * wx;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    let dx = p[0] - T::from(best_closest[0] as f64);
    let dy = p[1] - T::from(best_closest[1] as f64);
    let unsigned = safe_sqrt(dx * dx + dy * dy);
    unsigned * T::from(s as f64)
}

/// Generic mirror of `sketch2d::sdf_path`. The unsigned distance is the
/// min over segments (argmin on real parts, then the winning segment is
/// evaluated in dual via `sketch_segment_distance_generic`). For a
/// closed path the even-odd sign on the flattened contour is computed on
/// real parts and multiplied in.
fn sdf_path2d_generic<T: DualNum<f64> + Copy + PartialOrd>(
    p: [T; 2],
    segments: &[fielddraft_protocol::sketch2d::SketchSegment],
    closed: bool,
) -> T {
    if segments.is_empty() {
        return T::from(f64::INFINITY);
    }
    let pr = [p[0].re() as f32, p[1].re() as f32];

    // Argmin segment on real parts — shared pruned scan with the f32 path.
    let (_, best_i) = fielddraft_protocol::sketch2d::path_min_distance(pr, segments);
    let unsigned = sketch_segment_distance_generic(&segments[best_i], p);

    if !closed {
        return unsigned;
    }

    // Even-odd sign on real parts — shared straddle-filtered test.
    if fielddraft_protocol::sketch2d::path_is_inside(pr, segments) {
        -unsigned
    } else {
        unsigned
    }
}

/// Generic mirror of `BoundedSketch2d::sdf`: full BVH traversal with
/// real-part AABB pruning (pruning only chooses which branch to
/// evaluate; it never changes the returned value, so it is
/// derivative-safe).
fn eval_sketch_generic<T: DualNum<f64> + Copy + PartialOrd>(
    sketch: &crate::sketch2d::BoundedSketch2d,
    p: [T; 2],
) -> T {
    use crate::sketch2d::BoundedSketch2d as B;
    let pr = [p[0].re() as f32, p[1].re() as f32];
    match sketch {
        B::Empty => T::from(f64::INFINITY),
        B::Prim { primitive, .. } => sdf_primitive2d_generic(primitive, p),
        B::Union { left, right, .. } => {
            let dist_left = distance_point_to_aabb2(pr, left.aabb());
            let dist_right = distance_point_to_aabb2(pr, right.aabb());
            let (first_node, second_node, second_bound) = if dist_left <= dist_right {
                (&**left, &**right, dist_right)
            } else {
                (&**right, &**left, dist_left)
            };
            let d_first = eval_sketch_generic(first_node, p);
            if second_bound > 0.0 && (d_first.re() as f32) < second_bound {
                d_first
            } else {
                let d_second = eval_sketch_generic(second_node, p);
                if d_first < d_second { d_first } else { d_second }
            }
        }
        B::Difference { left, right, .. } => {
            let d_left = eval_sketch_generic(left, p);
            let dist_right = distance_point_to_aabb2(pr, right.aabb());
            if dist_right > 0.0 && (d_left.re() as f32) >= -dist_right {
                d_left
            } else {
                let neg_right = -eval_sketch_generic(right, p);
                if d_left > neg_right { d_left } else { neg_right }
            }
        }
        B::Intersection { left, right, .. } => {
            let dl = eval_sketch_generic(left, p);
            let dr = eval_sketch_generic(right, p);
            if dl > dr { dl } else { dr }
        }
        B::Offset { inner, distance, .. } => {
            eval_sketch_generic(inner, p) - T::from(*distance as f64)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Layer C: Public API
// ═══════════════════════════════════════════════════════════════════════

/// Evaluate the SDF and its gradient at a world-space point.
///
/// Returns `(distance, [df/dx, df/dy, df/dz])` where the gradient
/// components are computed via exact dual-number propagation (no finite
/// differences, except for sketch-backed and ImportedMesh primitives
/// which use a local FD fallback).
///
/// When the dual evaluation produces NaN gradients (e.g. `sqrt(0)` on
/// a symmetry axis), falls back to central finite differences.
pub fn sdf_gradient(node: &BoundedCsg, p: [f32; 3]) -> (f32, [f32; 3]) {
    sdf_gradient_mode(node, p, EvalMode::Visual)
}

/// Like [`sdf_gradient`] but evaluates in an explicit [`EvalMode`].
///
/// Rendering and meshing want [`EvalMode::Visual`] (smooth blends), which is
/// what the [`sdf_gradient`] wrapper passes. The B-Rep extraction pipeline
/// must instead evaluate in [`EvalMode::Logical`] (hard CSG, well-defined
/// branches) so the projected surface and phantom test agree with the
/// Logical-mode octree classification — see DL-9. For blend-free geometry the
/// two modes coincide exactly.
pub fn sdf_gradient_mode(node: &BoundedCsg, p: [f32; 3], mode: EvalMode) -> (f32, [f32; 3]) {
    let px = seed_d3(p[0] as f64, 0);
    let py = seed_d3(p[1] as f64, 1);
    let pz = seed_d3(p[2] as f64, 2);
    let result = eval_bounded_d3(node, [px, py, pz], mode);
    let eps = result
        .eps
        .unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
    let grad = [eps[0] as f32, eps[1] as f32, eps[2] as f32];

    // Fall back to FD when the dual gradient is degenerate (NaN/Inf).
    if grad[0].is_finite() && grad[1].is_finite() && grad[2].is_finite() {
        (result.re as f32, grad)
    } else {
        let fd_eps = 1e-4_f32;
        let dx = (node.sdf([p[0] + fd_eps, p[1], p[2]], mode)
            - node.sdf([p[0] - fd_eps, p[1], p[2]], mode))
            / (2.0 * fd_eps);
        let dy = (node.sdf([p[0], p[1] + fd_eps, p[2]], mode)
            - node.sdf([p[0], p[1] - fd_eps, p[2]], mode))
            / (2.0 * fd_eps);
        let dz = (node.sdf([p[0], p[1], p[2] + fd_eps], mode)
            - node.sdf([p[0], p[1], p[2] - fd_eps], mode))
            / (2.0 * fd_eps);
        (result.re as f32, [dx, dy, dz])
    }
}

// ─── f64 value+gradient (no f32 downcast) ────────────────────────────
//
// The public `sdf_gradient*` API casts to f32 for the renderer/mesher. The
// B-Rep edge tracer instead needs the *full f64* value and gradient: it runs
// Newton iterations on the joint zero-set of two fields, where the f32 floor
// (catastrophic cancellation in finite differences, a rank guard near
// tangencies that lives below f32-ε) would cap the achievable exactness. The
// dual evaluation is already f64 internally — these two entry points just skip
// the downcast.

/// Helper: extract the f64 gradient out of a finished `D3`.
#[inline]
fn d3_grad_f64(result: &D3) -> [f64; 3] {
    let eps = result
        .eps
        .unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
    [eps[0], eps[1], eps[2]]
}

/// Value and exact gradient of a single analytic per-face [`HalfSpace`] at a
/// world-space point, in full f64. `xf` maps the half-space's centred local
/// frame to world (the same `(HalfSpace, Transform)` pair that
/// `brep::weaving::branch_halfspace_map` produces). The half-space value is the
/// *local* field; we omit the `min_scale` factor that `eval_bounded_d3`
/// applies — it is a positive constant, so it changes neither the zero set
/// (where the tracer lives) nor the gradient direction.
pub(crate) fn halfspace_value_grad(
    hs: &HalfSpace,
    xf: &Transform,
    p: [f64; 3],
) -> (f64, [f64; 3]) {
    let pd = [seed_d3(p[0], 0), seed_d3(p[1], 1), seed_d3(p[2], 2)];
    let local = world_to_local_d3(xf, pd);
    let result = sdf_halfspace_generic(hs, local);
    (result.re, d3_grad_f64(&result))
}

/// Value and exact gradient of an arbitrary [`BoundedCsg`] subtree at a
/// world-space point, in full f64. Used for the *general* (non-analytic) edge
/// fields: an Extrude/Revolve/sweep leaf has no closed-form half-space, but its
/// surface is still the zero set of this evaluable field with a usable
/// gradient (exact dual where the primitive is dual-aware, FD fallback inside
/// for sketch/mesh internals). For a single leaf there is no smooth blend, so
/// [`EvalMode::Logical`] is the right choice (hard, well-defined branch).
pub(crate) fn bounded_value_grad(
    node: &BoundedCsg,
    p: [f64; 3],
    mode: EvalMode,
) -> (f64, [f64; 3]) {
    let pd = [seed_d3(p[0], 0), seed_d3(p[1], 1), seed_d3(p[2], 2)];
    let result = eval_bounded_d3(node, pd, mode);
    (result.re, d3_grad_f64(&result))
}

/// Field value + world gradient of an **extrude lateral wall** — the *signed*
/// profile SDF swept **infinitely** along the extrude axis (DL-77), i.e. the
/// extrude leaf **without** its cap clamp `sdf_extrude_generic`. The full leaf
/// `max(d_profile, |z|−h)` is C0 exactly at the rim (`z = ±h`, on the profile),
/// where the gradient flips between the horizontal profile normal and the
/// vertical cap normal — so projecting/marching the rim seam on the leaf is
/// ill-conditioned and the tracer is disabled there. Dropping the cap clamp
/// leaves the cap-free swept profile, smooth across the rim (no `z` dependence),
/// so the rim seam — this wall ∩ the analytic cap plane — traces cleanly.
///
/// The field is the **signed** profile distance (`eval_sketch_generic`), so its
/// gradient on the wall is the well-defined unit surface normal — an *unsigned*
/// per-segment distance would vanish/kink exactly on its zero set (where the
/// seed projects), collapsing the edge tangent. The whole profile is one field;
/// which segment-arc a given seam group recovers is fixed by the per-group
/// bounding corner vertices, not by the field. Rescaled by the placement's min
/// uniform scale to stay a true world distance.
pub(crate) fn extrude_wall_value_grad(
    sketch: &crate::sketch2d::BoundedSketch2d,
    world_transform: &Transform,
    p: [f64; 3],
) -> (f64, [f64; 3]) {
    let scale = world_transform.min_scale() as f64;
    // Exact f64 local profile coordinates (constant duals — no seeded derivative).
    let pc = [D3::from(p[0]), D3::from(p[1]), D3::from(p[2])];
    let local = world_to_local_d3(world_transform, pc);
    let (lx, ly) = (local[0].re, local[1].re);

    // Closest boundary segment + its analytic curve tangent there. The normal is
    // the tangent rotated 90° — a clean unit surface normal **even on the curve**,
    // where `∇‖p−closest‖` is 0/0 and the flattened-polygon even-odd sign is
    // numerically inconsistent with the true Bézier boundary. The signed distance
    // is the perpendicular component `(p−closest)·n`; because the f32 closest-point
    // error is tangential (along the curve, ⟂ n), it drops out of the dot product,
    // keeping the value precise enough for the tracer's 1e-9 Newton projection.
    let Some((c, t)) = closest_boundary_segment(sketch, [lx as f32, ly as f32]) else {
        return (f64::INFINITY, [0.0, 0.0, 0.0]);
    };
    let n = [-(t[1] as f64), t[0] as f64];
    let value = ((lx - c[0] as f64) * n[0] + (ly - c[1] as f64) * n[1]) * scale;
    // d(value)/d(world p): for a rigid + uniform-scale placement the scale cancels
    // and the local normal (z = 0 — the wall is independent of the extrude axis)
    // rotates to world.
    let gw = crate::quat_rotate(world_transform.rotation, [n[0] as f32, n[1] as f32, 0.0]);
    (value, [gw[0] as f64, gw[1] as f64, gw[2] as f64])
}

/// The closest boundary segment's closest point and unit curve tangent over a
/// sketch's [`SketchSegment`] boundary (Path prims). `None` if the sketch holds
/// no segment boundary (e.g. only analytic prims) — the caller then yields an
/// infinite field, so the seam falls back to the sample fits. Boolean sketch
/// nodes are walked transparently; the per-group bounding corners isolate the
/// relevant arc, so interior boundary pieces of a boolean profile do no harm.
fn closest_boundary_segment(
    sketch: &crate::sketch2d::BoundedSketch2d,
    p: [f32; 2],
) -> Option<([f32; 2], [f32; 2])> {
    use crate::sketch2d::BoundedSketch2d as B;
    fn walk(s: &B, p: [f32; 2], best: &mut Option<(f32, [f32; 2], [f32; 2])>) {
        match s {
            B::Prim { primitive, .. } => {
                if let Sketch2dPrimitive::Path { segments, .. } = primitive {
                    for seg in segments {
                        let (c, t) = seg.closest_point_and_tangent(p);
                        let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
                        if best.is_none_or(|(bd, ..)| d < bd) {
                            *best = Some((d, c, t));
                        }
                    }
                }
            }
            B::Union { left, right, .. }
            | B::Difference { left, right, .. }
            | B::Intersection { left, right, .. } => {
                walk(left, p, best);
                walk(right, p, best);
            }
            B::Offset { inner, .. } => walk(inner, p, best),
            B::Empty => {}
        }
    }
    let mut best = None;
    walk(sketch, p, &mut best);
    best.map(|(_, c, t)| (c, t))
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, HalfSpace, Primitive, Transform};

    #[test]
    fn sphere_gradient_matches_analytical() {
        // For a sphere centred at origin with radius 10,
        // the gradient at any point is the normalised direction from centre.
        let p = [3.0_f64, 4.0, 0.0]; // distance from origin = 5
        let prim = Primitive::Sphere {
            center: [0.0, 0.0, 0.0],
            radius: 10.0,
        };
        let px = seed_d3(p[0], 0);
        let py = seed_d3(p[1], 1);
        let pz = seed_d3(p[2], 2);
        let result = sdf_primitive_generic::<D3>(&prim, [px, py, pz]);
        // Distance should be 5 - 10 = -5
        assert!(
            (result.re - (-5.0)).abs() < 1e-10,
            "distance: expected -5, got {}",
            result.re
        );
        // Gradient should be (3/5, 4/5, 0) = (0.6, 0.8, 0.0)
        let eps = result
            .eps
            .unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
        assert!(
            (eps[0] - 0.6).abs() < 1e-10,
            "d/dx: expected 0.6, got {}",
            eps[0]
        );
        assert!(
            (eps[1] - 0.8).abs() < 1e-10,
            "d/dy: expected 0.8, got {}",
            eps[1]
        );
        assert!(
            (eps[2] - 0.0).abs() < 1e-10,
            "d/dz: expected 0.0, got {}",
            eps[2]
        );
    }

    #[test]
    fn thread_gradient_is_finite_on_axis() {
        // The Thread profile unwraps the helix via atan2(z, x); its derivative
        // ∂θ/∂(x,z) = (-z, x)/r² diverges as r→0, producing NaN dual gradients on
        // the helix axis (40 NaNs measured pre-guard). The axis guard freezes θ's
        // derivative below eps_axis, so on- and near-axis gradients stay finite.
        let prim = Primitive::Thread {
            nominal_radius: 0.5,
            pitch: 0.2,
            depth: 0.1,
            half_height: 1.0,
        };
        for p in [
            [0.0_f64, 0.05, 0.0],   // exactly on axis
            [1e-6, -0.03, 1e-6],    // sub-eps_axis
            [1e-5, 0.0, -2e-5],     // sub-eps_axis, mixed signs
        ] {
            let d = sdf_primitive_generic::<D3>(
                &prim,
                [seed_d3(p[0], 0), seed_d3(p[1], 1), seed_d3(p[2], 2)],
            );
            assert!(d.re.is_finite(), "value NaN at {p:?}");
            let g = d.eps.unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
            assert!(
                g[0].is_finite() && g[1].is_finite() && g[2].is_finite(),
                "gradient {:?} not finite at {p:?}",
                [g[0], g[1], g[2]]
            );
        }
    }

    #[test]
    fn thread_field_is_lipschitz_after_normalization() {
        // B: the raw Thread Chebyshev field has |∇f| up to ≈1.86 (the y-derivative
        // of r_profile from the helix unwrap) → it overestimates distance and the
        // sphere-march overshoots. The /sqrt(1+(π·depth/pitch)²) normalization
        // makes it a conservative ≤1 distance. Sample off-axis (away from the
        // atan2 singularity) over a grid and assert the dual gradient magnitude
        // never exceeds 1 by more than a hair. Pre-normalization this peaked at
        // 1.86 and would fail.
        let prim = Primitive::Thread {
            nominal_radius: 0.5,
            pitch: 0.2,
            depth: 0.1,
            half_height: 1.0,
        };
        let mut max_grad = 0.0_f64;
        for ri in 1..=8 {
            let r = 0.2 + 0.08 * ri as f64; // 0.28 .. 0.84, off-axis
            for ti in 0..12 {
                let theta = ti as f64 * std::f64::consts::TAU / 12.0;
                let (x, z) = (r * theta.cos(), r * theta.sin());
                for yi in -4..=4 {
                    let y = 0.12 * yi as f64;
                    let d = sdf_primitive_generic::<D3>(
                        &prim,
                        [seed_d3(x, 0), seed_d3(y, 1), seed_d3(z, 2)],
                    );
                    let g = d.eps.unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
                    let n = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                    if n.is_finite() {
                        max_grad = max_grad.max(n);
                    }
                }
            }
        }
        assert!(
            max_grad <= 1.02,
            "normalized thread must be ~Lipschitz-1 off-axis, got |∇f|max = {max_grad}"
        );
    }

    #[test]
    fn box_face_gradient_is_axis_aligned() {
        // For a box centred at origin, a point on the +X face should have gradient (1,0,0)
        let prim = Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [5.0, 5.0, 5.0],
        };
        let p = [7.0_f64, 0.0, 0.0]; // outside +X face
        let px = seed_d3(p[0], 0);
        let py = seed_d3(p[1], 1);
        let pz = seed_d3(p[2], 2);
        let result = sdf_primitive_generic::<D3>(&prim, [px, py, pz]);
        let eps = result
            .eps
            .unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
        assert!(
            (result.re - 2.0).abs() < 1e-10,
            "distance should be 2, got {}",
            result.re
        );
        assert!(
            (eps[0] - 1.0).abs() < 1e-10,
            "d/dx should be 1, got {}",
            eps[0]
        );
        assert!(eps[1].abs() < 1e-10, "d/dy should be 0, got {}", eps[1]);
        assert!(eps[2].abs() < 1e-10, "d/dz should be 0, got {}", eps[2]);
    }

    fn make_feature(
        id: u32,
        name: &str,
        op: BoolOp,
        primitive: Primitive,
    ) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            name.into(),
            op,
            primitive,
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    #[test]
    fn sdf_gradient_on_bvh_matches_fd() {
        use crate::bvh::BoundedCsg;
        use crate::csg::CsgNode;

        let features = vec![
            make_feature(
                1,
                "Box",
                BoolOp::New,
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [10.0, 10.0, 10.0],
                },
            ),
            make_feature(
                2,
                "Sphere",
                BoolOp::Remove,
                Primitive::Sphere {
                    center: [10.0, 0.0, 0.0],
                    radius: 5.0,
                },
            ),
        ];
        let csg = CsgNode::from_decoded(&features);
        let bvh = BoundedCsg::from_csg(&csg);
        let p = [5.0_f32, 3.0, 2.0];
        let (d_dual, grad_dual) = sdf_gradient(&bvh, p);

        // FD reference
        let eps = 1e-4_f32;
        let m = EvalMode::Visual;
        let dx = (bvh.sdf([p[0] + eps, p[1], p[2]], m)
            - bvh.sdf([p[0] - eps, p[1], p[2]], m))
            / (2.0 * eps);
        let dy = (bvh.sdf([p[0], p[1] + eps, p[2]], m)
            - bvh.sdf([p[0], p[1] - eps, p[2]], m))
            / (2.0 * eps);
        let dz = (bvh.sdf([p[0], p[1], p[2] + eps], m)
            - bvh.sdf([p[0], p[1], p[2] - eps], m))
            / (2.0 * eps);

        assert!(
            (d_dual - bvh.sdf(p, m)).abs() < 1e-4,
            "distance mismatch: dual={} f32={}",
            d_dual,
            bvh.sdf(p, m)
        );
        assert!(
            (grad_dual[0] - dx).abs() < 1e-2,
            "d/dx mismatch: dual={} fd={}",
            grad_dual[0],
            dx
        );
        assert!(
            (grad_dual[1] - dy).abs() < 1e-2,
            "d/dy mismatch: dual={} fd={}",
            grad_dual[1],
            dy
        );
        assert!(
            (grad_dual[2] - dz).abs() < 1e-2,
            "d/dz mismatch: dual={} fd={}",
            grad_dual[2],
            dz
        );
    }

    #[test]
    fn dual_eval_matches_f32_eval() {
        use crate::bvh::BoundedCsg;
        use crate::csg::CsgNode;

        let features = vec![make_feature(
            1,
            "Cyl",
            BoolOp::New,
            Primitive::Cylinder {
                center: [0.0, 0.0, 0.0],
                radius: 5.0,
                half_height: 10.0,
            },
        )];
        let csg = CsgNode::from_decoded(&features);
        let bvh = BoundedCsg::from_csg(&csg);
        for p in [
            [3.0f32, 0.0, 0.0],
            [0.0, 12.0, 0.0],
            [4.0, 4.0, 4.0],
            [0.0, 0.0, 0.0],
        ] {
            let (d_dual, _) = sdf_gradient(&bvh, p);
            let d_f32 = bvh.sdf(p, EvalMode::Visual);
            assert!(
                (d_dual - d_f32).abs() < 1e-4,
                "p={:?} dual={} f32={}",
                p,
                d_dual,
                d_f32
            );
        }
    }

    fn halfspace_gradient(hs: &HalfSpace, p: [f64; 3]) -> [f64; 3] {
        let px = seed_d3(p[0], 0);
        let py = seed_d3(p[1], 1);
        let pz = seed_d3(p[2], 2);
        let result = sdf_halfspace_generic(hs, [px, py, pz]);
        let eps = result.eps.unwrap_generic(nalgebra::Const::<3>, nalgebra::Const::<1>);
        [eps[0], eps[1], eps[2]]
    }

    #[test]
    fn halfspace_plane_gradient_is_normal() {
        let n = [0.6_f32, 0.8, 0.0];
        let hs = HalfSpace::Plane { normal: n, offset: 5.0 };
        let g = halfspace_gradient(&hs, [10.0, 20.0, 3.0]);
        assert!((g[0] - n[0] as f64).abs() < 1e-7);
        assert!((g[1] - n[1] as f64).abs() < 1e-7);
        assert!(g[2].abs() < 1e-7);
    }

    #[test]
    fn halfspace_sphere_gradient_is_radial() {
        let hs = HalfSpace::Sphere { radius: 5.0 };
        let g = halfspace_gradient(&hs, [3.0, 4.0, 0.0]);
        let len = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-6, "gradient should be unit length, got {len}");
        assert!((g[0] - 0.6).abs() < 1e-6);
        assert!((g[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn halfspace_cylinder_gradient_zero_y() {
        let hs = HalfSpace::InfiniteCylinder { radius: 5.0 };
        let g = halfspace_gradient(&hs, [3.0, 99.0, 4.0]);
        assert!(g[1].abs() < 1e-10, "Y gradient should be zero for infinite cylinder");
        let len_xz = (g[0] * g[0] + g[2] * g[2]).sqrt();
        assert!((len_xz - 1.0).abs() < 1e-6, "XZ gradient should be unit length");
    }

    // ─── sketch segment gradient tests ────────────────────────────────
    // Exercise `sketch_segment_distance_generic` over
    // `fielddraft_protocol::sketch2d::SketchSegment` (Line / QuadBezier /
    // circular Arc).
    mod sketch_segment_duals_sa1 {
    use super::*;

    type D2 = DualSVec64<2>;

    fn seed_d2(val: f64, axis: usize) -> D2 {
        let mut eps = SVector::<f64, 2>::zeros();
        eps[axis] = 1.0;
        D2::new(val, Derivative::some(eps))
    }

    fn segment_gradient(
        seg: &fielddraft_protocol::sketch2d::SketchSegment,
        p: [f64; 2],
    ) -> (f64, [f64; 2]) {
        let px = seed_d2(p[0], 0);
        let py = seed_d2(p[1], 1);
        let result = sketch_segment_distance_generic(seg, [px, py]);
        let eps = result
            .eps
            .unwrap_generic(nalgebra::Const::<2>, nalgebra::Const::<1>);
        (result.re, [eps[0], eps[1]])
    }

    fn fd_segment_gradient(
        seg: &fielddraft_protocol::sketch2d::SketchSegment,
        p: [f64; 2],
    ) -> [f64; 2] {
        let eps = 1e-3_f32;
        let px = p[0] as f32;
        let py = p[1] as f32;
        let dx = (seg.unsigned_distance([px + eps, py])
            - seg.unsigned_distance([px - eps, py])) as f64
            / (2.0 * eps as f64);
        let dy = (seg.unsigned_distance([px, py + eps])
            - seg.unsigned_distance([px, py - eps])) as f64
            / (2.0 * eps as f64);
        [dx, dy]
    }

    #[test]
    fn line_segment_gradient_perpendicular_to_line() {
        use fielddraft_protocol::sketch2d::SketchSegment;
        let seg = SketchSegment::Line {
            start: [0.0, 0.0],
            end: [10.0, 0.0],
        };
        let (d, g) = segment_gradient(&seg, [5.0, 3.0]);
        assert!((d - 3.0).abs() < 1e-6, "distance: expected 3, got {d}");
        assert!(g[0].abs() < 1e-6, "dx should be 0, got {}", g[0]);
        assert!((g[1] - 1.0).abs() < 1e-6, "dy should be 1, got {}", g[1]);
    }

    #[test]
    fn quad_bezier_gradient_matches_fd() {
        use fielddraft_protocol::sketch2d::SketchSegment;
        let seg = SketchSegment::QuadBezier {
            p0: [0.0, 0.0],
            p1: [5.0, 10.0],
            p2: [10.0, 0.0],
        };
        let p = [3.0, 2.0];
        let (_, g_dual) = segment_gradient(&seg, [p[0], p[1]]);
        let g_fd = fd_segment_gradient(&seg, [p[0], p[1]]);
        assert!(
            (g_dual[0] - g_fd[0]).abs() < 0.05,
            "dx mismatch: dual={} fd={}",
            g_dual[0],
            g_fd[0]
        );
        assert!(
            (g_dual[1] - g_fd[1]).abs() < 0.05,
            "dy mismatch: dual={} fd={}",
            g_dual[1],
            g_fd[1]
        );
    }

    #[test]
    fn arc_gradient_matches_fd() {
        use fielddraft_protocol::sketch2d::SketchSegment;
        // Circular arc, full sweep, radius 10 centred at origin.
        let seg = SketchSegment::Arc {
            center: [0.0, 0.0],
            radius: 10.0,
            start_angle: 0.0,
            sweep_angle: std::f32::consts::TAU,
        };
        let p = [12.0, 3.0];
        let (_, g_dual) = segment_gradient(&seg, [p[0], p[1]]);
        let g_fd = fd_segment_gradient(&seg, [p[0], p[1]]);
        assert!(
            (g_dual[0] - g_fd[0]).abs() < 0.05,
            "dx mismatch: dual={} fd={}",
            g_dual[0],
            g_fd[0]
        );
        assert!(
            (g_dual[1] - g_fd[1]).abs() < 0.05,
            "dy mismatch: dual={} fd={}",
            g_dual[1],
            g_fd[1]
        );
    }

    #[test]
    fn segment_gradient_is_unit_length() {
        use fielddraft_protocol::sketch2d::SketchSegment;
        let segments: Vec<SketchSegment> = vec![
            SketchSegment::Line {
                start: [0.0, 0.0],
                end: [10.0, 0.0],
            },
            SketchSegment::QuadBezier {
                p0: [0.0, 0.0],
                p1: [5.0, 10.0],
                p2: [10.0, 0.0],
            },
        ];
        for seg in &segments {
            let (d, g) = segment_gradient(seg, [5.0, 4.0]);
            if d > 0.1 {
                let len = (g[0] * g[0] + g[1] * g[1]).sqrt();
                assert!(
                    (len - 1.0).abs() < 0.05,
                    "gradient should be unit length, got {len} for {:?}",
                    seg
                );
            }
        }
    }
    } // mod sketch_segment_duals_sa1

    // ─── extrude / revolve chain-rule gate tests ──────────────────────
    // These prove the world→local→2D-sketch→extrude/revolve dual chain
    // closes: the dual gradient must match a central finite difference of
    // the SAME f32 evaluator (`bvh::BoundedCsg::sdf`, via the public
    // `sdf_gradient_mode`). Sample points are chosen AWAY from medial
    // axes and corners, where both dual and FD are well-defined.
    mod extrude_revolve_chain_rule {
        use super::*;
        use fielddraft_protocol::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};
        use fielddraft_protocol::Primitive;
        use crate::bvh::BoundedCsg;

        /// Central finite difference of `node.sdf(p, mode)`.
        fn fd_grad(node: &BoundedCsg, p: [f32; 3], mode: EvalMode) -> [f32; 3] {
            let eps = 1e-3_f32;
            let dx = (node.sdf([p[0] + eps, p[1], p[2]], mode)
                - node.sdf([p[0] - eps, p[1], p[2]], mode))
                / (2.0 * eps);
            let dy = (node.sdf([p[0], p[1] + eps, p[2]], mode)
                - node.sdf([p[0], p[1] - eps, p[2]], mode))
                / (2.0 * eps);
            let dz = (node.sdf([p[0], p[1], p[2] + eps], mode)
                - node.sdf([p[0], p[1], p[2] - eps], mode))
                / (2.0 * eps);
            [dx, dy, dz]
        }

        /// Unit square Path, CCW, from (-2,-2) to (2,2).
        fn square_sketch() -> Sketch2d {
            Sketch2d::Prim(Sketch2dPrimitive::Path {
                segments: vec![
                    SketchSegment::Line { start: [-2.0, -2.0], end: [2.0, -2.0] },
                    SketchSegment::Line { start: [2.0, -2.0], end: [2.0, 2.0] },
                    SketchSegment::Line { start: [2.0, 2.0], end: [-2.0, 2.0] },
                    SketchSegment::Line { start: [-2.0, 2.0], end: [-2.0, -2.0] },
                ],
                closed: true,
            })
        }

        fn extrude_node(sketch: Sketch2d, half_height: f32, transform: Transform) -> BoundedCsg {
            use crate::csg::{CsgNode, DecodedFeature, PrimitiveModifiers};
            let feature = DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Extrude".into(),
                BoolOp::New,
                Primitive::Extrude {
                    sketch,
                    half_depth_z: half_height,
                    center: [0.0, 0.0, 0.0],
                },
                transform,
                PrimitiveModifiers::NONE,
                vec![],
            );
            let csg = CsgNode::from_decoded(&[feature]);
            BoundedCsg::from_csg(&csg)
        }

        #[test]
        fn extrude_identity_gradient_matches_fd() {
            let node = extrude_node(square_sketch(), 5.0, Transform::IDENTITY);
            let mode = EvalMode::Logical;
            // Points on a wall (away from corners) and just in/out.
            let pts = [
                [3.0_f32, 0.3, 0.0],   // outside the +x wall
                [1.5, 0.4, 0.0],       // inside, nearest the +x wall
                [0.4, 0.0, 7.0],       // above the top cap
                [0.0, 3.0, 0.0],       // outside the +y wall
            ];
            let mut max_res = 0.0_f32;
            for p in pts {
                let (_, g) = sdf_gradient_mode(&node, p, mode);
                let fd = fd_grad(&node, p, mode);
                for k in 0..3 {
                    let res = (g[k] - fd[k]).abs();
                    max_res = max_res.max(res);
                    assert!(res < 1e-2, "p={p:?} axis {k}: dual={} fd={}", g[k], fd[k]);
                }
            }
            assert!(max_res < 1e-2, "max residual {max_res}");
        }

        #[test]
        fn extrude_placement_jacobian_matches_fd() {
            // Translation + non-identity quaternion rotation + non-uniform
            // scale. Quaternion: rotate ~30° about a tilted axis.
            let (s, c) = (30.0_f32.to_radians() * 0.5).sin_cos();
            let ax = (1.0_f32, 0.5, 0.2);
            let n = (ax.0 * ax.0 + ax.1 * ax.1 + ax.2 * ax.2).sqrt();
            let rotation = [s * ax.0 / n, s * ax.1 / n, s * ax.2 / n, c];
            let transform = Transform {
                translate: [3.0, -2.0, 1.0],
                rotation,
                scale: [1.5, 0.8, 1.2],
            };
            let node = extrude_node(square_sketch(), 5.0, transform);
            let mode = EvalMode::Logical;
            // Pick points well off any wall/corner in world space.
            let pts = [
                [7.0_f32, 0.0, 1.0],
                [3.5, 1.0, 1.2],
                [3.0, -2.0, 9.0],
            ];
            let mut max_res = 0.0_f32;
            for p in pts {
                let (_, g) = sdf_gradient_mode(&node, p, mode);
                let fd = fd_grad(&node, p, mode);
                for k in 0..3 {
                    let res = (g[k] - fd[k]).abs();
                    max_res = max_res.max(res);
                    assert!(res < 1e-2, "p={p:?} axis {k}: dual={} fd={}", g[k], fd[k]);
                }
            }
            assert!(max_res < 1e-2, "max residual {max_res}");
        }

        #[test]
        fn revolve_gradient_matches_fd() {
            use crate::csg::CsgNode;
            // Profile: a circle at (r=10, y=0) radius 2 → a torus tube.
            let profile = Sketch2d::Prim(Sketch2dPrimitive::Circle {
                center: [10.0, 0.0],
                radius: 2.0,
            });
            let feature = DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Revolve".into(),
                BoolOp::New,
                Primitive::Revolve {
                    sketch: profile,
                    center: [0.0, 0.0, 0.0],
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            );
            let csg = CsgNode::from_decoded(&[feature]);
            let node = BoundedCsg::from_csg(&csg);
            let mode = EvalMode::Logical;
            // Points off the revolve axis, away from the tube's medial
            // circle (r=10).
            let pts = [
                [13.0_f32, 0.5, 0.0],
                [7.0, 0.3, 0.0],
                [0.0, 0.4, 13.0],
            ];
            let mut max_res = 0.0_f32;
            for p in pts {
                let (_, g) = sdf_gradient_mode(&node, p, mode);
                let fd = fd_grad(&node, p, mode);
                for k in 0..3 {
                    let res = (g[k] - fd[k]).abs();
                    max_res = max_res.max(res);
                    assert!(res < 1e-2, "p={p:?} axis {k}: dual={} fd={}", g[k], fd[k]);
                }
            }
            assert!(max_res < 1e-2, "max residual {max_res}");
        }
    }
}
