//! BVH-wrapped CSG tree.
//!
//! `BoundedCsg` is a query-optimised mirror of [`CsgNode`]. The
//! logical IR ([`CsgNode`]) is kept around unchanged — it's still the
//! diff-friendly form that undo/redo, serialisation and the K2
//! contributor tests work against — but the hot paths (the Dual
//! Contouring mesher and the sphere-tracing raycaster) ask questions
//! millions of times per frame, and asking them against the raw
//! `CsgNode` wastes work on two fronts:
//!
//!   1. Every query that needs a bounding box recomputes it bottom-up,
//!      which is `O(N)` on a linear spine of `N` features.
//!   2. Every `sdf` descent walks a `CsgNode::Transform` wrapper per
//!      leaf, applying one quaternion inverse-rotation per hop.
//!
//! `BoundedCsg` fixes both by precomputing a world-space AABB at
//! every node during construction and by folding chains of
//! `CsgNode::Transform` into each leaf's accumulated world transform.
//! It also provides the pruning variant of `sdf` that's the main
//! K3 win: when a sub-tree's AABB is far enough away from the query
//! point to be dominated by the current running value, the sub-tree
//! is skipped entirely.
//!
//! K3 only implements the scaffolding and the single-body pruning.
//! Multi-body isolation and async meshing live in the next slice.

use fielddraft_protocol::{Primitive, Transform};

use crate::csg::{Aabb, Contributors, CsgNode, EvalMode, NodeId};
use crate::{compose_transforms, sdf_primitive, world_to_local};

/// BVH-annotated CSG node. Every variant carries a world-space AABB
/// — `Empty`'s is the empty sentinel, and leaves additionally carry
/// an already-composed world transform so the hot query path doesn't
/// walk intermediate `Transform` nodes.
#[derive(Debug, Clone)]
pub enum BoundedCsg {
    Empty,
    Leaf {
        id: NodeId,
        primitive: Primitive,
        /// Transform from the leaf's primitive frame to world space.
        /// Built by composing every `CsgNode::Transform` from the
        /// root to this leaf during [`BoundedCsg::from_csg`].
        world_transform: Transform,
        /// World-space AABB of the (transformed) primitive.
        aabb: Aabb,
    },
    Union {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        aabb: Aabb,
    },
    Difference {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        aabb: Aabb,
    },
    Intersection {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        aabb: Aabb,
    },
}

impl BoundedCsg {
    /// Fold a logical [`CsgNode`] into a query-optimised BVH. The
    /// result carries precomputed AABBs at every node and has
    /// `CsgNode::Transform` chains collapsed into each leaf.
    pub fn from_csg(csg: &CsgNode) -> Self {
        build(csg, &Transform::IDENTITY)
    }

    /// World-space axis-aligned bounding box of this sub-tree.
    pub fn aabb(&self) -> Aabb {
        match self {
            BoundedCsg::Empty => Aabb::EMPTY,
            BoundedCsg::Leaf { aabb, .. }
            | BoundedCsg::Union { aabb, .. }
            | BoundedCsg::Difference { aabb, .. }
            | BoundedCsg::Intersection { aabb, .. } => *aabb,
        }
    }

