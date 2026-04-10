//! CSG abstract syntax tree.
//!
//! K1 introduces the CSG graph as a sibling representation of the
//! existing linear feature spine. `CsgNode::from_features` folds a
//! `Feature` list into a binary tree whose `sdf` evaluator is
//! mathematically identical to the linear `sdf_features` fold — the
//! tree is just a richer form that later kernel slices (provenance,
//! BVH, multi-body isolation) will extend. For K1 the tree only exists
//! so that the Dual Contouring mesher can traverse it polymorphically
//! instead of special-casing the linear list.

use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

use crate::sketch2d::Sketch2d;
use crate::{quat_rotate, sdf_primitive, world_to_local};

/// Stable identifier of a CSG leaf. Inherited from `FeatureId` so
/// provenance survives outliner reorders, undo/redo and serialisation
/// without a separate naming scheme (the README's TNP-solution).
pub type NodeId = FeatureId;

/// Maximum number of leaves that can simultaneously be "active" at a
/// single SDF evaluation. Three is the theoretical lower bound for a
/// `Vertex` of the B-Rep proxy (three faces meeting at a point); four
/// leaves us headroom for near-degenerate configurations without ever
/// forcing a heap allocation on the hot evaluate path.
pub const MAX_CONTRIBUTORS: usize = 4;

/// Small inline set of leaf ids that dominate a particular SDF
/// evaluation within the provenance epsilon. Ordering inside the set
/// is insertion order; duplicates are silently ignored.
///
/// After K4.5.1 this is `Clone`-only rather than `Copy`, because
/// [`FeatureId`] carries a heap pointer for hierarchical ids. The
/// per-vertex allocation cost is still minimal for flat-numeric ids
/// (SmolStr inlines strings ≤22 chars), but every caller that used
/// to copy a Contributors now has to `.clone()` it explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contributors {
    ids: [NodeId; MAX_CONTRIBUTORS],
    len: u8,
}

impl Contributors {
    pub fn empty() -> Self {
        Self {
            ids: Default::default(),
            len: 0,
        }
    }

    pub fn single(id: NodeId) -> Self {
        let mut out = Self::empty();
        out.ids[0] = id;
        out.len = 1;
        out
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[NodeId] {
        &self.ids[..self.len as usize]
    }

    pub fn contains(&self, id: &NodeId) -> bool {
        self.as_slice().iter().any(|x| x == id)
    }

    /// Insert `id` if not already present. Silently drops additions
    /// past `MAX_CONTRIBUTORS` — extremely high multiplicities are a
    /// sign of numerical trouble (too large an epsilon), not real
    /// geometry, and we prefer a stable upper bound to a panic.
    pub fn push(&mut self, id: NodeId) {
        if self.contains(&id) {
            return;
        }
        if (self.len as usize) < MAX_CONTRIBUTORS {
            self.ids[self.len as usize] = id;
            self.len += 1;
        }
    }

    pub fn extend(&mut self, other: Self) {
        for id in other.into_iter_owned() {
            self.push(id);
        }
    }

    /// Consume the set and yield its ids. Used by [`Contributors::extend`]
    /// to move the inner ids into the target set without cloning.
    fn into_iter_owned(self) -> impl Iterator<Item = NodeId> {
        let len = self.len as usize;
        self.ids.into_iter().take(len)
    }

    /// Classification of a mesh vertex or picking hit based on
    /// contributor multiplicity — the heart of the B-Rep proxy.
    pub fn element(&self) -> ProxyElement {
        match self.len() {
            0 => ProxyElement::None,
            1 => ProxyElement::Face,
            2 => ProxyElement::Edge,
            _ => ProxyElement::Vertex,
        }
    }
}

impl Default for Contributors {
    fn default() -> Self {
        Self::empty()
    }
}

/// Classification of a point on the CSG boundary. Mirrors the README's
/// B-Rep proxy taxonomy: `Face` = single dominant contributor, `Edge`
/// = two simultaneously dominant contributors (the intersection curve
/// of two leaf surfaces), `Vertex` = three or more (the crossing of
/// three or more surfaces). `None` is returned only for points far
/// from any surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyElement {
    None,
    Face,
    Edge,
    Vertex,
}

