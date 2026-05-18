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

use serde::{Deserialize, Serialize};

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
            // K7.3: the extrude primitive sweeps its 2D sketch
            // along the local Z axis, so the 3D AABB is the
            // 2D sketch AABB on the XY axes plus ±half_depth_z
            // on Z, translated to `center`.
            Primitive::Extrude {
                sketch,
                half_depth_z,
                center,
            } => {
                let bb2 = sketch.aabb();
                if bb2.is_empty() {
                    return Self::EMPTY;
                }
                Self {
                    min: [
                        center[0] + bb2.min[0],
                        center[1] + bb2.min[1],
                        center[2] - *half_depth_z,
                    ],
                    max: [
                        center[0] + bb2.max[0],
                        center[1] + bb2.max[1],
                        center[2] + *half_depth_z,
                    ],
                }
            }
            // K8.4: revolve sweeps the 2D sketch around the
            // local Y axis. Radial extent is the max |x| the
            // sketch reaches; Y range comes straight from the
            // sketch's Y extent.
            Primitive::Revolve { sketch, center } => {
                let bb2 = sketch.aabb();
                if bb2.is_empty() {
                    return Self::EMPTY;
                }
                let r_max = bb2.max[0].abs().max(bb2.min[0].abs());
                Self {
                    min: [
                        center[0] - r_max,
                        center[1] + bb2.min[1],
                        center[2] - r_max,
                    ],
                    max: [
                        center[0] + r_max,
                        center[1] + bb2.max[1],
                        center[2] + r_max,
                    ],
                }
            }
            Primitive::Torus { center: c, major_radius, minor_radius } => {
                let r = *major_radius + minor_radius;
                Self {
                    min: [c[0] - r, c[1] - minor_radius, c[2] - r],
                    max: [c[0] + r, c[1] + minor_radius, c[2] + r],
                }
            }
            // Gyroid repeats infinitely — return a large conservative AABB.
            Primitive::Gyroid { .. } => Self {
                min: [-1000.0; 3],
                max: [1000.0; 3],
            },
            Primitive::Thread { nominal_radius, half_height, .. } => {
                let r = *nominal_radius;
                let hh = *half_height;
                Self {
                    min: [-r, -hh, -r],
                    max: [r, hh, r],
                }
            }
            Primitive::Cone { r_bottom, r_top, half_height } => {
                let r_max = r_bottom.max(*r_top);
                let hh = *half_height;
                Self {
                    min: [-r_max, -hh, -r_max],
                    max: [r_max, hh, r_max],
                }
            }
            Primitive::Capsule { radius: r, half_height: hh } => Self {
                min: [-r, -(hh + r), -r],
                max: [*r, hh + r, *r],
            },
            Primitive::HexPrism { apothem, half_height: hh } => {
                let r = apothem / 0.866_025_4;
                Self {
                    min: [-r, -*hh, -r],
                    max: [r, *hh, r],
                }
            }
            Primitive::ImportedMesh { mesh } => Aabb {
                min: mesh.aabb_min,
                max: mesh.aabb_max,
            },
            Primitive::Ellipsoid { radii } => Aabb {
                min: [-radii[0], -radii[1], -radii[2]],
                max: [radii[0], radii[1], radii[2]],
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
/// K10.2: how to blend the junction between two Boolean
/// operands. Fillet uses the cubic smin polynomial (rounded
/// edges); Chamfer uses a linear 45° cut.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BlendMode {
    Fillet,
    Chamfer,
}

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
    /// K10.0: smooth-min union with a blend radius. Inside the
    /// blend zone (where both children's distances are within `r`
    /// of the surface) the operator produces a fillet of radius
    /// `r` via the IQ `smin` polynomial:
    ///   h = max(r - |da - db|, 0) / r
    ///   d = min(da, db) - h³ · r / 6
    /// Outside the blend zone it degenerates to the hard `min`
    /// (== `Union`), so existing geometry is pixel-identical
    /// beyond the fillet.
    SmoothUnion(Box<CsgNode>, Box<CsgNode>, f32, BlendMode),
    /// K10.0: smooth-max difference with the same blend radius
    /// semantics. The fillet rounds concave edges where material
    /// is subtracted.
    SmoothDifference(Box<CsgNode>, Box<CsgNode>, f32, BlendMode),
    /// K11.1: twist the child around the local Y axis. The twist
    /// rate is in radians per millimetre along Y — a rate of
    /// `π / 100` turns the body 180° over a 100 mm span. The
    /// domain warp rotates XZ by `rate * p.y` before evaluating
    /// the child. Not an exact SDF (the metric stretches with
    /// radial distance), but the DC mesher tolerates the slight
    /// under-estimation at practical twist rates.
    Twist(Box<CsgNode>, f32),
    /// K11.2: bend the child in the XY plane along the X axis.
    /// The rate is radians per mm — `rate * p.x` rotates XY
    /// before evaluating the child. Turns a straight bar into
    /// an arc, a box into a curved bracket, etc. Same
    /// metric-distortion caveat as Twist.
    Bend(Box<CsgNode>, f32),
    /// K11.3: displace the child's surface by a 3D sine-product
    /// pattern: `child.sdf(p) - depth * sin(fx·x) * sin(fy·y) * sin(fz·z)`.
    /// `depth` is the amplitude in mm, `freq` scales the spatial
    /// frequency (cycles per mm). Produces an egg-crate-like bump
    /// texture. The AABB expands by `depth` in every direction.
    Displace(Box<CsgNode>, f32, f32),
    /// Shell: hollow the child by replacing its SDF with
    /// `|sdf(p)| - thickness`. Turns a solid body into a
    /// thin-walled shell of uniform thickness.
    Shell(Box<CsgNode>, f32),
    /// K10.3: spatially-masked blend. Evaluates `a` and `b` at
    /// every point, then checks a capsule mask SDF (defined by
    /// `mask_p0`, `mask_p1`, `mask_radius`). Inside the mask:
    /// smooth blend with `blend_radius` and `blend_mode`.
    /// Outside: hard `min` (union) or `max` (difference) based
    /// on `is_difference`. Allows per-edge fillet selection.
    /// K11.5: morph (linear interpolation) between two SDFs.
    /// `f = (1 − t) · a + t · b` where `t ∈ [0, 1]`.
    /// For `t=0` returns a, for `t=1` returns b.
    Morph(Box<CsgNode>, Box<CsgNode>, f32),
    MaskedBlend {
        a: Box<CsgNode>,
        b: Box<CsgNode>,
        mask_p0: [f32; 3],
        mask_p1: [f32; 3],
        mask_radius: f32,
        blend_radius: f32,
        blend_mode: BlendMode,
        is_difference: bool,
    },
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
            // K7.3: Primitive::Extrude lowers directly to a
            // CsgNode::Extrude that the K4 mesher knows how
            // to walk. Every other primitive stays in the
            // CsgNode::Leaf wrapper path so the existing
            // BoundedCsg builder keeps working unchanged.
            let node = match &f.primitive {
                Primitive::Extrude {
                    sketch,
                    half_depth_z,
                    center,
                } => {
                    // Fold the extrude's own `center` into a
                    // pre-rotation translation so the
                    // canonical `CsgNode::Extrude` slab sits
                    // at the origin of the local frame. The
                    // outer `CsgNode::Transform` then applies
                    // the feature's own transform on top.
                    let centered = Self::Extrude {
                        id: f.id.clone(),
                        sketch: Box::new(sketch.clone()),
                        half_height: *half_depth_z,
                    };
                    // Shift by `center` via an inner
                    // Transform so the extrude SDF sees local
                    // coordinates centred on its own origin.
                    if *center == [0.0, 0.0, 0.0] {
                        centered
                    } else {
                        Self::Transform(
                            Transform {
                                translate: *center,
                                rotation: [0.0, 0.0, 0.0, 1.0],
                                scale: 1.0,
                            },
                            Box::new(centered),
                        )
                    }
                }
                // K8.4: same lowering dance as Extrude, but the
                // payload is CsgNode::Revolve and the sketch is
                // interpreted in (r, y) rather than (x, y).
                Primitive::Revolve { sketch, center } => {
                    let centered = Self::Revolve {
                        id: f.id.clone(),
                        sketch: Box::new(sketch.clone()),
                    };
                    if *center == [0.0, 0.0, 0.0] {
                        centered
                    } else {
                        Self::Transform(
                            Transform {
                                translate: *center,
                                rotation: [0.0, 0.0, 0.0, 1.0],
                                scale: 1.0,
                            },
                            Box::new(centered),
                        )
                    }
                }
                _ => Self::Leaf {
                    id: f.id.clone(),
                    primitive: f.primitive.clone(),
                },
            };
            let mut leaf: CsgNode = Self::Transform(f.transform, Box::new(node));
            if f.twist_rate != 0.0 {
                leaf = Self::Twist(Box::new(leaf), f.twist_rate);
            }
            if f.bend_rate != 0.0 {
                leaf = Self::Bend(Box::new(leaf), f.bend_rate);
            }
            if f.displace_depth != 0.0 && f.displace_freq != 0.0 {
                leaf = Self::Displace(Box::new(leaf), f.displace_depth, f.displace_freq);
            }
            if f.shell_thickness > 0.0 {
                leaf = Self::Shell(Box::new(leaf), f.shell_thickness);
            }
            // Blend selection: fillet (blend_radius) wins over
            // chamfer (chamfer_size) when both are set.
            let (blend_r, blend_mode) = if f.blend_radius > 0.0 {
                (f.blend_radius, BlendMode::Fillet)
            } else if f.chamfer_size > 0.0 {
                (f.chamfer_size, BlendMode::Chamfer)
            } else {
                (0.0, BlendMode::Fillet)
            };
            acc = match (acc, f.op) {
                (Self::Empty, _) => leaf,
                (a, BoolOp::Union) if blend_r > 0.0 => {
                    Self::SmoothUnion(Box::new(a), Box::new(leaf), blend_r, blend_mode)
                }
                (a, BoolOp::Union) => Self::Union(Box::new(a), Box::new(leaf)),
                (a, BoolOp::Difference) if blend_r > 0.0 => {
                    Self::SmoothDifference(Box::new(a), Box::new(leaf), blend_r, blend_mode)
                }
                (a, BoolOp::Difference) => Self::Difference(Box::new(a), Box::new(leaf)),
                (a, BoolOp::Intersect) => Self::Intersection(Box::new(a), Box::new(leaf)),
            };
        }
        acc
    }

    /// Evaluate the signed distance at a world-space point. Matches
    /// K10.4: post-process the tree by replacing Union/Difference
    /// nodes with MaskedBlend where an EdgeFillet matches. A fillet
    /// matches a junction when one subtree contains feature_a and
    /// the other contains feature_b (either order).
    pub fn apply_edge_fillets(self, fillets: &[EdgeFillet]) -> Self {
        if fillets.is_empty() {
            return self;
        }
        match self {
            CsgNode::Union(a, b) => {
                let a = a.apply_edge_fillets(fillets);
                let b = b.apply_edge_fillets(fillets);
                if let Some(f) = find_matching_fillet(&a, &b, fillets) {
                    CsgNode::MaskedBlend {
                        a: Box::new(a),
                        b: Box::new(b),
                        mask_p0: f.mask_p0,
                        mask_p1: f.mask_p1,
                        mask_radius: f.mask_radius,
                        blend_radius: f.blend_radius,
                        blend_mode: f.blend_mode,
                        is_difference: false,
                    }
                } else {
                    CsgNode::Union(Box::new(a), Box::new(b))
                }
            }
            CsgNode::Difference(a, b) => {
                let a = a.apply_edge_fillets(fillets);
                let b = b.apply_edge_fillets(fillets);
                if let Some(f) = find_matching_fillet(&a, &b, fillets) {
                    CsgNode::MaskedBlend {
                        a: Box::new(a),
                        b: Box::new(b),
                        mask_p0: f.mask_p0,
                        mask_p1: f.mask_p1,
                        mask_radius: f.mask_radius,
                        blend_radius: f.blend_radius,
                        blend_mode: f.blend_mode,
                        is_difference: true,
                    }
                } else {
                    CsgNode::Difference(Box::new(a), Box::new(b))
                }
            }
            // Recurse into wrappers but don't match on them.
            CsgNode::Transform(t, inner) => {
                CsgNode::Transform(t, Box::new(inner.apply_edge_fillets(fillets)))
            }
            CsgNode::Twist(inner, r) => {
                CsgNode::Twist(Box::new(inner.apply_edge_fillets(fillets)), r)
            }
            CsgNode::Bend(inner, r) => {
                CsgNode::Bend(Box::new(inner.apply_edge_fillets(fillets)), r)
            }
            CsgNode::Shell(inner, t) => {
                CsgNode::Shell(Box::new(inner.apply_edge_fillets(fillets)), t)
            }
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.apply_edge_fillets(fillets)), d, f)
            }
            // Leaves and already-blended nodes pass through.
            other => other,
        }
    }

    /// K3.5C: collect a map of leaf FeatureId → Primitive reference.
    pub fn leaf_primitives(&self) -> std::collections::HashMap<u32, &Primitive> {
        let mut out = std::collections::HashMap::new();
        self.collect_leaf_primitives(&mut out);
        out
    }

    fn collect_leaf_primitives<'a>(&'a self, out: &mut std::collections::HashMap<u32, &'a Primitive>) {
        match self {
            CsgNode::Leaf { id, primitive, .. } => {
                if let Some(flat) = id.as_u32_if_flat() {
                    out.insert(flat, primitive);
                }
            }
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Shell(inner, _)
            | CsgNode::Displace(inner, _, _) => inner.collect_leaf_primitives(out),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b)
            | CsgNode::SmoothUnion(a, b, _, _)
            | CsgNode::SmoothDifference(a, b, _, _)
            | CsgNode::Morph(a, b, _) => {
                a.collect_leaf_primitives(out);
                b.collect_leaf_primitives(out);
            }
            CsgNode::MaskedBlend { a, b, .. } => {
                a.collect_leaf_primitives(out);
                b.collect_leaf_primitives(out);
            }
            CsgNode::Extrude { .. } | CsgNode::Revolve { .. } | CsgNode::Empty => {}
        }
    }

    /// Collect all leaf FeatureIds reachable from this node.
    pub fn leaf_ids(&self) -> Vec<FeatureId> {
        let mut out = Vec::new();
        self.collect_leaf_ids(&mut out);
        out
    }

    fn collect_leaf_ids(&self, out: &mut Vec<FeatureId>) {
        match self {
            CsgNode::Leaf { id, .. } => out.push(id.clone()),
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Shell(inner, _)
            | CsgNode::Displace(inner, _, _) => inner.collect_leaf_ids(out),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b)
            | CsgNode::SmoothUnion(a, b, _, _)
            | CsgNode::SmoothDifference(a, b, _, _)
            | CsgNode::Morph(a, b, _) => {
                a.collect_leaf_ids(out);
                b.collect_leaf_ids(out);
            }
            CsgNode::MaskedBlend { a, b, .. } => {
                a.collect_leaf_ids(out);
                b.collect_leaf_ids(out);
            }
            CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } => {
                out.push(id.clone());
            }
            CsgNode::Empty => {}
        }
    }

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
            CsgNode::SmoothUnion(a, b, r, mode_blend) => {
                let da = a.sdf(p, mode);
                let db = b.sdf(p, mode);
                match mode {
                    EvalMode::Visual => crate::csg::blend_min(da, db, *r, *mode_blend),
                    EvalMode::Logical => da.min(db),
                }
            }
            CsgNode::SmoothDifference(a, b, r, mode_blend) => {
                let da = a.sdf(p, mode);
                let db = b.sdf(p, mode);
                match mode {
                    EvalMode::Visual => -crate::csg::blend_min(-da, db, *r, *mode_blend),
                    EvalMode::Logical => da.max(-db),
                }
            }
            CsgNode::Twist(inner, rate) => {
                let angle = *rate * p[1];
                let (s, c) = angle.sin_cos();
                let twisted = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
                inner.sdf(twisted, mode)
            }
            CsgNode::Bend(inner, rate) => {
                let angle = *rate * p[0];
                let (s, c) = angle.sin_cos();
                let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
                inner.sdf(bent, mode)
            }
            CsgNode::Displace(inner, depth, freq) => {
                let d = inner.sdf(p, mode);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                d - disp
            }
            CsgNode::Shell(inner, thickness) => {
                inner.sdf(p, mode).abs() - *thickness
            }
            CsgNode::Morph(a, b, t) => {
                let da = a.sdf(p, mode);
                let db = b.sdf(p, mode);
                (1.0 - t) * da + t * db
            }
            CsgNode::MaskedBlend {
                a,
                b,
                mask_p0,
                mask_p1,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
            } => {
                let da = a.sdf(p, mode);
                let db_raw = b.sdf(p, mode);
                match mode {
                    EvalMode::Logical => {
                        if *is_difference { da.max(-db_raw) } else { da.min(db_raw) }
                    }
                    EvalMode::Visual => {
                        let mask_d = sdf_capsule_3d(p, *mask_p0, *mask_p1, *mask_radius);
                        if mask_d < 0.0 {
                            if *is_difference {
                                -blend_min(-da, db_raw, *blend_radius, *blend_mode)
                            } else {
                                blend_min(da, db_raw, *blend_radius, *blend_mode)
                            }
                        } else {
                            if *is_difference { da.max(-db_raw) } else { da.min(db_raw) }
                        }
                    }
                }
            }
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
            CsgNode::SmoothUnion(a, b, r, mode_blend) => {
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db, cb) = b.evaluate(p, eps, mode);
                match mode {
                    EvalMode::Visual => {
                        let d = crate::csg::blend_min(da, db, *r, *mode_blend);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => combine_min(da, ca, db, cb, eps),
                }
            }
            CsgNode::SmoothDifference(a, b, r, mode_blend) => {
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db, cb) = b.evaluate(p, eps, mode);
                match mode {
                    EvalMode::Visual => {
                        let d = -crate::csg::blend_min(-da, db, *r, *mode_blend);
                        let mut c = ca;
                        c.extend(cb);
                        (d, c)
                    }
                    EvalMode::Logical => combine_max(da, ca, -db, cb, eps),
                }
            }
            CsgNode::Twist(inner, rate) => {
                let angle = *rate * p[1];
                let (s, c) = angle.sin_cos();
                let twisted = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
                inner.evaluate(twisted, eps, mode)
            }
            CsgNode::Bend(inner, rate) => {
                let angle = *rate * p[0];
                let (s, c) = angle.sin_cos();
                let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
                inner.evaluate(bent, eps, mode)
            }
            CsgNode::Displace(inner, depth, freq) => {
                let (d, c) = inner.evaluate(p, eps, mode);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                (d - disp, c)
            }
            CsgNode::Shell(inner, thickness) => {
                let (d, c) = inner.evaluate(p, eps, mode);
                (d.abs() - *thickness, c)
            }
            CsgNode::Morph(a, b, t) => {
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db, cb) = b.evaluate(p, eps, mode);
                let d = (1.0 - t) * da + t * db;
                let mut c = ca;
                c.extend(cb);
                (d, c)
            }
            CsgNode::MaskedBlend {
                a,
                b,
                mask_p0,
                mask_p1,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
            } => {
                let (da, ca) = a.evaluate(p, eps, mode);
                let (db_raw, cb) = b.evaluate(p, eps, mode);
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
                        let mask_d = sdf_capsule_3d(p, *mask_p0, *mask_p1, *mask_radius);
                        if mask_d < 0.0 {
                            let d = if *is_difference {
                                -blend_min(-da, db_raw, *blend_radius, *blend_mode)
                            } else {
                                blend_min(da, db_raw, *blend_radius, *blend_mode)
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
            CsgNode::Union(a, b) | CsgNode::SmoothUnion(a, b, _, _) => {
                Aabb::union(a.aabb(), b.aabb())
            }
            // Difference can only shrink the left operand — left AABB
            // is always a conservative bound. The smooth variant may
            // bulge outward by up to `r` in the blend zone, but the
            // AABB of the union-of-children is always a superset.
            CsgNode::Difference(a, _) | CsgNode::SmoothDifference(a, _, _, _) => a.aabb(),
            CsgNode::Twist(inner, _) => {
                let bb = inner.aabb();
                if bb.is_empty() {
                    return Aabb::EMPTY;
                }
                let rx = bb.max[0].abs().max(bb.min[0].abs());
                let rz = bb.max[2].abs().max(bb.min[2].abs());
                let r_max = (rx * rx + rz * rz).sqrt();
                Aabb {
                    min: [-r_max, bb.min[1], -r_max],
                    max: [r_max, bb.max[1], r_max],
                }
            }
            CsgNode::Bend(inner, _) => {
                let bb = inner.aabb();
                if bb.is_empty() {
                    return Aabb::EMPTY;
                }
                let rx = bb.max[0].abs().max(bb.min[0].abs());
                let ry = bb.max[1].abs().max(bb.min[1].abs());
                let r_max = (rx * rx + ry * ry).sqrt();
                Aabb {
                    min: [-r_max, -r_max, bb.min[2]],
                    max: [r_max, r_max, bb.max[2]],
                }
            }
            CsgNode::Displace(inner, depth, _) => {
                let bb = inner.aabb();
                if bb.is_empty() {
                    return Aabb::EMPTY;
                }
                let d = depth.abs();
                Aabb {
                    min: [bb.min[0] - d, bb.min[1] - d, bb.min[2] - d],
                    max: [bb.max[0] + d, bb.max[1] + d, bb.max[2] + d],
                }
            }
            CsgNode::Shell(inner, thickness) => {
                let bb = inner.aabb();
                if bb.is_empty() {
                    return Aabb::EMPTY;
                }
                let t = *thickness;
                Aabb {
                    min: [bb.min[0] - t, bb.min[1] - t, bb.min[2] - t],
                    max: [bb.max[0] + t, bb.max[1] + t, bb.max[2] + t],
                }
            }
            CsgNode::Morph(a, b, _) => Aabb::union(a.aabb(), b.aabb()),
            CsgNode::MaskedBlend { a, b, .. } => {
                Aabb::union(a.aabb(), b.aabb())
            }
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

/// K10.0: polynomial smooth minimum (IQ's cubic variant).
/// `r` is the blend radius. When the two input distances are
/// far apart (|da - db| >= r) the result degenerates to
/// `min(da, db)`. In the blend zone the result is strictly
/// less than both inputs, producing a fillet of approximate
/// radius `r`.
///
/// Reference: <https://iquilezles.org/articles/smin/>
pub(crate) fn smin_poly(da: f32, db: f32, r: f32) -> f32 {
    if r <= 0.0 {
        return da.min(db);
    }
    let h = (r - (da - db).abs()).max(0.0) / r;
    da.min(db) - h * h * h * r / 6.0
}

/// K10.4: find the first EdgeFillet that matches a junction
/// between two subtrees. A fillet matches when one subtree
/// contains feature_a and the other contains feature_b.
fn find_matching_fillet<'a>(
    left: &CsgNode,
    right: &CsgNode,
    fillets: &'a [EdgeFillet],
) -> Option<&'a EdgeFillet> {
    let left_ids = left.leaf_ids();
    let right_ids = right.leaf_ids();
    fillets.iter().find(|f| {
        let a_in_left = left_ids.iter().any(|id| *id == f.feature_a);
        let b_in_right = right_ids.iter().any(|id| *id == f.feature_b);
        let a_in_right = right_ids.iter().any(|id| *id == f.feature_a);
        let b_in_left = left_ids.iter().any(|id| *id == f.feature_b);
        (a_in_left && b_in_right) || (a_in_right && b_in_left)
    })
}

/// K10.4: per-edge fillet specification. Identifies a Boolean
/// junction by the feature IDs of its two operands and places
/// a capsule mask along the edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeFillet {
    /// Feature id of the "left" operand at the junction.
    pub feature_a: FeatureId,
    /// Feature id of the "right" operand.
    pub feature_b: FeatureId,
    /// Capsule start point (world space).
    pub mask_p0: [f32; 3],
    /// Capsule end point.
    pub mask_p1: [f32; 3],
    /// Capsule radius — how far the mask extends from the edge.
    pub mask_radius: f32,
    /// Blend radius for the fillet.
    pub blend_radius: f32,
    /// Fillet or chamfer.
    pub blend_mode: BlendMode,
}

