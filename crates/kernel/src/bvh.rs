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

use serde::Serialize;

use fielddraft_protocol::{FeatureId, Primitive, Transform};

use crate::csg::{
    extrude_contributors, OpenBranchSet,
    revolve_contributors, sdf_extrude,
    Aabb, BlendMode, Contributors, CsgNode, EvalMode, OpenSurface,
};
use crate::sketch2d::BoundedSketch2d;
use crate::{compose_transforms, sdf_primitive, world_to_local};

// ── Hybrid renderer classification ────────────────────────────────

/// A non-rigid domain warp the web mesher applies to a leaf's geometry
/// (in WORLD space — every BoundedCsg operator runs in world coords since
/// transforms are baked into leaves). Twist/Bend wrap a child subtree, so
/// `classify_walk` tags every descendant leaf with the warp; the mesher
/// bakes the world transform, subdivides along the warp axis, then deforms
/// the vertices. See [`CsgNode::Twist`] / [`CsgNode::Bend`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LeafWarp {
    /// Rotate XZ by `rate · y` around world Y. A diffeomorphism — never
    /// self-intersects, so no validity gate (only adaptive subdivision).
    Twist { rate: f32 },
    /// Rotate XY by `rate · x` around world Z. Self-intersects once the
    /// inner side wraps past the axis, so it is emitted ONLY when the
    /// analytic gate `|rate| · y_extent < 1` holds (else SDF fallback).
    Bend { rate: f32 },
}