/// Dual-path evaluation mode for the CSG graph.
///
/// K2/K3 collapse both paths to the same bit-identical evaluator —
/// there are no smooth operators in the language yet. The distinction
/// is introduced now, ahead of K10, so every call site can already be
/// tagged and the divergence can land as a pure code change instead of
/// a flood-the-world API sweep. The rules, once K10 ships (see
/// README §4.1 "Ghost Topology"):
///
/// * `Visual` — what the mesher renders: Spatial Masks, smooth
///   blends, displacement and morph terminal-features are all active.
///   This is the path that feeds Dual Contouring.
/// * `Logical` — the underlying hard CSG, unmasked. Contributor sets
///   stay well-defined across every surface because no operator has
///   smeared the boundary. This is the path that feeds picking, the
///   raycaster and the B-Rep proxy construction.
///
/// Rule of thumb: "what does the user see?" → `Visual`. "what leaf
/// actually owns this surface?" → `Logical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalMode {
    Visual,
    Logical,
}

impl ProxyElement {
    /// Compact numeric tag for the zero-copy mesh bridge: 0 = off-
    /// surface, 1 = face, 2 = edge, 3 = vertex. Matches
    /// `Contributors::element()` by construction.
    pub fn tag(self) -> u8 {
        match self {
            ProxyElement::None => 0,
            ProxyElement::Face => 1,
            ProxyElement::Edge => 2,
            ProxyElement::Vertex => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProxyElement::None => "none",
            ProxyElement::Face => "face",
            ProxyElement::Edge => "edge",
            ProxyElement::Vertex => "vertex",
        }
    }
}

/// Axis-aligned bounding box in world space. The `EMPTY` sentinel
/// carries inverted extents so that `union(empty, x) == x`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1] || self.min[2] > self.max[2]
    }

    pub fn union(a: Self, b: Self) -> Self {
        if a.is_empty() {
            return b;
        }
        if b.is_empty() {
            return a;
        }
        Self {
            min: [
                a.min[0].min(b.min[0]),
                a.min[1].min(b.min[1]),
                a.min[2].min(b.min[2]),
            ],
            max: [
                a.max[0].max(b.max[0]),
                a.max[1].max(b.max[1]),
                a.max[2].max(b.max[2]),
            ],
        }
    }

    pub fn expanded(&self, margin: f32) -> Self {
        if self.is_empty() {
            return *self;
        }
        Self {
            min: [self.min[0] - margin, self.min[1] - margin, self.min[2] - margin],
            max: [self.max[0] + margin, self.max[1] + margin, self.max[2] + margin],
        }
    }

    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Local-frame bounding box of a primitive.
    pub fn of_primitive(p: &Primitive) -> Self {
        match p {
            Primitive::Box { center, half_extents } => Self {
                min: [
                    center[0] - half_extents[0],
                    center[1] - half_extents[1],
                    center[2] - half_extents[2],
                ],
                max: [
                    center[0] + half_extents[0],
                    center[1] + half_extents[1],
                    center[2] + half_extents[2],
                ],
            },
            Primitive::Sphere { center, radius } => Self {
                min: [center[0] - radius, center[1] - radius, center[2] - radius],
                max: [center[0] + radius, center[1] + radius, center[2] + radius],
            },
            Primitive::Cylinder { center, radius, half_height } => Self {
                min: [
                    center[0] - radius,
                    center[1] - half_height,
                    center[2] - radius,
                ],
                max: [
                    center[0] + radius,
                    center[1] + half_height,
                    center[2] + radius,
                ],
            },
        }
    }

    /// Conservative AABB after applying an affine transform: all eight
    /// corners are mapped into world space and re-bounded. Looser than
    /// the tightest post-rotation box, but trivially correct.
    pub fn transformed(&self, t: &Transform) -> Self {
        if self.is_empty() {
            return *self;
        }
        let corners = [
            [self.min[0], self.min[1], self.min[2]],
            [self.max[0], self.min[1], self.min[2]],
            [self.min[0], self.max[1], self.min[2]],
            [self.max[0], self.max[1], self.min[2]],
            [self.min[0], self.min[1], self.max[2]],
            [self.max[0], self.min[1], self.max[2]],
            [self.min[0], self.max[1], self.max[2]],
            [self.max[0], self.max[1], self.max[2]],
        ];
        let mut out = Self::EMPTY;
        for c in corners {
            let scaled = [c[0] * t.scale, c[1] * t.scale, c[2] * t.scale];
            let rotated = quat_rotate(t.rotation, scaled);
            let world = [
                rotated[0] + t.translate[0],
                rotated[1] + t.translate[1],
                rotated[2] + t.translate[2],
            ];
            out = Self::union(
                out,
                Self {
                    min: world,
                    max: world,
                },
            );
        }
        out
    }
}