    /// Pruned signed-distance query. Produces the same value as the
    /// equivalent [`CsgNode::sdf`] call (modulo the `EvalMode` split
    /// that K10 will introduce) but short-circuits boolean branches
    /// whose AABB already dominates the running accumulator.
    pub fn sdf(&self, p: [f32; 3], mode: EvalMode) -> f32 {
        match self {
            BoundedCsg::Empty => f32::INFINITY,
            BoundedCsg::Leaf {
                primitive,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                sdf_primitive(primitive, local) * world_transform.scale
            }
            BoundedCsg::Union { left, right, .. } => {
                // Visit the AABB-closer child first — it's more
                // likely to give a tight `min` that prunes the other.
                let dist_left = distance_point_to_aabb(p, left.aabb());
                let dist_right = distance_point_to_aabb(p, right.aabb());
                let (first_node, second_node, second_bound) = if dist_left <= dist_right {
                    (&**left, &**right, dist_right)
                } else {
                    (&**right, &**left, dist_left)
                };
                let d_first = first_node.sdf(p, mode);
                // Safe prune: when `p` sits *outside* the second
                // child's AABB (`second_bound > 0`), that child's SDF
                // is ≥ `second_bound` by the Lipschitz-1 property of
                // exact SDFs. If the closer child's exact value is
                // already ≤ the bound, the other branch can't win
                // the min. When `second_bound == 0` we're inside
                // both AABBs and have no lower bound — fall back to
                // the full recursion.
                if second_bound > 0.0 && d_first < second_bound {
                    d_first
                } else {
                    d_first.min(second_node.sdf(p, mode))
                }
            }
            BoundedCsg::Difference { left, right, .. } => {
                // result = max(d_left, -d_right). If the query point
                // sits clearly outside the right's AABB (by
                // `dist_right > 0`), `d_right ≥ dist_right` so
                // `-d_right ≤ -dist_right`. When `d_left ≥ -dist_right`,
                // `max(d_left, -d_right) = d_left` — the subtracted
                // body can't carve anything at this point.
                let d_left = left.sdf(p, mode);
                let dist_right = distance_point_to_aabb(p, right.aabb());
                if dist_right > 0.0 && d_left >= -dist_right {
                    d_left
                } else {
                    d_left.max(-right.sdf(p, mode))
                }
            }
            BoundedCsg::Intersection { left, right, .. } => {
                // Intersection = `max(left, right)`. The AABB of the
                // right only gives us a *lower* bound on its SDF
                // (`d_right ≥ dist_right`), not an upper bound, so
                // we can't safely prune either side — both matter.
                // Kept naive for K3; K10 may revisit.
                left.sdf(p, mode).max(right.sdf(p, mode))
            }
        }
    }

    /// Provenance-aware evaluation. Mirrors [`CsgNode::evaluate`]
    /// with the same tie-merging rules, but traverses the BVH form
    /// so it shares leaf transforms and AABBs with the `sdf` hot
    /// path. No pruning yet — contributor ties across an
    /// almost-equal branch need to see both sides, and the
    /// classification pass in DC only calls this once per mesh
    /// vertex, so the saved branches wouldn't pay for the extra
    /// correctness footwork.
    pub fn evaluate(
        &self,
        p: [f32; 3],
        eps: f32,
        mode: EvalMode,
    ) -> (f32, Contributors) {
        match self {
            BoundedCsg::Empty => (f32::INFINITY, Contributors::empty()),
            BoundedCsg::Leaf {
                id,
                primitive,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let d = sdf_primitive(primitive, local) * world_transform.scale;
                (d, Contributors::single(*id))
            }
            BoundedCsg::Union { left, right, .. } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                combine_min(da, ca, db, cb, eps)
            }
            BoundedCsg::Difference { left, right, .. } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                combine_max(da, ca, -db, cb, eps)
            }
            BoundedCsg::Intersection { left, right, .. } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                combine_max(da, ca, db, cb, eps)
            }
        }
    }
}

/// Recursive constructor used by [`BoundedCsg::from_csg`]. Carries an
/// accumulated transform from the root — initially identity — that is
/// composed with every `CsgNode::Transform` node on the way down and
/// folded into each leaf.
fn build(csg: &CsgNode, carry: &Transform) -> BoundedCsg {
    match csg {
        CsgNode::Empty => BoundedCsg::Empty,
        CsgNode::Leaf { id, primitive } => {
            let local_aabb = Aabb::of_primitive(primitive);
            let aabb = local_aabb.transformed(carry);
            BoundedCsg::Leaf {
                id: *id,
                primitive: primitive.clone(),
                world_transform: *carry,
                aabb,
            }
        }
        CsgNode::Transform(t, inner) => {
            let combined = compose_transforms(carry, t);
            build(inner, &combined)
        }
        CsgNode::Union(a, b) => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = Aabb::union(left.aabb(), right.aabb());
            BoundedCsg::Union {
                left: Box::new(left),
                right: Box::new(right),
                aabb,
            }
        }
        CsgNode::Difference(a, b) => {
            let left = build(a, carry);
            let right = build(b, carry);
            // `max(left, -right)` can only ever shrink the left's
            // extent, so the left AABB is a conservative bound.
            let aabb = left.aabb();
            BoundedCsg::Difference {
                left: Box::new(left),
                right: Box::new(right),
                aabb,
            }
        }
        CsgNode::Intersection(a, b) => {
            let left = build(a, carry);
            let right = build(b, carry);
            // Intersection is a subset of either child; use the left
            // so the bound stays well-defined even when the right is
            // empty or unbounded.
            let aabb = left.aabb();
            BoundedCsg::Intersection {
                left: Box::new(left),
                right: Box::new(right),
                aabb,
            }
        }
    }
}

