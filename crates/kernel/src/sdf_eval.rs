//! Referenz-SDF-Auswertung in purem Rust.
//!
//! [`sdf_primitive_generic`] ist die **eine autoritative** Implementierung
//! der geschlossenen Primitiv-SDFs (B7): generisch über `T: DualNum<F>`,
//! sodass dieselbe Formel sowohl als f32-Referenz ([`sdf_primitive`],
//! Tests / Picking-Fallback / CPU-Mesher) als auch als Dual-Zahl-Eval mit
//! exakten Gradienten (`sdf_dual`, `T = DualSVec64<3>`) instanziiert wird —
//! keine zweite Formel-Kopie mehr zwischen den beiden.
//!
//! Die vier sketch-/mesh-getragenen Varianten (Extrude, Revolve, Helix,
//! ImportedMesh) bleiben bewusst f32-Arme in
//! [`sdf_sketch_or_mesh_primitive`]: ihr 2D-Sketch- bzw. Mesh-Kern hat
//! keine generische Primitiv-Form — der Dual-Pfad behandelt sie auf
//! Baum-Ebene (`eval_bounded_d3` mit generischem Sketch-Eval bzw.
//! FD-Fallback), nie über diese Funktion. Die
//! `helix_profile_*`-Helfer sind die geteilte Helix-Sweep-Mathematik,
//! die BVH, Dual-Eval und GPU-Opcode-Emitter spiegeln.

use num_dual::{DualNum, DualNumFloat};

use fielddraft_protocol::Primitive;

/// `sqrt` that avoids NaN derivatives at zero. Many SDF primitives
/// use `sqrt(x² + z²)` for radial distance, which has an undefined
/// gradient on the axis (x=z=0). Adding a tiny epsilon keeps the
/// derivative well-defined without measurably changing the SDF value
/// in the f64 real part. NICHT für die f32-Referenz verwenden — dort
/// muss `sqrt(0)` exakt 0.0 bleiben (siehe [`SdfSqrt`]).
#[inline]
pub(crate) fn safe_sqrt<T: DualNum<F> + Copy, F: DualNumFloat>(x: T) -> T {
    (x + T::from(F::from_f64(1e-20).unwrap())).sqrt()
}

/// Sqrt-Strategie der geteilten Primitiv-Eval ([`sdf_primitive_generic`]).
///
/// Die **f32-Referenz** nutzt die pure `sqrt`: sie muss auf der Oberfläche
/// eine **exakte 0.0** liefern — der DC-Mesher klassifiziert Grid-Punkte
/// per Vorzeichen, und Box-Flächen landen regelmäßig exakt auf
/// Grid-Koordinaten. Ein `sqrt(x + 1e-20)` macht aus der exakten 0 ein
/// +1e-10 und kippt diese Punkte von „innen/auf" nach „außen" — das
/// blähte die Narrow-Band-Vertexzahl einer 25-mm-Box von 578 auf 5270
/// auf (LOD-Test-Regression beim B7-Umbau).
///
/// Die **Dual-Instanziierung** addiert das Epsilon ([`safe_sqrt`]): ohne
/// es liefert `sqrt(0)` auf Achsen/Ecken NaN-Ableitungen. Im f64-Realteil
/// ist das Epsilon wert-neutral genug (Gradient-Konsument, kein
/// Grid-Klassifikator).
pub(crate) trait SdfSqrt {
    fn sdf_sqrt(self) -> Self;
}

impl SdfSqrt for f32 {
    #[inline]
    fn sdf_sqrt(self) -> Self {
        self.sqrt()
    }
}

impl<const N: usize> SdfSqrt for num_dual::DualSVec64<N> {
    #[inline]
    fn sdf_sqrt(self) -> Self {
        safe_sqrt(self)
    }
}

