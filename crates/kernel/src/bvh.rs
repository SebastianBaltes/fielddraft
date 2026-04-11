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

use crate::csg::{sdf_extrude, Aabb, Contributors, CsgNode, EvalMode, NodeId};
use crate::sketch2d::BoundedSketch2d;
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
    /// K10.0: smooth-min union with blend radius.
    SmoothUnion {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        radius: f32,
        aabb: Aabb,
    },
    /// K10.0: smooth-max difference with blend radius.
    SmoothDifference {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        radius: f32,
        aabb: Aabb,
    },
    /// BVH-wrapped extrude. The 2D sketch is materialised into a
    /// [`BoundedSketch2d`] at construction time so every 3D query
    /// hits the pruned 2D path. `world_transform` carries the
    /// accumulated [`CsgNode::Transform`] chain that led here —
    /// exactly like [`BoundedCsg::Leaf`].
    Extrude {
        id: NodeId,
        sketch: Box<BoundedSketch2d>,
        half_height: f32,
        world_transform: Transform,
        aabb: Aabb,
    },
    /// BVH-wrapped revolve. Same sketch-bounding and transform
    /// collapse as [`BoundedCsg::Extrude`]; the query maps 3D to
    /// `(√(x² + z²), y)` before hitting the 2D tree.
    Revolve {
        id: NodeId,
        sketch: Box<BoundedSketch2d>,
        world_transform: Transform,
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
            | BoundedCsg::Intersection { aabb, .. }
            | BoundedCsg::SmoothUnion { aabb, .. }
            | BoundedCsg::SmoothDifference { aabb, .. }
            | BoundedCsg::Extrude { aabb, .. }
            | BoundedCsg::Revolve { aabb, .. } => *aabb,
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
                left.sdf(p, mode).max(right.sdf(p, mode))
            }
            // K10.0: smooth variants. No AABB pruning in the
            // blend zone — both children must be evaluated to
            // feed smin_poly. The prune heuristic from Union
            // is only valid for the hard-min case.
            BoundedCsg::SmoothUnion {
                left,
                right,
                radius,
                ..
            } => {
                let da = left.sdf(p, mode);
                let db = right.sdf(p, mode);
                match mode {
                    EvalMode::Visual => crate::csg::smin_poly(da, db, *radius),
                    EvalMode::Logical => da.min(db),
                }
            }
            BoundedCsg::SmoothDifference {
                left,
                right,
                radius,
                ..
            } => {
                let da = left.sdf(p, mode);
                let db = right.sdf(p, mode);
                match mode {
                    EvalMode::Visual => -crate::csg::smin_poly(-da, db, *radius),
                    EvalMode::Logical => da.max(-db),
                }
            }
            BoundedCsg::Extrude {
                sketch,
                half_height,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let d2 = sketch.sdf([local[0], local[1]]);
                sdf_extrude(d2, local[2], *half_height) * world_transform.scale
            }
            BoundedCsg::Revolve {
                sketch,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let r = (local[0] * local[0] + local[2] * local[2]).sqrt();
                sketch.sdf([r, local[1]]) * world_transform.scale
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
                (d, Contributors::single(id.clone()))
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
            BoundedCsg::SmoothUnion {
                left,
                right,
                radius,
                ..
            } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                match mode {
                    EvalMode::Visual => {
                        let d = crate::csg::smin_poly(da, db, *radius);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => combine_min(da, ca, db, cb, eps),
                }
            }
            BoundedCsg::SmoothDifference {
                left,
                right,
                radius,
                ..
            } => {
                let (da, ca) = left.evaluate(p, eps, mode);
                let (db, cb) = right.evaluate(p, eps, mode);
                match mode {
                    EvalMode::Visual => {
                        let d = -crate::csg::smin_poly(-da, db, *radius);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => combine_max(da, ca, -db, cb, eps),
                }
            }
            BoundedCsg::Extrude {
                id,
                sketch,
                half_height,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let d2 = sketch.sdf([local[0], local[1]]);
                let d = sdf_extrude(d2, local[2], *half_height) * world_transform.scale;
                (d, Contributors::single(id.clone()))
            }
            BoundedCsg::Revolve {
                id,
                sketch,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let r = (local[0] * local[0] + local[2] * local[2]).sqrt();
                let d = sketch.sdf([r, local[1]]) * world_transform.scale;
                (d, Contributors::single(id.clone()))
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
                id: id.clone(),
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
        CsgNode::SmoothUnion(a, b, r) => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = Aabb::union(left.aabb(), right.aabb());
            BoundedCsg::SmoothUnion {
                left: Box::new(left),
                right: Box::new(right),
                radius: *r,
                aabb,
            }
        }
        CsgNode::SmoothDifference(a, b, r) => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = left.aabb();
            BoundedCsg::SmoothDifference {
                left: Box::new(left),
                right: Box::new(right),
                radius: *r,
                aabb,
            }
        }
        CsgNode::Extrude {
            id,
            sketch,
            half_height,
        } => {
            let bounded_sketch = BoundedSketch2d::from_sketch(sketch);
            let s2 = bounded_sketch.aabb();
            let local_aabb = if s2.is_empty() {
                Aabb::EMPTY
            } else {
                Aabb {
                    min: [s2.min[0], s2.min[1], -*half_height],
                    max: [s2.max[0], s2.max[1], *half_height],
                }
            };
            let aabb = local_aabb.transformed(carry);
            BoundedCsg::Extrude {
                id: id.clone(),
                sketch: Box::new(bounded_sketch),
                half_height: *half_height,
                world_transform: *carry,
                aabb,
            }
        }
        CsgNode::Revolve { id, sketch } => {
            let bounded_sketch = BoundedSketch2d::from_sketch(sketch);
            let s2 = bounded_sketch.aabb();
            let local_aabb = if s2.is_empty() {
                Aabb::EMPTY
            } else {
                let r_max = s2.max[0].abs().max(s2.min[0].abs());
                Aabb {
                    min: [-r_max, s2.min[1], -r_max],
                    max: [r_max, s2.max[1], r_max],
                }
            };
            let aabb = local_aabb.transformed(carry);
            BoundedCsg::Revolve {
                id: id.clone(),
                sketch: Box::new(bounded_sketch),
                world_transform: *carry,
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
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Box {id}"),
            op,
            primitive: Primitive::Box {
                center,
                half_extents: half,
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
        }
    }

    fn translated_box_feature(id: u32, translate: [f32; 3], half: [f32; 3]) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
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
            blend_radius: 0.0,
        }
    }

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center, radius },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
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
                assert_eq!(*id, FeatureId::from_u32(1));
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
        assert_eq!(ca.as_slice(), &[FeatureId::from_u32(1)]);
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

    // ─── K4: extrude / revolve through the BVH ────────────────────

    use crate::csg::ProxyElement;
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

    #[test]
    fn bvh_extrude_matches_raw_csg_extrude() {
        // A rectangle lifted into a slab — BoundedCsg and CsgNode
        // must agree on every sample point because K4 has no
        // pruning to miss; the BVH form only folds in the
        // transform chain and caches the AABB.
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let samples = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 2.0],
            [5.0, 3.0, 2.0],
            [7.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
            [-1.0, -1.0, -1.0],
        ];
        for p in samples {
            let a = bvh.sdf(p, EvalMode::Visual);
            let b = csg.sdf(p, EvalMode::Visual);
            assert!((a - b).abs() < 1e-4, "mismatch at {p:?}: {a} vs {b}");
        }
    }

    #[test]
    fn bvh_extrude_returns_single_contributor_in_evaluate() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        });
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(7),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        // Top face centre → surface, contributor = {7}, Face.
        let (d, c) = bvh.evaluate([0.0, 0.0, 2.0], 0.25, EvalMode::Logical);
        assert!(d.abs() < 1e-4);
        assert_eq!(c.as_slice(), &[FeatureId::from_u32(7)]);
        assert_eq!(c.element(), ProxyElement::Face);
    }

    #[test]
    fn bvh_revolve_aabb_sweeps_the_sketch_radially() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 0.0],
            half_extents: [2.0, 10.0],
        });
        let csg = CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let bb = bvh.aabb();
        assert_eq!(bb.min, [-7.0, -10.0, -7.0]);
        assert_eq!(bb.max, [7.0, 10.0, 7.0]);
    }

    #[test]
    fn dual_contour_produces_a_mesh_from_an_extruded_ring() {
        // Integration test: a ring sketch extruded into a washer,
        // BVH-wrapped and fed to the DC mesher. We don't check
        // exact vertex counts — just that a non-trivial mesh falls
        // out and every vertex lies near the surface of the CSG.
        use crate::dc::dual_contour;

        let outer = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 20.0,
        });
        let inner = Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 10.0,
        });
        let ring = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let csg = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(ring),
            half_height: 5.0,
        };
        let bvh = BoundedCsg::from_csg(&csg);
        let mesh = dual_contour(&bvh, 32);
        assert!(
            mesh.vertex_count() > 200,
            "expected a non-trivial washer mesh, got {}",
            mesh.vertex_count(),
        );
        assert!(mesh.triangle_count() > 0);

        // Every vertex should hug the SDF zero level. Cell size at
        // resolution 32 on a 40-mm-square footprint is ≈ 1.25 mm,
        // so we allow drift up to a few cells.
        let mut worst = 0.0_f32;
        for i in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[i * 3],
                mesh.positions[i * 3 + 1],
                mesh.positions[i * 3 + 2],
            ];
            let d = bvh.sdf(p, EvalMode::Visual).abs();
            if d > worst {
                worst = d;
            }
        }
        assert!(worst < 3.0, "washer mesh drifted off the surface by {worst}");
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
