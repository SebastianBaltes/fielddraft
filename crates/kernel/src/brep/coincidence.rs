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

/// The per-body coincidence registry (plan Phase 1). Only classes with ≥ 2
/// members are stored — a branch without coincidence partners is not listed.
#[derive(Debug, Default)]
pub struct CoincidenceRegistry {
    classes: Vec<CoincidenceClass>,
    by_branch: HashMap<u32, (usize, RelOrientation)>,
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
        out
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
        HalfSpace::Cone { .. } | HalfSpace::Torus { .. } | HalfSpace::Gyroid { .. } => None,
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