/// Profile-frame anchor of a [`Primitive::Helix`]: `(y_anchor, r_inner)`
/// — the sketch's axial centre (per-turn instance reference) and its
/// inner radius (clamps the near-axis Lipschitz blow-up).
pub fn helix_profile_anchor(sketch: &fielddraft_protocol::Sketch2d) -> (f32, f32) {
    let bb = sketch.aabb();
    if bb.is_empty() {
        (0.0, 0.0)
    } else {
        ((bb.min[1] + bb.max[1]) * 0.5, bb.min[0].max(0.0))
    }
}

/// Map a helix-LOCAL point into the swept profile's 2D `(r, w)` frame
/// and return `(q, ell, scale)`: the profile point, the signed
/// end-overshoot coordinate, and the conservative Lipschitz scale of
/// the unwrap. The helix winds about the local Y axis.
///
/// The finite sweep runs over the CONTINUOUS parameter `t ∈ [0,
/// 2π·turns]` (fractional turns end mid-winding). The nearest pass for
/// the query's angular position is found by rounding, then its
/// parameter is clamped to the sweep span — clamping `t` (not an
/// integer winding index) keeps the field continuous across the
/// `atan2` branch seam at the helix ends (ticket vtyh: the per-winding
/// clamp jumped `w` by a full pitch there, flipping signs → phantom
/// ribbon + checkerboard marching artifacts).
///
/// `ell` is the angular distance to the sweep span mapped to a chord
/// length at the clamped radius (≤ 0 inside the span, > 0 beyond an
/// end, halved as a safety margin — the zero crossing, i.e. the end-cap
/// surface, is unaffected). Callers combine it with the profile
/// distance via [`helix_compose`].
///
/// `taper` shears the radial coordinate along +y. The scale bounds the
/// composed map: `|∇w|² ≤ 1 + (pitch/(2πr))²` (tangential unwrap),
/// `|∇r'|² ≤ 1 + taper²`, `|∇ell|² ≤ 1.25` — the constant 2.25 covers
/// the `ell` component within the square root so the composed field
/// stays ≤ true distance (sphere-march safe), the same
/// self-normalisation idea as `Primitive::Thread` / Gyroid. `r` is
/// clamped below by `r_inner` (and a pitch-derived floor) so the
/// near-axis bound does not collapse the field to zero inside the bore.
/// Mirrored in the web GPU interpreter (`OP_WARP_HELIX`/`OP_HELIX_SCALE`).
/// Returns `(profile_point, ell, scale, cap_side)`. `cap_side` names which open
/// end the query overshoots — `-1` past the START (t < 0), `+1` past the END
/// (t > t_end), `0` within the swept span — so the provenance eval can tag the
/// two end caps with distinct sub-branches (pickable faces, ticket exus).
pub fn helix_profile_point(
    p: [f32; 3],
    pitch: f32,
    turns: f32,
    left_handed: bool,
    taper: f32,
    y_anchor: f32,
    r_inner: f32,
    start_angle: f32,
) -> ([f32; 2], f32, f32, f32) {
    use std::f32::consts::{PI, TAU};
    let pitch = pitch.max(1e-3);
    let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
    let theta = p[2].atan2(p[0]);
    let hand = if left_handed { -1.0 } else { 1.0 };
    // Handed sweep angle of the query relative to the start angle, and
    // the sweep parameter of the nearest helix pass at that position.
    let th = hand * theta - start_angle;
    let n = ((p[1] - y_anchor) / pitch - th / TAU).round();
    let t_n = th + TAU * n;
    let t_end = TAU * turns.max(0.0);
    let t_c = t_n.clamp(0.0, t_end);
    // Profile frame at the (possibly end-clamped) pass.
    let w = p[1] - pitch * t_c / TAU;
    let rp = r - taper * (p[1] - y_anchor);
    // Per-feature constant clamp radius: dropping the per-point `r`
    // only ever increases `q`, i.e. damps more — still conservative,
    // and it makes the Lipschitz scale a constant the opcode stream
    // can bake in at emission time (exact CPU/GPU parity).
    let rr = r_inner.max(pitch / TAU * 0.25).max(1e-3);
    // Signed angular distance to the sweep span, as a chord at the
    // query's radius (capped at the clamp radius, halved for safety).
    let l_t = (t_n - t_end * 0.5).abs() - t_end * 0.5;
    let ell = l_t.signum() * r.min(rr) * (l_t.abs().min(PI) * 0.5).sin();
    let q = pitch / (TAU * rr);
    let scale = 1.0 / (2.25 + q * q + taper * taper).sqrt();
    // Which half of the sweep the pass falls in: −1 = start side, +1 = end
    // side. Only consulted by the provenance eval when the cap term `ell` ties
    // (so an interior wall point ignores it); naming the half — not a strict
    // overshoot — keeps the rim itself (exactly at the cap) tagged.
    let cap_side = if t_n < t_end * 0.5 { -1.0 } else { 1.0 };
    ([rp, w], ell, scale, cap_side)
}

