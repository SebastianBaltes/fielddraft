//! Static branch enumeration over the CSG tree (parametric-weaving rebuild,
//! Phase 1 foundation — see `docs/PARAMETRIC-WEAVING-PLAN.md`).
//!
//! A *branch* is one analytic sub-surface of a leaf primitive — a box face, a
//! cylinder wall, an extrude cap, a revolve wall group, … — identified by the
//! same [`BranchId`] the SDF evaluate path tags points with. This module walks
//! a [`BoundedCsg`] and produces the flat set of every branch in the tree, each
//! carrying the owning leaf's world-space [`Aabb`] as a safe over-approximation
//! of the branch's support.
//!
//! Scope is deliberately tight: this is *only* the enumeration. Candidate-pair
//! generation, intrinsic-multiplicity SSI, and per-pair seeding (Phase 1 proper)
//! and the active-interval march (Phase 2) all build on top of this and live
//! elsewhere.
//!
//! ## Branch decomposition per leaf kind
//!
//! - **Analytic primitives** (Box / Cylinder / Cone / Sphere / Torus): the
//!   sub-faces of [`primitive_halfspaces`] — the single source of truth shared
//!   with the SDF tie-break — mapped to a [`SurfaceKind`] by their half-space.
//! - **Extrude**: one [`SurfaceKind::ExtrudeWall`] per sketch boundary group
//!   (`0 .. sketch.edge_count()`) plus the two reserved caps
//!   ([`BranchId::EXTRUDE_CAP_TOP_SUB`] / [`BranchId::EXTRUDE_CAP_BOTTOM_SUB`],
//!   tagged [`SurfaceKind::ExtrudeCap`]).
//! - **Revolve**: one [`SurfaceKind::RevolveWall`] per sketch boundary group; a
//!   revolve has **no** caps.
//! - **Out-of-scope carriers** (Twist / Bend / Displace and any non-half-space
//!   primitive such as Thread / Gyroid / Ellipsoid / Capsule / HexPrism /
//!   ImportedMesh): these have no closed-form branch-pair seeding. We do **not**
//!   invent branches for them — they fall back to the octree — but we *do* still
//!   recurse the tree structurally so branches under them (e.g. a box inside a
//!   `Twist`) are not lost.

use crate::bvh::BoundedCsg;
use crate::csg::{primitive_halfspaces, Aabb, BranchId};
use fielddraft_protocol::{FeatureId, HalfSpace};

/// The analytic surface class a [`Branch`] lies on. Mirrors the half-space
/// families plus the sketch-sweep wall/cap split; [`SurfaceKind::Tessellated`]
/// is reserved for branches that exist topologically but have no closed-form
/// surface (currently unused by the enumerator — out-of-scope carriers emit no
/// branches at all — but kept so downstream pair logic can tag fallbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    ExtrudeWall,
    ExtrudeCap,
    RevolveWall,
    Tessellated,
}

/// One enumerated branch: its [`BranchId`], analytic [`SurfaceKind`], and the
/// owning leaf's world-space [`Aabb`]. The AABB is a conservative bound on the
/// branch's support (the whole leaf, not the trimmed face), which is exactly
/// what Phase 1 pair-pruning wants — a cheap, sound overlap test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Branch {
    pub id: BranchId,
    pub kind: SurfaceKind,
    pub aabb: Aabb,
}

/// Statically enumerate **every** branch in the CSG tree, each carrying its
/// owning leaf's world AABB. Out-of-scope carriers (Twist/Bend/Displace and
/// non-half-space primitives) contribute no branches of their own but are still
/// recursed through, so in-scope branches nested beneath them are preserved.
pub fn enumerate_branches(body: &BoundedCsg) -> Vec<Branch> {
    let mut out = Vec::new();
    walk(body, &mut out);
    out
}

/// Map a primitive sub-face's [`HalfSpace`] to its [`SurfaceKind`]. The
/// half-space family *is* the surface class for analytic primitives, so this is
/// a direct 1:1 mapping. `Gyroid` cannot reach here (no primitive that emits a
/// `HalfSpace::Gyroid` is enumerated by [`primitive_halfspaces`]) but is mapped
/// to `Tessellated` for totality.
fn kind_of_halfspace(hs: &HalfSpace) -> SurfaceKind {
    match hs {
        HalfSpace::Plane { .. } => SurfaceKind::Plane,
        HalfSpace::InfiniteCylinder { .. } => SurfaceKind::Cylinder,
        HalfSpace::Cone { .. } => SurfaceKind::Cone,
        HalfSpace::Sphere { .. } => SurfaceKind::Sphere,
        HalfSpace::Torus { .. } => SurfaceKind::Torus,
        HalfSpace::Gyroid { .. } => SurfaceKind::Tessellated,
    }
}

