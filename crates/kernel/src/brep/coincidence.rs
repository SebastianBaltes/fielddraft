//! Phase 1 of `docs/BREP-CLASSIFICATION-PLAN.md`: the coincidence registry.
//!
//! Builds oriented equivalence classes over the analytic carrier surfaces of
//! all branches of a body (`branch_halfspace_map`), decided **symbolically on
//! the construction parameters** — never by sampling (the plan's Leitregel).
//! Two flush-stacked extrudes share a wall plane; two boss cylinders share an
//! axis and radius; the registry is where that discrete fact is established,
//! once, for every downstream consumer (SSI's `CoincidentSurface` case, the
//! interior cull, later the octree contributor logic).
//!
//! Orientation is **body-outward**: a branch's primitive-outward normal is
//! flipped by its complement parity (`complemented_features` — subtraction
//! sides of the CSG tree, shell inner walls). A flush `add` therefore pairs
//! ON-opposite (membrane), stacked walls pair ON-same (seam), and a flush
//! `remove` tool face pairs ON-opposite with the wall it erases.
//!
//! ## Tolerance footprint
//!
//! Exactly ONE relative tolerance, [`tol::COINCIDENCE_REL_TOL`], scaled by
//! the body's AABB diagonal for lengths and used raw for angles (see the
//! sizing rationale there). Where surface parameters are
//! construction-identical (a face-extrude seeded from a body face, a sketch
//! on a face) the upstream f32 values are bit-equal and any tolerance
//! passes; the constant only covers honest numeric drift through transform
//! chains.
//!
//! ## Contact order
//!
//! Per the design directive in `docs/ARCHITECTURE-ROADMAP.md` (item 2),
//! contact is modelled as an *order* ([`ContactOrder`]), not a bool. Phase 1
//! registers only order-∞ (coincident carriers); order-1 (tangential along a
//! curve — fillet rails, G1 spline joints) docks here later without redesign.

use std::collections::HashMap;

use crate::brep::weaving::{
    branch_halfspace_map, branch_is_complemented, complemented_features,
};
use crate::bvh::BoundedCsg;
use crate::csg::BranchId;
use fielddraft_protocol::{HalfSpace, Transform};

use super::tol;
use super::vec3::{cross, norm, normalize, scale as vscale, sub};

/// Order of contact between two carrier surfaces. Phase 1 registers only
/// [`ContactOrder::Coincident`]; [`ContactOrder::Tangent`] is the reserved
/// docking point for tangential contact (contact order 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactOrder {
    /// G1 contact along a curve (reserved, not yet computed).
    Tangent,
    /// Identical carrier surface (contact order ∞).
    Coincident,
}

/// Relative orientation of two coincident branches, compared **body-outward**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelOrientation {
    /// Same body-outward normal — coplanar walls of stacked features (the
    /// ON-same lattice entry: keep both faces, joined by a seam edge).
    Same,
    /// Opposing body-outward normals — a flush-add membrane or a flush-remove
    /// zero-thickness wall (the ON-opposite entry: cull / op semantics).
    Opposite,
}

/// A branch's carrier surface in world (body-local weave frame) coordinates,
/// canonicalised for comparison. `Plane` is oriented body-outward (complement
/// parity folded in); for `Cylinder`/`Sphere` the radial orientation is the
/// `complemented` flag (false = material inside, true = a hole/cavity wall).
#[derive(Debug, Clone)]
enum Carrier {
    Plane { normal: [f64; 3], offset: f64 },
    Cylinder { axis: [f64; 3], anchor: [f64; 3], radius: f64, complemented: bool },
    Sphere { center: [f64; 3], radius: f64, complemented: bool },
}

/// One coincidence class: all branches sharing a carrier surface, each with
/// its orientation relative to the class representative (the first member).
#[derive(Debug)]
pub struct CoincidenceClass {
    /// `(branch, orientation vs. the representative)`. The representative is
    /// `members[0]` with [`RelOrientation::Same`] by construction.
    pub members: Vec<(BranchId, RelOrientation)>,
    /// Reserved: Phase 1 only ever stores [`ContactOrder::Coincident`].
    pub order: ContactOrder,
}

/// A detected **order-1 (tangential)** contact between two coaxial
/// revolution carriers (ARCHITECTURE-ROADMAP Punkt 2): the exact tangent
/// circle, expressed in `frame`'s local coordinates (axis = local +Y,
/// circle of `radius` in the plane `y = y_local`) — exactly the inputs of
/// the parametric circle builder, so the seam is materialised symbolically
/// and the tracer is never pointed at the degenerate curve.
#[derive(Debug, Clone)]
pub struct TangentSeam {
    pub a: BranchId,
    pub b: BranchId,
    /// Local → weave-frame placement of the circle's axis frame (one of the
    /// two carriers' transforms).
    pub frame: Transform,
    /// Circle radius in `frame`-local units.
    pub radius: f32,
    /// Circle plane height in `frame`-local units.
    pub y_local: f32,
}

