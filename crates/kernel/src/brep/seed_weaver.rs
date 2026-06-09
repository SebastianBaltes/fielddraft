//! Branch-seed weaver — Phase 2b of the Parametric-Weaving rebuild
//! (`docs/PARAMETRIC-WEAVING-PLAN.md`).
//!
//! The analytic counterpart to the octree route ([`crate::brep::weaving::weave`]).
//! Where the octree weaver samples a narrow band to *find* edges, this weaver
//! seeds edges directly from CSG branch adjacency
//! ([`crate::brep::branch_seed::seed_branch_pairs`]), traces each one with the
//! active-interval tracer ([`crate::brep::edge_trace::trace_edge`] +
//! `both_branches_active` as the active predicate), deduplicates the trim
//! boundaries into corner vertices, and hands the resulting wireframe to the
//! shared [`crate::brep::weaving::finish_weave`]. The downstream face grouping,
//! loop ordering, watertightness and stable `id_hash` naming are byte-identical
//! to the octree route — only the wireframe's *provenance* differs.
//!
//! This module replaces the four hand-written `weave_parametric` special cases
//! (box / cylinder / edgeless) with one general pipeline that also covers
//! Boolean combinations and analytic sweeps. It is an all-or-nothing path: a
//! body is either fully seed-woven or fully octree-woven (see [`scope_supported`]
//! and the dynamic gates in `build_seed_wireframe`), so the `id_hash` toggle-
//! invariance contract is preserved.

use crate::brep::edge_walker::{body_has_warp, collect_non_lipschitz_features};
use crate::brep::weaving::finish_weave;
use crate::brep::BRep;
use crate::bvh::BoundedCsg;

/// Weave `body` analytically from branch-pair seeds, or return `None` to fall
/// back to the octree route ([`crate::brep::weaving::weave`]).
///
/// `None` is returned for any body outside the analytic scope ([`scope_supported`])
/// or that the seed pipeline cannot fully resolve (unresolved branch pairs, a
/// stalled march, or a spurious corner — see `build_seed_wireframe`). The
/// caller treats `None` as "use the octree".
pub fn weave_branch_seeds(body: &BoundedCsg) -> Option<BRep> {
    if !scope_supported(body) {
        return None;
    }
    let wireframe = build_seed_wireframe(body)?;
    Some(finish_weave(wireframe, body))
}

/// Static structural scope gate. Returns `false` (→ octree) for bodies the
/// analytic branch-seed path provably cannot handle:
///
/// - **Domain warps** (Twist / Bend / Displace): [`branch_halfspace_map`] and
///   the branch fields describe the *un-warped* primitive, so seeds and traces
///   would lie on the wrong surface (the same reason the octree route disables
///   its analytic fast-path under a warp).
/// - **Non-Lipschitz features** (Thread / Gyroid): their field is not a clean
///   Euclidean distance, so neither the SSI seeding nor the tracer can condition
///   an edge there.
///
/// Everything else — analytic primitives, Booleans over them, and analytic
/// Extrude/Revolve sweeps — is admitted here; the *dynamic* gates in
/// `build_seed_wireframe` reject anything that still fails to resolve at trace
/// time.
///
/// [`branch_halfspace_map`]: crate::brep::weaving::branch_halfspace_map
fn scope_supported(body: &BoundedCsg) -> bool {
    if body_has_warp(body) {
        return false;
    }
    let mut non_lipschitz = std::collections::HashSet::new();
    collect_non_lipschitz_features(body, &mut non_lipschitz);
    if !non_lipschitz.is_empty() {
        return false;
    }
    true
}

/// Build the wireframe `BRep` (deduplicated corner vertices + signed traced
/// edges) for an in-scope `body`, or `None` if the analytic pipeline cannot
/// fully resolve it (handed off to the octree).
///
/// Phase 2b-i stub: the seed→trace→topology pipeline lands in 2b-ii/2b-iii.
fn build_seed_wireframe(_body: &BoundedCsg) -> Option<BRep> {
    None
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

    #[test]
    fn scope_admits_plain_box() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        assert!(scope_supported(&body));
    }

    #[test]
    fn scope_admits_boolean_of_analytic_primitives() {
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        assert!(scope_supported(&body));
    }

    #[test]
    fn scope_admits_extrude() {
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
        assert!(scope_supported(&body));
    }

    #[test]
    fn scope_rejects_warp() {
        // A Twist wrapping a box: out of scope (the half-space map describes the
        // un-warped box, so analytic seeding would trace the wrong surface).
        let csg = CsgNode::Twist(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            0.01,
        );
        let body = BoundedCsg::from_csg(&csg);
        assert!(!scope_supported(&body));
    }

    #[test]
    fn scope_rejects_non_lipschitz_gyroid() {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Gyroid { cell_size: 4.0, thickness: 1.0 },
        };
        let body = BoundedCsg::from_csg(&csg);
        assert!(!scope_supported(&body));
    }

    #[test]
    fn stub_weave_returns_none() {
        // 2b-i: the pipeline is not wired yet, so even an in-scope body yields
        // None (→ octree). Replaced by real coverage in 2b-iv.
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        assert!(weave_branch_seeds(&body).is_none());
    }
}