/// Combine a helix profile distance with the signed end-overshoot
/// coordinate from [`helix_profile_point`] — IQ's extrude composition
/// (`(d2, ell)` as a 2D box SDF), so the finite sweep ends with clean
/// cap cross-sections instead of a discontinuity.
pub fn helix_compose(d2: f32, ell: f32) -> f32 {
    let outside = (d2.max(0.0)).hypot(ell.max(0.0));
    let inside = d2.max(ell).min(0.0);
    outside + inside
}

/// SDF of a single primitive at world point `p` — the f32 reference.
///
/// The closed-form primitives are the `T = f32` instantiation of
/// [`sdf_primitive_generic`] (B7: one authoritative formula, shared with
/// the dual-number gradient eval). The sketch-/mesh-backed variants are
/// dispatched to their dedicated f32 arms.
pub fn sdf_primitive(primitive: &Primitive, p: [f32; 3]) -> f32 {
    match primitive {
        Primitive::Extrude { .. }
        | Primitive::Revolve { .. }
        | Primitive::Helix { .. }
        | Primitive::ImportedMesh { .. } => sdf_sketch_or_mesh_primitive(primitive, p),
        other => sdf_primitive_generic::<f32, f32>(other, p),
    }
}

/// The four sketch-/mesh-backed primitives, f32-only (their 2D-sketch /
/// triangle-mesh cores have no generic dual form; the dual path evaluates
/// them at the TREE level — `eval_bounded_d3` — with a generic sketch eval
/// resp. an FD fallback, never through here).
pub(crate) fn sdf_sketch_or_mesh_primitive(primitive: &Primitive, p: [f32; 3]) -> f32 {
    match primitive {
        // K7.3: lift the 2D sketch SDF into 3D via IQ's extrude
        // composition — `(d_2d, |z| - half_depth_z)` treated as
        // a 2D box SDF. Matches what `CsgNode::Extrude` does at
        // the mesher level; duplicated here so the
        // non-CSG-tree `sdf_primitive` path (picking fallback)
        // still returns a sensible distance.
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center,
        } => {
            let local_xy = [p[0] - center[0], p[1] - center[1]];
            let d_xy = sketch.sdf(local_xy);
            let d_z = (p[2] - center[2]).abs() - *half_depth_z;
            let outside = d_xy.max(0.0).hypot(d_z.max(0.0));
            let inside = d_xy.max(d_z).min(0.0);
            outside + inside
        }
        // K8.4: 2D sketch revolved around the local Y axis.
        // Map (x, y, z) to (r, y) where r = √((x - cx)² + (z - cz)²),
        // then evaluate the 2D sketch SDF. Mirrors
        // `CsgNode::Revolve::sdf` so the non-CSG-tree path
        // (picking fallback) returns a sensible distance.
        Primitive::Revolve { sketch, center } => {
            let dx = p[0] - center[0];
            let dz = p[2] - center[2];
            let r = (dx * dx + dz * dz).sqrt();
            sketch.sdf([r, p[1] - center[1]])
        }
        // Helical sweep: unwrap into the profile's (r, w) frame and
        // evaluate the 2D sketch SDF, Lipschitz-normalised. Mirrors
        // `CsgNode::Helix`'s tree eval for the non-tree (picking) path.
        Primitive::Helix {
            sketch, center, pitch, turns, left_handed, taper, start_angle,
        } => {
            let (y_anchor, r_inner) = helix_profile_anchor(sketch);
            let local = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
            let (q, ell, scale, _) = helix_profile_point(
                local,
                *pitch,
                *turns,
                *left_handed,
                *taper,
                y_anchor,
                r_inner,
                *start_angle,
            );
            helix_compose(sketch.sdf(q), ell) * scale
        }
        Primitive::ImportedMesh { mesh } => mesh.sdf(p),
        other => unreachable!(
            "sdf_sketch_or_mesh_primitive called with closed-form primitive {other:?} — \
             route it through sdf_primitive_generic"
        ),
    }
}

