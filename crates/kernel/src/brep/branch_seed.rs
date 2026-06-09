//! Per-edge seed generation by wiring branch-pair enumeration to the SSI core —
//! Phase 1b-ii of the Parametric-Weaving rebuild (`docs/PARAMETRIC-WEAVING-PLAN.md`).
//!
//! This is the integration that makes Phase 1 produce actual per-edge seeds
//! end-to-end. It composes three existing pieces, adding no new geometry of its
//! own:
//!
//! - [`crate::brep::branch_pairs::enumerate_branch_pairs`] — the canonicalised,
//!   AABB-pruned candidate pairs `(a, b)` with their overlap box.
//! - [`crate::brep::weaving::branch_halfspace_map`] — each branch's analytic
//!   `(HalfSpace, Transform)`. Only analytically-liftable branches are present
//!   (box/cyl/cone faces, extrude caps + straight/circular walls, revolve
//!   walls); tessellated branches (curved paths, torus walls) are absent.
//! - [`crate::brep::ssi::ssi_seeds`] — counts the connected components of the two
//!   surfaces' intersection inside the overlap box and returns one world-space
//!   seed per component.
//!
//! A pair is *unresolved* — handed to the octree fallback — only when one of its
//! branches is missing from the half-space map, or SSI reports `Unsupported`. A
//! pair that SSI evaluates and finds **no** intersection in the box (e.g. two
//! parallel box faces) is a candidate correctly rejected: it emits no seed and
//! is *not* unresolved.

use crate::brep::branch_pairs::{enumerate_branch_pairs, BranchPair};
use crate::brep::ssi::{ssi_seeds, SsiResult};
use crate::brep::weaving::branch_halfspace_map;
use crate::bvh::BoundedCsg;
use crate::csg::BranchId;

/// One world-space seed on the shared edge of two branches. One [`EdgeSeed`] is
/// produced per connected component of the branches' intersection curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeSeed {
    pub a: BranchId,
    pub b: BranchId,
    /// World-space seed on the shared edge.
    pub point: [f32; 3],
}

/// Outcome of seeding every candidate branch pair of a body.
#[derive(Debug, Clone, Default)]
pub struct SeedResult {
    pub seeds: Vec<EdgeSeed>,
    /// Candidate pairs that could NOT be analytically seeded — either branch
    /// missing from [`branch_halfspace_map`], or SSI returned `Unsupported`.
    /// These fall back to the octree route. The [`BranchPair`] is kept for
    /// diagnostics.
    pub unresolved: Vec<BranchPair>,
}

