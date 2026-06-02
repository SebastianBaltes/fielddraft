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
/// (`T::from(f32_result as f64)`) because they rely on sketch or mesh
/// internals that are not yet dual-number aware. These are handled
/// properly at the tree level (Layer B) via finite-difference fallbacks.
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
            let theta = p[2].atan2(p[0]);
            let tau = T::from(std::f64::consts::TAU);
            let y_unwrapped = p[1] - T::from(*pitch as f64) * theta / tau;
            let phase = (y_unwrapped / T::from(*pitch as f64)) * tau;
            let r_profile = T::from(*nominal_radius as f64)
                - T::from(*depth as f64) * T::from(0.5) * (T::from(1.0) - phase.cos());
            let d_radial = r - r_profile;
            let d_axial = p[1].abs() - T::from(*half_height as f64);
            if d_radial > d_axial { d_radial } else { d_axial }
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

#[allow(dead_code)] // used in tests; production callers come in Phase C (B-Rep extraction)
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
        BoundedCsg::Shell { inner, thickness, .. } => {
            let d = eval_bounded_d3(inner, p, mode);
            // `abs` while keeping the dual derivative: flip the whole
            // dual when the real part is negative (mirrors bvh.rs's
            // `inner.sdf(p).abs() - thickness`).
            let abs_d = if d.re < 0.0 { -d } else { d };
            abs_d - D3::from(*thickness as f64)
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

    // Argmin segment on real parts.
    let mut best_i = 0usize;
    let mut best_d = f32::INFINITY;
    for (i, seg) in segments.iter().enumerate() {
        let d = seg.unsigned_distance(pr);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    let unsigned = sketch_segment_distance_generic(&segments[best_i], p);

    if !closed {
        return unsigned;
    }

    // Even-odd crossing test over the flattened contour (real parts).
    let mut inside = false;
    for seg in segments {
        let steps = match seg {
            fielddraft_protocol::sketch2d::SketchSegment::Line { .. } => 1,
            _ => fielddraft_protocol::sketch2d::PATH_FLATTEN_SAMPLES,
        };
        let mut a = seg.point_at(0.0);
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            let b = seg.point_at(t);
            if (a[1] > pr[1]) != (b[1] > pr[1]) {
                let x_cross = a[0] + (pr[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if pr[0] < x_cross {
                    inside = !inside;
                }
            }
            a = b;
        }
    }
    if inside {
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
                BoolOp::Union,
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [10.0, 10.0, 10.0],
                },
            ),
            make_feature(
                2,
                "Sphere",
                BoolOp::Difference,
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
            BoolOp::Union,
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
                BoolOp::Union,
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
                BoolOp::Union,
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