/// One node of the CSG graph. `Empty` is the identity element of union
/// (SDF ≡ +∞, i.e. nothing exists), used as the seed of
/// `from_features`. Each `Leaf` carries a stable [`NodeId`] that
/// propagates through the provenance-aware `evaluate` fold, so every
/// surface hit can be traced back to the primitives that produced it.
#[derive(Debug, Clone, PartialEq)]
pub enum CsgNode {
    Empty,
    Leaf {
        id: NodeId,
        primitive: Primitive,
    },
    Transform(Transform, Box<CsgNode>),
    Union(Box<CsgNode>, Box<CsgNode>),
    Difference(Box<CsgNode>, Box<CsgNode>),
    Intersection(Box<CsgNode>, Box<CsgNode>),
    /// Lift a 2D sketch into a 3D slab of `±half_height` along the
    /// local Z axis. The whole extruded region contributes a
    /// single [`NodeId`] to the provenance set — sub-primitives
    /// inside the sketch don't get individual ids until the K5/K6
    /// sketcher gives them identities. Use the extrude's own `id`
    /// in the feature tree to trace back to the originating
    /// outliner entry.
    Extrude {
        id: NodeId,
        sketch: Box<Sketch2d>,
        half_height: f32,
    },
    /// Revolve a 2D sketch around the local Y axis. The sketch's
    /// `x` coordinate maps to the radial distance
    /// `√(x² + z²)` in 3D, its `y` stays vertical. Authoring
    /// convention: sketch primitives should live at `x ≥ 0`;
    /// a primitive that crosses the axis of revolution produces
    /// a valid SDF but geometrically silly results (the axis
    /// sweeps through the centre). Single [`NodeId`] in the
    /// contributor set, same rationale as Extrude.
    Revolve {
        id: NodeId,
        sketch: Box<Sketch2d>,
    },
}

impl CsgNode {
    /// Fold a linear feature spine into a binary CSG tree.
    ///
    /// The first feature seeds the accumulator regardless of its op
    /// (exactly matching the `sdf_features` rule that the empty-space
    /// start means union/difference/intersection all degenerate to
    /// "take this feature as-is"). Each subsequent feature combines
    /// with the accumulator via its own op.
    ///
    /// Each leaf inherits the feature's id as its `NodeId`, so
    /// contributor analysis can always name the original outliner row
    /// that produced a face/edge/vertex.
    pub fn from_features(features: &[Feature]) -> Self {
        let mut acc = Self::Empty;
        for f in features {
            let leaf = Self::Transform(
                f.transform,
                Box::new(Self::Leaf {
                    id: f.id.clone(),
                    primitive: f.primitive.clone(),
                }),
            );
            acc = match (acc, f.op) {
                (Self::Empty, _) => leaf,
                (a, BoolOp::Union) => Self::Union(Box::new(a), Box::new(leaf)),
                (a, BoolOp::Difference) => Self::Difference(Box::new(a), Box::new(leaf)),
                (a, BoolOp::Intersect) => Self::Intersection(Box::new(a), Box::new(leaf)),
            };
        }
        acc
    }