/// K10.3: capsule SDF for spatial masks. The capsule is a
/// line segment from `p0` to `p1` swept by `radius`.
pub(crate) fn sdf_capsule_3d(
    p: [f32; 3],
    p0: [f32; 3],
    p1: [f32; 3],
    radius: f32,
) -> f32 {
    let pa = [p[0] - p0[0], p[1] - p0[1], p[2] - p0[2]];
    let ba = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let dot_pa_ba = pa[0] * ba[0] + pa[1] * ba[1] + pa[2] * ba[2];
    let dot_ba_ba = ba[0] * ba[0] + ba[1] * ba[1] + ba[2] * ba[2];
    let h = if dot_ba_ba > 0.0 {
        (dot_pa_ba / dot_ba_ba).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = pa[0] - ba[0] * h;
    let dy = pa[1] - ba[1] * h;
    let dz = pa[2] - ba[2] * h;
    (dx * dx + dy * dy + dz * dz).sqrt() - radius
}

/// K10.2: chamfer union — linear 45° cut instead of a rounded
/// fillet. `min(min(a, b), (a + b - r) * SQRT_HALF)`.
/// Dispatch smooth-min by blend mode.
pub(crate) fn blend_min(da: f32, db: f32, r: f32, mode: BlendMode) -> f32 {
    match mode {
        BlendMode::Fillet => smin_poly(da, db, r),
        BlendMode::Chamfer => chamfer_union(da, db, r),
    }
}

pub(crate) fn chamfer_union(da: f32, db: f32, r: f32) -> f32 {
    const SQRT_HALF: f32 = 0.707_106_8;
    da.min(db).min((da + db - r) * SQRT_HALF)
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
    use fielddraft_protocol::{
        BoolOp, Feature, FeatureId, Primitive, Sketch2d as ProtoSketch2d,
        Sketch2dPrimitive as ProtoSketch2dPrimitive, Transform,
    };

    // ─── K7.3: from_features → CsgNode::Extrude ────────────────

    #[test]
    fn from_features_lowers_primitive_extrude_to_csgnode_extrude() {
        // A radius-5 circle sketch extruded ±3 mm along Z
        // should produce a cylindrical SDF: a point at the
        // centre of the slab is inside, a point on the
        // circular boundary sits on the surface (SDF ≈ 0),
        // and a point far from the slab is outside.
        let circle_sketch = ProtoSketch2d::Prim(ProtoSketch2dPrimitive::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        });
        let features = vec![Feature {
            id: FeatureId::from_u32(1),
            name: "extrude".to_string(),
            op: BoolOp::Union,
            primitive: Primitive::Extrude {
                sketch: circle_sketch,
                half_depth_z: 3.0,
                center: [0.0, 0.0, 0.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        }];
        let csg = CsgNode::from_features(&features);
        // Inside at the origin.
        assert!(csg.sdf([0.0, 0.0, 0.0], EvalMode::Logical) < 0.0);
        // On the cylindrical surface at (5, 0, 0).
        let on_surface = csg.sdf([5.0, 0.0, 0.0], EvalMode::Logical).abs();
        assert!(on_surface < 0.01, "expected ~0, got {on_surface}");
        // Outside above the slab.
        assert!(csg.sdf([0.0, 0.0, 10.0], EvalMode::Logical) > 0.0);
        // The bounding AABB matches the cylinder extents.
        let aabb = csg.aabb();
        assert!((aabb.min[0] - (-5.0)).abs() < 1e-5);
        assert!((aabb.max[0] - 5.0).abs() < 1e-5);
        assert!((aabb.min[2] - (-3.0)).abs() < 1e-5);
        assert!((aabb.max[2] - 3.0).abs() < 1e-5);
    }

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
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
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
                blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
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
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
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
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
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
                blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
            },
        ];
        let bb = CsgNode::from_features(&features).aabb();
        assert_eq!(bb.min, [-10.0, -10.0, -10.0]);
        assert_eq!(bb.max, [10.0, 10.0, 10.0]);
    }

    // ─── K10.0: smooth blend ─────────────────────────────────

    #[test]
    fn smin_poly_degenerates_to_hard_min_outside_blend_zone() {
        assert_eq!(super::smin_poly(10.0, -5.0, 2.0), -5.0);
        assert_eq!(super::smin_poly(-5.0, 10.0, 2.0), -5.0);
    }

    #[test]
    fn smin_poly_is_strictly_less_than_hard_min_in_blend_zone() {
        let da = 1.0;
        let db = 1.0;
        let r = 4.0;
        let smooth = super::smin_poly(da, db, r);
        let hard = da.min(db);
        assert!(smooth < hard, "smooth {smooth} not < hard {hard}");
    }

    #[test]
    fn smooth_union_rounds_a_box_sphere_junction() {
        let a = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
        };
        let b = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere {
                center: [10.0, 0.0, 0.0],
                radius: 8.0,
            },
        };
        let hard = CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()));
        let smooth = CsgNode::SmoothUnion(Box::new(a), Box::new(b), 4.0, BlendMode::Fillet);

        // At the junction seam the smooth blend pulls the
        // distance further negative than the hard union.
        let p = [10.0, 8.0, 0.0];
        let d_hard = hard.sdf(p, EvalMode::Visual);
        let d_smooth = smooth.sdf(p, EvalMode::Visual);
        assert!(d_smooth < d_hard, "smooth {d_smooth} not < hard {d_hard}");
    }

    #[test]
    fn masked_blend_fillets_only_inside_the_capsule() {
        // Two boxes side by side: a at origin, b shifted +X.
        let a = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
        };
        let b = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box {
                center: [10.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
        };
        // Capsule mask along the Y axis at x=10 (the junction edge).
        let masked = CsgNode::MaskedBlend {
            a: Box::new(a.clone()),
            b: Box::new(b.clone()),
            mask_p0: [10.0, -15.0, 0.0],
            mask_p1: [10.0, 15.0, 0.0],
            mask_radius: 5.0,
            blend_radius: 4.0,
            blend_mode: super::BlendMode::Fillet,
            is_difference: false,
        };
        let hard = CsgNode::Union(Box::new(a), Box::new(b));

        // Near the junction edge inside the mask → smooth blend
        // produces a different (more negative) distance than hard.
        let p_inside_mask = [10.0, 0.0, 8.0];
        let d_masked = masked.sdf(p_inside_mask, EvalMode::Visual);
        let d_hard = hard.sdf(p_inside_mask, EvalMode::Visual);
        assert!(
            d_masked < d_hard || (d_masked - d_hard).abs() < 1e-5,
            "masked {d_masked} should be <= hard {d_hard} inside the mask"
        );

        // Far from the mask → same as hard union.
        let p_outside_mask = [0.0, 0.0, 0.0];
        let d_masked_far = masked.sdf(p_outside_mask, EvalMode::Visual);
        let d_hard_far = hard.sdf(p_outside_mask, EvalMode::Visual);
        assert!(
            (d_masked_far - d_hard_far).abs() < 1e-5,
            "outside mask: masked {d_masked_far} != hard {d_hard_far}"
        );

        // Logical mode always uses hard ops.
        let d_logical = masked.sdf(p_inside_mask, EvalMode::Logical);
        assert_eq!(d_logical, d_hard);
    }

    #[test]
    fn apply_edge_fillets_replaces_matching_junction() {
        let a = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [10.0, 10.0, 10.0],
            },
        };
        let b = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere {
                center: [5.0, 0.0, 0.0],
                radius: 8.0,
            },
        };
        let tree = CsgNode::Union(Box::new(a), Box::new(b));
        let fillets = vec![EdgeFillet {
            feature_a: FeatureId::from_u32(1),
            feature_b: FeatureId::from_u32(2),
            mask_p0: [5.0, -15.0, 0.0],
            mask_p1: [5.0, 15.0, 0.0],
            mask_radius: 5.0,
            blend_radius: 3.0,
            blend_mode: BlendMode::Fillet,
        }];
        let result = tree.apply_edge_fillets(&fillets);
        // The top-level Union should have been replaced by
        // MaskedBlend.
        assert!(matches!(result, CsgNode::MaskedBlend { .. }));
    }

    #[test]
    fn apply_edge_fillets_leaves_unmatched_junctions() {
        let a = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0; 3],
                half_extents: [5.0; 3],
            },
        };
        let b = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere {
                center: [0.0; 3],
                radius: 3.0,
            },
        };
        let tree = CsgNode::Difference(Box::new(a), Box::new(b));
        // Fillet references features 3 and 4 which don't exist
        // in the tree → no match, tree stays unchanged.
        let fillets = vec![EdgeFillet {
            feature_a: FeatureId::from_u32(3),
            feature_b: FeatureId::from_u32(4),
            mask_p0: [0.0; 3],
            mask_p1: [0.0, 10.0, 0.0],
            mask_radius: 5.0,
            blend_radius: 3.0,
            blend_mode: BlendMode::Fillet,
        }];
        let result = tree.apply_edge_fillets(&fillets);
        assert!(matches!(result, CsgNode::Difference(_, _)));
    }

    #[test]
    fn fillet_all_edges_stress_test() {
        // Build a box with 4 holes subtracted — each junction
        // gets a fillet. Verifies that multiple MaskedBlend nodes
        // compose without panicking.
        let base = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0; 3],
                half_extents: [50.0, 50.0, 50.0],
            },
        };
        let mut tree = base;
        for i in 2..=5 {
            let hole = CsgNode::Leaf {
                id: FeatureId::from_u32(i),
                primitive: Primitive::Cylinder {
                    center: [(i as f32 - 3.5) * 20.0, 0.0, 0.0],
                    radius: 5.0,
                    half_height: 60.0,
                },
            };
            tree = CsgNode::Difference(Box::new(tree), Box::new(hole));
        }
        // Fillet every hole junction.
        let fillets: Vec<EdgeFillet> = (2..=5)
            .map(|i| EdgeFillet {
                feature_a: FeatureId::from_u32(1),
                feature_b: FeatureId::from_u32(i),
                mask_p0: [(i as f32 - 3.5) * 20.0, -55.0, 0.0],
                mask_p1: [(i as f32 - 3.5) * 20.0, 55.0, 0.0],
                mask_radius: 8.0,
                blend_radius: 2.0,
                blend_mode: BlendMode::Fillet,
            })
            .collect();
        let result = tree.apply_edge_fillets(&fillets);
        // Should produce exactly 4 MaskedBlend nodes (one per
        // hole junction). Test that the SDF evaluates without
        // panicking at a few sample points.
        for x in [-30.0, 0.0, 30.0] {
            let d = result.sdf([x, 0.0, 0.0], EvalMode::Visual);
            assert!(d.is_finite(), "SDF at ({x}, 0, 0) = {d}");
        }
    }

    #[test]
    fn smooth_blend_logical_mode_degrades_to_hard_ops() {
        let a = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere {
                center: [0.0; 3],
                radius: 5.0,
            },
        };
        let b = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Sphere {
                center: [4.0, 0.0, 0.0],
                radius: 5.0,
            },
        };
        let hard_u = CsgNode::Union(Box::new(a.clone()), Box::new(b.clone()));
        let smooth_u = CsgNode::SmoothUnion(Box::new(a.clone()), Box::new(b.clone()), 3.0, BlendMode::Fillet);
        let hard_d = CsgNode::Difference(Box::new(a.clone()), Box::new(b.clone()));
        let smooth_d = CsgNode::SmoothDifference(Box::new(a), Box::new(b), 3.0, BlendMode::Fillet);

        let p = [2.0, 3.0, 0.0];
        assert_eq!(
            hard_u.sdf(p, EvalMode::Logical),
            smooth_u.sdf(p, EvalMode::Logical),
        );
        assert_eq!(
            hard_d.sdf(p, EvalMode::Logical),
            smooth_d.sdf(p, EvalMode::Logical),
        );
    }
}