/// The per-body coincidence registry (plan Phase 1). Only classes with ≥ 2
/// members are stored — a branch without coincidence partners is not listed.
#[derive(Debug, Default)]
pub struct CoincidenceRegistry {
    classes: Vec<CoincidenceClass>,
    by_branch: HashMap<u32, (usize, RelOrientation)>,
    /// Order-1 contacts (tangent circles between coaxial revolution
    /// carriers) — the Phase-1 docking point for tangential contact, now
    /// populated for the analytic pair zoo (sphere↔cylinder, sphere↔cone,
    /// plane↔torus, cylinder↔torus, sphere↔torus).
    tangent_seams: Vec<TangentSeam>,
}

impl CoincidenceRegistry {
    /// Build the registry for a body by pairwise carrier comparison over the
    /// analytic half-space map. Branch counts are small (≤ dozens), so the
    /// O(n²) sweep is irrelevant next to one octree cell.
    pub fn build(body: &BoundedCsg) -> Self {
        let aabb = body.aabb();
        let diag = norm([
            (aabb.max[0] - aabb.min[0]) as f64,
            (aabb.max[1] - aabb.min[1]) as f64,
            (aabb.max[2] - aabb.min[2]) as f64,
        ]);
        let tol_len = tol::coincidence_len(diag);
        let tol_ang = tol::COINCIDENCE_ANG_TOL;

        let complemented = complemented_features(body);
        let halfspaces = branch_halfspace_map(body);

        // Deterministic iteration order: sort by branch id.
        let mut branches: Vec<u32> = halfspaces.keys().copied().collect();
        branches.sort_unstable();

        let carriers: Vec<(u32, Carrier)> = branches
            .iter()
            .filter_map(|&b| {
                let (hs, xf) = &halfspaces[&b];
                let comp = branch_is_complemented(BranchId(b), &complemented);
                carrier_of(hs, xf, comp).map(|c| (b, c))
            })
            .collect();

        // Incremental clustering against each class representative.
        let mut classes: Vec<(Carrier, Vec<(BranchId, RelOrientation)>)> = Vec::new();
        for (b, carrier) in carriers {
            let mut placed = false;
            for (rep, members) in classes.iter_mut() {
                if let Some(rel) = coincident(rep, &carrier, tol_len, tol_ang) {
                    members.push((BranchId(b), rel));
                    placed = true;
                    break;
                }
            }
            if !placed {
                classes.push((carrier, vec![(BranchId(b), RelOrientation::Same)]));
            }
        }

        let mut out = CoincidenceRegistry::default();
        for (_, members) in classes.into_iter().filter(|(_, m)| m.len() >= 2) {
            let idx = out.classes.len();
            for &(branch, rel) in &members {
                out.by_branch.insert(branch.as_u32(), (idx, rel));
            }
            out.classes.push(CoincidenceClass { members, order: ContactOrder::Coincident });
        }

        // Order-1 sweep: pairwise tangency of coaxial revolution carriers,
        // decided on exact surface parameters with the SAME single tolerance
        // as coincidence. Cross-feature pairs only — same-feature G1 joints
        // are already one wall branch (the sketch G1 boundary groups).
        for i in 0..branches.len() {
            for j in (i + 1)..branches.len() {
                let (a, b) = (branches[i], branches[j]);
                if BranchId(a).feature_id() == BranchId(b).feature_id() {
                    continue;
                }
                let comp_a = branch_is_complemented(BranchId(a), &complemented);
                let comp_b = branch_is_complemented(BranchId(b), &complemented);
                // A smooth (G1) join needs material on the same side of both
                // carriers; a solid wall tangent to a cavity wall is a cusp.
                if comp_a != comp_b {
                    continue;
                }
                let (hs_a, xf_a) = &halfspaces[&a];
                let (hs_b, xf_b) = &halfspaces[&b];
                if let Some(seam) =
                    tangent_circle(hs_a, xf_a, hs_b, xf_b, tol_len, tol_ang)
                {
                    out.tangent_seams.push(TangentSeam {
                        a: BranchId(a),
                        b: BranchId(b),
                        ..seam
                    });
                }
            }
        }
        out
    }

    /// The detected order-1 (tangent-circle) contacts of the body.
    pub fn tangent_seams(&self) -> &[TangentSeam] {
        &self.tangent_seams
    }

    /// All coincidence classes of the body.
    pub fn classes(&self) -> &[CoincidenceClass] {
        &self.classes
    }

    /// The class id and representative-relative orientation of a branch, if
    /// it has coincidence partners.
    pub fn class_of(&self, branch: BranchId) -> Option<(usize, RelOrientation)> {
        self.by_branch.get(&branch.as_u32()).copied()
    }

