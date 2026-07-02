//! Referenz-SDF-Auswertung in purem Rust.
//!
//! [`sdf_primitive`] ist die kanonische CPU-Implementierung der
//! Primitiv-SDFs (Tests, Picking-Fallback, CPU-Mesher); die
//! `helix_profile_*`-Helfer sind die geteilte Helix-Sweep-Mathematik,
//! die BVH, Dual-Eval und GPU-Opcode-Emitter spiegeln.

use fielddraft_protocol::Primitive;

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

/// SDF of a single primitive at world point `p`.
pub fn sdf_primitive(primitive: &Primitive, p: [f32; 3]) -> f32 {
    match primitive {
        Primitive::Box { center, half_extents } => {
            let q = [
                (p[0] - center[0]).abs() - half_extents[0],
                (p[1] - center[1]).abs() - half_extents[1],
                (p[2] - center[2]).abs() - half_extents[2],
            ];
            let outside =
                (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
            let inside = q[0].max(q[1]).max(q[2]).min(0.0);
            outside + inside
        }
        Primitive::Sphere { center, radius } => {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            let dz = p[2] - center[2];
            (dx * dx + dy * dy + dz * dz).sqrt() - *radius
        }
        Primitive::Cylinder { center, radius, half_height } => {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            let dz = p[2] - center[2];
            let radial = (dx * dx + dz * dz).sqrt() - *radius;
            let axial = dy.abs() - *half_height;
            let outside = radial.max(0.0).hypot(axial.max(0.0));
            let inside = radial.max(axial).min(0.0);
            outside + inside
        }
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
        Primitive::Torus { center, major_radius, minor_radius } => {
            let dx = p[0] - center[0];
            let dy = p[1] - center[1];
            let dz = p[2] - center[2];
            let q = ((dx * dx + dz * dz).sqrt() - major_radius, dy);
            (q.0 * q.0 + q.1 * q.1).sqrt() - minor_radius
        }
        Primitive::Gyroid { cell_size, thickness } => {
            let s = *cell_size;
            let x = p[0] / s;
            let y = p[1] / s;
            let z = p[2] / s;
            let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            (f.abs() - *thickness) * s * 0.2887
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let theta = p[2].atan2(p[0]);
            let y_unwrapped = p[1] - *pitch * theta / std::f32::consts::TAU;
            let phase = (y_unwrapped / pitch) * std::f32::consts::TAU;
            let r_profile = *nominal_radius - *depth * 0.5 * (1.0 - phase.cos());
            let d_radial = r - r_profile;
            let d_axial = p[1].abs() - *half_height;
            // Lipschitz normalization: |∇f| peaks at sqrt(1 + (π·depth/pitch)²);
            // divide by it for a conservative ≤1 distance (cf. sdf_dual::Thread,
            // Gyroid 0.2887, Cone wall_scale).
            let lip = (1.0 + (std::f32::consts::PI * *depth / pitch.max(1e-6)).powi(2)).sqrt();
            d_radial.max(d_axial) / lip
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let hh = *half_height;
            // Interpolated radius at p.y
            let t = ((p[1] + hh) / (2.0 * hh)).clamp(0.0, 1.0);
            let r_at_y = r_bottom * (1.0 - t) + r_top * t;
            // Perpendicular distance to the inclined wall: project the raw
            // radial gap onto the wall normal so the field stays Lipschitz-1.
            // For a cylinder (dr == 0) the factor is 1.
            let dr = r_bottom - r_top;
            let slant_len = (dr * dr + (2.0 * hh) * (2.0 * hh)).sqrt();
            let wall_scale = if slant_len > 1e-9 { (2.0 * hh) / slant_len } else { 1.0 };
            let d_wall = (r - r_at_y) * wall_scale;
            let d_axial = p[1].abs() - hh;
            if d_wall > 0.0 && d_axial > 0.0 {
                (d_wall * d_wall + d_axial * d_axial).sqrt()
            } else {
                d_wall.max(d_axial)
            }
        }
        Primitive::Capsule { radius, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let y_clamped = p[1].clamp(-*half_height, *half_height);
            let dy = p[1] - y_clamped;
            (r * r + dy * dy).sqrt() - *radius
        }
        Primitive::HexPrism { apothem, half_height } => {
            // 2D hex distance in XZ plane.
            let qx = p[0].abs();
            let qz = p[2].abs();
            // Hex: max(qx * sqrt(3)/2 + qz * 0.5, qz) - apothem
            let d_hex = (qx * 0.866_025_4 + qz * 0.5).max(qz) - *apothem;
            let d_axial = p[1].abs() - *half_height;
            if d_hex > 0.0 && d_axial > 0.0 {
                (d_hex * d_hex + d_axial * d_axial).sqrt()
            } else {
                d_hex.max(d_axial)
            }
        }
        Primitive::ImportedMesh { mesh } => mesh.sdf(p),
        Primitive::Ellipsoid { radii } => {
            let r_min = radii[0].min(radii[1]).min(radii[2]);
            let q = [p[0] / radii[0], p[1] / radii[1], p[2] / radii[2]];
            let len = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2]).sqrt();
            (len - 1.0) * r_min
        }
    }
}