/// Distance from a point to the surface of an AABB. Zero if the point
/// sits inside the box (the Lipschitz bound is trivial there); the
/// usual `sqrt(Σ max(0, dist_i)²)` when outside.
fn distance_point_to_aabb(p: [f32; 3], aabb: Aabb) -> f32 {
    if aabb.is_empty() {
        return f32::INFINITY;
    }
    let dx = (aabb.min[0] - p[0]).max(0.0).max(p[0] - aabb.max[0]);
    let dy = (aabb.min[1] - p[1]).max(0.0).max(p[1] - aabb.max[1]);
    let dz = (aabb.min[2] - p[2]).max(0.0).max(p[2] - aabb.max[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

// Re-export the contributor combinators used by `evaluate`. They live
// in `csg.rs` as helpers for `CsgNode::evaluate` and are small enough
// that duplicating the code would be silly.
fn combine_min(
    da: f32,
    ca: Contributors,
    db: f32,
    cb: Contributors,
    eps: f32,
) -> (f32, Contributors) {
    if (da - db).abs() <= eps {
        let mut c = ca;
        c.extend(cb);
        (da.min(db), c)
    } else if da < db {
        (da, ca)
    } else {
        (db, cb)
    }
}

fn combine_max(
    da: f32,
    ca: Contributors,
    db: f32,
    cb: Contributors,
    eps: f32,
) -> (f32, Contributors) {
    if (da - db).abs() <= eps {
        let mut c = ca;
        c.extend(cb);
        (da.max(db), c)
    } else if da > db {
        (da, ca)
    } else {
        (db, cb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{BoolOp, Feature, Primitive, Transform};

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id,
            name: format!("Box {id}"),
            op,
            primitive: Primitive::Box {
                center,
                half_extents: half,
            },
            transform: Transform::IDENTITY,
        }
    }

    fn translated_box_feature(id: u32, translate: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id,
            name: format!("TBox {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
            transform: Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: 1.0,
            },
        }
    }

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id,
            name: format!("Sphere {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center, radius },
            transform: Transform::IDENTITY,
        }
    }

    #[test]
    fn empty_tree_has_empty_aabb_and_infinite_sdf() {
        let bvh = BoundedCsg::from_csg(&CsgNode::from_features(&[]));
        assert!(bvh.aabb().is_empty());
        assert!(bvh.sdf([0.0, 0.0, 0.0], EvalMode::Logical).is_infinite());
    }

    #[test]
    fn leaf_world_transform_is_folded_from_csg_transform_wrapper() {
        // CsgNode::from_features wraps each feature in Transform(t, Leaf).
        // After BoundedCsg::from_csg the wrapper is gone and the
        // leaf's stored world_transform equals the feature's own
        // transform.
        let features = vec![translated_box_feature(
            1,
            [10.0, 20.0, 30.0],
            [5.0, 5.0, 5.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_features(&features));
        match &bvh {
            BoundedCsg::Leaf {
                id, world_transform, aabb, ..
            } => {
                assert_eq!(*id, 1);
                assert_eq!(world_transform.translate, [10.0, 20.0, 30.0]);
                assert_eq!(aabb.min, [5.0, 15.0, 25.0]);
                assert_eq!(aabb.max, [15.0, 25.0, 35.0]);
            }
            other => panic!("expected Leaf, got {other:?}"),
        }
    }

    #[test]
    fn aabb_is_union_of_children_for_union_nodes() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feature(2, BoolOp::Union, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_features(&features));
        let bb = bvh.aabb();
        assert_eq!(bb.min, [-5.0, -5.0, -5.0]);
        assert_eq!(bb.max, [25.0, 5.0, 5.0]);
    }

    /// Main correctness test: for a handful of representative trees
    /// the BVH-wrapped `sdf` must match the naive `CsgNode::sdf` to
    /// within f32 noise at every sample point. Establishes that
    /// pruning never strays from the exact Lipschitz bound.
    #[test]
    fn bvh_sdf_matches_csg_sdf_on_mixed_scene() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Difference;
                f
            },
            box_feature(
                3,
                BoolOp::Union,
                [-30.0, 0.0, 0.0],
                [5.0, 5.0, 5.0],
            ),
        ];
        let csg = CsgNode::from_features(&features);
        let bvh = BoundedCsg::from_csg(&csg);
        let samples = [
            [0.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
            [-30.0, 0.0, 0.0],
            [-60.0, 0.0, 0.0],
            [100.0, 100.0, 100.0],
            [15.0, 5.0, 5.0],
            [-4.0, -4.0, -4.0],
        ];
        for p in samples {
            let a = bvh.sdf(p, EvalMode::Visual);
            let b = csg.sdf(p, EvalMode::Visual);
            assert!(
                (a - b).abs() < 1e-4,
                "bvh {a} vs csg {b} at {p:?}",
            );
        }
    }

    #[test]
    fn bvh_evaluate_matches_csg_evaluate_contributors() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Difference;
                f
            },
        ];
        let csg = CsgNode::from_features(&features);
        let bvh = BoundedCsg::from_csg(&csg);
        // A point on the box's +x face well away from the sphere →
        // only the box contributes.
        let p = [20.0, -10.0, -10.0];
        let (ba, ca) = bvh.evaluate(p, 0.5, EvalMode::Logical);
        let (cb, cc) = csg.evaluate(p, 0.5, EvalMode::Logical);
        assert!((ba - cb).abs() < 1e-4);
        assert_eq!(ca.as_slice(), cc.as_slice());
        assert_eq!(ca.as_slice(), &[1]);
    }

    #[test]
    fn pruning_does_not_lose_precision_for_points_near_far_branch() {
        // A two-body scene: one box at the origin, one box 500 mm
        // away. Query a point that sits inside the origin box. The
        // far box's AABB is 500 mm away, which should trigger the
        // Union prune, but the returned value must still equal the
        // naive answer to within f32 roundoff.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::Union, [500.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        ];
        let csg = CsgNode::from_features(&features);
        let bvh = BoundedCsg::from_csg(&csg);
        let p = [5.0, 0.0, 0.0];
        let a = bvh.sdf(p, EvalMode::Visual);
        let b = csg.sdf(p, EvalMode::Visual);
        assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        // Sanity: the origin box has the closer surface, so the
        // pruned result should equal its own sdf (5 mm inside the
        // +x face).
        assert!((a - (-5.0)).abs() < 1e-4);
    }

    #[test]
    fn distance_to_aabb_is_zero_inside_and_positive_outside() {
        let bb = Aabb {
            min: [-10.0, -10.0, -10.0],
            max: [10.0, 10.0, 10.0],
        };
        assert_eq!(distance_point_to_aabb([0.0, 0.0, 0.0], bb), 0.0);
        assert_eq!(distance_point_to_aabb([10.0, 0.0, 0.0], bb), 0.0);
        let d = distance_point_to_aabb([20.0, 0.0, 0.0], bb);
        assert!((d - 10.0).abs() < 1e-5);
        let d = distance_point_to_aabb([13.0, 14.0, 0.0], bb);
        assert!((d - 5.0).abs() < 1e-4, "{d}");
    }
}