    /// Relative body-outward orientation of two branches IF they share a
    /// carrier surface, else `None` (transversal or different carriers).
    pub fn relative(&self, a: BranchId, b: BranchId) -> Option<RelOrientation> {
        let (ca, ra) = self.class_of(a)?;
        let (cb, rb) = self.class_of(b)?;
        if ca != cb {
            return None;
        }
        Some(if ra == rb { RelOrientation::Same } else { RelOrientation::Opposite })
    }

    /// Whether the registry is empty (no coincident geometry in the body) —
    /// the fast path every transversal model takes.
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

/// Lift a branch's half-space into its canonical world carrier. Returns
/// `None` for surface families outside Phase-1 scope (Cone/Torus/Gyroid —
/// the plan's declared scope is Plane/Cylinder/Sphere) and for non-uniform
/// scale (which deforms cylinders/spheres out of their family and planes'
/// normals via the inverse transpose; not produced by today's features).
/// Exact-parameter tangency test for two coaxial revolution carriers.
/// Returns the tangent circle (as a partial [`TangentSeam`] — the caller
/// fills the branch ids) or `None`. Every geometric fact is decided on the
/// carriers' construction parameters with the ONE coincidence tolerance;
/// nothing is sampled.
///
/// Covered pairs (both directions): sphere↔cylinder (coaxial, equal radius
/// → equator circle), sphere↔cone (centre on axis, inscribed-sphere radius
/// → contact circle), plane↔torus (normal ∥ axis, plane at tube extreme →
/// equator-height circle), cylinder↔torus (coaxial, `R_c = R ± r` → torus
/// equator circle), sphere↔torus (concentric, `R_s = R ± r` → equator
/// circle). Each condition also implies the pair meets NOWHERE else, so
/// replacing all traced fragments of the pair with the symbolic circle is
/// sound.
fn tangent_circle(
    hs_a: &HalfSpace,
    xf_a: &Transform,
    hs_b: &HalfSpace,
    xf_b: &Transform,
    tol_len: f64,
    tol_ang: f64,
) -> Option<TangentSeam> {
    // Uniform scales only (like `carrier_of`): under anisotropy these are no
    // longer revolution surfaces.
    let uscale = |xf: &Transform| -> Option<f64> {
        let s = xf.scale;
        (((s[0] - s[1]).abs() as f64) < 1e-6 && ((s[0] - s[2]).abs() as f64) < 1e-6)
            .then_some(s[0] as f64)
    };
    let (sa, sb) = (uscale(xf_a)?, uscale(xf_b)?);
    let origin = |xf: &Transform| -> [f64; 3] {
        [xf.translate[0] as f64, xf.translate[1] as f64, xf.translate[2] as f64]
    };
    let axis = |xf: &Transform| -> [f64; 3] {
        let r = crate::quat_rotate(xf.rotation, [0.0, 1.0, 0.0]);
        normalize([r[0] as f64, r[1] as f64, r[2] as f64])
    };
    let parallel = |u: [f64; 3], v: [f64; 3]| -> bool { norm(cross(u, v)) <= tol_ang };
    // Perpendicular distance of a point to an axis line.
    let axis_dist = |p: [f64; 3], o: [f64; 3], a: [f64; 3]| -> f64 {
        let d = sub(p, o);
        let along = d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
        norm(sub(d, vscale(a, along)))
    };
    let along = |p: [f64; 3], o: [f64; 3], a: [f64; 3]| -> f64 {
        let d = sub(p, o);
        d[0] * a[0] + d[1] * a[1] + d[2] * a[2]
    };

    // Dispatch both orders through one canonical arm each.
    match (hs_a, hs_b) {
        (HalfSpace::Sphere { .. }, HalfSpace::InfiniteCylinder { .. })
        | (HalfSpace::Sphere { .. }, HalfSpace::Cone { .. })
        | (HalfSpace::Sphere { .. }, HalfSpace::Torus { .. })
        | (HalfSpace::Plane { .. }, HalfSpace::Torus { .. })
        | (HalfSpace::InfiniteCylinder { .. }, HalfSpace::Torus { .. })
        | (HalfSpace::RevolvedProfile { .. }, _) => {}
        (HalfSpace::InfiniteCylinder { .. }, HalfSpace::Sphere { .. })
        | (HalfSpace::Cone { .. }, HalfSpace::Sphere { .. })
        | (HalfSpace::Torus { .. }, HalfSpace::Sphere { .. })
        | (HalfSpace::Torus { .. }, HalfSpace::Plane { .. })
        | (HalfSpace::Torus { .. }, HalfSpace::InfiniteCylinder { .. })
        | (_, HalfSpace::RevolvedProfile { .. }) => {
            return tangent_circle(hs_b, xf_b, hs_a, xf_a, tol_len, tol_ang);
        }
        _ => return None,
    }

    match (hs_a, hs_b) {
        // Sphere cap on a cylinder: coaxial, equal radii → the equator
        // circle at the sphere-centre height, in the CYLINDER's frame.
        (HalfSpace::Sphere { radius: rs }, HalfSpace::InfiniteCylinder { radius: rc }) => {
            let c = origin(xf_a);
            let (o, ax) = (origin(xf_b), axis(xf_b));
            let rs_w = *rs as f64 * sa;
            let rc_w = *rc as f64 * sb;
            if axis_dist(c, o, ax) > tol_len || (rs_w - rc_w).abs() > tol_len {
                return None;
            }
            Some(TangentSeam {
                a: BranchId(0),
                b: BranchId(0),
                frame: *xf_b,
                radius: *rc,
                y_local: (along(c, o, ax) / sb) as f32,
            })
        }
        // Inscribed sphere in a cone (ice-cream): centre on the axis at
        // apex-distance d with `R_s = |d|·sin α` → contact circle radius
        // `R_s·cos α` at `y = d − sign(d)·R_s·sin α` (apex-local frame).
        (HalfSpace::Sphere { radius: rs }, HalfSpace::Cone { half_angle }) => {
            let c = origin(xf_a);
            let (apex, ax) = (origin(xf_b), axis(xf_b));
            if axis_dist(c, apex, ax) > tol_len {
                return None;
            }
            let rs_w = *rs as f64 * sa;
            let d = along(c, apex, ax);
            let sin_a = (*half_angle as f64).sin().abs();
            let cos_a = (*half_angle as f64).cos().abs();
            if (rs_w - d.abs() * sin_a).abs() > tol_len {
                return None;
            }
            let rs_l = rs_w / sb; // sphere radius in cone-local units
            Some(TangentSeam {
                a: BranchId(0),
                b: BranchId(0),
                frame: *xf_b,
                radius: (rs_l * cos_a) as f32,
                y_local: ((d / sb) - d.signum() * rs_l * sin_a) as f32,
            })
        }
        // Concentric sphere and torus: `R_s = R ± r` → the torus-equator
        // circle of that radius in the TORUS frame.
        (
            HalfSpace::Sphere { radius: rs },
            HalfSpace::Torus { major_radius, minor_radius },
        ) => {
            let c = origin(xf_a);
            let (o, ax) = (origin(xf_b), axis(xf_b));
            let rs_w = *rs as f64 * sa;
            let rmaj_w = *major_radius as f64 * sb;
            let rmin_w = *minor_radius as f64 * sb;
            if axis_dist(c, o, ax) > tol_len || along(c, o, ax).abs() > tol_len {
                return None;
            }
            let radius = if (rs_w - (rmaj_w + rmin_w)).abs() <= tol_len {
                *major_radius + *minor_radius
            } else if (rs_w - (rmaj_w - rmin_w)).abs() <= tol_len && rmaj_w > rmin_w {
                *major_radius - *minor_radius
            } else {
                return None;
            };
            Some(TangentSeam {
                a: BranchId(0),
                b: BranchId(0),
                frame: *xf_b,
                radius,
                y_local: 0.0,
            })
        }
        // Torus resting on a plane (donut on the table): normal ∥ axis,
        // plane at a tube extreme → circle of the MAJOR radius at that
        // height (torus frame).
        (HalfSpace::Plane { normal, offset }, HalfSpace::Torus { major_radius, minor_radius }) => {
            let n = {
                let r = crate::quat_rotate(xf_a.rotation, *normal);
                normalize([r[0] as f64, r[1] as f64, r[2] as f64])
            };
            let (o, ax) = (origin(xf_b), axis(xf_b));
            if !parallel(n, ax) {
                return None;
            }
            let d_world = sa * (*offset as f64)
                + n[0] * origin(xf_a)[0]
                + n[1] * origin(xf_a)[1]
                + n[2] * origin(xf_a)[2];
            // Plane height along the TORUS axis (sign-normalise n onto ax).
            let sgn = if n[0] * ax[0] + n[1] * ax[1] + n[2] * ax[2] >= 0.0 { 1.0 } else { -1.0 };
            let h = sgn * d_world - (o[0] * ax[0] + o[1] * ax[1] + o[2] * ax[2]);
            let rmin_w = *minor_radius as f64 * sb;
            if (h.abs() - rmin_w).abs() > tol_len {
                return None;
            }
            Some(TangentSeam {
                a: BranchId(0),
                b: BranchId(0),
                frame: *xf_b,
                radius: *major_radius,
                y_local: (h / sb) as f32,
            })
        }
        // Coaxial cylinder and torus: `R_c = R ± r` → the torus-equator
        // circle at the torus centre height (torus frame).
        (
            HalfSpace::InfiniteCylinder { radius: rc },
            HalfSpace::Torus { major_radius, minor_radius },
        ) => {
            let (oc, axc) = (origin(xf_a), axis(xf_a));
            let (o, ax) = (origin(xf_b), axis(xf_b));
            if !parallel(axc, ax) || axis_dist(oc, o, ax) > tol_len {
                return None;
            }
            let rc_w = *rc as f64 * sa;
            let rmaj_w = *major_radius as f64 * sb;
            let rmin_w = *minor_radius as f64 * sb;
            let ok = (rc_w - (rmaj_w + rmin_w)).abs() <= tol_len
                || ((rc_w - (rmaj_w - rmin_w)).abs() <= tol_len && rmaj_w > rmin_w);
            if !ok {
                return None;
            }
            Some(TangentSeam {
                a: BranchId(0),
                b: BranchId(0),
                frame: *xf_b,
                radius: (rc_w / sb) as f32,
                y_local: 0.0,
            })
        }
        // ── Revolved-profile walls: the CONSTRUCTED G1 case (vase stacks).
        // Tangency is decided on the profiles' own chain ENDS in the shared
        // (r, y) half-plane — a shared endpoint with a parallel end tangent
        // (RP↔RP), or an endpoint lying tangentially on the partner's
        // profile line (RP↔cylinder/cone/plane). All 2D exact construction
        // data; nothing is sampled.
        (HalfSpace::RevolvedProfile { segments, offset, .. }, partner) => {
            let (oa, axa) = (origin(xf_a), axis(xf_a));
            let (ob, axb) = (origin(xf_b), axis(xf_b));
            // Both carriers must revolve about the SAME axis line (a plane
            // partner only needs its normal parallel to the axis).
            let coaxial_ok = match partner {
                HalfSpace::Plane { normal, .. } => {
                    let n = {
                        let r = crate::quat_rotate(xf_b.rotation, *normal);
                        normalize([r[0] as f64, r[1] as f64, r[2] as f64])
                    };
                    parallel(n, axa)
                }
                _ => parallel(axa, axb) && axis_dist(ob, oa, axa) <= tol_len,
            };
            if !coaxial_ok {
                return None;
            }
            let ends = revolved_profile_ends(segments, *offset)?;
            let axis_sign = (axa[0] * axb[0] + axa[1] * axb[1] + axa[2] * axb[2]).signum();
            for (p_a, t_a) in ends {
                // A's endpoint as a world point on its meridian plane.
                let end_world = [
                    // radial direction is frame-relative; only radius and
                    // axial height matter for coaxial comparison.
                    p_a[0] * sa, // world radius
                    along(
                        [
                            oa[0] + axa[0] * p_a[1] * sa,
                            oa[1] + axa[1] * p_a[1] * sa,
                            oa[2] + axa[2] * p_a[1] * sa,
                        ],
                        ob,
                        axb,
                    ), // world axial height in B's frame direction
                ];
                let r_b = end_world[0] / sb; // partner-local radius
                let y_b = end_world[1] / sb; // partner-local axial coord
                // A's end tangent in B's (r, y) orientation.
                let t_b = [t_a[0], t_a[1] * axis_sign];
                let matched = match partner {
                    // Another revolved profile: shared chain endpoint with a
                    // parallel end tangent.
                    HalfSpace::RevolvedProfile { segments: seg_b, offset: off_b, .. } => {
                        revolved_profile_ends(seg_b, *off_b).is_some_and(|ends_b| {
                            ends_b.iter().any(|(p_b, tb)| {
                                let dp = (((p_b[0] - r_b) * sb).powi(2)
                                    + ((p_b[1] - y_b) * sb).powi(2))
                                .sqrt();
                                let cross = (t_b[0] * tb[1] - t_b[1] * tb[0]).abs();
                                dp <= tol_len && cross <= tol_ang
                            })
                        })
                    }
                    // Cylinder wall: profile line r = R — endpoint on it
                    // with a VERTICAL (axis-parallel) end tangent.
                    HalfSpace::InfiniteCylinder { radius } => {
                        ((r_b - *radius as f64) * sb).abs() <= tol_len
                            && t_b[0].abs() <= tol_ang
                    }
                    // Cone wall: profile line r = |y|·tan α through the
                    // apex — endpoint on the line, tangent parallel to it.
                    HalfSpace::Cone { half_angle } => {
                        let ta = (*half_angle as f64).tan().abs();
                        let dir = {
                            let d = [ta * y_b.signum(), 1.0];
                            let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
                            [d[0] / n, d[1] / n]
                        };
                        let on_line = ((r_b - y_b.abs() * ta) * sb).abs() <= tol_len;
                        let cross = (t_b[0] * dir[1] - t_b[1] * dir[0]).abs();
                        on_line && cross <= tol_ang
                    }
                    // Axis-normal plane: profile line y = h — endpoint on it
                    // with a HORIZONTAL (radial) end tangent.
                    HalfSpace::Plane { normal, offset: d } => {
                        let n = {
                            let r = crate::quat_rotate(xf_b.rotation, *normal);
                            normalize([r[0] as f64, r[1] as f64, r[2] as f64])
                        };
                        let plane_h = sb * (*d as f64)
                            + n[0] * ob[0]
                            + n[1] * ob[1]
                            + n[2] * ob[2];
                        let end_h = {
                            let w = [
                                oa[0] + axa[0] * p_a[1] * sa,
                                oa[1] + axa[1] * p_a[1] * sa,
                                oa[2] + axa[2] * p_a[1] * sa,
                            ];
                            n[0] * w[0] + n[1] * w[1] + n[2] * w[2]
                        };
                        (end_h - plane_h).abs() <= tol_len && t_a[1].abs() <= tol_ang
                    }
                    _ => false,
                };
                if matched {
                    return Some(TangentSeam {
                        a: BranchId(0),
                        b: BranchId(0),
                        frame: *xf_a,
                        radius: p_a[0] as f32,
                        y_local: p_a[1] as f32,
                    });
                }
            }
            None
        }
        _ => None,
    }
}

/// The two chain ends of a revolved-profile wall group, as
/// `(point, unit_tangent)` in the profile's `(r, y)` plane — the exact
/// construction data the roadmap's "konstruierte Provenienz" rule keys on.
/// `None` for a closed chain (no rim ends), an offset profile (the offset
/// surface's ends are not the raw segment ends) or an empty group.
fn revolved_profile_ends(
    segments: &[fielddraft_protocol::SketchSegment],
    offset: f32,
) -> Option<[([f64; 2], [f64; 2]); 2]> {
    use fielddraft_protocol::SketchSegment as Seg;
    if segments.is_empty() || offset.abs() > 1e-9 {
        return None;
    }
    let pt = |p: [f32; 2]| [p[0] as f64, p[1] as f64];
    let unit = |d: [f64; 2]| -> Option<[f64; 2]> {
        let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
        (n > 1e-12).then(|| [d[0] / n, d[1] / n])
    };
    // Endpoint + OUTGOING tangent of a segment at its start / end.
    let seg_end = |seg: &Seg, at_start: bool| -> Option<([f64; 2], [f64; 2])> {
        match seg {
            Seg::Line { start, end } => {
                let d = unit([end[0] as f64 - start[0] as f64, end[1] as f64 - start[1] as f64])?;
                Some((pt(if at_start { *start } else { *end }), d))
            }
            Seg::Arc { center, radius, start_angle, sweep_angle } => {
                let th = *start_angle as f64
                    + if at_start { 0.0 } else { *sweep_angle as f64 };
                let p = [
                    center[0] as f64 + *radius as f64 * th.cos(),
                    center[1] as f64 + *radius as f64 * th.sin(),
                ];
                let s = (*sweep_angle as f64).signum();
                Some((p, [-th.sin() * s, th.cos() * s]))
            }
            Seg::QuadBezier { p0, p1, p2 } => {
                let (a, b) = if at_start { (p0, p1) } else { (p1, p2) };
                let d = unit([b[0] as f64 - a[0] as f64, b[1] as f64 - a[1] as f64])?;
                Some((pt(if at_start { *p0 } else { *p2 }), d))
            }
        }
    };
    let (start_p, start_t) = seg_end(segments.first()?, true)?;
    let (end_p, end_t) = seg_end(segments.last()?, false)?;
    // A closed chain has no rim ends.
    let gap = ((start_p[0] - end_p[0]).powi(2) + (start_p[1] - end_p[1]).powi(2)).sqrt();
    if gap < 1e-9 {
        return None;
    }
    Some([(start_p, start_t), (end_p, end_t)])
}

fn carrier_of(hs: &HalfSpace, xf: &Transform, complemented: bool) -> Option<Carrier> {
    let s = xf.scale;
    let uniform = ((s[0] - s[1]).abs() as f64) < 1e-6 && ((s[0] - s[2]).abs() as f64) < 1e-6;
    if !uniform {
        return None;
    }
    let s = s[0] as f64;
    let t = [xf.translate[0] as f64, xf.translate[1] as f64, xf.translate[2] as f64];
    let rot = |d: [f32; 3]| -> [f64; 3] {
        let r = crate::quat_rotate(xf.rotation, d);
        normalize([r[0] as f64, r[1] as f64, r[2] as f64])
    };
    match hs {
        HalfSpace::Plane { normal, offset } => {
            // Local plane n·p = offset under p_world = s·R(p_local) + t:
            // world plane N·p = s·offset + N·t with N = R(n). Body-outward
            // folds the complement parity directly into (N, d).
            let n = rot(*normal);
            let d = s * (*offset as f64) + n[0] * t[0] + n[1] * t[1] + n[2] * t[2];
            if complemented {
                Some(Carrier::Plane { normal: [-n[0], -n[1], -n[2]], offset: -d })
            } else {
                Some(Carrier::Plane { normal: n, offset: d })
            }
        }
        HalfSpace::InfiniteCylinder { radius } => {
            let axis = rot([0.0, 1.0, 0.0]);
            // Anchor: the axis point closest to the origin (translation
            // component perpendicular to the axis) — line-canonical.
            let along = t[0] * axis[0] + t[1] * axis[1] + t[2] * axis[2];
            let anchor = sub(t, vscale(axis, along));
            Some(Carrier::Cylinder {
                axis,
                anchor,
                radius: s * (*radius as f64),
                complemented,
            })
        }
        HalfSpace::Sphere { radius } => Some(Carrier::Sphere {
            center: t,
            radius: s * (*radius as f64),
            complemented,
        }),
        HalfSpace::Cone { .. }
        | HalfSpace::Torus { .. }
        | HalfSpace::Gyroid { .. }
        | HalfSpace::RevolvedProfile { .. }
        | HalfSpace::Blend { .. } => None,
    }
}

/// Whether two carriers are the same surface (contact order ∞), and if so
/// with which relative body-outward orientation. Pure parameter comparison —
/// the plan's Leitregel forbids deciding this from samples.
fn coincident(a: &Carrier, b: &Carrier, tol_len: f64, tol_ang: f64) -> Option<RelOrientation> {
    match (a, b) {
        (
            Carrier::Plane { normal: n1, offset: d1 },
            Carrier::Plane { normal: n2, offset: d2 },
        ) => {
            if norm(cross(*n1, *n2)) > tol_ang {
                return None;
            }
            let dot = n1[0] * n2[0] + n1[1] * n2[1] + n1[2] * n2[2];
            let same = dot > 0.0;
            // Same carrier plane: offsets agree in the shared direction.
            let d2_in_n1 = if same { *d2 } else { -d2 };
            if (d1 - d2_in_n1).abs() > tol_len {
                return None;
            }
            Some(if same { RelOrientation::Same } else { RelOrientation::Opposite })
        }
        (
            Carrier::Cylinder { axis: a1, anchor: p1, radius: r1, complemented: c1 },
            Carrier::Cylinder { axis: a2, anchor: p2, radius: r2, complemented: c2 },
        ) => {
            // Axes parallel (either direction), same line, same radius.
            if norm(cross(*a1, *a2)) > tol_ang {
                return None;
            }
            let d = sub(*p2, *p1);
            let along = d[0] * a1[0] + d[1] * a1[1] + d[2] * a1[2];
            if norm(sub(d, vscale(*a1, along))) > tol_len {
                return None;
            }
            if (r1 - r2).abs() > tol_len {
                return None;
            }
            Some(if c1 == c2 { RelOrientation::Same } else { RelOrientation::Opposite })
        }
        (
            Carrier::Sphere { center: p1, radius: r1, complemented: c1 },
            Carrier::Sphere { center: p2, radius: r2, complemented: c2 },
        ) => {
            if norm(sub(*p2, *p1)) > tol_len || (r1 - r2).abs() > tol_len {
                return None;
            }
            Some(if c1 == c2 { RelOrientation::Same } else { RelOrientation::Opposite })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    use fielddraft_protocol::{FeatureId, Primitive};

    fn box_leaf(id: u32, center: [f32; 3], half_extents: [f32; 3]) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Box { center, half_extents },
        }
    }

    fn cylinder_leaf(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Cylinder { center, radius, half_height },
        }
    }