/// Spine payload for a parametric `"sweep"` leaf: the web mesher
/// (`buildSweepGeometry`) sweeps the leaf's `contour` along these points
/// (re-deriving the same RMF frames). Local-space; `transform` places it.
#[derive(Debug, Clone, Serialize)]
pub struct SweepLeafData {
    pub points: Vec<[f32; 3]>,
    pub closed: bool,
    /// Total profile twist (radians) the web mesher distributes by arclength.
    pub twist: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct HybridLeafInfo {
    pub kind: String,
    pub aabb: [f32; 6],
    pub transform: [f32; 10],
    pub params: Vec<f32>,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contour: Option<crate::sketch2d::ContourData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warp: Option<LeafWarp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sweep: Option<SweepLeafData>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdfRegionInfo {
    pub aabb: [f32; 6],
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HybridInfo {
    pub leaves: Vec<HybridLeafInfo>,
    pub sdf_regions: Vec<SdfRegionInfo>,
    pub needs_full_sdf: bool,
    pub is_union_only: bool,
}

/// BVH-annotated CSG node. Every variant carries a world-space AABB
/// — `Empty`'s is the empty sentinel, and leaves additionally carry
/// an already-composed world transform so the hot query path doesn't
/// walk intermediate `Transform` nodes.
#[derive(Debug, Clone)]
pub enum BoundedCsg {
    Empty,
    Leaf {
        id: FeatureId,
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
        mode: BlendMode,
        aabb: Aabb,
    },
    /// K10.0: smooth-max difference with blend radius.
    SmoothDifference {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        radius: f32,
        mode: BlendMode,
        aabb: Aabb,
    },
    /// K11.1: twist around local Y axis with a radians/mm rate.
    Twist {
        inner: Box<BoundedCsg>,
        rate: f32,
        aabb: Aabb,
    },
    /// K11.2: bend in the XY plane along X with a radians/mm rate.
    Bend {
        inner: Box<BoundedCsg>,
        rate: f32,
        aabb: Aabb,
    },
    /// K11.3: displace the child's surface by a sine-product pattern.
    Displace {
        inner: Box<BoundedCsg>,
        depth: f32,
        freq: f32,
        aabb: Aabb,
    },
    /// Shell: hollow the child into an inward wall of given thickness, with
    /// optional `open_faces` where the wall breaks through with constant
    /// thickness (see [`crate::csg::shell_wall_sdf`]).
    Shell {
        inner: Box<BoundedCsg>,
        thickness: f32,
        open_faces: Vec<OpenSurface>,
        aabb: Aabb,
    },
    /// General SDF offset `inner.sdf − δ` (see [`CsgNode::Offset`]). Dilates the
    /// body for `δ > 0`, erodes for `δ < 0`; the surface's branch identity is
    /// preserved so the offset face stays selectable.
    Offset {
        inner: Box<BoundedCsg>,
        delta: f32,
        aabb: Aabb,
    },
    /// BVH-wrapped extrude. The 2D sketch is materialised into a
    /// [`BoundedSketch2d`] at construction time so every 3D query
    /// hits the pruned 2D path. `world_transform` carries the
    /// accumulated [`CsgNode::Transform`] chain that led here —
    /// exactly like [`BoundedCsg::Leaf`].
    Extrude {
        id: FeatureId,
        sketch: Box<BoundedSketch2d>,
        half_height: f32,
        world_transform: Transform,
        aabb: Aabb,
    },
    /// BVH-wrapped revolve. Same sketch-bounding and transform
    /// collapse as [`BoundedCsg::Extrude`]; the query maps 3D to
    /// `(√(x² + z²), y)` before hitting the 2D tree.
    Revolve {
        id: FeatureId,
        sketch: Box<BoundedSketch2d>,
        world_transform: Transform,
        aabb: Aabb,
    },
    /// BVH-wrapped helical sweep (see [`CsgNode::Helix`]). The query
    /// maps 3D to the profile's `(r, w)` frame via
    /// [`crate::helix_profile_point`] (nearest-turn unwrap + Lipschitz
    /// scale) before hitting the 2D tree.
    Helix {
        id: FeatureId,
        sketch: Box<BoundedSketch2d>,
        pitch: f32,
        turns: f32,
        left_handed: bool,
        taper: f32,
        start_angle: f32,
        y_anchor: f32,
        r_inner: f32,
        world_transform: Transform,
        aabb: Aabb,
    },
    /// BVH-wrapped general sweep (see [`CsgNode::Sweep`]). The query is
    /// mapped into the nearest spine point's rotation-minimizing frame via
    /// the shared [`crate::sweep_path::SweepPath`], then hits the 2D tree.
    /// `Arc` keeps `BoundedCsg` clones cheap (no deep BVH copy).
    Sweep {
        id: FeatureId,
        sketch: Box<BoundedSketch2d>,
        path: std::sync::Arc<crate::sweep_path::SweepPath>,
        world_transform: Transform,
        aabb: Aabb,
    },
    /// K10.3: spatially-masked blend. BVH mirror of
    /// [`CsgNode::MaskedBlend`]. The AABB is the union of both
    /// children (same conservative bound as SmoothUnion).
    MaskedBlend {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        mask_polyline: Vec<[f32; 3]>,
        mask_radius: f32,
        blend_radius: f32,
        blend_mode: BlendMode,
        is_difference: bool,
        aabb: Aabb,
    },
    /// K11.5: morph (linear interpolation) between two SDFs.
    /// `d = (1 − t) · left.sdf(p) + t · right.sdf(p)` where `t ∈ [0, 1]`.
    Morph {
        left: Box<BoundedCsg>,
        right: Box<BoundedCsg>,
        t: f32,
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

    /// Fold a logical [`CsgNode`] into a BVH starting from an explicit
    /// accumulated transform `carry` rather than identity. Every leaf's
    /// `world_transform` is then `carry ∘ (the leaf's own transform
    /// chain)`, and the SDF/AABBs are expressed in `carry`'s output
    /// frame. Passing `carry = inverse(placement)` re-expresses a body in
    /// its body-local frame so the woven B-Rep is placement-free (the
    /// DL-50 correction); `from_csg` is the `carry = IDENTITY` special
    /// case (world frame).
    pub fn from_csg_with_carry(csg: &CsgNode, carry: &Transform) -> Self {
        build(csg, carry)
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
            | BoundedCsg::Twist { aabb, .. }
            | BoundedCsg::Bend { aabb, .. }
            | BoundedCsg::Displace { aabb, .. }
            | BoundedCsg::Shell { aabb, .. }
            | BoundedCsg::Offset { aabb, .. }
            | BoundedCsg::Extrude { aabb, .. }
            | BoundedCsg::Revolve { aabb, .. }
            | BoundedCsg::Helix { aabb, .. }
            | BoundedCsg::Sweep { aabb, .. }
            | BoundedCsg::MaskedBlend { aabb, .. }
            | BoundedCsg::Morph { aabb, .. } => *aabb,
        }
    }

    /// Collect the accumulated world transform for every leaf,
    /// keyed by the flat `u32` feature id. Used by Phase C
    /// (developable-surface simplification) to convert mesh
    /// vertices from world space back to primitive-local space.
    pub fn leaf_world_transforms(&self) -> std::collections::HashMap<u32, Transform> {
        let mut out = std::collections::HashMap::new();
        self.collect_leaf_transforms(&mut out);
        out
    }

    fn collect_leaf_transforms(&self, out: &mut std::collections::HashMap<u32, Transform>) {
        match self {
            BoundedCsg::Empty => {}
            BoundedCsg::Leaf { id, world_transform, .. } => {
                if let Some(flat) = id.as_u32_if_flat() {
                    out.insert(flat, *world_transform);
                }
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. }
            | BoundedCsg::SmoothUnion { left, right, .. }
            | BoundedCsg::SmoothDifference { left, right, .. }
            | BoundedCsg::MaskedBlend { left, right, .. }
            | BoundedCsg::Morph { left, right, .. } => {
                left.collect_leaf_transforms(out);
                right.collect_leaf_transforms(out);
            }
            BoundedCsg::Twist { inner, .. }
            | BoundedCsg::Bend { inner, .. }
            | BoundedCsg::Displace { inner, .. }
            | BoundedCsg::Offset { inner, .. }
            | BoundedCsg::Shell { inner, .. } => {
                inner.collect_leaf_transforms(out);
            }
            BoundedCsg::Extrude { .. }
            | BoundedCsg::Revolve { .. }
            | BoundedCsg::Helix { .. }
            | BoundedCsg::Sweep { .. } => {}
        }
    }

    /// Classify this CSG tree for the hybrid mesh+SDF renderer.
    /// Returns leaf info for meshable primitives, SDF overlay regions
    /// for boolean overlap zones, and flags for full-SDF fallback.
    pub fn classify_for_hybrid(&self) -> HybridInfo {
        let mut info = HybridInfo {
            leaves: Vec::new(),
            sdf_regions: Vec::new(),
            needs_full_sdf: false,
            is_union_only: true,
        };
        classify_walk(self, &mut info, "mesh", None);
        info
    }

    /// Pruned signed-distance query. Produces the same value as the
    /// equivalent [`CsgNode::sdf`] call (modulo the `EvalMode` split
    /// that K10 will introduce) but short-circuits boolean branches
    /// whose AABB already dominates the running accumulator.
    pub fn sdf(&self, p: [f32; 3], mode: EvalMode) -> f32 {
        self.sdf_open(p, mode, &OpenBranchSet::EMPTY)
    }

    /// [`Self::sdf`] under a set of opened shell branches: leaves owning an
    /// opened sub-face evaluate their RETAINED boundary distance instead
    /// (sign unchanged) — see [`crate::csg::shell_wall_sdf`].
    pub(crate) fn sdf_open(&self, p: [f32; 3], mode: EvalMode, open: &OpenBranchSet<'_>) -> f32 {
        match self {
            BoundedCsg::Empty => f32::INFINITY,
            BoundedCsg::Leaf {
                id,
                primitive,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    crate::csg::leaf_sdf_retained(id, primitive, local, open)
                } else {
                    sdf_primitive(primitive, local)
                };
                d * world_transform.min_scale()
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
                let d_first = first_node.sdf_open(p, mode, open);
                // Safe prune: when `p` sits *outside* the second
                // child's AABB (`second_bound > 0`), that child's SDF
                // is ≥ `second_bound` by the Lipschitz-1 property of
                // exact SDFs. If the closer child's exact value is
                // already ≤ the bound, the other branch can't win
                // the min. When `second_bound == 0` we're inside
                // both AABBs and have no lower bound — fall back to
                // the full recursion. Retained-boundary fields (open
                // shells) extend past their AABB, so pruning is off
                // whenever an open set is active.
                if open.is_empty() && second_bound > 0.0 && d_first < second_bound {
                    d_first
                } else {
                    d_first.min(second_node.sdf_open(p, mode, open))
                }
            }
            BoundedCsg::Difference { left, right, .. } => {
                // result = max(d_left, -d_right). If the query point
                // sits clearly outside the right's AABB (by
                // `dist_right > 0`), `d_right ≥ dist_right` so
                // `-d_right ≤ -dist_right`. When `d_left ≥ -dist_right`,
                // `max(d_left, -d_right) = d_left` — the subtracted
                // body can't carve anything at this point.
                let d_left = left.sdf_open(p, mode, open);
                let dist_right = distance_point_to_aabb(p, right.aabb());
                if open.is_empty() && dist_right > 0.0 && d_left >= -dist_right {
                    d_left
                } else {
                    d_left.max(-right.sdf_open(p, mode, open))
                }
            }
            BoundedCsg::Intersection { left, right, .. } => {
                left.sdf_open(p, mode, open).max(right.sdf_open(p, mode, open))
            }
            // K10.0: smooth variants. No AABB pruning in the
            // blend zone — both children must be evaluated to
            // feed smin_poly. The prune heuristic from Union
            // is only valid for the hard-min case.
            BoundedCsg::SmoothUnion {
                left,
                right,
                radius,
                mode: blend_mode,
                ..
            } => {
                let da = left.sdf_open(p, mode, open);
                let db = right.sdf_open(p, mode, open);
                match mode {
                    EvalMode::Visual => crate::csg::blend_min(da, db, *radius, *blend_mode),
                    EvalMode::Logical => da.min(db),
                }
            }
            BoundedCsg::SmoothDifference {
                left,
                right,
                radius,
                mode: blend_mode,
                ..
            } => {
                let da = left.sdf_open(p, mode, open);
                let db = right.sdf_open(p, mode, open);
                match mode {
                    EvalMode::Visual => -crate::csg::blend_min(-da, db, *radius, *blend_mode),
                    EvalMode::Logical => da.max(-db),
                }
            }
            BoundedCsg::Twist { inner, rate, .. } => {
                let angle = *rate * p[1];
                let (s, c) = angle.sin_cos();
                let twisted = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
                inner.sdf_open(twisted, mode, open)
            }
            BoundedCsg::Bend { inner, rate, .. } => {
                let angle = *rate * p[0];
                let (s, c) = angle.sin_cos();
                let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
                inner.sdf_open(bent, mode, open)
            }
            BoundedCsg::Displace { inner, depth, freq, .. } => {
                let d = inner.sdf_open(p, mode, open);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                d - disp
            }
            BoundedCsg::Shell { inner, thickness, open_faces, .. } => {
                let d = inner.sdf_open(p, mode, open);
                let d_ret = if open_faces.is_empty() {
                    d
                } else {
                    let merged = OpenBranchSet::push(open, open_faces);
                    inner.sdf_open(p, mode, &merged)
                };
                crate::csg::shell_wall_sdf(d, d_ret, *thickness)
            }
            BoundedCsg::Offset { inner, delta, .. } => inner.sdf_open(p, mode, open) - delta,
            BoundedCsg::MaskedBlend {
                left,
                right,
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                ..
            } => {
                let da = left.sdf_open(p, mode, open);
                let db_raw = right.sdf_open(p, mode, open);
                match mode {
                    EvalMode::Logical => {
                        if *is_difference { da.max(-db_raw) } else { da.min(db_raw) }
                    }
                    EvalMode::Visual => {
                        let mask_d = crate::csg::sdf_polyline_tube(p, mask_polyline, *mask_radius);
                        if mask_d < 0.0 {
                            if *is_difference {
                                -crate::csg::blend_min(-da, db_raw, *blend_radius, *blend_mode)
                            } else {
                                crate::csg::blend_min(da, db_raw, *blend_radius, *blend_mode)
                            }
                        } else {
                            if *is_difference { da.max(-db_raw) } else { da.min(db_raw) }
                        }
                    }
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
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let d2 = sketch
                        .sdf_retained([local[0], local[1]], &mut |g| open.contains_sub(id, g));
                    crate::csg::extrude_retained_combine(id, d2, local[2], *half_height, open)
                } else {
                    sdf_extrude(sketch.sdf([local[0], local[1]]), local[2], *half_height)
                };
                d * world_transform.min_scale()
            }
            BoundedCsg::Revolve {
                id,
                sketch,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let r = (local[0] * local[0] + local[2] * local[2]).sqrt();
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_retained([r, local[1]], &mut |g| open.contains_sub(id, g))
                } else {
                    sketch.sdf([r, local[1]])
                };
                d * world_transform.min_scale()
            }
            BoundedCsg::Helix {
                id,
                sketch,
                pitch,
                turns,
                left_handed,
                taper,
                start_angle,
                y_anchor,
                r_inner,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let (q, ell, scale, _) = crate::helix_profile_point(
                    local, *pitch, *turns, *left_handed, *taper, *y_anchor, *r_inner,
                    *start_angle,
                );
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_retained(q, &mut |g| open.contains_sub(id, g))
                } else {
                    sketch.sdf(q)
                };
                crate::helix_compose(d, ell) * scale * world_transform.min_scale()
            }
            BoundedCsg::Sweep { id, sketch, path, world_transform, .. } => {
                // The nearest-spine-point map + cap composition lives on the
                // shared SweepPath; the gate guarantees the foot is unique.
                // Shell-retained eval mirrors the Helix arm: project into the
                // profile plane, gate the opened profile segments there, and
                // re-compose with the axial cap coordinate.
                let local = world_to_local(world_transform, p);
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let cs = path.cross_section(local);
                    let d2 = sketch
                        .sdf_retained([cs.u, cs.v], &mut |g| open.contains_sub(id, g));
                    crate::sweep_path::sweep_compose(d2, cs.w)
                } else {
                    path.sdf(sketch, local)
                };
                d * world_transform.min_scale()
            }
            BoundedCsg::Morph { left, right, t, .. } => {
                let da = left.sdf_open(p, mode, open);
                let db = right.sdf_open(p, mode, open);
                (1.0 - t) * da + t * db
            }
        }
    }

    /// Provenance-aware evaluation. Mirrors [`CsgNode::evaluate`]
    /// with the same tie-merging rules, but traverses the BVH form
    /// so it shares leaf transforms and AABBs with the `sdf` hot
    /// path. Union/Difference prune eps-aware: a branch whose AABB
    /// distance lower bound clears the other side's value by more
    /// than `eps` can neither win the combine nor enter the
    /// contributor band, so skipping it is value- AND
    /// contributor-exact. (The B-Rep octree hammers this call; see
    /// also the cell-local specialization in [`crate::cell_csg`].)
    pub fn evaluate(
        &self,
        p: [f32; 3],
        eps: f32,
        mode: EvalMode,
    ) -> (f32, Contributors) {
        self.evaluate_open(p, eps, mode, &OpenBranchSet::EMPTY)
    }

    /// [`Self::evaluate`] under a set of opened shell branches — opened
    /// sub-faces are removed from both the distance and the contributor band.
    pub(crate) fn evaluate_open(
        &self,
        p: [f32; 3],
        eps: f32,
        mode: EvalMode,
        open: &OpenBranchSet<'_>,
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
                let (d, c) =
                    crate::csg::expand_evaluate_leaf_retained(id, primitive, local, eps, open);
                (d * world_transform.min_scale(), c)
            }
            BoundedCsg::Union { left, right, .. } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                // Left stays evaluated first (contributor tie ORDER must
                // not change), only the right side may be skipped: outside
                // its AABB `d_right ≥ dist_right`, so when `da + eps <
                // dist_right` it can neither win the min nor tie into the
                // eps band. Off under open shells (retained fields extend
                // past their AABB, as in `sdf_open`).
                if open.is_empty() {
                    let dist_right = distance_point_to_aabb(p, right.aabb());
                    if dist_right > 0.0 && da + eps < dist_right {
                        return (da, ca);
                    }
                }
                let (db, cb) = right.evaluate_open(p, eps, mode, open);
                combine_min(da, ca, db, cb, eps)
            }
            BoundedCsg::Difference { left, right, .. } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                // result = max(da, −db). Outside the tool's AABB
                // `−db ≤ −dist_right`; when `da + dist_right > eps` the
                // tool can neither carve (`−db < da`) nor reach the eps
                // band (`−db < da − eps`).
                if open.is_empty() {
                    let dist_right = distance_point_to_aabb(p, right.aabb());
                    if dist_right > 0.0 && da + dist_right > eps {
                        return (da, ca);
                    }
                }
                let (db, cb) = right.evaluate_open(p, eps, mode, open);
                combine_max(da, ca, -db, cb, eps)
            }
            BoundedCsg::Intersection { left, right, .. } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                let (db, cb) = right.evaluate_open(p, eps, mode, open);
                combine_max(da, ca, db, cb, eps)
            }
            BoundedCsg::SmoothUnion {
                left,
                right,
                radius,
                mode: blend_mode,
                ..
            } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                let (db, cb) = right.evaluate_open(p, eps, mode, open);
                match mode {
                    EvalMode::Visual => {
                        let d = crate::csg::blend_min(da, db, *radius, *blend_mode);
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
                mode: blend_mode,
                ..
            } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                let (db, cb) = right.evaluate_open(p, eps, mode, open);
                match mode {
                    EvalMode::Visual => {
                        let d = -crate::csg::blend_min(-da, db, *radius, *blend_mode);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => combine_max(da, ca, -db, cb, eps),
                }
            }
            BoundedCsg::Twist { inner, rate, .. } => {
                let angle = *rate * p[1];
                let (s, c) = angle.sin_cos();
                let twisted = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
                inner.evaluate_open(twisted, eps, mode, open)
            }
            BoundedCsg::Bend { inner, rate, .. } => {
                let angle = *rate * p[0];
                let (s, c) = angle.sin_cos();
                let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
                inner.evaluate_open(bent, eps, mode, open)
            }
            BoundedCsg::Displace { inner, depth, freq, .. } => {
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                (d - disp, c)
            }
            BoundedCsg::Shell { inner, thickness, open_faces, .. } => {
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                // Shell is `max(d, −(d_ret + t))`: a hard combine_max of the
                // outer surface (term `d`, outer contributors) against the
                // inner wall (the RETAINED faces offset inward → inner-wall
                // contributors, DL-75). Opened faces never gain an inner wall;
                // the band at the opening rim sees the outer face + the inner
                // wall where the two surfaces meet.
                let (d_ret, c_ret) = if open_faces.is_empty() {
                    (d, c)
                } else {
                    let merged = OpenBranchSet::push(open, open_faces);
                    inner.evaluate_open(p, eps, mode, &merged)
                };
                combine_max(d, c, -(d_ret + *thickness), c_ret.mapped_inner(), eps)
            }
            BoundedCsg::Offset { inner, delta, .. } => {
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                (d - delta, c)
            }
            BoundedCsg::MaskedBlend {
                left,
                right,
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                ..
            } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                let (db_raw, cb) = right.evaluate_open(p, eps, mode, open);
                let db = if *is_difference { -db_raw } else { db_raw };
                match mode {
                    EvalMode::Logical => {
                        if *is_difference {
                            combine_max(da, ca, db, cb, eps)
                        } else {
                            combine_min(da, ca, db, cb, eps)
                        }
                    }
                    EvalMode::Visual => {
                        let mask_d = crate::csg::sdf_polyline_tube(p, mask_polyline, *mask_radius);
                        if mask_d < 0.0 {
                            let d = if *is_difference {
                                -crate::csg::blend_min(-da, db_raw, *blend_radius, *blend_mode)
                            } else {
                                crate::csg::blend_min(da, db_raw, *blend_radius, *blend_mode)
                            };
                            let mut c = ca;
                            c.extend(cb);
                            (d, c)
                        } else {
                            if *is_difference {
                                combine_max(da, ca, db, cb, eps)
                            } else {
                                combine_min(da, ca, db, cb, eps)
                            }
                        }
                    }
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
                // Per-segment identity (DL-46) + cap split (DL-50) +
                // eps-banded multi-contributors (DL-65): the sketch band
                // reports every boundary group within `eps`, plus the cap
                // term when it ties, so a genuine extrude edge names both
                // colliding branches — uniform with the analytic half-space
                // band, so the B-Rep both-branches gate needs no sweep
                // special-case.
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let band = sketch.sdf_with_edges_retained(
                        [local[0], local[1]],
                        eps,
                        &mut |g| open.contains_sub(id, g),
                    );
                    let d = crate::csg::extrude_retained_combine(
                        id,
                        band.distance,
                        local[2],
                        *half_height,
                        open,
                    ) * world_transform.min_scale();
                    let c = crate::csg::extrude_contributors_retained(
                        id,
                        &band,
                        local[2],
                        *half_height,
                        eps,
                        open,
                    );
                    (d, c)
                } else {
                    let band = sketch.sdf_with_edges([local[0], local[1]], eps);
                    let d = sdf_extrude(band.distance, local[2], *half_height)
                        * world_transform.min_scale();
                    let c = extrude_contributors(id, &band, local[2], *half_height, eps);
                    (d, c)
                }
            }
            BoundedCsg::Revolve {
                id,
                sketch,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let r = (local[0] * local[0] + local[2] * local[2]).sqrt();
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained([r, local[1]], eps, &mut |g| {
                        open.contains_sub(id, g)
                    })
                } else {
                    sketch.sdf_with_edges([r, local[1]], eps)
                };
                let d = band.distance * world_transform.min_scale();
                let c = revolve_contributors(id, &band);
                (d, c)
            }
            BoundedCsg::Helix {
                id,
                sketch,
                pitch,
                turns,
                left_handed,
                taper,
                start_angle,
                y_anchor,
                r_inner,
                world_transform,
                ..
            } => {
                let local = world_to_local(world_transform, p);
                let (q, ell, scale, cap_side) = crate::helix_profile_point(
                    local, *pitch, *turns, *left_handed, *taper, *y_anchor, *r_inner,
                    *start_angle,
                );
                // Widen the edge band by the inverse Lipschitz scale so
                // profile-space matching stays comparable to world eps.
                let beps = eps / scale.max(1e-6);
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained(q, beps, &mut |g| open.contains_sub(id, g))
                } else {
                    sketch.sdf_with_edges(q, beps)
                };
                let d = crate::helix_compose(band.distance, ell)
                    * scale
                    * world_transform.min_scale();
                // Tag the end caps (ell ties) so they are pickable faces (exus).
                let c = crate::csg::helix_contributors(id, &band, ell, cap_side, beps);
                (d, c)
            }
            BoundedCsg::Sweep { id, sketch, path, world_transform, .. } => {
                let local = world_to_local(world_transform, p);
                let cs = path.cross_section(local);
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained([cs.u, cs.v], eps, &mut |g| {
                        open.contains_sub(id, g)
                    })
                } else {
                    sketch.sdf_with_edges([cs.u, cs.v], eps)
                };
                let d = crate::sweep_path::sweep_compose(band.distance, cs.w)
                    * world_transform.min_scale();
                let c = revolve_contributors(id, &band);
                (d, c)
            }
            BoundedCsg::Morph { left, right, t, .. } => {
                let (da, ca) = left.evaluate_open(p, eps, mode, open);
                let (db, cb) = right.evaluate_open(p, eps, mode, open);
                let d = (1.0 - t) * da + t * db;
                let mut c = ca;
                c.extend(cb);
                (d, c)
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
            // A∩B is a subset of BOTH children, so the intersection of the two
            // child AABBs is a valid — and much tighter — bound. Tightening
            // matters a lot for meshing: a small `∩ box` clip of a huge body
            // (ticket 8l06) must report the box's few-mm extent, not the
            // body's, or the export grid is sized for the whole body and the
            // clipped region gets a coarse cell (the resolution "clamp"). An
            // unbounded/space-filling child (gyroid, half-space) reports an
            // EMPTY sentinel AABB — intersecting with that would wrongly
            // collapse the bound to nothing, so fall back to the finite side.
            let (la, ra) = (left.aabb(), right.aabb());
            let aabb = if la.is_empty() {
                ra
            } else if ra.is_empty() {
                la
            } else {
                Aabb::intersect(la, ra)
            };
            BoundedCsg::Intersection {
                left: Box::new(left),
                right: Box::new(right),
                aabb,
            }
        }
        CsgNode::SmoothUnion(a, b, r, blend_mode) => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = Aabb::union(left.aabb(), right.aabb());
            BoundedCsg::SmoothUnion {
                left: Box::new(left),
                right: Box::new(right),
                radius: *r,
                mode: *blend_mode,
                aabb,
            }
        }
        CsgNode::SmoothDifference(a, b, r, blend_mode) => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = left.aabb();
            BoundedCsg::SmoothDifference {
                left: Box::new(left),
                right: Box::new(right),
                radius: *r,
                mode: *blend_mode,
                aabb,
            }
        }
        CsgNode::Twist(inner, rate) => {
            let child = build(inner, carry);
            let bb = child.aabb();
            let aabb = if bb.is_empty() {
                Aabb::EMPTY
            } else {
                let rx = bb.max[0].abs().max(bb.min[0].abs());
                let rz = bb.max[2].abs().max(bb.min[2].abs());
                let r_max = (rx * rx + rz * rz).sqrt();
                Aabb {
                    min: [-r_max, bb.min[1], -r_max],
                    max: [r_max, bb.max[1], r_max],
                }
            };
            BoundedCsg::Twist {
                inner: Box::new(child),
                rate: *rate,
                aabb,
            }
        }
        CsgNode::Bend(inner, rate) => {
            let child = build(inner, carry);
            let bb = child.aabb();
            let aabb = if bb.is_empty() {
                Aabb::EMPTY
            } else {
                let rx = bb.max[0].abs().max(bb.min[0].abs());
                let ry = bb.max[1].abs().max(bb.min[1].abs());
                let r_max = (rx * rx + ry * ry).sqrt();
                Aabb {
                    min: [-r_max, -r_max, bb.min[2]],
                    max: [r_max, r_max, bb.max[2]],
                }
            };
            BoundedCsg::Bend {
                inner: Box::new(child),
                rate: *rate,
                aabb,
            }
        }
        CsgNode::Displace(inner, depth, freq) => {
            let child = build(inner, carry);
            let bb = child.aabb();
            let aabb = if bb.is_empty() {
                Aabb::EMPTY
            } else {
                let d = depth.abs();
                Aabb {
                    min: [bb.min[0] - d, bb.min[1] - d, bb.min[2] - d],
                    max: [bb.max[0] + d, bb.max[1] + d, bb.max[2] + d],
                }
            };
            BoundedCsg::Displace {
                inner: Box::new(child),
                depth: *depth,
                freq: *freq,
                aabb,
            }
        }
        CsgNode::Shell(inner, thickness, open_faces) => {
            let child = build(inner, carry);
            // Inward shell ⊆ the solid: the outer surface is preserved, so the
            // bounds are the child's — no expansion.
            let aabb = child.aabb();
            // Carry the accumulated transform into each open-face placement, just
            // like a Transform node folds into leaves (DL-75). The placements are
            // world-space (`body_placement ∘ face_transform`); for the world build
            // `carry` is identity (no-op), but the body-LOCAL weave seeds `carry =
            // inverse(placement)`, which cancels the body placement so the gates
            // sit in the same body-local frame as the leaves. This is what lets
            // the weave see the hollow with correctly-placed openings.
            let open_faces = open_faces
                .iter()
                .map(|f| OpenSurface {
                    branch: f.branch,
                    surface: f.surface.clone(),
                    placement: compose_transforms(carry, &f.placement),
                })
                .collect();
            BoundedCsg::Shell {
                inner: Box::new(child),
                thickness: *thickness,
                open_faces,
                aabb,
            }
        }
        CsgNode::Offset(inner, delta) => {
            let child = build(inner, carry);
            let bb = child.aabb();
            let aabb = if bb.is_empty() {
                Aabb::EMPTY
            } else {
                // Conservative for either sign: dilation grows by `δ`, erosion
                // shrinks — expanding by `|δ|` stays a superset.
                let e = delta.abs();
                Aabb {
                    min: [bb.min[0] - e, bb.min[1] - e, bb.min[2] - e],
                    max: [bb.max[0] + e, bb.max[1] + e, bb.max[2] + e],
                }
            };
            BoundedCsg::Offset {
                inner: Box::new(child),
                delta: *delta,
                aabb,
            }
        }
        CsgNode::MaskedBlend {
            a,
            b,
            mask_polyline,
            mask_radius,
            blend_radius,
            blend_mode,
            is_difference,
        } => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = Aabb::union(left.aabb(), right.aabb());
            BoundedCsg::MaskedBlend {
                left: Box::new(left),
                right: Box::new(right),
                mask_polyline: mask_polyline.clone(),
                mask_radius: *mask_radius,
                blend_radius: *blend_radius,
                blend_mode: *blend_mode,
                is_difference: *is_difference,
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
        CsgNode::Helix {
            id, sketch, pitch, turns, left_handed, taper, start_angle,
            y_anchor, r_inner,
        } => {
            let bounded_sketch = BoundedSketch2d::from_sketch(sketch);
            let s2 = bounded_sketch.aabb();
            let local_aabb = if s2.is_empty() {
                Aabb::EMPTY
            } else {
                let axial = pitch.max(0.0) * (turns.max(0.0).ceil() + 1.0);
                let span_y = (s2.max[1] - s2.min[1]) + axial;
                let r_max = s2.max[0].abs().max(s2.min[0].abs()) + taper.abs() * span_y;
                Aabb {
                    min: [-r_max, s2.min[1] - pitch.max(0.0), -r_max],
                    max: [r_max, s2.max[1] + axial, r_max],
                }
            };
            let aabb = local_aabb.transformed(carry);
            BoundedCsg::Helix {
                id: id.clone(),
                sketch: Box::new(bounded_sketch),
                pitch: *pitch,
                turns: *turns,
                left_handed: *left_handed,
                taper: *taper,
                start_angle: *start_angle,
                y_anchor: *y_anchor,
                r_inner: *r_inner,
                world_transform: *carry,
                aabb,
            }
        }
        CsgNode::Sweep { id, sketch, path, closed, twist } => {
            let bounded_sketch = BoundedSketch2d::from_sketch(sketch);
            let s2 = bounded_sketch.aabb();
            let sweep_path = crate::sweep_path::SweepPath::new(path.clone(), *closed, *twist);
            match sweep_path {
                Some(sp) if !s2.is_empty() => {
                    // Conservative hull: each spine sample ± the profile's
                    // bounding radius (covers the cross-section in any frame).
                    let r_max = (s2.max[0].abs().max(s2.min[0].abs()))
                        .hypot(s2.max[1].abs().max(s2.min[1].abs()));
                    let mut local_aabb = Aabb::EMPTY;
                    for p in &sp.points {
                        local_aabb = Aabb::union(
                            local_aabb,
                            Aabb {
                                min: [p[0] - r_max, p[1] - r_max, p[2] - r_max],
                                max: [p[0] + r_max, p[1] + r_max, p[2] + r_max],
                            },
                        );
                    }
                    BoundedCsg::Sweep {
                        id: id.clone(),
                        sketch: Box::new(bounded_sketch),
                        path: std::sync::Arc::new(sp),
                        world_transform: *carry,
                        aabb: local_aabb.transformed(carry),
                    }
                }
                // Degenerate path (<2 points) or empty profile → nothing.
                _ => BoundedCsg::Empty,
            }
        }
        CsgNode::Morph(a, b, t) => {
            let left = build(a, carry);
            let right = build(b, carry);
            let aabb = Aabb::union(left.aabb(), right.aabb());
            BoundedCsg::Morph {
                left: Box::new(left),
                right: Box::new(right),
                t: *t,
                aabb,
            }
        }
    }
}