    /// Evaluate the signed distance at a world-space point. Matches
    /// `sdf_features` when built from the same feature list.
    ///
    /// `mode` is threaded through for future Ghost-Topology dual-path
    /// support (see [`EvalMode`]). Until K10 lands both modes return
    /// bit-identical values.
    pub fn sdf(&self, p: [f32; 3], mode: EvalMode) -> f32 {
        match self {
            CsgNode::Empty => f32::INFINITY,
            CsgNode::Leaf { primitive, .. } => sdf_primitive(primitive, p),
            CsgNode::Transform(t, inner) => {
                let local = world_to_local(t, p);
                inner.sdf(local, mode) * t.scale
            }
            CsgNode::Union(a, b) => a.sdf(p, mode).min(b.sdf(p, mode)),
            CsgNode::Difference(a, b) => a.sdf(p, mode).max(-b.sdf(p, mode)),
            CsgNode::Intersection(a, b) => a.sdf(p, mode).max(b.sdf(p, mode)),
            CsgNode::Extrude {
                sketch,
                half_height,
                ..
            } => sdf_extrude(sketch.sdf([p[0], p[1]]), p[2], *half_height),
            CsgNode::Revolve { sketch, .. } => {
                let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                sketch.sdf([r, p[1]])
            }
        }
    }

    /// Provenance-aware SDF evaluation.
    ///
    /// Returns `(distance, contributors)`. The contributor set is the
    /// list of leaf ids whose individual distance fields are within
    /// `eps` of the dominating value at `p`, i.e. the leaves whose
    /// surface passes (approximately) through that point.
    ///
    /// For a point well inside a single leaf the set has one element;
    /// for a point on the intersection curve of two surfaces it has
    /// two; for a point at the meeting of three or more surfaces it
    /// grows further — modulo the `MAX_CONTRIBUTORS` cap.
    ///
    /// The epsilon must be chosen by the caller. The raycaster and
    /// mesher scale it to the local sampling resolution (cell size
    /// for DC, a fraction of the ray step for picking). A fixed
    /// absolute value would either miss real intersections on coarse
    /// grids or flood faces with spurious neighbours on fine ones.
    ///
    /// `mode` is threaded through for the K10 Ghost-Topology split;
    /// see [`EvalMode`]. Until then, Visual == Logical.
    pub fn evaluate(
        &self,
        p: [f32; 3],
        eps: f32,
        mode: EvalMode,
    ) -> (f32, Contributors) {
        match self {
            CsgNode::Empty => (f32::INFINITY, Contributors::empty()),
            CsgNode::Leaf { id, primitive } => {
                (sdf_primitive(primitive, p), Contributors::single(id.clone()))
            }
            CsgNode::Transform(t, inner) => {
                let local = world_to_local(t, p);
                let (d, c) = inner.evaluate(local, eps, mode);
                (d * t.scale, c)
            }
            CsgNode::Union(a, b) => {
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db, cb) = b.evaluate(p, eps, mode);
                combine_min(da, ca, db, cb, eps)
            }
            CsgNode::Difference(a, b) => {
                // result = max(da, -db). Sign flip on `db` does NOT
                // change the contributor identity — it still names
                // the subtracted leaf.
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db, cb) = b.evaluate(p, eps, mode);
                combine_max(da, ca, -db, cb, eps)
            }
            CsgNode::Intersection(a, b) => {
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db, cb) = b.evaluate(p, eps, mode);
                combine_max(da, ca, db, cb, eps)
            }
            CsgNode::Extrude {
                id,
                sketch,
                half_height,
            } => {
                let d = sdf_extrude(sketch.sdf([p[0], p[1]]), p[2], *half_height);
                (d, Contributors::single(id.clone()))
            }
            CsgNode::Revolve { id, sketch } => {
                let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                (sketch.sdf([r, p[1]]), Contributors::single(id.clone()))
            }
        }
    }

    /// World-space AABB (loose — good enough to bound the mesher grid).
    pub fn aabb(&self) -> Aabb {
        match self {
            CsgNode::Empty => Aabb::EMPTY,
            CsgNode::Leaf { primitive, .. } => Aabb::of_primitive(primitive),
            CsgNode::Transform(t, inner) => inner.aabb().transformed(t),
            CsgNode::Union(a, b) => Aabb::union(a.aabb(), b.aabb()),
            // Difference can only shrink the left operand — left AABB
            // is always a conservative bound.
            CsgNode::Difference(a, _) => a.aabb(),
            // Intersection is a subset of either child; keep the left
            // so the bound stays well-defined even when the right child
            // is empty or unbounded.
            CsgNode::Intersection(a, _) => a.aabb(),
            CsgNode::Extrude {
                sketch,
                half_height,
                ..
            } => {
                let s = sketch.aabb();
                if s.is_empty() {
                    Aabb::EMPTY
                } else {
                    Aabb {
                        min: [s.min[0], s.min[1], -half_height],
                        max: [s.max[0], s.max[1], *half_height],
                    }
                }
            }
            CsgNode::Revolve { sketch, .. } => {
                let s = sketch.aabb();
                if s.is_empty() {
                    Aabb::EMPTY
                } else {
                    // Radial extent = max distance from the axis
                    // the sketch reaches. A sketch authored purely
                    // at x ≥ 0 gives `r_max = s.max[0]`; if a user
                    // crosses the axis we conservatively take the
                    // farther of the two edges.
                    let r_max = s.max[0].abs().max(s.min[0].abs());
                    Aabb {
                        min: [-r_max, s.min[1], -r_max],
                        max: [r_max, s.max[1], r_max],
                    }
                }
            }
        }
    }
}

