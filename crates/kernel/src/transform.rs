//! Quaternion- und Rigid-Transform-Algebra des Kernels.
//!
//! Gesammelt aus `lib.rs`/`features.rs`: Quaternion-Rotation und
//! -Komposition, Welt↔Lokal-Abbildung von [`Transform`]s sowie die
//! Euler-XYZ-Konvertierung der Transform-Mutatoren. Genutzt von CSG-
//! Auswertung, BVH, B-Rep-Weaving und den Feature-Mutatoren.

use fielddraft_protocol::Transform;

/// Rotate a vector by the conjugate of a unit quaternion `(x, y, z, w)`.
/// This is the inverse rotation of `q`, used to map a world-space point
/// into the primitive's local frame.
fn quat_inverse_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // For unit q, q⁻¹ = (-x, -y, -z, w). We then apply the standard
    // p' = q⁻¹ * p * q rotation, optimised via the cross-product form.
    let qx = -q[0];
    let qy = -q[1];
    let qz = -q[2];
    let qw = q[3];
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Hamilton product of two unit quaternions, both in the project's
/// `(x, y, z, w)` storage order. Used when collapsing chains of
/// `CsgNode::Transform` into a single accumulated world transform at
/// each BVH leaf (see `bvh.rs`).
pub fn quat_multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Compose two affine transforms into one that applies `child` first,
/// then `parent`: the resulting transform is equivalent to the chain
/// `parent ∘ child`. Used to fold nested `CsgNode::Transform` nodes
/// into each leaf's accumulated world transform.
pub fn compose_transforms(parent: &Transform, child: &Transform) -> Transform {
    let scaled_child_t = [
        child.translate[0] * parent.scale[0],
        child.translate[1] * parent.scale[1],
        child.translate[2] * parent.scale[2],
    ];
    let rotated = quat_rotate(parent.rotation, scaled_child_t);
    Transform {
        translate: [
            parent.translate[0] + rotated[0],
            parent.translate[1] + rotated[1],
            parent.translate[2] + rotated[2],
        ],
        rotation: quat_multiply(parent.rotation, child.rotation),
        scale: [
            parent.scale[0] * child.scale[0],
            parent.scale[1] * child.scale[1],
            parent.scale[2] * child.scale[2],
        ],
    }
}

/// Rotate a vector by a unit quaternion `(x, y, z, w)`. Counterpart to
/// [`quat_inverse_rotate`] — this one applies the rotation itself,
/// used when mapping an already-local point (or an AABB corner) out to
/// world space.
pub fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let qx = q[0];
    let qy = q[1];
    let qz = q[2];
    let qw = q[3];
    let t = [
        2.0 * (qy * v[2] - qz * v[1]),
        2.0 * (qz * v[0] - qx * v[2]),
        2.0 * (qx * v[1] - qy * v[0]),
    ];
    [
        v[0] + qw * t[0] + qy * t[2] - qz * t[1],
        v[1] + qw * t[1] + qz * t[0] - qx * t[2],
        v[2] + qw * t[2] + qx * t[1] - qy * t[0],
    ]
}

/// Bring a world-space point into a feature's primitive-local frame
/// using its transform: untranslate, unrotate, unscale.
pub fn world_to_local(t: &Transform, p_world: [f32; 3]) -> [f32; 3] {
    let p = [
        p_world[0] - t.translate[0],
        p_world[1] - t.translate[1],
        p_world[2] - t.translate[2],
    ];
    let p = quat_inverse_rotate(t.rotation, p);
    [p[0] / t.scale[0], p[1] / t.scale[1], p[2] / t.scale[2]]
}

/// Invert a rigid + **uniform-scale** [`Transform`]. The body placement
/// (see [`body::Body::placement`]) is restricted to this class precisely
/// so its inverse exists as another `Transform`: `compose(t, inverse(t))`
/// and `compose(inverse(t), t)` both round-trip to identity.
///
/// For a transform `T(p) = s·R·p + t`, the inverse is
/// `T⁻¹(q) = (1/s)·R⁻¹·(q − t) = R⁻¹·(q/s) − R⁻¹·(t/s)`, which is again
/// an affine transform with rotation `R⁻¹` (the quaternion conjugate),
/// scale `1/s` and translation `−R⁻¹·(t/s)`. The input's scale is read as
/// uniform via [`Transform::min_scale`]; callers that build the placement
/// guarantee uniformity, so non-uniform residue never reaches here.
pub fn inverse(t: &Transform) -> Transform {
    let s = t.min_scale();
    let inv_s = if s.abs() < 1e-12 { 0.0 } else { 1.0 / s };
    // Conjugate of a unit quaternion (x, y, z, w) is (−x, −y, −z, w).
    let inv_rot = [-t.rotation[0], -t.rotation[1], -t.rotation[2], t.rotation[3]];
    // −R⁻¹·(t/s): scale the translation, rotate by the inverse rotation,
    // negate. `quat_rotate(inv_rot, ·)` applies R⁻¹.
    let scaled = [
        t.translate[0] * inv_s,
        t.translate[1] * inv_s,
        t.translate[2] * inv_s,
    ];
    let rotated = quat_rotate(inv_rot, scaled);
    Transform {
        translate: [-rotated[0], -rotated[1], -rotated[2]],
        rotation: inv_rot,
        scale: [inv_s, inv_s, inv_s],
    }
}

/// Build a unit quaternion from XYZ (Tait-Bryan) Euler angles in
/// degrees — the wire format of the `rotate`/`quat_edit` mutators.
pub(crate) fn euler_xyz_to_quat(rx_deg: f32, ry_deg: f32, rz_deg: f32) -> [f32; 4] {
    let (s1, c1) = (rx_deg.to_radians() * 0.5).sin_cos();
    let (s2, c2) = (ry_deg.to_radians() * 0.5).sin_cos();
    let (s3, c3) = (rz_deg.to_radians() * 0.5).sin_cos();
    [
        s1 * c2 * c3 + c1 * s2 * s3,
        c1 * s2 * c3 - s1 * c2 * s3,
        c1 * c2 * s3 + s1 * s2 * c3,
        c1 * c2 * c3 - s1 * s2 * s3,
    ]
}

pub(crate) fn quat_to_euler_xyz(qx: f32, qy: f32, qz: f32, qw: f32) -> [f32; 3] {
    let m11 = 1.0 - 2.0 * (qy * qy + qz * qz);
    let m12 = 2.0 * (qx * qy - qw * qz);
    let m13 = 2.0 * (qx * qz + qw * qy);
    let m23 = 2.0 * (qy * qz - qw * qx);
    let m33 = 1.0 - 2.0 * (qx * qx + qy * qy);

    let ry = m13.clamp(-1.0, 1.0).asin();
    let (rx, rz) = if m13.abs() < 0.9999999 {
        ((-m23).atan2(m33), (-m12).atan2(m11))
    } else {
        let m32 = 2.0 * (qy * qz + qw * qx);
        let m22 = 1.0 - 2.0 * (qx * qx + qz * qz);
        (m32.atan2(m22), 0.0)
    };
    [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()]
}
