//! Sketch-Ebenen-Frames: [`SketchFrame`] (Origin + Basis) sowie die
//! Frame-Ableitung aus Datum-Planes und B-Rep-Flächen.

use fielddraft_protocol::{FeatureId};
/// C1: a 3D placement for a 2D sketch — an orthonormal right-handed
/// frame `(origin; x_axis, y_axis, normal)`. The sketch's `(u, v)`
/// coordinates map to world as `origin + u·x_axis + v·y_axis`; the
/// `normal` (== `x_axis × y_axis`) is the extrude/offset direction.
///
/// Geometry-neutral: this is just a coordinate frame, no datum-plane
/// or mate-connector semantics baked in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SketchFrame {
    pub origin: [f32; 3],
    pub x_axis: [f32; 3],
    pub y_axis: [f32; 3],
    pub normal: [f32; 3],
}

impl SketchFrame {
    /// The identity frame: origin at world 0, axes aligned with
    /// `+X`/`+Y`/`+Z`. [`SketchFrame::to_transform`] returns
    /// [`Transform::IDENTITY`] for this frame, so a sketch on it lowers
    /// exactly as a pre-C1 XY sketch did.
    pub const IDENTITY: Self = Self {
        origin: [0.0, 0.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        normal: [0.0, 0.0, 1.0],
    };

    /// World point → the frame's `(u, v)` plane coordinates, by
    /// orthogonal projection: `u = (p − origin)·x_axis`,
    /// `v = (p − origin)·y_axis`. The inverse of
    /// `origin + u·x_axis + v·y_axis`; for a point already on the plane
    /// it round-trips exactly. Off-plane points project orthogonally
    /// (the normal component is dropped). Used to express a picked axis
    /// line in the sketch's own 2D coordinates.
    pub fn world_to_local_2d(&self, p: [f32; 3]) -> [f32; 2] {
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        let dot = |a: [f32; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        [dot(self.x_axis), dot(self.y_axis)]
    }

    /// Rigid frame→world [`Transform`]: a rotation whose columns are the
    /// frame axes plus a translation to the origin. Composed into the
    /// lowered feature transform so the 2D profile (built in XY) lands on
    /// this plane.
    pub fn to_transform(&self) -> fielddraft_protocol::Transform {
        fielddraft_protocol::Transform {
            translate: self.origin,
            rotation: basis_to_quat(self.x_axis, self.y_axis, self.normal),
            scale: [1.0, 1.0, 1.0],
        }
    }

    /// Build a frame from a coordinate system given as origin + Z + X —
    /// the shape a `mate_connector` resolves to, and the shape most CAD
    /// interchange formats use (STEP's `AXIS2_PLACEMENT_3D` is exactly
    /// this triple).
    ///
    /// `x_axis` is RE-ORTHOGONALISED against `normal` (Gram-Schmidt)
    /// rather than trusted: a connector's X comes from an anchor's
    /// in-plane basis or from a user rotation, and a degree of drift there
    /// would leave `to_transform` building a quaternion from a
    /// non-orthonormal basis — which does not fail loudly, it shears the
    /// sketch. `y = normal × x` closes the right-handed triple, matching
    /// the convention `mate_connector`'s own realignment offset uses.
    ///
    /// Falls back to an arbitrary well-conditioned in-plane basis when
    /// `x_axis` is degenerate or parallel to the normal.
    pub fn from_csys(origin: [f32; 3], normal: [f32; 3], x_axis: [f32; 3]) -> Self {
        let normal = normalize3(normal);
        let dot = x_axis[0] * normal[0] + x_axis[1] * normal[1] + x_axis[2] * normal[2];
        let rejected = [
            x_axis[0] - dot * normal[0],
            x_axis[1] - dot * normal[1],
            x_axis[2] - dot * normal[2],
        ];
        let len2 = rejected[0] * rejected[0] + rejected[1] * rejected[1] + rejected[2] * rejected[2];
        // Parallel to the normal (or zero) → no in-plane direction survives
        // the rejection; synthesise one the same way `face_plane_frame` does.
        let x_axis = if len2 > 1e-12 {
            normalize3(rejected)
        } else {
            let alt = if normal[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            normalize3(cross(alt, normal))
        };
        Self {
            origin,
            x_axis,
            y_axis: cross(normal, x_axis),
            normal,
        }
    }
}

/// Build a unit quaternion `(x, y, z, w)` from an orthonormal
/// right-handed basis given as the three rotation-matrix columns
/// (`col0 = x_axis`, `col1 = y_axis`, `col2 = z_axis`). Standard
/// matrix→quaternion extraction (Shepperd's method, trace branch).
pub(crate) fn basis_to_quat(c0: [f32; 3], c1: [f32; 3], c2: [f32; 3]) -> [f32; 4] {
    let m00 = c0[0];
    let m10 = c0[1];
    let m20 = c0[2];
    let m01 = c1[0];
    let m11 = c1[1];
    let m21 = c1[2];
    let m02 = c2[0];
    let m12 = c2[1];
    let m22 = c2[2];
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    }
}

/// C1: the placement frame for one of the three well-known origin
/// datum planes, addressed by the feature id the web emits
/// (`qCreatedBy(origin_plane_*)`). Returns `None` for any other id.
///
/// * `origin_plane_xy` → origin 0, normal `+Z`, axes `+X`/`+Y`
///   (the identity frame — keeps legacy XY sketches byte-for-byte).
/// * `origin_plane_xz` → normal `+Y`, axes `+X`/`+Z`.
/// * `origin_plane_yz` → normal `+X`, axes `+Y`/`+Z`.
pub fn datum_plane_frame(id: &FeatureId) -> Option<SketchFrame> {
    match id.to_string().as_str() {
        "origin_plane_xy" => Some(SketchFrame::IDENTITY),
        "origin_plane_xz" => Some(SketchFrame {
            origin: [0.0, 0.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
        }),
        "origin_plane_yz" => Some(SketchFrame {
            origin: [0.0, 0.0, 0.0],
            x_axis: [0.0, 1.0, 0.0],
            y_axis: [0.0, 0.0, 1.0],
            normal: [1.0, 0.0, 0.0],
        }),
        _ => None,
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 1e-9 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// C1: build a sketch frame on a B-Rep face. The origin is the face
/// centroid and the normal is the face's (world-space) analytic plane
/// normal; the in-plane `x_axis`/`y_axis` are synthesised by crossing
/// the normal with the world axis least aligned with it (so the basis
/// is well-conditioned for any face orientation).
///
/// `placement` lifts the body-local woven face into world space (the
/// weave is body-local, [`crate::body::Body::placement`] applied at
/// consumption).
/// The body-LOCAL anchor a sketch frame on face `id` sits on — the single
/// source of truth for „where a face-bound sketch's origin is".
///
/// The fingerprint centroid, projected onto the face's exact analytic plane.
/// The projection matters (ticket o2j9): the centroid is sample-derived and
/// sits up to ~µm off the carrier, and a face-extrude anchored here has to land
/// flush so the coincidence registry takes its exact path instead of chasing
/// drift.
///
/// Public and separate from [`face_plane_frame`] because the WEB side needs the
/// very same point (#442). `bodyBrepFaces` used to report a different one — the
/// unweighted mean of the outer loop's edge samples — and the Onshape importer
/// computed its sketch-frame correction against that: 7.53 mm off on the #416
/// document's `Sketch 3`, whose carrier is a trapezoid where the two centroids
/// genuinely differ. Two answers to „where is the face's centre" is one too
/// many when one of them decides where a body lands.
pub fn face_plane_origin_local(brep: &crate::brep::BRep, id: crate::brep::FaceId) -> [f32; 3] {
    use crate::brep::naming::Fingerprint;

    let fp = Fingerprint::of_face(brep, id);
    let mut c_local = [
        fp.centroid[0] as f32,
        fp.centroid[1] as f32,
        fp.centroid[2] as f32,
    ];
    if let crate::brep::SurfaceGeometry::Analytic {
        surface: fielddraft_protocol::HalfSpace::Plane { normal, offset },
        transform,
    } = &brep.face(id).surface
    {
        let n = normalize3(crate::quat_rotate(transform.rotation, *normal));
        let s = transform.scale[0];
        let d = s * offset
            + n[0] * transform.translate[0]
            + n[1] * transform.translate[1]
            + n[2] * transform.translate[2];
        let err = c_local[0] * n[0] + c_local[1] * n[1] + c_local[2] * n[2] - d;
        c_local = [
            c_local[0] - err * n[0],
            c_local[1] - err * n[1],
            c_local[2] - err * n[2],
        ];
    }
    c_local
}

pub fn face_plane_frame(
    brep: &crate::brep::BRep,
    id: crate::brep::FaceId,
    placement: &fielddraft_protocol::Transform,
) -> SketchFrame {
    // Centroid (body-local) → world via the placement.
    let c_local = face_plane_origin_local(brep, id);
    let origin = {
        let p = crate::quat_rotate(
            placement.rotation,
            [
                c_local[0] * placement.scale[0],
                c_local[1] * placement.scale[1],
                c_local[2] * placement.scale[2],
            ],
        );
        [
            p[0] + placement.translate[0],
            p[1] + placement.translate[1],
            p[2] + placement.translate[2],
        ]
    };

    // Local analytic plane normal, rotated into world. Curved faces
    // fall back to +Z locally.
    let n_local = match &brep.face(id).surface {
        crate::brep::SurfaceGeometry::Analytic {
            surface: fielddraft_protocol::HalfSpace::Plane { normal, .. },
            transform,
        } => crate::quat_rotate(transform.rotation, *normal),
        _ => [0.0, 0.0, 1.0],
    };
    let normal = normalize3(crate::quat_rotate(placement.rotation, n_local));

    // Pick the world axis least aligned with the normal, cross twice
    // for an orthonormal in-plane basis.
    let ax = normal[0].abs();
    let ay = normal[1].abs();
    let az = normal[2].abs();
    let seed = if ax <= ay && ax <= az {
        [1.0, 0.0, 0.0]
    } else if ay <= az {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let x_axis = normalize3(cross(seed, normal));
    let y_axis = normalize3(cross(normal, x_axis));
    SketchFrame {
        origin,
        x_axis,
        y_axis,
        normal,
    }
}