/// Lift a 2D signed distance `d2` into 3D extrude space along the
/// local Z axis with `half_height`. Mirrors Inigo Quilez's
/// `opExtrusion` from <https://iquilezles.org/articles/distfunctions/>:
/// treats `(d2, |z| − h)` as a 2D "rectangle SDF" so the corners
/// of the extruded solid report exact Euclidean distances instead
/// of an over-conservative Chebyshev approximation.
pub(crate) fn sdf_extrude(d2: f32, z: f32, half_height: f32) -> f32 {
    let wz = z.abs() - half_height;
    let outside = (d2.max(0.0).powi(2) + wz.max(0.0).powi(2)).sqrt();
    let inside = d2.max(wz).min(0.0);
    outside + inside
}

/// Boolean-min fold with contributor propagation: within `eps` the
/// two sides are considered tied and both contribute.
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

/// Boolean-max fold with contributor propagation (same tie rule as
/// `combine_min` but picking the larger branch).
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
    use crate::sdf_features;
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
        }
    }

    #[test]
    fn empty_tree_has_infinite_sdf_and_empty_aabb() {
        let tree = CsgNode::from_features(&[]);
        assert!(tree.sdf([0.0, 0.0, 0.0], EvalMode::Logical).is_infinite());
        assert!(tree.aabb().is_empty());
    }

    #[test]
    fn single_box_tree_matches_primitive_sdf() {
        let features = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        )];
        let tree = CsgNode::from_features(&features);
        for &p in &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]] {
            let a = tree.sdf(p, EvalMode::Logical);
            let b = sdf_features(&features, p);
            assert!((a - b).abs() < 1e-5, "{a} vs {b} at {p:?}");
        }
    }

    #[test]
    fn tree_sdf_matches_linear_fold_for_difference() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            Feature {
                id: FeatureId::from_u32(2),
                name: "hole".into(),
                op: BoolOp::Difference,
                primitive: Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                },
                transform: Transform::IDENTITY,
            },
        ];
        let tree = CsgNode::from_features(&features);
        let samples = [
            [0.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [-5.0, -5.0, -5.0],
            [50.0, 0.0, 0.0],
        ];
        for p in samples {
            let a = tree.sdf(p, EvalMode::Logical);
            let b = sdf_features(&features, p);
            assert!(
                (a - b).abs() < 1e-4,
                "tree {a} vs linear {b} at {p:?}",
            );
        }
    }

    #[test]
    fn aabb_of_box_matches_half_extents() {
        let tree = CsgNode::from_features(&[box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [5.0, 7.0, 9.0],
        )]);
        let bb = tree.aabb();
        assert_eq!(bb.min, [-5.0, -7.0, -9.0]);
        assert_eq!(bb.max, [5.0, 7.0, 9.0]);
    }

    #[test]
    fn union_aabb_covers_both_children() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feature(2, BoolOp::Union, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bb = CsgNode::from_features(&features).aabb();
        assert_eq!(bb.min, [-5.0, -5.0, -5.0]);
        assert_eq!(bb.max, [25.0, 5.0, 5.0]);
    }

    // ─── provenance / evaluate ─────────────────────────────────────

    fn cyl_feature(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Cyl {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Cylinder {
                center,
                radius,
                half_height,
            },
            transform: Transform::IDENTITY,
        }
    }

    fn sphere_feature(id: u32, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op: BoolOp::Union,
            primitive: Primitive::Sphere { center, radius },
            transform: Transform::IDENTITY,
        }
    }

    /// Tiny convenience used in the contributor-equality assertions:
    /// build a `Vec<FeatureId>` from a slice of bare `u32` literals.
    fn ids(raw: &[u32]) -> Vec<FeatureId> {
        raw.iter().copied().map(FeatureId::from_u32).collect()
    }

    #[test]
    fn evaluate_on_solo_face_returns_single_contributor() {
        // Union(Box=1, Cylinder=2) with the cylinder poking out the
        // top of the box. Point on the box top, but outside the
        // cylinder footprint → only the box contributes.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 50.0),
        ];
        let tree = CsgNode::from_features(&features);
        let (d, c) = tree.evaluate([15.0, 20.0, 0.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4, "point should sit on box top, got d={d}");
        assert_eq!(c.element(), ProxyElement::Face);
        assert_eq!(c.as_slice(), ids(&[1]).as_slice());
    }

    #[test]
    fn evaluate_on_emergent_edge_returns_two_contributors() {
        // The cylinder's lateral surface (x²+z²=100) meets the box's
        // top face (y=20) along the circle x²+z²=100 at y=20. At
        // (10, 20, 0) both leaves' individual SDFs are zero → the
        // point is an Edge in the B-Rep proxy.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 50.0),
        ];
        let tree = CsgNode::from_features(&features);
        let (d, c) = tree.evaluate([10.0, 20.0, 0.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4, "should lie on both surfaces, got d={d}");
        assert_eq!(c.element(), ProxyElement::Edge);
        let mut got: Vec<FeatureId> = c.as_slice().to_vec();
        got.sort_by_key(|id| id.as_u32_if_flat().unwrap());
        assert_eq!(got, ids(&[1, 2]));
    }

    #[test]
    fn evaluate_on_triple_intersection_returns_vertex() {
        // Three axis-aligned boxes meeting at a single corner (+x,+y,
        // +z). At the shared corner (10, 10, 10) all three box
        // surfaces pass through the same point → contributors = 3 →
        // classification = Vertex.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::Union, [20.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(3, BoolOp::Union, [10.0, 20.0, 0.0], [10.0, 10.0, 10.0]),
        ];
        let tree = CsgNode::from_features(&features);
        // All three boxes meet at (10, 10, 10).
        let (d, c) = tree.evaluate([10.0, 10.0, 10.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4);
        assert_eq!(c.element(), ProxyElement::Vertex);
        assert_eq!(c.len(), 3);
        let mut got: Vec<FeatureId> = c.as_slice().to_vec();
        got.sort_by_key(|id| id.as_u32_if_flat().unwrap());
        assert_eq!(got, ids(&[1, 2, 3]));
    }

    #[test]
    fn difference_preserves_both_contributors_on_hole_rim() {
        // Box (id=1) minus a sphere (id=2) punched out of its corner.
        // On the circle where the sphere exits the box's +x face the
        // point lies on *both* the box's +x face and the sphere's
        // lateral surface → the difference SDF is zero via the tie
        // `max(box_sdf, -sphere_sdf)`.
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            {
                let mut f = sphere_feature(2, [10.0, 0.0, 0.0], 4.0);
                f.op = BoolOp::Difference;
                f
            },
        ];
        let tree = CsgNode::from_features(&features);
        // (10, 0, 4) is on the box's +x face (x=10) AND on the
        // sphere's surface (distance from (10,0,0) = 4).
        let (d, c) = tree.evaluate([10.0, 0.0, 4.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4, "should lie on both boundaries, got d={d}");
        assert_eq!(c.element(), ProxyElement::Edge);
        let mut got: Vec<FeatureId> = c.as_slice().to_vec();
        got.sort_by_key(|id| id.as_u32_if_flat().unwrap());
        assert_eq!(got, ids(&[1, 2]));
    }

    #[test]
    fn evaluate_far_from_surface_still_has_one_contributor() {
        // Deep inside a single leaf — the dominant contributor is
        // unambiguous, but provenance still records which leaf owns
        // that slab of space.
        let features = vec![box_feature(
            1,
            BoolOp::Union,
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        )];
        let tree = CsgNode::from_features(&features);
        let (d, c) = tree.evaluate([0.0, 0.0, 0.0], 0.5, EvalMode::Logical);
        assert!(d < 0.0);
        assert_eq!(c.as_slice(), ids(&[1]).as_slice());
    }

    #[test]
    fn contributors_push_dedupes_and_caps() {
        let mut c = Contributors::empty();
        c.push(FeatureId::from_u32(1));
        c.push(FeatureId::from_u32(2));
        c.push(FeatureId::from_u32(1)); // dup
        c.push(FeatureId::from_u32(3));
        c.push(FeatureId::from_u32(4));
        c.push(FeatureId::from_u32(5)); // over cap — silently dropped
        assert_eq!(c.len(), MAX_CONTRIBUTORS);
        let mut got: Vec<FeatureId> = c.as_slice().to_vec();
        got.sort_by_key(|id| id.as_u32_if_flat().unwrap());
        assert_eq!(got, ids(&[1, 2, 3, 4]));
    }

    // ─── extrude / revolve ─────────────────────────────────────────

    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};

    fn rect_sketch(cx: f32, cy: f32, hx: f32, hy: f32) -> Sketch2d {
        Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [cx, cy],
            half_extents: [hx, hy],
        })
    }

    fn circle_sketch(cx: f32, cy: f32, r: f32) -> Sketch2d {
        Sketch2d::Prim(Sketch2dPrimitive::Circle {
            center: [cx, cy],
            radius: r,
        })
    }

    #[test]
    fn extrude_of_rectangle_matches_3d_box_primitive() {
        // Extruding an XY rectangle of half-extents (hx, hy) by
        // half_height hh should give exactly the same signed
        // distance as a 3D Box primitive of half-extents
        // (hx, hy, hh). The corner-handling matches iq's
        // `opExtrusion` which computes the true Euclidean distance.
        let hx = 7.0;
        let hy = 4.0;
        let hh = 3.0;
        let sketch = rect_sketch(0.0, 0.0, hx, hy);
        let extrude = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: hh,
        };
        let boxy = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [hx, hy, hh],
            },
        };

        // Inside (centre), deep negative.
        let center = [0.0, 0.0, 0.0];
        assert!(
            (extrude.sdf(center, EvalMode::Visual) - boxy.sdf(center, EvalMode::Visual)).abs() < 1e-5,
        );
        // On each of the three face midpoints → 0.
        for p in [[hx, 0.0, 0.0], [0.0, hy, 0.0], [0.0, 0.0, hh]] {
            let a = extrude.sdf(p, EvalMode::Visual);
            let b = boxy.sdf(p, EvalMode::Visual);
            assert!((a - b).abs() < 1e-4, "surface mismatch at {p:?}: {a} vs {b}");
            assert!(a.abs() < 1e-4);
        }
        // Diagonal corner — true Euclidean distance to corner.
        let corner = [hx + 3.0, hy + 4.0, hh + 12.0];
        let a = extrude.sdf(corner, EvalMode::Visual);
        let b = boxy.sdf(corner, EvalMode::Visual);
        assert!((a - b).abs() < 1e-3, "corner mismatch: {a} vs {b}");
        assert!(a > 12.0); // definitely outside
    }

    #[test]
    fn extrude_aabb_lifts_sketch_by_half_height() {
        let extrude = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(rect_sketch(0.0, 0.0, 5.0, 2.0)),
            half_height: 3.0,
        };
        let bb = extrude.aabb();
        assert_eq!(bb.min, [-5.0, -2.0, -3.0]);
        assert_eq!(bb.max, [5.0, 2.0, 3.0]);
    }

    #[test]
    fn extrude_returns_single_contributor() {
        let extrude = CsgNode::Extrude {
            id: FeatureId::from_u32(42),
            sketch: Box::new(rect_sketch(0.0, 0.0, 5.0, 2.0)),
            half_height: 3.0,
        };
        let (d, c) = extrude.evaluate([5.0, 0.0, 0.0], 0.5, EvalMode::Visual);
        assert!(d.abs() < 1e-4);
        assert_eq!(c.as_slice(), ids(&[42]).as_slice());
        assert_eq!(c.element(), ProxyElement::Face);
    }

    #[test]
    fn revolve_of_offset_rectangle_gives_a_hollow_tube() {
        // A 4 mm × 20 mm rectangle parked at (r=5, y=0) revolves
        // around Y into a tube: inner radius 3, outer radius 7,
        // axial extent ±10.
        let rect = rect_sketch(5.0, 0.0, 2.0, 10.0);
        let tube = CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(rect),
        };

        // Outer surface (r=7, y=0).
        assert!(tube.sdf([7.0, 0.0, 0.0], EvalMode::Visual).abs() < 1e-4);
        // Inner surface (r=3, y=0) is also on the 2D rectangle's
        // inner edge → distance 0.
        assert!(tube.sdf([3.0, 0.0, 0.0], EvalMode::Visual).abs() < 1e-4);
        // Midway between the two walls → deep inside the tube.
        assert!(tube.sdf([5.0, 0.0, 0.0], EvalMode::Visual) < -1.9);
        // On the central axis → well outside the tube.
        assert!(tube.sdf([0.0, 0.0, 0.0], EvalMode::Visual) > 2.9);
        // Different angular direction — rotational symmetry check.
        assert!(tube.sdf([0.0, 0.0, 7.0], EvalMode::Visual).abs() < 1e-4);
        assert!(tube.sdf([0.0, 0.0, 3.0], EvalMode::Visual).abs() < 1e-4);
    }

    #[test]
    fn revolve_aabb_is_the_radial_sweep_of_the_sketch() {
        let tube = CsgNode::Revolve {
            id: FeatureId::from_u32(1),
            sketch: Box::new(rect_sketch(5.0, 0.0, 2.0, 10.0)),
        };
        let bb = tube.aabb();
        // Sketch x in [3, 7] → radial extent r_max = 7.
        assert_eq!(bb.min, [-7.0, -10.0, -7.0]);
        assert_eq!(bb.max, [7.0, 10.0, 7.0]);
    }

    #[test]
    fn difference_of_two_extrudes_carves_a_washer() {
        // Extrude-of-difference ≈ difference-of-extrudes here
        // because the sketch difference is the natural "ring"
        // region, and extruding the ring gives a washer.
        let outer = circle_sketch(0.0, 0.0, 10.0);
        let inner = circle_sketch(0.0, 0.0, 4.0);
        let ring = Sketch2d::Difference(Box::new(outer), Box::new(inner));
        let washer = CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(ring),
            half_height: 2.0,
        };
        // Between the two radii, on the mid-plane → inside.
        assert!(washer.sdf([7.0, 0.0, 0.0], EvalMode::Visual) < 0.0);
        // Inside the hole → outside the body.
        assert!(washer.sdf([0.0, 0.0, 0.0], EvalMode::Visual) > 0.0);
        // Top face at r=7 → on the surface. Note the extrude axis
        // is Z, not Y — [7, 0, 2] means "sketch point (7,0) lifted
        // to the top face of the slab".
        assert!(washer.sdf([7.0, 0.0, 2.0], EvalMode::Visual).abs() < 1e-4);
    }

    #[test]
    fn difference_aabb_stays_with_left_operand() {
        let features = vec![
            box_feature(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            Feature {
                id: FeatureId::from_u32(2),
                name: "hole".into(),
                op: BoolOp::Difference,
                primitive: Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                },
                transform: Transform::IDENTITY,
            },
        ];
        let bb = CsgNode::from_features(&features).aabb();
        assert_eq!(bb.min, [-10.0, -10.0, -10.0]);
        assert_eq!(bb.max, [10.0, 10.0, 10.0]);
    }
}