/// Seed every analytically-resolvable candidate branch pair of `body`.
///
/// For each candidate pair, both branches' `(HalfSpace, Transform)` are looked
/// up in the half-space map and intersected via [`ssi_seeds`] inside the pair's
/// overlap box. See the module docs for the resolved/unresolved contract.
pub fn seed_branch_pairs(body: &BoundedCsg) -> SeedResult {
    let map = branch_halfspace_map(body);
    let mut result = SeedResult::default();

    for pair in enumerate_branch_pairs(body) {
        let (Some((ha, ta)), Some((hb, tb))) =
            (map.get(&pair.a.as_u32()), map.get(&pair.b.as_u32()))
        else {
            // One (or both) branches are non-analytic → octree fallback.
            result.unresolved.push(pair);
            continue;
        };

        match ssi_seeds(ha, ta, hb, tb, &pair.overlap) {
            SsiResult::Components(pts) => {
                for point in pts {
                    result.seeds.push(EdgeSeed {
                        a: pair.a,
                        b: pair.b,
                        point,
                    });
                }
                // Empty `pts` = a valid candidate SSI correctly rejected (no
                // real intersection in the box, e.g. parallel faces). Emit
                // nothing; this is NOT unresolved.
            }
            SsiResult::Unsupported => result.unresolved.push(pair),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{sdf_halfspace, CsgNode};
    use crate::world_to_local;
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

    /// On-surface tolerance: the SSI bisection drives residuals far below 1e-4.
    const ON_SURF: f32 = 1e-3;

    fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol && (a[2] - b[2]).abs() <= tol
    }

    #[test]
    fn centered_box_yields_twelve_edge_seeds() {
        // The headline test: a box has C(6,2)=15 intrinsic candidate pairs; the
        // 3 opposite-face (parallel) pairs yield 0 components and drop out,
        // leaving exactly the 12 real cube edges. Every branch is analytic, so
        // nothing is unresolved.
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let res = seed_branch_pairs(&body);

        assert_eq!(res.seeds.len(), 12, "12 cube edges (15 pairs − 3 parallel)");
        assert!(
            res.unresolved.is_empty(),
            "a plain box is fully analytic: {:?}",
            res.unresolved
        );
    }

    #[test]
    fn centered_box_seeds_lie_on_both_branch_surfaces() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let map = branch_halfspace_map(&body);
        let res = seed_branch_pairs(&body);

        for seed in &res.seeds {
            let (ha, ta) = map.get(&seed.a.as_u32()).expect("branch a in map");
            let (hb, tb) = map.get(&seed.b.as_u32()).expect("branch b in map");
            let da = sdf_halfspace(ha, world_to_local(ta, seed.point)).abs();
            let db = sdf_halfspace(hb, world_to_local(tb, seed.point)).abs();
            assert!(da < ON_SURF, "seed {:?} not on branch a: |sdf|={da}", seed.point);
            assert!(db < ON_SURF, "seed {:?} not on branch b: |sdf|={db}", seed.point);
        }
    }

    #[test]
    fn centered_box_seeds_match_known_edge_midpoints() {
        // The 12 cube-edge midpoints of a half-extent-10 box centred at origin:
        // each edge holds two coords at ±10 and one at 0.
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let res = seed_branch_pairs(&body);

        let mut expected: Vec<[f32; 3]> = Vec::new();
        for &s0 in &[-10.0_f32, 10.0] {
            for &s1 in &[-10.0_f32, 10.0] {
                // Edges parallel to each of the three axes (the free axis = 0).
                expected.push([0.0, s0, s1]); // X-parallel
                expected.push([s0, 0.0, s1]); // Y-parallel
                expected.push([s0, s1, 0.0]); // Z-parallel
            }
        }
        assert_eq!(expected.len(), 12);

        // Every seed matches a distinct expected midpoint, and vice versa.
        let tol = 1e-3;
        for seed in &res.seeds {
            assert!(
                expected.iter().any(|&e| near(seed.point, e, tol)),
                "seed {:?} is not at any known cube-edge midpoint",
                seed.point
            );
        }
        for &e in &expected {
            assert!(
                res.seeds.iter().any(|s| near(s.point, e, tol)),
                "expected edge midpoint {e:?} has no seed"
            );
        }
    }

    #[test]
    fn box_minus_concentric_cylinder_seeds() {
        // Cylinder radius 5, half-height 12 > box half-extent 10, axis Y → it
        // punches fully through the box. Candidate seeds (16 total, ALL analytic,
        // ALL on both surfaces):
        //   • 12 = the box's own edges (intrinsic f1×f1).
        //   •  2 = the hole rims where the cylinder wall meets the box top/bottom
        //          faces at y=±10 (cross f1×f2) — real edges of the result.
        //   •  2 = the cylinder's own cap rims at y=±12 (intrinsic f2×f2) — real
        //          edges of the cylinder operand, but TRIMMED AWAY by the
        //          difference (they sit outside the box). They are legitimate
        //          candidates; Phase 2's active-interval march drops them. Over-
        //          seeding here is the safe direction ("never miss a real edge").
        // The cylinder wall (radius 5) never meets a box SIDE face (radius 5 < 10)
        // so those cross pairs correctly yield 0 components.
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        let map = branch_halfspace_map(&body);
        let res = seed_branch_pairs(&body);

        assert!(res.unresolved.is_empty(), "fully analytic body");
        assert_eq!(res.seeds.len(), 16, "12 box edges + 2 hole rims + 2 cap rims");

        // EVERY seed must lie on both of its branch surfaces — now that the SSI
        // plane∩plane line is properly box-clipped, there are no off-surface
        // seeds at all.
        for s in &res.seeds {
            let (ha, ta) = map.get(&s.a.as_u32()).expect("branch a in map");
            let (hb, tb) = map.get(&s.b.as_u32()).expect("branch b in map");
            let da = sdf_halfspace(ha, world_to_local(ta, s.point)).abs();
            let db = sdf_halfspace(hb, world_to_local(tb, s.point)).abs();
            assert!(
                da < ON_SURF && db < ON_SURF,
                "seed {:?} off-surface: da={da} db={db}",
                s.point
            );
        }

        // The 12 intrinsic box edges are all present.
        let box_edges = res
            .seeds
            .iter()
            .filter(|s| s.a.feature_id() == 1 && s.b.feature_id() == 1)
            .count();
        assert_eq!(box_edges, 12, "the 12 box-internal edges");

        // Both hole rims (cross f1×f2, on the cylinder wall x²+z²≈25, at y=±10)
        // are present — the boundary-coincident edges the padded scan mask now
        // captures.
        let on_wall = |s: &EdgeSeed, y: f32| {
            let r = (s.point[0] * s.point[0] + s.point[2] * s.point[2]).sqrt();
            (r - 5.0).abs() < 1e-2 && (s.point[1] - y).abs() < 1e-2
        };
        let cross = |s: &&EdgeSeed| s.a.feature_id() != s.b.feature_id();
        assert!(
            res.seeds.iter().filter(cross).any(|s| on_wall(s, 10.0)),
            "hole rim at y=+10 missing"
        );
        assert!(
            res.seeds.iter().filter(cross).any(|s| on_wall(s, -10.0)),
            "hole rim at y=-10 missing"
        );
    }
}