/// Generic SDF of a single **closed-form** primitive at point `p` — the
/// single authoritative formula set (B7). Instantiated as
/// `<f32, f32>` for the CPU reference ([`sdf_primitive`]) and as
/// `<DualSVec64<3>, f64>` for exact analytic gradients (`sdf_dual`).
///
/// Branches are taken on the REAL part (`re()`) where a dual-number
/// max/clamp needs a well-defined active branch; for a plain float
/// instantiation `re()` is the identity, so both instantiations follow
/// the exact same control flow.
///
/// The sketch-/mesh-backed variants (Extrude, Revolve, Helix,
/// ImportedMesh) return a **zero-derivative fallback** (the f32 result
/// lifted into `T`), which real bodies never take: an extrude/revolve
/// always reaches the kernel as a `BoundedCsg::{Extrude,Revolve,Helix}`
/// carrying its `BoundedSketch2d`, and that is evaluated at the tree
/// level (`sdf_dual::eval_bounded_d3`) with an EXACT dual gradient — the
/// sketch SDF is fully `DualNum`-generic there. Only `ImportedMesh` truly
/// has no analytic gradient and uses the tree-level finite-difference
/// fallback (`fd_fallback_3d`). The fallback calls
/// [`sdf_sketch_or_mesh_primitive`] directly (not [`sdf_primitive`]), so
/// a future variant can never create a dispatch cycle.
///
/// This `match` is deliberately **exhaustive without a wildcard**: adding
/// a `Primitive` variant fails compilation here until it is given either
/// a generic formula or an explicit fallback — the parity tests in
/// `sdf_parity_tests.rs` then hold the implementations to each other.
pub(crate) fn sdf_primitive_generic<T, F>(prim: &Primitive, p: [T; 3]) -> T
where
    T: DualNum<F> + Copy + PartialOrd + SdfSqrt,
    F: DualNumFloat + From<f32>,
{
    // Constant lifts: `cf` raises an f32 primitive parameter, `cd` a
    // formula literal (f64) into the computation type.
    let cf = |v: f32| T::from(<F as From<f32>>::from(v));
    let cd = |v: f64| T::from(F::from_f64(v).unwrap());
    match prim {
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0] - cf(center[0])).abs() - cf(half_extents[0]),
                (p[1] - cf(center[1])).abs() - cf(half_extents[1]),
                (p[2] - cf(center[2])).abs() - cf(half_extents[2]),
            ];
            let zero = T::from(F::zero());
            let q0 = if q[0] > zero { q[0] } else { zero };
            let q1 = if q[1] > zero { q[1] } else { zero };
            let q2 = if q[2] > zero { q[2] } else { zero };
            let outside = SdfSqrt::sdf_sqrt(q0 * q0 + q1 * q1 + q2 * q2);
            // inside = min(max(q[0], q[1], q[2]), 0)
            let qmax = if q[0] > q[1] { q[0] } else { q[1] };
            let qmax = if qmax > q[2] { qmax } else { q[2] };
            let inside = if qmax < zero { qmax } else { zero };
            outside + inside
        }
        Primitive::Sphere { center, radius } => {
            let dx = p[0] - cf(center[0]);
            let dy = p[1] - cf(center[1]);
            let dz = p[2] - cf(center[2]);
            SdfSqrt::sdf_sqrt(dx * dx + dy * dy + dz * dz) - cf(*radius)
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = p[0] - cf(center[0]);
            let dy = p[1] - cf(center[1]);
            let dz = p[2] - cf(center[2]);
            let radial = SdfSqrt::sdf_sqrt(dx * dx + dz * dz) - cf(*radius);
            let axial = dy.abs() - cf(*half_height);
            let zero = T::from(F::zero());
            let r_clamped = if radial > zero { radial } else { zero };
            let a_clamped = if axial > zero { axial } else { zero };
            let outside = SdfSqrt::sdf_sqrt(r_clamped * r_clamped + a_clamped * a_clamped);
            let inside_max = if radial > axial { radial } else { axial };
            let inside = if inside_max < zero { inside_max } else { zero };
            outside + inside
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            let dx = p[0] - cf(center[0]);
            let dy = p[1] - cf(center[1]);
            let dz = p[2] - cf(center[2]);
            let q0 = SdfSqrt::sdf_sqrt(dx * dx + dz * dz) - cf(*major_radius);
            let q1 = dy;
            SdfSqrt::sdf_sqrt(q0 * q0 + q1 * q1) - cf(*minor_radius)
        }
        Primitive::Gyroid { cell_size, thickness } => {
            let s = cf(*cell_size);
            let x = p[0] / s;
            let y = p[1] / s;
            let z = p[2] / s;
            let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            (f.abs() - cf(*thickness)) * s * cd(0.2887)
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            let r = SdfSqrt::sdf_sqrt(p[0] * p[0] + p[2] * p[2]);
            // Axis guard: ∂θ/∂(x,z) = (-z, x)/r² diverges as r→0 and produces NaN
            // dual gradients on the helix axis. The axis is deep interior
            // (r ≪ nominal_radius), so the surface never depends on θ there —
            // freeze θ's derivative below eps_axis to keep the gradient finite.
            // Value-neutral for a plain float instantiation.
            let theta = if r.re() > F::from_f64(1e-4).unwrap() {
                p[2].atan2(p[0])
            } else {
                T::from(p[2].re().atan2(p[0].re()))
            };
            let tau = T::from(F::TAU());
            let pitch_t = cf(*pitch);
            let y_unwrapped = p[1] - pitch_t * theta / tau;
            let phase = (y_unwrapped / pitch_t) * tau;
            let r_profile =
                cf(*nominal_radius) - cf(*depth) * cd(0.5) * (T::from(F::one()) - phase.cos());
            let d_radial = r - r_profile;
            let d_axial = p[1].abs() - cf(*half_height);
            let d = if d_radial > d_axial { d_radial } else { d_axial };
            // Lipschitz normalization (constant in p → preserves gradient
            // direction, scales magnitude to ≤1): the helix folds y into
            // r_profile's phase, so ∂r_profile/∂y = π·depth/pitch and |∇f| peaks
            // at sqrt(1 + (π·depth/pitch)²) ≈ 1.86 for the default thread. The raw
            // Chebyshev max overestimates distance; dividing by the exact bound
            // makes it a conservative distance so the sphere-march stops
            // overshooting the groove walls (cf. Gyroid 0.2887, Cone wall_scale).
            let pitch_f = <F as From<f32>>::from(*pitch).max(F::from_f64(1e-6).unwrap());
            let lip = (F::one() + (F::PI() * <F as From<f32>>::from(*depth) / pitch_f).powi(2)).sqrt();
            d * T::from(F::one() / lip)
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = SdfSqrt::sdf_sqrt(p[0] * p[0] + p[2] * p[2]);
            let hh = cf(*half_height);
            let two_hh = hh * cd(2.0);
            // t = clamp((p[1] + hh) / (2*hh), 0, 1) — use re() for branch
            let t_raw = (p[1] + hh) / two_hh;
            let zero = T::from(F::zero());
            let one = T::from(F::one());
            let t = if t_raw.re() < F::zero() {
                zero
            } else if t_raw.re() > F::one() {
                one
            } else {
                t_raw
            };
            let r_at_y = cf(*r_bottom) * (one - t) + cf(*r_top) * t;
            // Project the radial gap onto the inclined wall normal (constant
            // scalar factor) so the field is the true perpendicular distance
            // and stays Lipschitz-1. For a cylinder (dr == 0) the factor is 1.
            let dr = <F as From<f32>>::from(*r_bottom - *r_top);
            let two_hh_f = <F as From<f32>>::from(2.0 * *half_height);
            let slant_len = (dr * dr + two_hh_f * two_hh_f).sqrt();
            let wall_scale = if slant_len > F::from_f64(1e-9).unwrap() {
                two_hh_f / slant_len
            } else {
                F::one()
            };
            let d_wall = (r - r_at_y) * T::from(wall_scale);
            let d_axial = p[1].abs() - hh;
            // Branch on real parts
            if d_wall.re() > F::zero() && d_axial.re() > F::zero() {
                SdfSqrt::sdf_sqrt(d_wall * d_wall + d_axial * d_axial)
            } else if d_wall > d_axial {
                d_wall
            } else {
                d_axial
            }
        }
        Primitive::Capsule { radius, half_height } => {
            let r = SdfSqrt::sdf_sqrt(p[0] * p[0] + p[2] * p[2]);
            let hh_f = <F as From<f32>>::from(*half_height);
            let y_clamped = if p[1].re() < -hh_f {
                cf(-*half_height)
            } else if p[1].re() > hh_f {
                cf(*half_height)
            } else {
                p[1]
            };
            let dy = p[1] - y_clamped;
            (r * r + dy * dy).sqrt() - cf(*radius)
        }
        Primitive::HexPrism { apothem, half_height } => {
            let qx = p[0].abs();
            let qz = p[2].abs();
            let hex_term = qx * cd(0.866_025_4) + qz * cd(0.5);
            let d_hex = (if hex_term > qz { hex_term } else { qz }) - cf(*apothem);
            let d_axial = p[1].abs() - cf(*half_height);
            if d_hex.re() > F::zero() && d_axial.re() > F::zero() {
                SdfSqrt::sdf_sqrt(d_hex * d_hex + d_axial * d_axial)
            } else if d_hex > d_axial {
                d_hex
            } else {
                d_axial
            }
        }
        Primitive::Ellipsoid { radii } => {
            let r_min = radii[0].min(radii[1]).min(radii[2]);
            let q = [p[0] / cf(radii[0]), p[1] / cf(radii[1]), p[2] / cf(radii[2])];
            let len = SdfSqrt::sdf_sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2]);
            (len - T::from(F::one())) * cf(r_min)
        }
        // Zero-derivative fallback — see the function docs. Calls the
        // dedicated f32 arms DIRECTLY (never `sdf_primitive`), so a
        // dispatch cycle is impossible by construction.
        Primitive::Extrude { .. }
        | Primitive::Revolve { .. }
        | Primitive::Helix { .. }
        | Primitive::ImportedMesh { .. } => {
            let p_f32 = [
                p[0].re().to_f32().unwrap_or(f32::NAN),
                p[1].re().to_f32().unwrap_or(f32::NAN),
                p[2].re().to_f32().unwrap_or(f32::NAN),
            ];
            T::from(<F as From<f32>>::from(sdf_sketch_or_mesh_primitive(prim, p_f32)))
        }
    }
}

#[cfg(test)]
#[path = "sdf_parity_tests.rs"]
mod parity_tests;