/// Distance from a point to the surface of an AABB. Zero if the point
/// sits inside the box (the Lipschitz bound is trivial there); the
/// usual `sqrt(Σ max(0, dist_i)²)` when outside.
pub(crate) fn distance_point_to_aabb(p: [f32; 3], aabb: Aabb) -> f32 {
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
// that duplicating the code would be silly. `pub(crate)` so the
// cell-specialized view (`cell_csg.rs`) mirrors the exact same
// tie-merging semantics.
pub(crate) fn combine_min(
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

pub(crate) fn combine_max(
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

fn classify_walk(node: &BoundedCsg, info: &mut HybridInfo, role: &str, warp: Option<LeafWarp>) {
    match node {
        BoundedCsg::Empty => {}
        BoundedCsg::Leaf { primitive, world_transform, aabb, .. } => {
            if let Some(mut leaf) = primitive_to_leaf_info(primitive, world_transform, aabb) {
                leaf.role = role.into();
                leaf.warp = warp;
                info.leaves.push(leaf);
            } else {
                info.needs_full_sdf = true;
            }
        }
        BoundedCsg::Union { left, right, .. } => {
            classify_walk(left, info, role, warp);
            classify_walk(right, info, role, warp);
        }
        BoundedCsg::Difference { left, right, .. } => {
            info.is_union_only = false;
            let overlap = Aabb::intersect(left.aabb(), right.aabb());
            if !overlap.is_empty() {
                info.sdf_regions.push(SdfRegionInfo {
                    aabb: aabb_to_flat(overlap),
                });
            }
            classify_walk(left, info, role, warp);
            classify_walk(right, info, "skip", warp);
        }
        BoundedCsg::Intersection { left, right, .. } => {
            info.is_union_only = false;
            let overlap = Aabb::intersect(left.aabb(), right.aabb());
            if !overlap.is_empty() {
                info.sdf_regions.push(SdfRegionInfo {
                    aabb: aabb_to_flat(overlap),
                });
            }
            classify_walk(left, info, "skip", warp);
            classify_walk(right, info, "skip", warp);
        }
        BoundedCsg::SmoothUnion { left, right, radius, .. } => {
            info.is_union_only = false;
            let overlap = Aabb::intersect(left.aabb(), right.aabb()).expanded(*radius);
            if !overlap.is_empty() {
                info.sdf_regions.push(SdfRegionInfo {
                    aabb: aabb_to_flat(overlap),
                });
            }
            classify_walk(left, info, role, warp);
            classify_walk(right, info, role, warp);
        }
        BoundedCsg::SmoothDifference { left, right, radius, .. } => {
            info.is_union_only = false;
            let overlap = Aabb::intersect(left.aabb(), right.aabb()).expanded(*radius);
            if !overlap.is_empty() {
                info.sdf_regions.push(SdfRegionInfo {
                    aabb: aabb_to_flat(overlap),
                });
            }
            classify_walk(left, info, role, warp);
            classify_walk(right, info, "skip", warp);
        }
        BoundedCsg::MaskedBlend { left, right, mask_polyline, mask_radius, blend_radius, is_difference, .. } => {
            info.is_union_only = false;
            let tube_aabb = polyline_tube_aabb(mask_polyline, *mask_radius).expanded(*blend_radius);
            let overlap = Aabb::intersect(
                Aabb::intersect(left.aabb(), right.aabb()),
                tube_aabb,
            );
            if !overlap.is_empty() {
                info.sdf_regions.push(SdfRegionInfo {
                    aabb: aabb_to_flat(overlap),
                });
            }
            classify_walk(left, info, role, warp);
            classify_walk(right, info, if *is_difference { "skip" } else { role }, warp);
        }
        // Domain warps: the SDF march stays the source of truth
        // (needs_full_sdf), but we ALSO emit the child subtree's leaves
        // tagged with the warp so the web mesher can deform them into a
        // tight render hull (WebGL has no raymarcher). The warp is applied
        // in WORLD space (every BoundedCsg op runs in world coords).
        BoundedCsg::Twist { inner, rate, .. } => {
            info.is_union_only = false;
            info.needs_full_sdf = true;
            // A pure XZ rotation about world Y is a diffeomorphism — never
            // self-intersects, so no validity gate. Single warp only; a
            // nested warp falls back to the SDF (no leaf emitted).
            if warp.is_none() {
                classify_walk(inner, info, role, Some(LeafWarp::Twist { rate: *rate }));
            }
        }
        BoundedCsg::Bend { inner, rate, .. } => {
            info.is_union_only = false;
            info.needs_full_sdf = true;
            // Bend self-intersects once the inner side wraps past the axis.
            // Analytic gate: |rate| · y_extent < 1 (constant curvature). The
            // same bound makes the mesher's fixed-point inverse a
            // contraction. Outside the gate (or nested) → SDF fallback.
            let bb = inner.aabb();
            let y_extent = bb.max[1].abs().max(bb.min[1].abs());
            if warp.is_none() && rate.abs() * y_extent < 1.0 {
                classify_walk(inner, info, role, Some(LeafWarp::Bend { rate: *rate }));
            }
        }
        BoundedCsg::Displace { .. }
        | BoundedCsg::Shell { .. }
        | BoundedCsg::Offset { .. }
        | BoundedCsg::Morph { .. } => {
            info.is_union_only = false;
            info.needs_full_sdf = true;
        }
        // The curved sweep always evaluates via the CPU SDF (DC `mesh` mode)
        // and, once wired, the GPU VM (needs_full_sdf). ADDITIONALLY, when the
        // Gatekeeper passes (kink-free, curvature radius > profile radius, no
        // self-intersection) we emit a parametric `"sweep"` leaf so the web
        // mesher renders a tight RMF hull instead of the AABB box. On a Fail
        // the SDF path alone covers it (it merges the self-overlap correctly).
        BoundedCsg::Sweep { sketch, path, world_transform, aabb, .. } => {
            info.is_union_only = false;
            info.needs_full_sdf = true;
            let s2 = sketch.aabb();
            if s2.is_empty() {
                return;
            }
            let r_max = (s2.max[0].abs().max(s2.min[0].abs()))
                .hypot(s2.max[1].abs().max(s2.min[1].abs()));
            if !crate::sweep_gate::check_sweep(path, r_max, &Default::default()).is_pass() {
                return;
            }
            let contours = crate::sketch2d::extract_contours(
                sketch,
                crate::sketch2d::CONTOUR_RESOLUTION,
            );
            let tf = [
                world_transform.translate[0], world_transform.translate[1],
                world_transform.translate[2],
                world_transform.rotation[0], world_transform.rotation[1],
                world_transform.rotation[2], world_transform.rotation[3],
                world_transform.scale[0], world_transform.scale[1],
                world_transform.scale[2],
            ];
            let flat_aabb = aabb_to_flat(*aabb);
            for contour in contours {
                info.leaves.push(HybridLeafInfo {
                    kind: "sweep".into(),
                    aabb: flat_aabb,
                    transform: tf,
                    params: vec![],
                    role: role.into(),
                    contour: Some(contour),
                    warp: None,
                    sweep: Some(SweepLeafData {
                        points: path.points.clone(),
                        closed: path.closed,
                        twist: path.twist,
                    }),
                });
            }
        }
        BoundedCsg::Extrude { sketch, half_height, world_transform, aabb, .. } => {
            let contours = crate::sketch2d::extract_contours(
                sketch,
                crate::sketch2d::CONTOUR_RESOLUTION,
            );
            if contours.is_empty() {
                info.needs_full_sdf = true;
            } else {
                let tf = [
                    world_transform.translate[0], world_transform.translate[1],
                    world_transform.translate[2],
                    world_transform.rotation[0], world_transform.rotation[1],
                    world_transform.rotation[2], world_transform.rotation[3],
                    world_transform.scale[0], world_transform.scale[1],
                    world_transform.scale[2],
                ];
                let flat_aabb = aabb_to_flat(*aabb);
                for contour in contours {
                    info.leaves.push(HybridLeafInfo {
                        kind: "extrude".into(),
                        aabb: flat_aabb,
                        transform: tf,
                        params: vec![*half_height],
                        role: role.into(),
                        contour: Some(contour),
                        warp,
                        sweep: None,
                    });
                }
            }
        }
        BoundedCsg::Revolve { sketch, world_transform, aabb, .. } => {
            let contours = crate::sketch2d::extract_contours(
                sketch,
                crate::sketch2d::CONTOUR_RESOLUTION,
            );
            if contours.is_empty() {
                info.needs_full_sdf = true;
            } else {
                let tf = [
                    world_transform.translate[0], world_transform.translate[1],
                    world_transform.translate[2],
                    world_transform.rotation[0], world_transform.rotation[1],
                    world_transform.rotation[2], world_transform.rotation[3],
                    world_transform.scale[0], world_transform.scale[1],
                    world_transform.scale[2],
                ];
                let flat_aabb = aabb_to_flat(*aabb);
                for contour in contours {
                    info.leaves.push(HybridLeafInfo {
                        kind: "revolve".into(),
                        aabb: flat_aabb,
                        transform: tf,
                        params: vec![],
                        role: role.into(),
                        contour: Some(contour),
                        warp,
                        sweep: None,
                    });
                }
            }
        }
        // A helical sweep has no hybrid raytracer mapping, so the
        // conservative SDF march still covers it (non-Lipschitz family) —
        // but we ALSO emit the swept profile as a parametric leaf so the
        // WebGL path (no raymarcher) and the deferred hull get a tight
        // solid mesh instead of an AABB box. `buildHelixGeometry` (web)
        // sweeps this contour via the forward `helix_profile_point` map.
        BoundedCsg::Helix {
            sketch, pitch, turns, left_handed, taper, start_angle,
            y_anchor, world_transform, aabb, ..
        } => {
            info.is_union_only = false;
            info.needs_full_sdf = true;
            let contours = crate::sketch2d::extract_contours(
                sketch,
                crate::sketch2d::CONTOUR_RESOLUTION,
            );
            let tf = [
                world_transform.translate[0], world_transform.translate[1],
                world_transform.translate[2],
                world_transform.rotation[0], world_transform.rotation[1],
                world_transform.rotation[2], world_transform.rotation[3],
                world_transform.scale[0], world_transform.scale[1],
                world_transform.scale[2],
            ];
            let flat_aabb = aabb_to_flat(*aabb);
            for contour in contours {
                info.leaves.push(HybridLeafInfo {
                    kind: "helix".into(),
                    aabb: flat_aabb,
                    transform: tf,
                    params: vec![
                        *pitch, *turns,
                        if *left_handed { 1.0 } else { 0.0 },
                        *taper, *start_angle, *y_anchor,
                    ],
                    role: role.into(),
                    contour: Some(contour),
                    warp,
                    sweep: None,
                });
            }
        }
    }
}

fn primitive_to_leaf_info(
    prim: &Primitive,
    wt: &Transform,
    aabb: &Aabb,
) -> Option<HybridLeafInfo> {
    let tf = [
        wt.translate[0], wt.translate[1], wt.translate[2],
        wt.rotation[0], wt.rotation[1], wt.rotation[2], wt.rotation[3],
        wt.scale[0], wt.scale[1], wt.scale[2],
    ];
    let flat_aabb = aabb_to_flat(*aabb);

    let role = String::new(); // set by caller
    let c = None; // no contour for standard primitives
    match prim {
        Primitive::Box { center, half_extents } => Some(HybridLeafInfo {
            kind: "box".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![
                half_extents[0], half_extents[1], half_extents[2],
                center[0], center[1], center[2],
            ],
        }),
        Primitive::Sphere { center, radius } => Some(HybridLeafInfo {
            kind: "sphere".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![*radius, center[0], center[1], center[2]],
        }),
        Primitive::Cylinder { center, radius, half_height } => Some(HybridLeafInfo {
            kind: "cylinder".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![*radius, *half_height, center[0], center[1], center[2]],
        }),
        Primitive::Torus { center, major_radius, minor_radius } => Some(HybridLeafInfo {
            kind: "torus".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![*major_radius, *minor_radius, center[0], center[1], center[2]],
        }),
        Primitive::Cone { r_bottom, r_top, half_height } => Some(HybridLeafInfo {
            kind: "cone".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![*r_bottom, *r_top, *half_height],
        }),
        Primitive::Capsule { radius, half_height } => Some(HybridLeafInfo {
            kind: "capsule".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![*radius, *half_height],
        }),
        Primitive::Thread { nominal_radius, half_height, .. } => Some(HybridLeafInfo {
            kind: "cylinder".into(), aabb: flat_aabb, transform: tf, role, contour: c, warp: None, sweep: None,
            params: vec![*nominal_radius, *half_height, 0.0, 0.0, 0.0],
        }),
        _ => None,
    }
}

fn aabb_to_flat(aabb: Aabb) -> [f32; 6] {
    [aabb.min[0], aabb.min[1], aabb.min[2], aabb.max[0], aabb.max[1], aabb.max[2]]
}

fn capsule_aabb(p0: [f32; 3], p1: [f32; 3], radius: f32) -> Aabb {
    Aabb {
        min: [
            p0[0].min(p1[0]) - radius,
            p0[1].min(p1[1]) - radius,
            p0[2].min(p1[2]) - radius,
        ],
        max: [
            p0[0].max(p1[0]) + radius,
            p0[1].max(p1[1]) + radius,
            p0[2].max(p1[2]) + radius,
        ],
    }
}

/// AABB of a polyline-tube mask: the union of the per-point spheres of
/// `radius`. Empty for an empty polyline.
fn polyline_tube_aabb(polyline: &[[f32; 3]], radius: f32) -> Aabb {
    let mut acc = Aabb::EMPTY;
    for pt in polyline {
        acc = Aabb::union(acc, capsule_aabb(*pt, *pt, radius));
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::{BranchId, DecodedFeature, PrimitiveModifiers};
    use fielddraft_protocol::{BoolOp, FeatureId, Primitive, Transform};

    fn box_feature(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Box {id}"),
            op,
            Primitive::Box {
                center,
                half_extents: half,
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn translated_box_feature(id: u32, translate: [f32; 3], half: [f32; 3]) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("TBox {id}"),
            BoolOp::New,
            Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
            Transform {
                translate,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Sphere {id}"),
            BoolOp::New,
            Primitive::Sphere { center, radius },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    #[test]
    fn empty_tree_has_empty_aabb_and_infinite_sdf() {
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&[]));
        assert!(bvh.aabb().is_empty());
        assert!(bvh.sdf([0.0, 0.0, 0.0], EvalMode::Logical).is_infinite());
    }

    #[test]
    fn leaf_world_transform_is_folded_from_csg_transform_wrapper() {
        // CsgNode::from_decoded wraps each feature in Transform(t, Leaf).
        // After BoundedCsg::from_csg the wrapper is gone and the
        // leaf's stored world_transform equals the feature's own
        // transform.
        let features = vec![translated_box_feature(
            1,
            [10.0, 20.0, 30.0],
            [5.0, 5.0, 5.0],
        )];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feature(2, BoolOp::New, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bvh = BoundedCsg::from_csg(&CsgNode::from_decoded(&features));
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Remove;
                f
            },
            box_feature(
                3,
                BoolOp::New,
                [-30.0, 0.0, 0.0],
                [5.0, 5.0, 5.0],
            ),
        ];
        let csg = CsgNode::from_decoded(&features);
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            {
                let mut f = sphere_feature(2, [10.0, 10.0, 10.0], 8.0);
                f.op = BoolOp::Remove;
                f
            },
        ];
        let csg = CsgNode::from_decoded(&features);
        let bvh = BoundedCsg::from_csg(&csg);
        // A point on the box's +x face well away from the sphere →
        // only the box contributes.
        let p = [20.0, -10.0, -10.0];
        let (ba, ca) = bvh.evaluate(p, 0.5, EvalMode::Logical);
        let (cb, cc) = csg.evaluate(p, 0.5, EvalMode::Logical);
        assert!((ba - cb).abs() < 1e-4);
        assert_eq!(ca.as_slice(), cc.as_slice());
        // Box +X face is sub-index 0.
        assert_eq!(ca.as_slice(), &[BranchId::from_feature_sub(&FeatureId::from_u32(1), 0)]);
    }

    #[test]
    fn pruning_does_not_lose_precision_for_points_near_far_branch() {
        // A two-body scene: one box at the origin, one box 500 mm
        // away. Query a point that sits inside the origin box. The
        // far box's AABB is 500 mm away, which should trigger the
        // Union prune, but the returned value must still equal the
        // naive answer to within f32 roundoff.
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::New, [500.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        ];
        let csg = CsgNode::from_decoded(&features);
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
        // Top cap centre → surface, single Face contributor. The cap is
        // now a distinct branch (DL-50): the reserved top-cap sub-id, not
        // the nearest wall group.
        let (d, c) = bvh.evaluate([0.0, 0.0, 2.0], 0.25, EvalMode::Logical);
        assert!(d.abs() < 1e-4);
        assert_eq!(
            c.as_slice(),
            &[BranchId::from_feature_sub(
                &FeatureId::from_u32(7),
                BranchId::EXTRUDE_CAP_TOP_SUB
            )]
        );
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

    #[test]
    fn shell_evaluate_tags_inner_wall_with_inner_branch_provenance() {
        // DL-75 Step 1: on the outer surface the contributor is the plain outer
        // face; on the inner wall it is that same face's inner-wall id. This is
        // what lets the weave classify the cavity as its own pickable face.
        let half = 5.0_f32;
        let thickness = 1.0_f32;
        let csg = CsgNode::Shell(
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(1),
                primitive: Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [half, half, half],
                },
            }),
            thickness,
            Vec::new(),
        );
        let body = BoundedCsg::from_csg(&csg);
        let eps = 1e-3;

        // On the +X outer face (x = 5): the outer term `d` dominates.
        let (d_out, c_out) = body.evaluate([half, 0.0, 0.0], eps, EvalMode::Logical);
        assert!(d_out.abs() < 1e-3, "on the outer surface");
        assert!(!c_out.is_empty(), "outer face has provenance");
        assert!(
            c_out.as_slice().iter().all(|b| !b.is_inner_wall()),
            "no inner-wall ids on the outer surface"
        );

        // On the +X inner wall (x = 4 = half − thickness): `−(d + e)` dominates.
        let (d_in, c_in) = body.evaluate([half - thickness, 0.0, 0.0], eps, EvalMode::Logical);
        assert!(d_in.abs() < 1e-3, "on the inner wall");
        assert!(
            c_in.as_slice().iter().any(|b| b.is_inner_wall()),
            "inner-wall id present on the cavity surface"
        );
        assert!(
            c_in.as_slice().iter().all(|b| b.feature_id() == 1),
            "inner wall keeps its owning feature"
        );
    }

    /// Shell-retained eval reaches a Sweep's profile (parity with the
    /// Extrude/Revolve/Helix arms): an open set naming one of the sweep
    /// profile's sub-groups must change the field near the wall skin, and an
    /// open set naming a DIFFERENT feature must leave it byte-identical.
    #[test]
    fn sweep_shell_retained_eval_gates_the_profile() {
        use crate::csg::{CsgNode, EvalMode, OpenBranchSet, OpenSurface};
        use fielddraft_protocol::{HalfSpace, Sketch2d, Sketch2dPrimitive, Transform};

        let sweep_id = FeatureId::from_u32(7);
        let node = CsgNode::Sweep {
            id: sweep_id.clone(),
            // Retained groups only exist for Path profiles (per-segment
            // branch sub-ids) — exactly what a committed sketch lowers to.
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Path {
                segments: vec![
                    fielddraft_protocol::SketchSegment::Line { start: [-2.0, -2.0], end: [2.0, -2.0] },
                    fielddraft_protocol::SketchSegment::Line { start: [2.0, -2.0], end: [2.0, 2.0] },
                    fielddraft_protocol::SketchSegment::Line { start: [2.0, 2.0], end: [-2.0, 2.0] },
                    fielddraft_protocol::SketchSegment::Line { start: [-2.0, 2.0], end: [-2.0, -2.0] },
                ],
                closed: true,
            })),
            path: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 5.0]],
            closed: false,
            twist: 0.0,
        };
        let body = BoundedCsg::from_csg(&node);
        // A probe point just outside the wall skin at the path's middle.
        let probe = [10.0, 2.5, 0.0];
        let baseline = body.sdf_open(probe, EvalMode::Logical, &OpenBranchSet::EMPTY);
        assert!(baseline.is_finite() && baseline > 0.0, "probe starts outside the skin");

        let open_surface = |feature: &FeatureId, sub: u32| OpenSurface {
            branch: crate::csg::BranchId::from_feature_sub(feature, sub).as_u32(),
            surface: HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: 0.0 },
            placement: Transform::IDENTITY,
        };

        // A different feature's open set must not perturb the sweep at all.
        let foreign = [open_surface(&FeatureId::from_u32(99), 0)];
        let set = OpenBranchSet::push(&OpenBranchSet::EMPTY, &foreign);
        let with_foreign = body.sdf_open(probe, EvalMode::Logical, &set);
        assert_eq!(baseline, with_foreign, "foreign feature must not gate the sweep");

        // Opening one of the sweep's own profile groups must change the wall
        // field (the retained profile drops that segment's boundary). The
        // exact group numbering is a sketch-SDF detail — any of the first few
        // firing proves the gate is wired end-to-end.
        let changed = (0..8).any(|g| {
            let own = [open_surface(&sweep_id, g)];
            let set = OpenBranchSet::push(&OpenBranchSet::EMPTY, &own);
            body.sdf_open(probe, EvalMode::Logical, &set) != baseline
        });
        assert!(changed, "opening a sweep profile group must reach the retained profile");
    }
}