    fn registry(csg: &CsgNode) -> CoincidenceRegistry {
        CoincidenceRegistry::build(&BoundedCsg::from_csg(csg))
    }

    fn count_by_orientation(reg: &CoincidenceRegistry) -> (usize, usize) {
        let mut same = 0;
        let mut opposite = 0;
        for class in reg.classes() {
            for (_, rel) in class.members.iter().skip(1) {
                match rel {
                    RelOrientation::Same => same += 1,
                    RelOrientation::Opposite => opposite += 1,
                }
            }
        }
        (same, opposite)
    }

    #[test]
    fn transversal_overlap_has_no_classes() {
        let csg = CsgNode::Union(
            Box::new(box_leaf(1, [0.0; 3], [5.0, 5.0, 5.0])),
            Box::new(box_leaf(2, [4.0, 3.0, 2.0], [5.0, 5.0, 5.0])),
        );
        assert!(registry(&csg).is_empty());
    }

    #[test]
    fn flush_boxes_pair_walls_same_and_membrane_opposite() {
        // The o2j9 scenario: identical cross-section, stacked flush at z = 5.
        // Four wall pairs are coplanar same-orientation (seam), the touching
        // top/bottom faces are the ON-opposite membrane pair.
        let csg = CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
            Box::new(box_leaf(2, [0.0, 0.0, 10.0], [5.0, 5.0, 5.0])),
        );
        let reg = registry(&csg);
        assert_eq!(reg.classes().len(), 5, "4 wall classes + 1 membrane class");
        assert_eq!(count_by_orientation(&reg), (4, 1));
    }

    #[test]
    fn flush_remove_pairs_opposite() {
        // Tool +X face coincides with body +X face; the tool is complemented,
        // so body-outward they oppose: the ON-opposite zero-thickness case.
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
            Box::new(box_leaf(2, [2.5, 0.0, 0.0], [2.5, 6.0, 6.0])),
        );
        let reg = registry(&csg);
        let (_, opposite) = count_by_orientation(&reg);
        assert_eq!(opposite, 1, "exactly the flush wall pair");
        // Look the pair up explicitly: branch sub-ids follow
        // primitive_halfspaces' face order; assert via relative() over all
        // class members instead of hard-coding sub indices.
        let class = &reg.classes()[0];
        assert_eq!(class.members.len(), 2);
        assert_eq!(
            reg.relative(class.members[0].0, class.members[1].0),
            Some(RelOrientation::Opposite)
        );
    }

    #[test]
    fn coaxial_equal_radius_cylinders_pair_same() {
        // Boss on boss: coincident *cylinder* walls, ON-same. The registry is
        // surface-generic, not plane-only.
        let csg = CsgNode::Union(
            Box::new(cylinder_leaf(1, [0.0, 0.0, 0.0], 5.0, 4.0)),
            Box::new(cylinder_leaf(2, [0.0, 8.0, 0.0], 5.0, 4.0)),
        );
        let reg = registry(&csg);
        // Wall pair ON-same + the flush cap pair ON-opposite.
        let (same, opposite) = count_by_orientation(&reg);
        assert_eq!((same, opposite), (1, 1), "wall seam pair + cap membrane pair");
    }

    #[test]
    fn pin_in_hole_pairs_opposite() {
        // Drill a hole, then refill it with a pin of the same radius: the
        // hole wall (complemented) and the pin wall coincide ON-opposite.
        let csg = CsgNode::Union(
            Box::new(CsgNode::Difference(
                Box::new(box_leaf(1, [0.0; 3], [5.0, 5.0, 5.0])),
                Box::new(cylinder_leaf(2, [0.0; 3], 2.5, 6.0)),
            )),
            Box::new(cylinder_leaf(3, [0.0; 3], 2.5, 5.0)),
        );
        let reg = registry(&csg);
        let wall_pairs: Vec<_> = reg
            .classes()
            .iter()
            .filter(|c| c.members.iter().any(|(b, _)| b.feature_id() == 2))
            .collect();
        assert_eq!(wall_pairs.len(), 1, "hole wall pairs with pin wall");
        assert!(wall_pairs[0]
            .members
            .iter()
            .any(|&(b, rel)| b.feature_id() == 3 && rel == RelOrientation::Opposite));
    }

    #[test]
    fn near_miss_within_feature_size_is_not_coincident() {
        // 1% of the diagonal apart: clearly distinct geometry, NOT a class.
        let csg = CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
            Box::new(box_leaf(2, [0.0, 0.0, 10.2], [5.0, 5.0, 5.0])),
        );
        let reg = registry(&csg);
        let (same, _) = count_by_orientation(&reg);
        assert_eq!(same, 4, "walls still pair, the gap faces must not");
    }

    #[test]
    fn extrude_stack_pairs_via_dl50_halfspaces() {
        // The o2j9 user scenario: two flush-stacked rectangle extrudes. Their
        // analytic faces come from the DL-50 extrude lift, not from
        // primitive_halfspaces — the registry must cover them identically.
        let extrude = |id: u32, z: f32| {
            let e = CsgNode::Extrude {
                id: FeatureId::from_u32(id),
                sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                    center: [0.0, 0.0],
                    half_extents: [5.0, 4.0],
                })),
                half_height: 2.0,
            };
            if z == 0.0 {
                e
            } else {
                CsgNode::Transform(
                    Transform { translate: [0.0, 0.0, z], ..Transform::IDENTITY },
                    Box::new(e),
                )
            }
        };
        let csg = CsgNode::Union(Box::new(extrude(1, 0.0)), Box::new(extrude(2, 4.0)));
        let reg = registry(&csg);
        let (same, opposite) = count_by_orientation(&reg);
        assert_eq!(
            (same, opposite),
            (4, 1),
            "4 coplanar wall pairs ON-same + cap membrane ON-opposite"
        );
    }
}