/// Emit the wall-group branches of a sketch sweep: subs `0 .. group_count`,
/// tagged with `wall_kind`. Shared by Extrude and Revolve.
fn push_sketch_walls(
    id: &FeatureId,
    group_count: u32,
    wall_kind: SurfaceKind,
    aabb: Aabb,
    out: &mut Vec<Branch>,
) {
    for g in 0..group_count {
        out.push(Branch {
            id: BranchId::from_feature_sub(id, g),
            kind: wall_kind,
            aabb,
        });
    }
}

fn walk(node: &BoundedCsg, out: &mut Vec<Branch>) {
    match node {
        BoundedCsg::Empty => {}

        BoundedCsg::Leaf { id, primitive, aabb, .. } => {
            // Analytic primitives decompose via the shared half-space source of
            // truth. Primitives with no closed-form split (Sphere/Torus return
            // a single face here; Thread/Gyroid/Ellipsoid/… return `None`) emit
            // only what `primitive_halfspaces` yields — no invented branches.
            if let Some(hs) = primitive_halfspaces(primitive) {
                for f in hs.faces() {
                    out.push(Branch {
                        id: BranchId::from_feature_sub(id, f.sub),
                        kind: kind_of_halfspace(&f.half_space),
                        aabb: *aabb,
                    });
                }
            }
            // else: out-of-scope primitive (Thread/Gyroid/Ellipsoid/Capsule/
            // HexPrism/ImportedMesh) → octree fallback, no branches.
        }

        BoundedCsg::Extrude { id, sketch, aabb, .. } => {
            // Wall groups `0 .. edge_count` …
            push_sketch_walls(
                id,
                sketch.edge_count(),
                SurfaceKind::ExtrudeWall,
                *aabb,
                out,
            );
            // … plus the two reserved caps (DL-50).
            for cap_sub in [
                BranchId::EXTRUDE_CAP_TOP_SUB,
                BranchId::EXTRUDE_CAP_BOTTOM_SUB,
            ] {
                out.push(Branch {
                    id: BranchId::from_feature_sub(id, cap_sub),
                    kind: SurfaceKind::ExtrudeCap,
                    aabb: *aabb,
                });
            }
        }

        BoundedCsg::Revolve { id, sketch, aabb, .. } => {
            // Revolve has no caps — only wall groups.
            push_sketch_walls(
                id,
                sketch.edge_count(),
                SurfaceKind::RevolveWall,
                *aabb,
                out,
            );
        }

        // Boolean / blend nodes: recurse both operands.
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            walk(left, out);
            walk(right, out);
        }

        // Single-child carriers: recurse the inner subtree. Twist/Bend/Displace
        // are out-of-scope warps (their *own* surface is non-analytic), but the
        // branches of the wrapped geometry must still be enumerated; Shell and
        // Offset preserve their inner branches' identity (DL-25 / Offset folds
        // analytic carriers back into a shifted half-space downstream).
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::Offset { inner, .. } => walk(inner, out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    use fielddraft_protocol::Primitive;

    fn aabb_non_degenerate(a: &Aabb) -> bool {
        a.min[0] <= a.max[0] && a.min[1] <= a.max[1] && a.min[2] <= a.max[2]
    }

    fn assert_all_aabbs_sane(branches: &[Branch]) {
        for b in branches {
            assert!(
                aabb_non_degenerate(&b.aabb),
                "branch {:?} has a degenerate AABB {:?}",
                b.id.as_u32(),
                b.aabb
            );
        }
    }

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

    #[test]
    fn box_has_six_plane_branches() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let branches = enumerate_branches(&body);
        assert_eq!(branches.len(), 6, "a box has six faces");
        assert!(
            branches.iter().all(|b| b.kind == SurfaceKind::Plane),
            "every box face is a plane: {branches:?}"
        );
        // Sub-ids are exactly 0..=5 (the `primitive_halfspaces` convention).
        let mut subs: Vec<u32> = branches.iter().map(|b| b.id.sub()).collect();
        subs.sort_unstable();
        assert_eq!(subs, vec![0, 1, 2, 3, 4, 5]);
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn cylinder_has_wall_plus_two_caps() {
        let body = BoundedCsg::from_csg(&cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 8.0));
        let branches = enumerate_branches(&body);
        assert_eq!(branches.len(), 3, "cylinder = wall + 2 caps");
        let cyls = branches.iter().filter(|b| b.kind == SurfaceKind::Cylinder).count();
        let planes = branches.iter().filter(|b| b.kind == SurfaceKind::Plane).count();
        assert_eq!(cyls, 1, "one cylindrical wall");
        assert_eq!(planes, 2, "two planar caps");
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn extrude_of_rectangle_has_four_walls_and_two_caps() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(7),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);

        let walls = branches.iter().filter(|b| b.kind == SurfaceKind::ExtrudeWall).count();
        let caps = branches.iter().filter(|b| b.kind == SurfaceKind::ExtrudeCap).count();
        assert_eq!(walls, 4, "rectangle → 4 wall groups");
        assert_eq!(caps, 2, "extrude → top + bottom cap");
        assert_eq!(branches.len(), 6);

        // The reserved cap sub-ids must be present.
        let subs: Vec<u32> = branches.iter().map(|b| b.id.sub()).collect();
        assert!(
            subs.contains(&BranchId::EXTRUDE_CAP_TOP_SUB),
            "top-cap sub 0x{:X} missing: {subs:?}",
            BranchId::EXTRUDE_CAP_TOP_SUB
        );
        assert!(
            subs.contains(&BranchId::EXTRUDE_CAP_BOTTOM_SUB),
            "bottom-cap sub 0x{:X} missing: {subs:?}",
            BranchId::EXTRUDE_CAP_BOTTOM_SUB
        );
        // Wall groups are subs 0..4.
        for g in 0..4u32 {
            assert!(subs.contains(&g), "wall group {g} missing: {subs:?}");
        }
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn revolve_has_wall_groups_and_no_caps() {
        // A rectangle profile at x≥0 revolved around Y → a hollow-free
        // cylinder-like solid. Rectangle = 4 boundary groups.
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 0.0],
            half_extents: [2.0, 10.0],
        });
        let csg = CsgNode::Revolve {
            id: FeatureId::from_u32(3),
            sketch: Box::new(sketch),
        };
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);

        assert_eq!(branches.len(), 4, "rectangle → 4 revolve wall groups");
        assert!(
            branches.iter().all(|b| b.kind == SurfaceKind::RevolveWall),
            "revolve emits only walls, no caps: {branches:?}"
        );
        // Explicitly assert no cap sub-ids leaked in.
        let subs: Vec<u32> = branches.iter().map(|b| b.id.sub()).collect();
        assert!(!subs.contains(&BranchId::EXTRUDE_CAP_TOP_SUB));
        assert!(!subs.contains(&BranchId::EXTRUDE_CAP_BOTTOM_SUB));
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn difference_is_the_union_of_both_operands_branches() {
        // Box − Cylinder: 6 box planes + (1 wall + 2 caps) = 9 branches.
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);

        assert_eq!(branches.len(), 6 + 3, "box(6) ∪ cylinder(3) branches");
        let planes = branches.iter().filter(|b| b.kind == SurfaceKind::Plane).count();
        let cyls = branches.iter().filter(|b| b.kind == SurfaceKind::Cylinder).count();
        assert_eq!(planes, 6 + 2, "6 box faces + 2 cylinder caps");
        assert_eq!(cyls, 1);

        // Branches from the two operands carry distinct feature ids.
        let feats: std::collections::HashSet<u32> =
            branches.iter().map(|b| b.id.feature_id()).collect();
        assert_eq!(feats.len(), 2, "two leaves → two feature ids: {feats:?}");
        assert_all_aabbs_sane(&branches);
    }

    #[test]
    fn out_of_scope_warp_still_recurses_to_inner_branches() {
        // A Twist wrapping a box: the warp itself is out of scope (no
        // analytic branch), but the inner box's 6 branches must survive.
        let csg = CsgNode::Twist(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            0.01,
        );
        let body = BoundedCsg::from_csg(&csg);
        let branches = enumerate_branches(&body);
        assert_eq!(branches.len(), 6, "inner box branches survive the warp");
        assert!(branches.iter().all(|b| b.kind == SurfaceKind::Plane));
    }
}
