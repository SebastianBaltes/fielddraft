//! CSG abstract syntax tree.
//!
//! K1 introduces the CSG graph as a sibling representation of the
//! existing linear feature spine. `CsgNode::from_decoded` folds a
//! [`DecodedFeature`] list into a binary tree whose `sdf` evaluator is
//! mathematically identical to evaluating the linear feature spine
//! op-by-op — the
//! tree is just a richer form that later kernel slices (provenance,
//! BVH, multi-body isolation) will extend. For K1 the tree only exists
//! so that the Dual Contouring mesher can traverse it polymorphically
//! instead of special-casing the linear list.

use serde::{Deserialize, Serialize};

use fielddraft_protocol::sketch2d::SketchEdgeBand;
use fielddraft_protocol::{BoolOp, FeatureId, HalfSpace, Primitive, Transform};

use crate::sketch2d::Sketch2d;
use crate::{quat_rotate, sdf_primitive, world_to_local};

/// Stable identifier of a CSG branch — a primitive *sub-face* (a box
/// face, a cylinder cap, a sketch segment), not just a whole feature.
///
/// Packs a `FeatureId`'s flat numeric id into the high bits and a local
/// sub-index into the low [`BranchId::SUB_BITS`] bits, so a single `u32`
/// names "the +X face of feature 7" without a separate side table. The
/// B-Rep substrate keys faces/edges off these ids; the sub-index lets a
/// primitive's six faces (or a sketch's many segments) each get a
/// distinct, Copy, hashable identity.
///
/// Being a plain `u32` newtype makes [`Contributors`] `Copy`, which the
/// hot evaluate path relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BranchId(pub u32);

impl BranchId {
    pub const SENTINEL: u32 = u32::MAX;
    /// Bits reserved for the per-feature sub-index. 12 bits → 4096
    /// sub-ids per feature (enough for sketch segments), leaving 20
    /// bits ≈ 1M feature ids.
    const SUB_BITS: u32 = 12;
    const SUB_MASK: u32 = 0xFFF;

    /// Bit 11 of the 12-bit sub field, reserved as the inner-wall flag (DL-75).
    /// A shelled body's inner wall is the outer face offset inward by the wall
    /// thickness — the *same* feature and analytic surface family, so it keeps
    /// the outer face's `feature_id()` (selection still resolves to the owning
    /// feature) but needs a *distinct* branch id so the weave classifies it as
    /// its own face. Setting this bit is that distinct id. All outer sub-ids
    /// (sketch boundary groups + the two extrude caps) live strictly below it
    /// (`< 0x800`), so the flag never collides; `feature_id()` (the top 20
    /// bits) is untouched.
    pub const INNER_WALL_FLAG: u32 = 0x800;

    /// Reserved extrude-cap sub-ids (DL-50). The top/bottom caps of an
    /// extrude are not sketch boundary groups, so they get distinct sub-ids at
    /// the top of the *outer* sub range (just below [`INNER_WALL_FLAG`]). Wall
    /// groups are always small (`< edge_count ≪ 0x7FE`), so there is no
    /// collision.
    pub const EXTRUDE_CAP_TOP_SUB: u32 = 0x7FF;
    pub const EXTRUDE_CAP_BOTTOM_SUB: u32 = 0x7FE;

    /// Branch id for a whole feature (sub-index 0).
    pub fn from_feature(id: &FeatureId) -> Self {
        let base = id.as_u32_or_root().unwrap_or(0);
        Self(base << Self::SUB_BITS)
    }

    /// Branch id for sub-face `sub` of feature `id`.
    pub fn from_feature_sub(id: &FeatureId, sub: u32) -> Self {
        let base = id.as_u32_or_root().unwrap_or(0);
        Self((base << Self::SUB_BITS) | (sub & Self::SUB_MASK))
    }

    /// The flat feature id (sub-index stripped).
    pub fn feature_id(self) -> u32 {
        self.0 >> Self::SUB_BITS
    }

    /// The per-feature sub-index (the low [`SUB_BITS`](Self::SUB_BITS) bits,
    /// inner-wall flag included). For a sketch-sweep wall this is the G1
    /// boundary-group index; for a cap it is one of the reserved cap sub-ids.
    pub fn sub(self) -> u32 {
        self.0 & Self::SUB_MASK
    }

    /// Map this (outer) branch to its shell inner-wall counterpart (DL-75) by
    /// setting [`INNER_WALL_FLAG`](Self::INNER_WALL_FLAG). Idempotent; leaves
    /// `feature_id()` unchanged.
    pub fn inner_wall(self) -> Self {
        Self(self.0 | Self::INNER_WALL_FLAG)
    }

    /// True if this branch is a shell inner-wall face (DL-75).
    pub fn is_inner_wall(self) -> bool {
        self.0 & Self::INNER_WALL_FLAG != 0
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

/// Maximum number of branches that can simultaneously be "active" at a
/// single SDF evaluation. A `Vertex` of the B-Rep proxy needs three
/// (three faces meeting at a point); the per-face decomposition pushes
/// the worst case higher (a box corner is three faces of one
/// primitive), so we keep generous headroom while staying heap-free on
/// the hot evaluate path.
pub const MAX_CONTRIBUTORS: usize = 8;

/// Small inline, `Copy` set of branch ids that dominate a particular
/// SDF evaluation within the provenance epsilon. Ordering inside the
/// set is insertion order; duplicates are silently ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contributors {
    ids: [BranchId; MAX_CONTRIBUTORS],
    len: u8,
}

impl Contributors {
    pub fn empty() -> Self {
        Self {
            ids: [BranchId::default(); MAX_CONTRIBUTORS],
            len: 0,
        }
    }

    pub fn single(id: BranchId) -> Self {
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

    pub fn as_slice(&self) -> &[BranchId] {
        &self.ids[..self.len as usize]
    }

    pub fn contains(&self, id: &BranchId) -> bool {
        self.as_slice().iter().any(|x| x == id)
    }

    /// Insert `id` if not already present. Silently drops additions
    /// past `MAX_CONTRIBUTORS` — extremely high multiplicities are a
    /// sign of numerical trouble (too large an epsilon), not real
    /// geometry, and we prefer a stable upper bound to a panic.
    pub fn push(&mut self, id: BranchId) {
        if self.contains(&id) {
            return;
        }
        if (self.len as usize) < MAX_CONTRIBUTORS {
            self.ids[self.len as usize] = id;
            self.len += 1;
        }
    }

    pub fn extend(&mut self, other: Self) {
        for &id in other.as_slice() {
            self.push(id);
        }
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

    /// Collapse to per-*feature* granularity: every contained branch is
    /// reduced to its owning feature (sub-index dropped) and duplicates
    /// merged. The analytic per-face decomposition stays in
    /// [`expand_evaluate_leaf`]; the mesh-level proxy (DC/MDC/RFTA
    /// classification + the flat contributor buffer + the topology
    /// extractor) consumes this collapsed view, so a single box still
    /// reads as one Face-classified contributor rather than splitting
    /// into its six faces. The richer per-face ids are reserved for the
    /// B-Rep weaver, which reads `evaluate` directly.
    pub fn feature_collapsed(&self) -> Self {
        let mut out = Self::empty();
        for &b in self.as_slice() {
            out.push(BranchId(b.feature_id() << BranchId::SUB_BITS));
        }
        out
    }

    /// Remap every contained branch to its shell inner-wall counterpart
    /// (DL-75). [`BoundedCsg::Shell`]'s `evaluate` calls this for the
    /// inner-wall term `−(d + e)` so the cavity surface is classified as the
    /// offset inner face, not the outer face it was eroded from.
    pub fn mapped_inner(&self) -> Self {
        let mut out = Self::empty();
        for &b in self.as_slice() {
            out.push(b.inner_wall());
        }
        out
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

    pub fn intersect(a: Self, b: Self) -> Self {
        if a.is_empty() || b.is_empty() {
            return Self::EMPTY;
        }
        let result = Self {
            min: [
                a.min[0].max(b.min[0]),
                a.min[1].max(b.min[1]),
                a.min[2].max(b.min[2]),
            ],
            max: [
                a.max[0].min(b.max[0]),
                a.max[1].min(b.max[1]),
                a.max[2].min(b.max[2]),
            ],
        };
        if result.is_empty() { Self::EMPTY } else { result }
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
            // Helical sweep: radial extent like Revolve (plus taper
            // growth over the axial span), axial extent grows by one
            // pitch per turn (instance k advances k·pitch; within a
            // turn the helix adds up to one more pitch).
            Primitive::Helix { sketch, center, pitch, turns, taper, .. } => {
                let bb2 = sketch.aabb();
                if bb2.is_empty() {
                    return Self::EMPTY;
                }
                let axial = pitch.max(0.0) * (turns.max(0.0).ceil() + 1.0);
                let span_y = (bb2.max[1] - bb2.min[1]) + axial;
                let r_max =
                    bb2.max[0].abs().max(bb2.min[0].abs()) + taper.abs() * span_y;
                Self {
                    min: [
                        center[0] - r_max,
                        center[1] + bb2.min[1] - pitch.max(0.0),
                        center[2] - r_max,
                    ],
                    max: [
                        center[0] + r_max,
                        center[1] + bb2.max[1] + axial,
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
            // Gyroid is infinite — partitioning uses target_ids, not AABB.
            Primitive::Gyroid { .. } => Self::EMPTY,
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
            let scaled = [c[0] * t.scale[0], c[1] * t.scale[1], c[2] * t.scale[2]];
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
/// `from_features`. Each `Leaf` carries a stable [`FeatureId`] that
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

/// A B-Rep face removed from a [`CsgNode::Shell`] to open it.
///
/// `branch` is the face's [`BranchId`] (feature id + sub-face index): the
/// shell's cavity erosion measures the distance to the **retained** boundary —
/// the owning leaf's SDF re-evaluated with this sub-face removed — so the wall
/// keeps a constant `thickness` right up to the opened face and ends there in
/// a crisp annular rim (`wall = max(d, −(d_ret + t))`). The earlier gate-taper
/// formulation (`e = clamp(d − g, 0, t)`) let the wall thin to a knife edge at
/// the opening and is gone.
///
/// `surface`/`placement` keep the face's analytic geometry (local [`HalfSpace`]
/// + rigid local→world transform) for consumers that need the opening's shape
/// — e.g. the weave's open-face annulus classification.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenSurface {
    pub branch: u32,
    pub surface: HalfSpace,
    pub placement: Transform,
}

/// Stack-chained set of opened branch ids, threaded through SDF evaluation
/// without per-eval allocation. Each [`CsgNode::Shell`] pushes its own open
/// faces for the retained-boundary (`d_ret`) re-evaluation of its subtree;
/// nested shells chain their sets.
#[derive(Clone, Copy)]
pub(crate) struct OpenBranchSet<'a> {
    faces: &'a [OpenSurface],
    parent: Option<&'a OpenBranchSet<'a>>,
}

impl<'a> OpenBranchSet<'a> {
    pub(crate) const EMPTY: OpenBranchSet<'static> =
        OpenBranchSet { faces: &[], parent: None };

    pub(crate) fn push(parent: &'a OpenBranchSet<'a>, faces: &'a [OpenSurface]) -> Self {
        OpenBranchSet { faces, parent: Some(parent) }
    }

    /// Whether the chain holds no opened branches at all. Load-bearing for
    /// AABB pruning: a retained-boundary field extends past its leaf's AABB
    /// (the solid is "extended through the opening"), so the Lipschitz
    /// ≥ dist-to-AABB bound only holds when the set is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.faces.is_empty() && self.parent.is_none_or(|p| p.is_empty())
    }

    /// Whether any opened branch belongs to `feature` — the cheap gate the
    /// leaf arms check before switching to the retained evaluation.
    pub(crate) fn has_feature(&self, feature: u32) -> bool {
        self.faces.iter().any(|f| BranchId(f.branch).feature_id() == feature)
            || self.parent.is_some_and(|p| p.has_feature(feature))
    }

    /// Whether the exact branch (feature + sub) is opened.
    pub(crate) fn contains(&self, branch: u32) -> bool {
        self.faces.iter().any(|f| f.branch == branch)
            || self.parent.is_some_and(|p| p.contains(branch))
    }

    pub(crate) fn contains_sub(&self, feature: &FeatureId, sub: u32) -> bool {
        self.contains(BranchId::from_feature_sub(feature, sub).as_u32())
    }
}

impl OpenSurface {
    /// Outward-oriented surface gate `g(p)` (≤ 0 inside the body, 0 on the face),
    /// expressed as a *world* Euclidean distance. Maps `p` world→surface-local
    /// (rigid inverse, undoing the placement scale), evaluates the face's analytic
    /// [`HalfSpace`] there, then Eikonal-normalises the result by the world
    /// gradient magnitude `|S⁻¹∇f|` so the gate measures true world distance even
    /// when the body placement carries a (possibly non-uniform) scale. This is
    /// exact for a uniform scale `s` (gate is multiplied by `s`) and a first-order
    /// correction otherwise — keeping the shell taper well-conditioned (∇g ≈ 1)
    /// instead of stretching it by the local scale factors.
    #[inline]
    pub fn eval(&self, p: [f32; 3]) -> f32 {
        let t = &self.placement;
        let d = [p[0] - t.translate[0], p[1] - t.translate[1], p[2] - t.translate[2]];
        let conj = [-t.rotation[0], -t.rotation[1], -t.rotation[2], t.rotation[3]];
        let r = crate::quat_rotate(conj, d);
        let lp = [r[0] / t.scale[0], r[1] / t.scale[1], r[2] / t.scale[2]];
        let f = sdf_halfspace(&self.surface, lp);
        let grad = grad_halfspace(&self.surface, lp);
        let gn = ((grad[0] / t.scale[0]).powi(2)
            + (grad[1] / t.scale[1]).powi(2)
            + (grad[2] / t.scale[2]).powi(2))
        .sqrt();
        if gn > 1e-9 {
            f / gn
        } else {
            f
        }
    }
}

/// Inward shell SDF with retained-boundary openings (see [`CsgNode::Shell`]).
///
/// `d` is the solid's signed distance at `p`; `d_ret` is the same subtree
/// re-evaluated with every opened sub-face removed from its *distance* (sign
/// unchanged) — the distance to the **retained** boundary. The cavity is the
/// solid eroded by `thickness` measured against that retained boundary, so the
/// wall keeps a constant thickness right up to an opened face and meets it in
/// a crisp annular rim: `wall = max(d, −(d_ret + thickness))`. With no open
/// faces `d_ret == d` → the classic closed inward shell `max(d, −(d + t))`.
#[inline]
pub fn shell_wall_sdf(d: f32, d_ret: f32, thickness: f32) -> f32 {
    d.max(-(d_ret + thickness))
}

#[derive(Debug, Clone, PartialEq)]
pub enum CsgNode {
    Empty,
    Leaf {
        id: FeatureId,
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
    /// Shell: hollow the child into a thin wall of `thickness`, grown
    /// *inward* from the surface (`max(d, −(d + e))`), so the outer
    /// dimensions are preserved. `e` is the local erosion depth — normally
    /// `thickness`, but tapered to `0` near any `open_faces` so the cavity
    /// breaks through there, leaving an opening (the classic CAD shell with
    /// removed faces). The open faces may be planar or curved (any analytic
    /// [`OpenSurface`]). With no open faces this is a closed inward shell of
    /// uniform thickness. See [`open_shell_sdf`].
    Shell(Box<CsgNode>, f32, Vec<OpenSurface>),
    /// General SDF offset: `child.sdf(p) − δ`. A positive `δ` dilates the body
    /// (grows the surface outward along its normal by `δ`); a negative `δ`
    /// erodes it. Self-intersection-free by construction (a virtue of the SDF
    /// over a B-Rep offset). The single general primitive behind the
    /// field-based Thicken/Offset: a thin wall is `Difference(Offset(c, out),
    /// Offset(c, −in))`, and unlike the analytic per-primitive path it works on
    /// *any* carrier field (sweep, revolve, twist, boolean), with the offset
    /// surface still woven into selectable B-Rep faces (analytic carriers fold
    /// back into a shifted `HalfSpace`, curved ones fall to `Field::Leaf`).
    Offset(Box<CsgNode>, f32),
    /// K10.3 / DL-61: spatially-masked blend. Evaluates `a` and `b` at
    /// every point, then checks a polyline-tube mask SDF (defined by
    /// `mask_polyline`, `mask_radius`). Inside the mask:
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
        /// Polyline of the queried edge (in CSG/world eval space). The
        /// mask is the union of capsules along each consecutive segment.
        mask_polyline: Vec<[f32; 3]>,
        mask_radius: f32,
        blend_radius: f32,
        blend_mode: BlendMode,
        is_difference: bool,
    },
    /// Lift a 2D sketch into a 3D slab of `±half_height` along the
    /// local Z axis. The whole extruded region contributes a
    /// single [`FeatureId`] to the provenance set — sub-primitives
    /// inside the sketch don't get individual ids until the K5/K6
    /// sketcher gives them identities. Use the extrude's own `id`
    /// in the feature tree to trace back to the originating
    /// outliner entry.
    Extrude {
        id: FeatureId,
        sketch: Box<Sketch2d>,
        half_height: f32,
    },
    /// Revolve a 2D sketch around the local Y axis. The sketch's
    /// `x` coordinate maps to the radial distance
    /// `√(x² + z²)` in 3D, its `y` stays vertical. Authoring
    /// convention: sketch primitives should live at `x ≥ 0`;
    /// a primitive that crosses the axis of revolution produces
    /// a valid SDF but geometrically silly results (the axis
    /// sweeps through the centre). Single [`FeatureId`] in the
    /// contributor set, same rationale as Extrude.
    Revolve {
        id: FeatureId,
        sketch: Box<Sketch2d>,
    },
    /// Helical sweep of a 2D sketch around the local Y axis (see
    /// [`Primitive::Helix`] for the profile/turn semantics). Carries
    /// the derived `y_anchor` / `r_inner` so the hot eval never
    /// recomputes the sketch AABB. Single [`FeatureId`] contributor,
    /// same rationale as Extrude/Revolve.
    Helix {
        id: FeatureId,
        sketch: Box<Sketch2d>,
        pitch: f32,
        turns: f32,
        left_handed: bool,
        taper: f32,
        start_angle: f32,
        y_anchor: f32,
        r_inner: f32,
    },
}

/// DL-61 Stage B: the decoded runtime IR of one feature.
///
/// Where the retired fat-feature spine struct was a field-union over
/// every built-in, `DecodedFeature` is the **open** form both built-ins and
/// (future) FieldSkript converge on: a per-feature geometry subtree
/// (`node`) plus the universals every feature has — identity, combine
/// semantics (`op` + junction blend), targeting, and a separable
/// `transform` placement.
///
/// `node` holds the feature's geometry in its **own local frame**
/// (primitive, sketch lift, and any domain-warp/shell modifiers),
/// *without* the outer placement. Keeping `transform` separate lets the
/// body layer split the geometry hash from the transform hash (so a pure
/// move reuses the woven B-Rep, DL-56) and extract per-body placement.
/// The CSG fold re-applies the placement via [`DecodedFeature::placed_node`].
///
/// A FieldSkript feature emits `node` directly (it has no single
/// `primitive`), so this struct never needs a new field per feature type.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFeature {
    pub id: FeatureId,
    pub name: String,
    pub op: BoolOp,
    /// Junction blend radius in mm; 0 = hard boolean. `blend_mode`
    /// selects fillet vs. chamfer when this is > 0.
    pub blend_radius: f32,
    pub blend_mode: BlendMode,
    /// Explicit body-targeting (see `body::partition`). Empty = AABB
    /// overlap fallback.
    pub target_ids: Vec<FeatureId>,
    /// Separable affine placement of `node` in world space.
    pub transform: Transform,
    /// Geometry subtree in the feature's local frame (placement excluded).
    pub node: CsgNode,
}

/// Domain-warp / shell / blend modifiers a primitive feature can carry.
/// [`DecodedFeature::from_parts`] lowers the warps into the node tree
/// (Twist/Bend/Displace/Shell) and resolves the junction blend
/// (`blend_radius`/`chamfer_size`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PrimitiveModifiers {
    pub blend_radius: f32,
    pub chamfer_size: f32,
    pub twist_rate: f32,
    pub bend_rate: f32,
    pub displace_depth: f32,
    pub displace_freq: f32,
    pub shell_thickness: f32,
}

impl PrimitiveModifiers {
    /// A plain hard-boolean primitive with no warps or blend.
    pub const NONE: Self = Self {
        blend_radius: 0.0,
        chamfer_size: 0.0,
        twist_rate: 0.0,
        bend_rate: 0.0,
        displace_depth: 0.0,
        displace_freq: 0.0,
        shell_thickness: 0.0,
    };
}

impl DecodedFeature {
    /// Build a decoded feature from its flat construction parameters and
    /// lower it into the open IR. The primitive (with the extrude/revolve
    /// self-centering folded in) is wrapped in its domain-warp/shell
    /// modifiers in the **local** frame; `transform` is kept separate as
    /// the placement; the junction blend resolves with fillet
    /// (`blend_radius`) winning over chamfer (`chamfer_size`).
    ///
    /// This is the single lowering boundary from the flat feature
    /// parameter set to the open IR — built-ins call it from their
    /// `execute`, tests build fixtures with it.
    #[allow(clippy::too_many_arguments)] // geometry constructor; mods already bundled
    pub fn from_parts(
        id: FeatureId,
        name: String,
        op: BoolOp,
        primitive: Primitive,
        transform: Transform,
        mods: PrimitiveModifiers,
        target_ids: Vec<FeatureId>,
    ) -> Self {
        // K7.3 / K8.4: Extrude/Revolve lower to their dedicated nodes,
        // with the primitive's own `center` folded into an inner
        // Transform so the slab/profile sits at the local origin. Every
        // other primitive stays in the Leaf wrapper.
        let prim_node = match primitive {
            Primitive::Extrude {
                ref sketch,
                half_depth_z,
                center,
            } => {
                let centered = CsgNode::Extrude {
                    id: id.clone(),
                    sketch: Box::new(sketch.clone()),
                    half_height: half_depth_z,
                };
                if center == [0.0, 0.0, 0.0] {
                    centered
                } else {
                    CsgNode::Transform(
                        Transform {
                            translate: center,
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0],
                        },
                        Box::new(centered),
                    )
                }
            }
            Primitive::Revolve { ref sketch, center } => {
                let centered = CsgNode::Revolve {
                    id: id.clone(),
                    sketch: Box::new(sketch.clone()),
                };
                if center == [0.0, 0.0, 0.0] {
                    centered
                } else {
                    CsgNode::Transform(
                        Transform {
                            translate: center,
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0],
                        },
                        Box::new(centered),
                    )
                }
            }
            Primitive::Helix {
                ref sketch, center, pitch, turns, left_handed, taper, start_angle,
            } => {
                let (y_anchor, r_inner) = crate::helix_profile_anchor(sketch);
                let centered = CsgNode::Helix {
                    id: id.clone(),
                    sketch: Box::new(sketch.clone()),
                    pitch,
                    turns,
                    left_handed,
                    taper,
                    start_angle,
                    y_anchor,
                    r_inner,
                };
                if center == [0.0, 0.0, 0.0] {
                    centered
                } else {
                    CsgNode::Transform(
                        Transform {
                            translate: center,
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0],
                        },
                        Box::new(centered),
                    )
                }
            }
            other => CsgNode::Leaf {
                id: id.clone(),
                primitive: other,
            },
        };
        let mut node = prim_node;
        if mods.twist_rate != 0.0 {
            node = CsgNode::Twist(Box::new(node), mods.twist_rate);
        }
        if mods.bend_rate != 0.0 {
            node = CsgNode::Bend(Box::new(node), mods.bend_rate);
        }
        if mods.displace_depth != 0.0 && mods.displace_freq != 0.0 {
            node = CsgNode::Displace(Box::new(node), mods.displace_depth, mods.displace_freq);
        }
        if mods.shell_thickness > 0.0 {
            node = CsgNode::Shell(Box::new(node), mods.shell_thickness, Vec::new());
        }
        // Blend selection: fillet (blend_radius) wins over chamfer
        // (chamfer_size) when both are set.
        let (blend_radius, blend_mode) = if mods.blend_radius > 0.0 {
            (mods.blend_radius, BlendMode::Fillet)
        } else if mods.chamfer_size > 0.0 {
            (mods.chamfer_size, BlendMode::Chamfer)
        } else {
            (0.0, BlendMode::Fillet)
        };
        DecodedFeature {
            id,
            name,
            op,
            blend_radius,
            blend_mode,
            target_ids,
            transform,
            node,
        }
    }

    /// The feature's geometry re-wrapped in its placement: the world-space
    /// subtree the CSG fold combines.
    pub fn placed_node(&self) -> CsgNode {
        CsgNode::Transform(self.transform, Box::new(self.node.clone()))
    }

    /// True for the `shell` feature's sentinel node `Shell(Empty, …)` — a
    /// whole-body post-process rather than geometry. [`CsgNode::from_decoded`]
    /// folds it into an inward shell of its target body; the body-LOCAL B-Rep
    /// weave skips it (the shell's world-space open planes don't belong in the
    /// local frame, and the analytic B-Rep tracks the outer surface).
    pub fn is_shell_modifier(&self) -> bool {
        matches!(&self.node, CsgNode::Shell(inner, _, _) if matches!(**inner, CsgNode::Empty))
    }

    /// Reconstruct this feature's export `Primitive` from the local
    /// geometry node, peeling the domain-warp/shell modifiers. Sketch
    /// lifts come back as `Primitive::Extrude`/`Revolve` with their
    /// self-centering offset restored. `None` for composite (e.g. future
    /// FieldSkript) nodes that have no single primitive. The placement
    /// (`self.transform`) is applied separately by the caller.
    pub fn export_primitive(&self) -> Option<Primitive> {
        let mut cur = &self.node;
        loop {
            match cur {
                CsgNode::Twist(inner, _)
                | CsgNode::Bend(inner, _)
                | CsgNode::Displace(inner, _, _)
                | CsgNode::Offset(inner, _)
                | CsgNode::Shell(inner, _, _) => cur = inner,
                CsgNode::Leaf { primitive, .. } => return Some(primitive.clone()),
                CsgNode::Extrude {
                    sketch,
                    half_height,
                    ..
                } => {
                    return Some(Primitive::Extrude {
                        sketch: (**sketch).clone(),
                        half_depth_z: *half_height,
                        center: [0.0, 0.0, 0.0],
                    })
                }
                CsgNode::Revolve { sketch, .. } => {
                    return Some(Primitive::Revolve {
                        sketch: (**sketch).clone(),
                        center: [0.0, 0.0, 0.0],
                    })
                }
                CsgNode::Helix {
                    sketch, pitch, turns, left_handed, taper, start_angle, ..
                } => {
                    return Some(Primitive::Helix {
                        sketch: (**sketch).clone(),
                        center: [0.0, 0.0, 0.0],
                        pitch: *pitch,
                        turns: *turns,
                        left_handed: *left_handed,
                        taper: *taper,
                        start_angle: *start_angle,
                    })
                }
                // The extrude/revolve self-centering Transform: restore
                // its translate as the primitive's `center`.
                CsgNode::Transform(t, inner) => match &**inner {
                    CsgNode::Extrude {
                        sketch,
                        half_height,
                        ..
                    } => {
                        return Some(Primitive::Extrude {
                            sketch: (**sketch).clone(),
                            half_depth_z: *half_height,
                            center: t.translate,
                        })
                    }
                    CsgNode::Revolve { sketch, .. } => {
                        return Some(Primitive::Revolve {
                            sketch: (**sketch).clone(),
                            center: t.translate,
                        })
                    }
                    CsgNode::Helix {
                        sketch, pitch, turns, left_handed, taper, start_angle, ..
                    } => {
                        return Some(Primitive::Helix {
                            sketch: (**sketch).clone(),
                            center: t.translate,
                            pitch: *pitch,
                            turns: *turns,
                            left_handed: *left_handed,
                            taper: *taper,
                            start_angle: *start_angle,
                        })
                    }
                    _ => cur = inner,
                },
                _ => return None,
            }
        }
    }
}

impl CsgNode {
    /// Fold a decoded feature spine into a binary CSG tree.
    ///
    /// The first feature seeds the accumulator regardless of its op
    /// (the empty-space start means union/difference/intersection all
    /// degenerate to "take this feature as-is"). Each subsequent feature
    /// combines with the accumulator via its own op, honouring per-feature
    /// junction blend and explicit body-targeting.
    ///
    /// Each leaf inherits the feature's id as its `FeatureId`, so
    /// contributor analysis can always name the original outliner row
    /// that produced a face/edge/vertex.
    pub fn from_decoded(features: &[DecodedFeature]) -> Self {
        let mut acc = Self::Empty;
        for f in features {
            // A body-shell modifier (the `shell` feature) is a post-process,
            // not geometry: it carries the sentinel node `Shell(Empty, t, open)`
            // and wraps the *already-accumulated* subtree of its target body in
            // an inward shell. Intercept it before the boolean fold — its
            // `Difference` op only serves `body::partition` (to dock it into the
            // target body), never the CSG combine.
            if let CsgNode::Shell(inner, thickness, open) = &f.node {
                if matches!(**inner, CsgNode::Empty) {
                    acc = wrap_shell(acc, &f.target_ids, *thickness, open);
                    continue;
                }
            }
            let leaf = f.placed_node();
            let blend_r = f.blend_radius;
            let blend_mode = f.blend_mode;
            acc = if !f.target_ids.is_empty() && !matches!(acc, Self::Empty) {
                wrap_targeted(acc, &f.target_ids, leaf, f.op, blend_r, blend_mode)
            } else {
                match (acc, f.op) {
                    (Self::Empty, _) => leaf,
                    (a, op) if op.is_union() && blend_r > 0.0 => {
                        Self::SmoothUnion(Box::new(a), Box::new(leaf), blend_r, blend_mode)
                    }
                    (a, op) if op.is_union() => Self::Union(Box::new(a), Box::new(leaf)),
                    (a, BoolOp::Remove) if blend_r > 0.0 => {
                        Self::SmoothDifference(Box::new(a), Box::new(leaf), blend_r, blend_mode)
                    }
                    (a, BoolOp::Remove) => Self::Difference(Box::new(a), Box::new(leaf)),
                    (a, _) => Self::Intersection(Box::new(a), Box::new(leaf)),
                }
            };
        }
        acc
    }

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
                        mask_polyline: f.mask_polyline.clone(),
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
                        mask_polyline: f.mask_polyline.clone(),
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
            CsgNode::Shell(inner, t, open) => {
                CsgNode::Shell(Box::new(inner.apply_edge_fillets(fillets)), t, open)
            }
            CsgNode::Offset(inner, delta) => {
                CsgNode::Offset(Box::new(inner.apply_edge_fillets(fillets)), delta)
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
            | CsgNode::Shell(inner, _, _)
            | CsgNode::Offset(inner, _)
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
            CsgNode::Extrude { .. } | CsgNode::Revolve { .. } | CsgNode::Helix { .. } | CsgNode::Empty => {}
        }
    }

    /// Collect all leaf FeatureIds reachable from this node.
    fn contains_any_target(&self, target_ids: &[FeatureId]) -> bool {
        match self {
            CsgNode::Leaf { id, .. } | CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } | CsgNode::Helix { id, .. } => {
                let root = id.root_ancestor();
                target_ids.iter().any(|tid| root == tid)
            }
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Shell(inner, _, _)
            | CsgNode::Offset(inner, _)
            | CsgNode::Displace(inner, _, _) => inner.contains_any_target(target_ids),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b)
            | CsgNode::SmoothUnion(a, b, _, _)
            | CsgNode::SmoothDifference(a, b, _, _)
            | CsgNode::Morph(a, b, _) => {
                a.contains_any_target(target_ids) || b.contains_any_target(target_ids)
            }
            CsgNode::MaskedBlend { a, b, .. } => {
                a.contains_any_target(target_ids) || b.contains_any_target(target_ids)
            }
            CsgNode::Empty => false,
        }
    }

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
            | CsgNode::Shell(inner, _, _)
            | CsgNode::Offset(inner, _)
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
            CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } | CsgNode::Helix { id, .. } => {
                out.push(id.clone());
            }
            CsgNode::Empty => {}
        }
    }

    /// Evaluate the signed distance to this CSG tree at `p`. Matches
    /// an op-by-op fold of the same feature spine.
    ///
    /// `mode` is threaded through for future Ghost-Topology dual-path
    /// support (see [`EvalMode`]). Until K10 lands both modes return
    /// bit-identical values.
    pub fn sdf(&self, p: [f32; 3], mode: EvalMode) -> f32 {
        self.sdf_open(p, mode, &OpenBranchSet::EMPTY)
    }

    /// [`Self::sdf`] under a set of opened shell branches: leaves owning an
    /// opened sub-face evaluate their RETAINED boundary distance instead
    /// (sign unchanged) — see [`shell_wall_sdf`].
    pub(crate) fn sdf_open(&self, p: [f32; 3], mode: EvalMode, open: &OpenBranchSet<'_>) -> f32 {
        match self {
            CsgNode::Empty => f32::INFINITY,
            CsgNode::Leaf { id, primitive } => {
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    leaf_sdf_retained(id, primitive, p, open)
                } else {
                    sdf_primitive(primitive, p)
                }
            }
            CsgNode::Transform(t, inner) => {
                let local = world_to_local(t, p);
                inner.sdf_open(local, mode, open) * t.min_scale()
            }
            CsgNode::Union(a, b) => a.sdf_open(p, mode, open).min(b.sdf_open(p, mode, open)),
            CsgNode::Difference(a, b) => a.sdf_open(p, mode, open).max(-b.sdf_open(p, mode, open)),
            CsgNode::Intersection(a, b) => a.sdf_open(p, mode, open).max(b.sdf_open(p, mode, open)),
            CsgNode::SmoothUnion(a, b, r, mode_blend) => {
                let da = a.sdf_open(p, mode, open);
                let db = b.sdf_open(p, mode, open);
                match mode {
                    EvalMode::Visual => crate::csg::blend_min(da, db, *r, *mode_blend),
                    EvalMode::Logical => da.min(db),
                }
            }
            CsgNode::SmoothDifference(a, b, r, mode_blend) => {
                let da = a.sdf_open(p, mode, open);
                let db = b.sdf_open(p, mode, open);
                match mode {
                    EvalMode::Visual => -crate::csg::blend_min(-da, db, *r, *mode_blend),
                    EvalMode::Logical => da.max(-db),
                }
            }
            CsgNode::Twist(inner, rate) => {
                let angle = *rate * p[1];
                let (s, c) = angle.sin_cos();
                let twisted = [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
                inner.sdf_open(twisted, mode, open)
            }
            CsgNode::Bend(inner, rate) => {
                let angle = *rate * p[0];
                let (s, c) = angle.sin_cos();
                let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
                inner.sdf_open(bent, mode, open)
            }
            CsgNode::Displace(inner, depth, freq) => {
                let d = inner.sdf_open(p, mode, open);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                d - disp
            }
            CsgNode::Shell(inner, thickness, faces) => {
                let d = inner.sdf_open(p, mode, open);
                let d_ret = if faces.is_empty() {
                    d
                } else {
                    let merged = OpenBranchSet::push(open, faces);
                    inner.sdf_open(p, mode, &merged)
                };
                shell_wall_sdf(d, d_ret, *thickness)
            }
            CsgNode::Offset(inner, delta) => inner.sdf_open(p, mode, open) - delta,
            CsgNode::Morph(a, b, t) => {
                let da = a.sdf_open(p, mode, open);
                let db = b.sdf_open(p, mode, open);
                (1.0 - t) * da + t * db
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
                let da = a.sdf_open(p, mode, open);
                let db_raw = b.sdf_open(p, mode, open);
                match mode {
                    EvalMode::Logical => hard_combine(*is_difference, da, db_raw),
                    EvalMode::Visual => {
                        let mask_d = sdf_polyline_tube(p, mask_polyline, *mask_radius);
                        if mask_d < 0.0 {
                            if *is_difference {
                                -blend_min(-da, db_raw, *blend_radius, *blend_mode)
                            } else {
                                blend_min(da, db_raw, *blend_radius, *blend_mode)
                            }
                        } else {
                            hard_combine(*is_difference, da, db_raw)
                        }
                    }
                }
            }
            CsgNode::Extrude {
                id,
                sketch,
                half_height,
            } => {
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let d2 = sketch
                        .sdf_retained([p[0], p[1]], &mut |g| open.contains_sub(id, g));
                    extrude_retained_combine(id, d2, p[2], *half_height, open)
                } else {
                    sdf_extrude(sketch.sdf([p[0], p[1]]), p[2], *half_height)
                }
            }
            CsgNode::Revolve { id, sketch } => {
                let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_retained([r, p[1]], &mut |g| open.contains_sub(id, g))
                } else {
                    sketch.sdf([r, p[1]])
                }
            }
            CsgNode::Helix {
                id, sketch, pitch, turns, left_handed, taper, start_angle,
                y_anchor, r_inner,
            } => {
                let (q, ell, scale) = crate::helix_profile_point(
                    p, *pitch, *turns, *left_handed, *taper, *y_anchor, *r_inner,
                    *start_angle,
                );
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_retained(q, &mut |g| open.contains_sub(id, g))
                } else {
                    sketch.sdf(q)
                };
                crate::helix_compose(d, ell) * scale
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
        self.evaluate_open(p, eps, mode, &OpenBranchSet::EMPTY)
    }

    /// [`Self::evaluate`] under a set of opened shell branches — the
    /// provenance sibling of [`Self::sdf_open`]: opened sub-faces are removed
    /// from both the distance and the contributor band.
    pub(crate) fn evaluate_open(
        &self,
        p: [f32; 3],
        eps: f32,
        mode: EvalMode,
        open: &OpenBranchSet<'_>,
    ) -> (f32, Contributors) {
        match self {
            CsgNode::Empty => (f32::INFINITY, Contributors::empty()),
            CsgNode::Leaf { id, primitive } => {
                expand_evaluate_leaf_retained(id, primitive, p, eps, open)
            }
            CsgNode::Transform(t, inner) => {
                let local = world_to_local(t, p);
                let (d, c) = inner.evaluate_open(local, eps, mode, open);
                (d * t.min_scale(), c)
            }
            CsgNode::Union(a, b) => {
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db, cb) = b.evaluate_open(p, eps, mode, open);
                combine_min(da, ca, db, cb, eps)
            }
            CsgNode::Difference(a, b) => {
                // result = max(da, -db). Sign flip on `db` does NOT
                // change the contributor identity — it still names
                // the subtracted leaf.
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db, cb) = b.evaluate_open(p, eps, mode, open);
                combine_max(da, ca, -db, cb, eps)
            }
            CsgNode::Intersection(a, b) => {
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db, cb) = b.evaluate_open(p, eps, mode, open);
                combine_max(da, ca, db, cb, eps)
            }
            CsgNode::SmoothUnion(a, b, r, mode_blend) => {
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db, cb) = b.evaluate_open(p, eps, mode, open);
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
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db, cb) = b.evaluate_open(p, eps, mode, open);
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
                inner.evaluate_open(twisted, eps, mode, open)
            }
            CsgNode::Bend(inner, rate) => {
                let angle = *rate * p[0];
                let (s, c) = angle.sin_cos();
                let bent = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
                inner.evaluate_open(bent, eps, mode, open)
            }
            CsgNode::Displace(inner, depth, freq) => {
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                let disp = *depth
                    * (p[0] * freq).sin()
                    * (p[1] * freq * 1.3).sin()
                    * (p[2] * freq * 0.7).sin();
                (d - disp, c)
            }
            CsgNode::Shell(inner, thickness, faces) => {
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                let (d_ret, c_ret) = if faces.is_empty() {
                    (d, c)
                } else {
                    let merged = OpenBranchSet::push(open, faces);
                    inner.evaluate_open(p, eps, mode, &merged)
                };
                // Shell is `max(d, −(d_ret + t))`: a hard combine_max of the
                // outer surface (term `d`) against the inner wall (the
                // RETAINED faces offset inward → inner-wall contributors,
                // DL-75). Opened faces never gain an inner wall.
                combine_max(d, c, -(d_ret + *thickness), c_ret.mapped_inner(), eps)
            }
            CsgNode::Offset(inner, delta) => {
                // The offset preserves the carrier's contributor identity — the
                // surface just shifts by `δ` along its normal — so the woven face
                // keeps the carrier's branch id and stays selectable.
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                (d - delta, c)
            }
            CsgNode::Morph(a, b, t) => {
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db, cb) = b.evaluate_open(p, eps, mode, open);
                let d = (1.0 - t) * da + t * db;
                let mut c = ca;
                c.extend(cb);
                (d, c)
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
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db_raw, cb) = b.evaluate_open(p, eps, mode, open);
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
                        let mask_d = sdf_polyline_tube(p, mask_polyline, *mask_radius);
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
                // Per-segment identity (DL-46) + cap split (DL-50) +
                // eps-banded multi-contributors (DL-65): the sketch band
                // reports every boundary group within `eps`, and the cap
                // term `|z| − hh` joins when it ties — so a genuine extrude
                // edge names both colliding branches (uniform with analytic
                // CSG; the both-branches gate no longer needs to special-
                // case sweeps).
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let band = sketch.sdf_with_edges_retained([p[0], p[1]], eps, &mut |g| {
                        open.contains_sub(id, g)
                    });
                    let d = extrude_retained_combine(id, band.distance, p[2], *half_height, open);
                    let c = extrude_contributors_retained(
                        id,
                        &band,
                        p[2],
                        *half_height,
                        eps,
                        open,
                    );
                    (d, c)
                } else {
                    let band = sketch.sdf_with_edges([p[0], p[1]], eps);
                    let d = sdf_extrude(band.distance, p[2], *half_height);
                    let c = extrude_contributors(id, &band, p[2], *half_height, eps);
                    (d, c)
                }
            }
            CsgNode::Revolve { id, sketch } => {
                let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained([r, p[1]], eps, &mut |g| {
                        open.contains_sub(id, g)
                    })
                } else {
                    sketch.sdf_with_edges([r, p[1]], eps)
                };
                let c = revolve_contributors(id, &band);
                (band.distance, c)
            }
            CsgNode::Helix {
                id, sketch, pitch, turns, left_handed, taper, start_angle,
                y_anchor, r_inner,
            } => {
                let (q, ell, scale) = crate::helix_profile_point(
                    p, *pitch, *turns, *left_handed, *taper, *y_anchor, *r_inner,
                    *start_angle,
                );
                // The contributor band epsilon lives in WORLD units; widen
                // it by the inverse Lipschitz scale so profile-space edge
                // matching stays comparable.
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained(q, eps / scale.max(1e-6), &mut |g| {
                        open.contains_sub(id, g)
                    })
                } else {
                    sketch.sdf_with_edges(q, eps / scale.max(1e-6))
                };
                let c = revolve_contributors(id, &band);
                (crate::helix_compose(band.distance, ell) * scale, c)
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
            // Inward shell ⊆ the solid (the outer surface is preserved), so
            // the bounds are exactly the child's — no expansion.
            CsgNode::Shell(inner, _thickness, _open) => inner.aabb(),
            // A `δ`-dilation grows the bounds by `δ`; an erosion shrinks them.
            // Expanding by `|δ|` is a conservative superset for either sign.
            CsgNode::Offset(inner, delta) => {
                let bb = inner.aabb();
                if bb.is_empty() {
                    return Aabb::EMPTY;
                }
                let e = delta.abs();
                Aabb {
                    min: [bb.min[0] - e, bb.min[1] - e, bb.min[2] - e],
                    max: [bb.max[0] + e, bb.max[1] + e, bb.max[2] + e],
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
            CsgNode::Helix { sketch, pitch, turns, taper, .. } => {
                let s = sketch.aabb();
                if s.is_empty() {
                    Aabb::EMPTY
                } else {
                    // Radial extent like Revolve, plus the taper shear's
                    // growth over the axial span; axially the instances
                    // advance one pitch per turn (+1 pitch of slack for
                    // the in-turn helical rise on both ends).
                    let axial = pitch.max(0.0) * (turns.max(0.0).ceil() + 1.0);
                    let span_y = (s.max[1] - s.min[1]) + axial;
                    let r_max =
                        s.max[0].abs().max(s.min[0].abs()) + taper.abs() * span_y;
                    Aabb {
                        min: [-r_max, s.min[1] - pitch.max(0.0), -r_max],
                        max: [r_max, s.max[1] + axial, r_max],
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

/// Retained-boundary combine for an extrude leaf: the 2D retained field
/// joined with whichever caps are NOT opened. A plain `max` (interior-exact;
/// the exact-corner blend of [`sdf_extrude`] only matters outside, where the
/// shell erosion never reads `d_ret`).
pub(crate) fn extrude_retained_combine(
    id: &FeatureId,
    d2_ret: f32,
    z: f32,
    half_height: f32,
    open: &OpenBranchSet<'_>,
) -> f32 {
    let mut d = d2_ret;
    if !open.contains_sub(id, BranchId::EXTRUDE_CAP_TOP_SUB) {
        d = d.max(z - half_height);
    }
    if !open.contains_sub(id, BranchId::EXTRUDE_CAP_BOTTOM_SUB) {
        d = d.max(-z - half_height);
    }
    d
}

/// Contributor band for an **extrude** point (DL-50 / DL-65). The extrude
/// SDF is `max(d2, |z| − hh)`, so the nearest surface is whichever term
/// ties the max within `eps`. A wall tie emits *every* banded sketch
/// boundary group (so a sketch corner reports both adjoining walls); a
/// cap tie emits the reserved top/bottom cap sub-id. At a genuine extrude
/// edge — a wall∩wall vertical edge or a wall∩cap rim — this returns
/// **both** colliding branches, exactly like the analytic half-space band
/// in [`expand_evaluate_leaf`], so the B-Rep both-branches gate sees a
/// real edge instead of a singleton. `z`/`half_height` are in the
/// canonical (centred) local extrude frame, so the cap split is
/// placement-independent.
pub(crate) fn extrude_contributors(
    id: &FeatureId,
    band: &SketchEdgeBand,
    z: f32,
    half_height: f32,
    eps: f32,
) -> Contributors {
    let wall = band.distance;
    let cap = z.abs() - half_height;
    let d_max = wall.max(cap);
    let mut c = Contributors::empty();
    if (wall - d_max).abs() <= eps {
        for &g in band.groups() {
            c.push(BranchId::from_feature_sub(id, g));
        }
    }
    if (cap - d_max).abs() <= eps {
        let sub = if z >= 0.0 {
            BranchId::EXTRUDE_CAP_TOP_SUB
        } else {
            BranchId::EXTRUDE_CAP_BOTTOM_SUB
        };
        c.push(BranchId::from_feature_sub(id, sub));
    }
    // Degenerate guard: never return an empty set — name the dominant
    // wall group so the point still has a provenance.
    if c.is_empty() {
        let g = band.groups().first().copied().unwrap_or(0);
        c.push(BranchId::from_feature_sub(id, g));
    }
    c
}

/// Retained sibling of [`extrude_contributors`]: opened caps neither enter
/// the max nor the band (the side band is already retained-filtered by the
/// caller).
pub(crate) fn extrude_contributors_retained(
    id: &FeatureId,
    band: &SketchEdgeBand,
    z: f32,
    half_height: f32,
    eps: f32,
    open: &OpenBranchSet<'_>,
) -> Contributors {
    let wall = band.distance;
    let top = if open.contains_sub(id, BranchId::EXTRUDE_CAP_TOP_SUB) {
        f32::NEG_INFINITY
    } else {
        z - half_height
    };
    let bot = if open.contains_sub(id, BranchId::EXTRUDE_CAP_BOTTOM_SUB) {
        f32::NEG_INFINITY
    } else {
        -z - half_height
    };
    let d_max = wall.max(top).max(bot);
    let mut c = Contributors::empty();
    if (wall - d_max).abs() <= eps {
        for &g in band.groups() {
            c.push(BranchId::from_feature_sub(id, g));
        }
    }
    if top.is_finite() && (top - d_max).abs() <= eps {
        c.push(BranchId::from_feature_sub(id, BranchId::EXTRUDE_CAP_TOP_SUB));
    }
    if bot.is_finite() && (bot - d_max).abs() <= eps {
        c.push(BranchId::from_feature_sub(id, BranchId::EXTRUDE_CAP_BOTTOM_SUB));
    }
    if c.is_empty() {
        let g = band.groups().first().copied().unwrap_or(0);
        c.push(BranchId::from_feature_sub(id, g));
    }
    c
}

/// Contributor band for a **revolve** point (DL-65). A revolve has no
/// caps, so every banded sketch boundary group is a wall contributor; a
/// sketch corner therefore reports both adjoining walls and the
/// both-branches gate sees the revolved edge.
pub(crate) fn revolve_contributors(id: &FeatureId, band: &SketchEdgeBand) -> Contributors {
    let mut c = Contributors::empty();
    for &g in band.groups() {
        c.push(BranchId::from_feature_sub(id, g));
    }
    if c.is_empty() {
        c.push(BranchId::from_feature_sub(id, 0));
    }
    c
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

/// Recursively descend through Union/SmoothUnion nodes in `tree`,
/// applying `op(subtree, wrapper)` only to the branch(es) that
/// contain a leaf whose root_ancestor matches one of `target_ids`.
/// Non-target branches pass through unchanged.
/// Wrap the minimal subtree containing *all* of `target_ids` (one body) in an
/// inward [`CsgNode::Shell`]. Descends through the body-separating unions while
/// exactly one side holds the body, then wraps where both sides do (the body
/// root) or at any non-union node — mirroring [`wrap_targeted`]'s descent, but
/// replacing the subtree with its shell rather than booleaning a wrapper onto
/// it. Used by [`CsgNode::from_decoded`] for the `shell` feature.
fn wrap_shell(
    tree: CsgNode,
    target_ids: &[FeatureId],
    thickness: f32,
    open: &[OpenSurface],
) -> CsgNode {
    if !tree.contains_any_target(target_ids) {
        return tree;
    }
    match tree {
        CsgNode::Union(a, b) => {
            let a_has = a.contains_any_target(target_ids);
            let b_has = b.contains_any_target(target_ids);
            match (a_has, b_has) {
                (true, false) => {
                    CsgNode::Union(Box::new(wrap_shell(*a, target_ids, thickness, open)), b)
                }
                (false, true) => {
                    CsgNode::Union(a, Box::new(wrap_shell(*b, target_ids, thickness, open)))
                }
                _ => CsgNode::Shell(Box::new(CsgNode::Union(a, b)), thickness, open.to_vec()),
            }
        }
        CsgNode::SmoothUnion(a, b, r, m) => {
            let a_has = a.contains_any_target(target_ids);
            let b_has = b.contains_any_target(target_ids);
            match (a_has, b_has) {
                (true, false) => CsgNode::SmoothUnion(
                    Box::new(wrap_shell(*a, target_ids, thickness, open)),
                    b,
                    r,
                    m,
                ),
                (false, true) => CsgNode::SmoothUnion(
                    a,
                    Box::new(wrap_shell(*b, target_ids, thickness, open)),
                    r,
                    m,
                ),
                _ => CsgNode::Shell(
                    Box::new(CsgNode::SmoothUnion(a, b, r, m)),
                    thickness,
                    open.to_vec(),
                ),
            }
        }
        // Difference / Intersection / etc. keep a body together — never split
        // them to shell only the base. Wrap the whole node.
        other => CsgNode::Shell(Box::new(other), thickness, open.to_vec()),
    }
}

fn wrap_targeted(
    tree: CsgNode,
    target_ids: &[FeatureId],
    wrapper: CsgNode,
    op: BoolOp,
    blend_r: f32,
    blend_mode: BlendMode,
) -> CsgNode {
    if !tree.contains_any_target(target_ids) {
        return tree;
    }
    match tree {
        CsgNode::Union(a, b) => {
            let a_has = a.contains_any_target(target_ids);
            let b_has = b.contains_any_target(target_ids);
            match (a_has, b_has) {
                (true, false) => CsgNode::Union(
                    Box::new(wrap_targeted(*a, target_ids, wrapper, op, blend_r, blend_mode)),
                    b,
                ),
                (false, true) => CsgNode::Union(
                    a,
                    Box::new(wrap_targeted(*b, target_ids, wrapper, op, blend_r, blend_mode)),
                ),
                _ => apply_op(CsgNode::Union(a, b), wrapper, op, blend_r, blend_mode),
            }
        }
        CsgNode::SmoothUnion(a, b, r, m) => {
            let a_has = a.contains_any_target(target_ids);
            let b_has = b.contains_any_target(target_ids);
            match (a_has, b_has) {
                (true, false) => CsgNode::SmoothUnion(
                    Box::new(wrap_targeted(*a, target_ids, wrapper, op, blend_r, blend_mode)),
                    b, r, m,
                ),
                (false, true) => CsgNode::SmoothUnion(
                    a,
                    Box::new(wrap_targeted(*b, target_ids, wrapper, op, blend_r, blend_mode)),
                    r, m,
                ),
                _ => apply_op(CsgNode::SmoothUnion(a, b, r, m), wrapper, op, blend_r, blend_mode),
            }
        }
        other => apply_op(other, wrapper, op, blend_r, blend_mode),
    }
}

fn apply_op(
    tree: CsgNode,
    operand: CsgNode,
    op: BoolOp,
    blend_r: f32,
    blend_mode: BlendMode,
) -> CsgNode {
    match op {
        op if op.is_union() && blend_r > 0.0 => {
            CsgNode::SmoothUnion(Box::new(tree), Box::new(operand), blend_r, blend_mode)
        }
        op if op.is_union() => CsgNode::Union(Box::new(tree), Box::new(operand)),
        BoolOp::Remove if blend_r > 0.0 => {
            CsgNode::SmoothDifference(Box::new(tree), Box::new(operand), blend_r, blend_mode)
        }
        BoolOp::Remove => CsgNode::Difference(Box::new(tree), Box::new(operand)),
        _ => CsgNode::Intersection(Box::new(tree), Box::new(operand)),
    }
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
        let a_in_left = left_ids.contains(&f.feature_a);
        let b_in_right = right_ids.contains(&f.feature_b);
        let a_in_right = right_ids.contains(&f.feature_a);
        let b_in_left = left_ids.contains(&f.feature_b);
        (a_in_left && b_in_right) || (a_in_right && b_in_left)
    })
}

/// K10.4 / DL-61: per-edge fillet specification. Identifies a Boolean
/// junction by the feature IDs of its two operands (derived from the
/// queried edge's branch-pair signature) and places a polyline-tube mask
/// along the edge's actual geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeFillet {
    /// Feature id of the "left" operand at the junction.
    pub feature_a: FeatureId,
    /// Feature id of the "right" operand.
    pub feature_b: FeatureId,
    /// The queried edge's polyline in CSG/world eval space. The mask is
    /// the union of capsules along consecutive segments.
    pub mask_polyline: Vec<[f32; 3]>,
    /// Mask radius — how far the tube extends from the edge. Derived
    /// from `blend_radius` (see `FILLET_MASK_SCALE`), not a user param.
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

/// DL-61: polyline-tube SDF for spatial masks. The mask is the union
/// (min) of [`sdf_capsule_3d`] over each consecutive segment of
/// `polyline`. Degenerate cases:
/// - `len >= 2` → min over segments (a 2-point polyline reduces exactly
///   to a single capsule).
/// - `len == 1` → a zero-length capsule, i.e. a sphere of `radius` at
///   that point.
/// - empty → `+INFINITY` (the mask is never active, so the MaskedBlend
///   falls through to the hard combine everywhere).
pub(crate) fn sdf_polyline_tube(p: [f32; 3], polyline: &[[f32; 3]], radius: f32) -> f32 {
    match polyline.len() {
        0 => f32::INFINITY,
        1 => sdf_capsule_3d(p, polyline[0], polyline[0], radius),
        _ => polyline
            .windows(2)
            .map(|seg| sdf_capsule_3d(p, seg[0], seg[1], radius))
            .fold(f32::INFINITY, f32::min),
    }
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

/// The hard (non-blended) masked-blend combine: subtraction (`da ∖ db`) or
/// union (`da ∪ db`). Shared by the analytic SDF and the opcode interpreter.
#[inline]
pub(crate) fn hard_combine(is_difference: bool, da: f32, db: f32) -> f32 {
    if is_difference {
        da.max(-db)
    } else {
        da.min(db)
    }
}

pub(crate) fn chamfer_union(da: f32, db: f32, r: f32) -> f32 {
    use std::f32::consts::FRAC_1_SQRT_2;
    da.min(db).min((da + db - r) * FRAC_1_SQRT_2)
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

/// One sub-face of a primitive's half-space decomposition: the local
/// sub-index (matching the `sub` of [`BranchId::from_feature_sub`]) and the
/// [`HalfSpace`] surface it lies on. The half-space is expressed in the
/// primitive's *centred* local frame (i.e. after subtracting the primitive
/// `center`, the same `lp` the SDF tie-break is evaluated at).
///
/// `local_offset` is an extra per-face pre-translation (in the centred local
/// frame) that moves the half-space's analytic *origin* to where its SDF
/// expects it. It is zero for every face whose surface is centred on the
/// primitive origin (all planes, cylinder, sphere, torus). The one exception
/// is a [`HalfSpace::Cone`] wall: its SDF (`r·cosα − |y|·sinα`) puts the apex
/// at the transform origin, but the frustum's true apex sits up the axis at
/// `y_apex`, so the wall carries `[0, y_apex, 0]` (DL-76). The weaver folds
/// this into the face transform; the hot SDF tie-break (which evaluates the
/// truncated-cone formula, not the half-space) ignores it.
#[derive(Debug, Clone)]
pub(crate) struct HalfSpaceFace {
    pub sub: u32,
    pub half_space: HalfSpace,
    pub local_offset: [f32; 3],
}

/// Fixed-capacity (no-heap) result of [`primitive_halfspaces`]: the primitive
/// `center` offset that maps the centred local frame to the leaf-local frame
/// (`lp = p - center`), plus up to six [`HalfSpaceFace`]s.
#[derive(Debug, Clone)]
pub(crate) struct PrimitiveHalfSpaces {
    // Consumed by the B-Rep weaver (SA-0b) to map the centred local
    // frame back to leaf-local space; the hot SDF path subtracts
    // `center` itself, so it is currently unread inside this crate.
    #[allow(dead_code)]
    pub center: [f32; 3],
    faces: [HalfSpaceFace; 6],
    len: usize,
}

impl PrimitiveHalfSpaces {
    pub fn faces(&self) -> &[HalfSpaceFace] {
        &self.faces[..self.len]
    }
}

/// Enumerate the analytic half-space sub-faces of a primitive — the single
/// **source of truth** for the per-face branch decomposition. Both
/// [`expand_evaluate_leaf`] (the hot SDF path, for branch tie-breaking) and the
/// B-Rep weaver consume this so the two never drift apart.
///
/// Each face's half-space is given in the primitive's centred local frame; the
/// returned `center` is the offset to undo it (`lp = p - center`). Returns
/// `None` for primitives with no closed-form half-space decomposition (Sphere
/// is a single un-split surface, Torus/Gyroid/sketch sweeps have no per-face
/// planar split here).
///
/// NOTE: keep in sync with [`expand_evaluate_leaf`] — `Box`/`Cylinder` consume
/// this directly, so they cannot drift. `Cone` is enumerated here for the
/// weaver (caps as planes, wall as [`HalfSpace::Cone`]) but its hot-path wall
/// distance uses a *truncated*-cone formula (`r - r_at_y`) that differs from the
/// infinite-apex [`HalfSpace::Cone`] SDF; the cube/analytic-plane weaving path
/// is unaffected. The cone wall's [`HalfSpaceFace::local_offset`] carries the
/// apex shift the weaver needs (DL-76).
pub(crate) fn primitive_halfspaces(prim: &Primitive) -> Option<PrimitiveHalfSpaces> {
    let plane = |normal: [f32; 3], offset: f32| HalfSpace::Plane { normal, offset };
    let mk = |center: [f32; 3], list: &[(u32, HalfSpace)]| {
        let mut faces = std::array::from_fn(|_| HalfSpaceFace {
            sub: 0,
            half_space: HalfSpace::Sphere { radius: 0.0 },
            local_offset: [0.0, 0.0, 0.0],
        });
        for (slot, (sub, half_space)) in faces.iter_mut().zip(list.iter()) {
            *slot = HalfSpaceFace {
                sub: *sub,
                half_space: half_space.clone(),
                local_offset: [0.0, 0.0, 0.0],
            };
        }
        PrimitiveHalfSpaces { center, faces, len: list.len() }
    };
    match prim {
        Primitive::Box { center, half_extents } => {
            let [hx, hy, hz] = *half_extents;
            Some(mk(
                *center,
                &[
                    (0, plane([1.0, 0.0, 0.0], hx)),
                    (1, plane([-1.0, 0.0, 0.0], hx)),
                    (2, plane([0.0, 1.0, 0.0], hy)),
                    (3, plane([0.0, -1.0, 0.0], hy)),
                    (4, plane([0.0, 0.0, 1.0], hz)),
                    (5, plane([0.0, 0.0, -1.0], hz)),
                ],
            ))
        }
        Primitive::Cylinder { center, radius, half_height } => Some(mk(
            *center,
            &[
                (0, HalfSpace::InfiniteCylinder { radius: *radius }),
                (1, plane([0.0, 1.0, 0.0], *half_height)),
                (2, plane([0.0, -1.0, 0.0], *half_height)),
            ],
        )),
        // A sphere is ONE closed analytic face (no edges, no caps): the whole
        // surface is sub-0 (SA-6). The primitive `center` folds into the face
        // transform exactly like the box/cylinder (DL-20).
        Primitive::Sphere { center, radius } => {
            Some(mk(*center, &[(0, HalfSpace::Sphere { radius: *radius })]))
        }
        // A torus is likewise ONE closed analytic face (no edges): same
        // edgeless-closed-surface topology as the sphere, mapped to
        // HalfSpace::Torus (→ TOROIDAL_SURFACE in STEP). SA-6.
        Primitive::Torus { center, major_radius, minor_radius } => Some(mk(
            *center,
            &[(0, HalfSpace::Torus {
                major_radius: *major_radius,
                minor_radius: *minor_radius,
            })],
        )),
        Primitive::Cone { r_bottom, r_top, half_height } => {
            // Wall half-angle of the truncated cone (apex direction along Y).
            let half_angle = ((r_bottom - r_top) / (2.0 * *half_height)).atan();
            let mut hs = mk(
                [0.0, 0.0, 0.0],
                &[
                    (0, HalfSpace::Cone { half_angle }),
                    (1, plane([0.0, 1.0, 0.0], *half_height)),
                    (2, plane([0.0, -1.0, 0.0], *half_height)),
                ],
            );
            // The frustum's radius is `r_bottom` at `y = −half_height` and
            // `r_top` at `y = +half_height`; its extrapolated apex (`r = 0`)
            // sits at `y_apex = half_height·(r_bottom + r_top)/(r_bottom −
            // r_top)`. `HalfSpace::Cone`'s SDF assumes the apex at the
            // transform origin, so the wall face carries that shift as its
            // local offset (DL-76) — placing the analytic cone exactly through
            // both rims so the weave classifies them as real seam edges. (A
            // degenerate equal-radius "cone" is really a cylinder and never
            // reaches here with `r_bottom ≠ r_top`; guard against ∞ anyway.)
            let denom = r_bottom - r_top;
            let apex_y = if denom.abs() > 1e-6 {
                half_height * (r_bottom + r_top) / denom
            } else {
                0.0
            };
            hs.faces[0].local_offset = [0.0, apex_y, 0.0];
            Some(hs)
        }
        _ => None,
    }
}

/// Retained-boundary SDF of a leaf primitive: the max over its half-space
/// faces with the opened sub-faces removed — the solid extended through the
/// openings. Exact in the interior (where the shell erosion reads it);
/// conservative outside corners. Primitives without a half-space split
/// (Sphere/Torus/Gyroid: the single face IS the whole boundary) have nothing
/// to retain, so exclusion is ignored and the wall stays closed there.
pub(crate) fn leaf_sdf_retained(
    id: &FeatureId,
    prim: &Primitive,
    p: [f32; 3],
    open: &OpenBranchSet<'_>,
) -> f32 {
    let Some(hs) = primitive_halfspaces(prim) else {
        return sdf_primitive(prim, p);
    };
    let lp = [p[0] - hs.center[0], p[1] - hs.center[1], p[2] - hs.center[2]];
    let mut d = f32::NEG_INFINITY;
    let mut excluded_any = false;
    for f in hs.faces() {
        if open.contains_sub(id, f.sub) {
            excluded_any = true;
            continue;
        }
        let fp = [
            lp[0] - f.local_offset[0],
            lp[1] - f.local_offset[1],
            lp[2] - f.local_offset[2],
        ];
        d = d.max(sdf_halfspace(&f.half_space, fp));
    }
    if !excluded_any {
        // No face of THIS leaf is opened — return the exact primitive SDF.
        return sdf_primitive(prim, p);
    }
    d
}

/// Contributor-banded sibling of [`leaf_sdf_retained`]: the retained max and
/// the retained faces within `eps` of it (opened faces never contribute).
pub(crate) fn expand_evaluate_leaf_retained(
    id: &FeatureId,
    prim: &Primitive,
    p: [f32; 3],
    eps: f32,
    open: &OpenBranchSet<'_>,
) -> (f32, Contributors) {
    if !open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
        return expand_evaluate_leaf(id, prim, p, eps);
    }
    let Some(hs) = primitive_halfspaces(prim) else {
        return expand_evaluate_leaf(id, prim, p, eps);
    };
    let lp = [p[0] - hs.center[0], p[1] - hs.center[1], p[2] - hs.center[2]];
    let face_d = |f: &HalfSpaceFace| {
        let fp = [
            lp[0] - f.local_offset[0],
            lp[1] - f.local_offset[1],
            lp[2] - f.local_offset[2],
        ];
        sdf_halfspace(&f.half_space, fp)
    };
    let mut d_max = f32::NEG_INFINITY;
    let mut excluded_any = false;
    for f in hs.faces() {
        if open.contains_sub(id, f.sub) {
            excluded_any = true;
            continue;
        }
        d_max = d_max.max(face_d(f));
    }
    if !excluded_any {
        return expand_evaluate_leaf(id, prim, p, eps);
    }
    let mut c = Contributors::empty();
    for f in hs.faces() {
        if open.contains_sub(id, f.sub) {
            continue;
        }
        if (face_d(f) - d_max).abs() <= eps {
            c.push(BranchId::from_feature_sub(id, f.sub));
        }
    }
    (d_max, c)
}

/// Evaluate a leaf primitive with half-space decomposition for branch
/// tracking. Returns the original (L2-exact) SDF value but computes
/// per-face BranchIds by projecting onto the half-space composition.
pub(crate) fn expand_evaluate_leaf(
    id: &FeatureId,
    prim: &Primitive,
    p: [f32; 3],
    eps: f32,
) -> (f32, Contributors) {
    let d = sdf_primitive(prim, p);
    match prim {
        Primitive::Box { center, .. }
        | Primitive::Cylinder { center, .. }
        | Primitive::Sphere { center, .. }
        | Primitive::Torus { center, .. } => {
            // Box/Cylinder/Sphere/Torus consume `primitive_halfspaces` directly
            // — single source of truth, no heap allocation. Sphere/Torus have a
            // single face (len == 1), so the loop trivially tags sub 0 (SA-6).
            let lp = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
            let hs = primitive_halfspaces(prim).expect("box/cylinder are enumerated");
            let mut d_max = f32::NEG_INFINITY;
            for f in hs.faces() {
                d_max = d_max.max(sdf_halfspace(&f.half_space, lp));
            }
            let mut c = Contributors::empty();
            for f in hs.faces() {
                if (sdf_halfspace(&f.half_space, lp) - d_max).abs() <= eps {
                    c.push(BranchId::from_feature_sub(id, f.sub));
                }
            }
            (d, c)
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let hh = *half_height;
            let t = ((p[1] + hh) / (2.0 * hh)).clamp(0.0, 1.0);
            let r_at_y = r_bottom * (1.0 - t) + r_top * t;
            // Project the raw radial gap onto the inclined wall normal so the
            // contributor comparison uses the true perpendicular distance
            // (matches sdf_primitive). For a cylinder (dr == 0) the factor is 1.
            let dr = r_bottom - r_top;
            let slant_len = (dr * dr + (2.0 * hh) * (2.0 * hh)).sqrt();
            let wall_scale = if slant_len > 1e-9 { (2.0 * hh) / slant_len } else { 1.0 };
            let d_wall = (r - r_at_y) * wall_scale;
            let halves = [
                (d_wall, 0u32),
                (sdf_halfspace(&HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset: hh }, p), 1),
                (sdf_halfspace(&HalfSpace::Plane { normal: [0.0, -1.0, 0.0], offset: hh }, p), 2),
            ];
            let d_max = halves.iter().fold(f32::NEG_INFINITY, |a, (v, _)| a.max(*v));
            let mut c = Contributors::empty();
            for &(di, sub) in &halves {
                if (di - d_max).abs() <= eps {
                    c.push(BranchId::from_feature_sub(id, sub));
                }
            }
            (d, c)
        }
        _ => (d, Contributors::single(BranchId::from_feature(id))),
    }
}

/// Signed distance to a single analytic [`HalfSpace`] surface, in the
/// primitive's centred local frame.
pub(crate) fn sdf_halfspace(hs: &HalfSpace, p: [f32; 3]) -> f32 {
    match hs {
        HalfSpace::Plane { normal, offset } => {
            p[0] * normal[0] + p[1] * normal[1] + p[2] * normal[2] - offset
        }
        HalfSpace::InfiniteCylinder { radius } => {
            (p[0] * p[0] + p[2] * p[2]).sqrt() - radius
        }
        HalfSpace::Sphere { radius } => {
            (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - radius
        }
        HalfSpace::Cone { half_angle } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let (sin_a, cos_a) = half_angle.sin_cos();
            r * cos_a - p[1].abs() * sin_a
        }
        HalfSpace::Torus { major_radius, minor_radius } => {
            let r_xz = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let q0 = r_xz - major_radius;
            (q0 * q0 + p[1] * p[1]).sqrt() - minor_radius
        }
        HalfSpace::Gyroid { cell_size, thickness } => {
            let s = *cell_size;
            let x = p[0] / s;
            let y = p[1] / s;
            let z = p[2] / s;
            let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
            (f.abs() - thickness) * s * 0.2887
        }
        HalfSpace::RevolvedProfile { segments, ccw, offset, extend_start, extend_end } => {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let (d, _) = fielddraft_protocol::sketch2d::group_signed_distance_and_normal(
                segments,
                *ccw,
                [r, p[1]],
                *extend_start,
                *extend_end,
            );
            d + offset
        }
    }
}

/// Analytic gradient `∇f` of [`sdf_halfspace`] in the surface-local frame. Each
/// branch is unit-norm where the SDF is smooth (Plane/Sphere/Cylinder/Cone/Torus
/// are Eikonal in local coordinates); at the central singularities (cylinder/cone
/// axis, sphere centre, torus core) it returns the zero vector so callers fall
/// back to the unscaled value. Used by [`OpenSurface::eval`] to convert a local
/// gate to a world Euclidean distance under a scaled placement.
pub(crate) fn grad_halfspace(hs: &HalfSpace, p: [f32; 3]) -> [f32; 3] {
    match hs {
        HalfSpace::Plane { normal, .. } => *normal,
        HalfSpace::InfiniteCylinder { .. } => {
            let rho = (p[0] * p[0] + p[2] * p[2]).sqrt();
            if rho > 1e-9 {
                [p[0] / rho, 0.0, p[2] / rho]
            } else {
                [0.0, 0.0, 0.0]
            }
        }
        HalfSpace::Sphere { .. } => {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            if len > 1e-9 {
                [p[0] / len, p[1] / len, p[2] / len]
            } else {
                [0.0, 0.0, 0.0]
            }
        }
        HalfSpace::Cone { half_angle } => {
            let rho = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let (sin_a, cos_a) = half_angle.sin_cos();
            if rho > 1e-9 {
                [cos_a * p[0] / rho, -p[1].signum() * sin_a, cos_a * p[2] / rho]
            } else {
                [0.0, -p[1].signum() * sin_a, 0.0]
            }
        }
        HalfSpace::Torus { major_radius, .. } => {
            let rho = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let q0 = rho - major_radius;
            let k = (q0 * q0 + p[1] * p[1]).sqrt();
            if k > 1e-9 && rho > 1e-9 {
                [q0 * p[0] / (rho * k), p[1] / k, q0 * p[2] / (rho * k)]
            } else {
                [0.0, 0.0, 0.0]
            }
        }
        HalfSpace::Gyroid { .. } => [0.0, 0.0, 0.0],
        HalfSpace::RevolvedProfile { segments, ccw, offset: _, extend_start, extend_end } => {
            let rho = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let (_, g2) = fielddraft_protocol::sketch2d::group_signed_distance_and_normal(
                segments,
                *ccw,
                [rho, p[1]],
                *extend_start,
                *extend_end,
            );
            if rho > 1e-9 {
                [g2[0] * p[0] / rho, g2[1], g2[0] * p[2] / rho]
            } else {
                [0.0, g2[1], 0.0]
            }
        }
    }
}

/// The analytic surface of a shell's *inner wall*: the body's outer surface
/// `hs` pushed inward (into the solid) by `thickness`. Because every analytic
/// [`HalfSpace`] gate is an exact signed Euclidean distance (`g < 0` inside, unit
/// gradient — see [`grad_halfspace`]), the inner wall is exactly the level set
/// `g = -thickness`, which for these surfaces is the same surface family with one
/// shifted parameter and an *unchanged* placement [`Transform`]:
///
/// - Plane    `offset → offset − t`
/// - Cylinder `radius → radius − t`
/// - Sphere   `radius → radius − t`
/// - Torus    `minor  → minor  − t`
///
/// Returns `None` when the wall would self-intersect or vanish (`radius`/`minor`
/// `≤ t`), and for surfaces whose inward offset is *not* a member of the same
/// family: a [`HalfSpace::Cone`] offset splits its two nappes apart (the apex is
/// no longer a single point), and [`HalfSpace::Gyroid`] / TPMS have no analytic
/// offset. Those inner walls fall back to a tessellated face, like other
/// non-analytic surfaces in the weaver.
pub fn offset_halfspace_inward(hs: &HalfSpace, thickness: f32) -> Option<HalfSpace> {
    let shrink = |r: f32| (r - thickness > 1e-4).then_some(r - thickness);
    match hs {
        HalfSpace::Plane { normal, offset } => Some(HalfSpace::Plane {
            normal: *normal,
            offset: offset - thickness,
        }),
        HalfSpace::InfiniteCylinder { radius } => {
            shrink(*radius).map(|radius| HalfSpace::InfiniteCylinder { radius })
        }
        HalfSpace::Sphere { radius } => shrink(*radius).map(|radius| HalfSpace::Sphere { radius }),
        HalfSpace::Torus { major_radius, minor_radius } => {
            shrink(*minor_radius).map(|minor_radius| HalfSpace::Torus {
                major_radius: *major_radius,
                minor_radius,
            })
        }
        // Cone nappes separate under offset; Gyroid/TPMS have no analytic offset.
        HalfSpace::Cone { .. } | HalfSpace::Gyroid { .. } => None,
        // The inner wall of a revolved profile group is the SAME carrier with a
        // shifted level (`g = signed_profile_distance + offset`). No degeneracy
        // gate is computable cheaply here (it would need the profile's minimum
        // curvature radius); an over-thick wall yields an empty active set and
        // is pruned by the activity tests downstream.
        HalfSpace::RevolvedProfile { segments, ccw, offset, extend_start, extend_end } => {
            Some(HalfSpace::RevolvedProfile {
                segments: segments.clone(),
                ccw: *ccw,
                offset: offset + thickness,
                extend_start: *extend_start,
                extend_end: *extend_end,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{
        BoolOp, FeatureId, Primitive, Sketch2d as ProtoSketch2d,
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
        let features = vec![DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "extrude".to_string(),
            BoolOp::New,
            Primitive::Extrude {
                sketch: circle_sketch,
                half_depth_z: 3.0,
                center: [0.0, 0.0, 0.0],
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )];
        let csg = CsgNode::from_decoded(&features);
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

    /// A body-shell modifier: the sentinel `Shell(Empty, t, open)` node on a
    /// `Difference` feature targeting `target`, exactly as `ShellFeature`
    /// emits it.
    fn shell_feature(id: u32, target: u32, thickness: f32, open: Vec<OpenSurface>) -> DecodedFeature {
        DecodedFeature {
            id: FeatureId::from_u32(id),
            name: "Shell".to_string(),
            op: BoolOp::Remove,
            blend_radius: 0.0,
            blend_mode: BlendMode::Fillet,
            target_ids: vec![FeatureId::from_u32(target)],
            transform: Transform::IDENTITY,
            node: CsgNode::Shell(Box::new(CsgNode::Empty), thickness, open),
        }
    }

    #[test]
    fn shell_wall_sdf_closed_is_an_inward_wall() {
        // Closed shell (d_ret = d) → wall from the surface (d = 0) inward to
        // d = −t, hollow beyond. Outer surface preserved.
        let t = 2.0;
        assert!((shell_wall_sdf(0.0, 0.0, t)).abs() < 1e-6, "surface on boundary");
        assert!(shell_wall_sdf(-1.0, -1.0, t) < 0.0, "mid-wall is solid");
        assert!((shell_wall_sdf(-2.0, -2.0, t)).abs() < 1e-6, "inner wall face");
        assert!(shell_wall_sdf(-5.0, -5.0, t) > 0.0, "deep interior is hollow");
    }

    #[test]
    fn open_shell_breaks_through_the_open_face_only() {
        // 20³ box centred at origin, top (+Z) face removed, 2 mm wall.
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            shell_feature(
                2,
                1,
                2.0,
                vec![OpenSurface {
                    // The box's +Z face is sub 5−1… see `primitive_halfspaces`:
                    // sub 4 = plane [0,0,1].
                    branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 4).as_u32(),
                    surface: HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 10.0 },
                    placement: Transform::IDENTITY,
                }],
            ),
        ];
        let tree = CsgNode::from_decoded(&features);
        let s = |p: [f32; 3]| tree.sdf(p, EvalMode::Logical);

        // Hollow centre, open top, but the side and bottom walls remain solid.
        assert!(s([0.0, 0.0, 0.0]) > 0.0, "centre is hollow");
        assert!(s([0.0, 0.0, 9.0]) > 0.0, "just under the open top is open");
        assert!(s([9.0, 0.0, 0.0]) < 0.0, "+X side wall is solid");
        assert!(s([0.0, 0.0, -9.0]) < 0.0, "bottom wall is solid (closed face)");
        // The outer surface is preserved everywhere — and the rim BAND still
        // ends exactly on the original face plane (the face centre, in
        // contrast, is now plain void: the material there is gone).
        assert!(s([10.0, 0.0, 0.0]).abs() < 1e-4, "+X outer face preserved");
        assert!(s([9.0, 0.0, 10.0]).abs() < 1e-4, "rim band ends on the original face");
        assert!(s([0.0, 0.0, 10.0]) > 0.0, "open face centre is void");
        // Retained-boundary semantics: the wall keeps its FULL 2 mm thickness
        // right up to the opened face — no knife-edge taper at the rim. The
        // rim band on the open plane is solid from x = 8…10.
        assert!(s([9.0, 0.0, 9.9]) < 0.0, "rim band keeps full thickness");
        assert!(s([8.1, 0.0, 9.9]) < 0.0, "rim annulus inner edge at x = 8");
        assert!(s([7.9, 0.0, 9.9]) > 0.0, "cavity reaches the open plane at x < 8");
    }

    #[test]
    fn open_shell_curved_face_leaves_the_retained_plates() {
        // Opening the LATERAL face of a cylinder (sub 0, the InfiniteCylinder
        // wall): the retained boundary is just the two caps, so the shell
        // degenerates to two 2 mm plates — the whole mid-band is open, the
        // material near each cap stays.
        let features = vec![
            DecodedFeature {
                id: FeatureId::from_u32(1),
                name: "Cylinder".to_string(),
                op: BoolOp::New,
                blend_radius: 0.0,
                blend_mode: BlendMode::Fillet,
                target_ids: vec![],
                transform: Transform::IDENTITY,
                node: CsgNode::Leaf {
                    id: FeatureId::from_u32(1),
                    primitive: Primitive::Cylinder {
                        center: [0.0, 0.0, 0.0],
                        radius: 10.0,
                        half_height: 10.0,
                    },
                },
            },
            shell_feature(
                2,
                1,
                2.0,
                vec![OpenSurface {
                    branch: BranchId::from_feature_sub(&FeatureId::from_u32(1), 0).as_u32(),
                    surface: HalfSpace::InfiniteCylinder { radius: 10.0 },
                    placement: Transform::IDENTITY,
                }],
            ),
        ];
        let tree = CsgNode::from_decoded(&features);
        let s = |p: [f32; 3]| tree.sdf(p, EvalMode::Logical);

        assert!(s([9.0, 0.0, 0.0]) > 0.0, "just inside the opened lateral wall is open");
        assert!(s([0.0, 0.0, 0.0]) > 0.0, "axis mid-height is open (no lateral wall left)");
        assert!(s([0.0, 9.0, 0.0]) < 0.0, "top cap plate is solid");
        assert!(s([0.0, -9.0, 0.0]) < 0.0, "bottom cap plate is solid");
        assert!(s([0.0, 7.0, 0.0]) > 0.0, "below the 2 mm top plate is hollow");
    }

    #[test]
    fn open_surface_gate_is_world_distance_under_scaled_placement() {
        // A unit sphere gate carried by a uniform-scale-2 placement is the world
        // sphere of radius 2. The gate must report the *world* Euclidean distance
        // (Eikonal-normalised), not the unscaled local SDF, so the shell taper
        // keeps its true millimetre width regardless of the body's placement scale.
        let scaled = OpenSurface {
            branch: 0,
            surface: HalfSpace::Sphere { radius: 1.0 },
            placement: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [2.0, 2.0, 2.0],
            },
        };
        // World point 2 mm outside the radius-2 surface ⇒ gate = +2.
        assert!((scaled.eval([4.0, 0.0, 0.0]) - 2.0).abs() < 1e-4, "uniform scale: exact world distance");
        // On the surface ⇒ gate ≈ 0.
        assert!(scaled.eval([2.0, 0.0, 0.0]).abs() < 1e-4, "on the scaled surface");

        // Non-uniform scale (x·2): on-axis the gate is still the true distance to
        // the ellipsoid's x semi-axis (= 2), proving the gradient normalisation
        // beats the raw local SDF (which would report 1).
        let aniso = OpenSurface {
            branch: 0,
            surface: HalfSpace::Sphere { radius: 1.0 },
            placement: Transform {
                translate: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [2.0, 1.0, 1.0],
            },
        };
        assert!((aniso.eval([4.0, 0.0, 0.0]) - 2.0).abs() < 1e-4, "non-uniform: on-axis world distance");
    }

    #[test]
    fn offset_halfspace_inward_is_the_inner_wall_surface() {
        // For each analytic kind, a point exactly `t` inside the original surface
        // (its g = -t level set) must lie ON the offset surface (g_offset ≈ 0),
        // and a point on the original surface must read +t (it is `t` *outside*
        // the smaller inner wall). This is the defining property of the inner wall.
        let t = 2.0;
        let cases: Vec<(HalfSpace, [f32; 3], [f32; 3])> = vec![
            // Plane (normal +X, offset 10): surface at x=10, inner wall at x=8.
            (
                HalfSpace::Plane { normal: [1.0, 0.0, 0.0], offset: 10.0 },
                [8.0, 1.0, 1.0],  // on inner wall
                [10.0, 1.0, 1.0], // on original surface
            ),
            // Cylinder radius 10 (axis Y): inner wall radius 8.
            (
                HalfSpace::InfiniteCylinder { radius: 10.0 },
                [8.0, 3.0, 0.0],
                [10.0, 3.0, 0.0],
            ),
            // Sphere radius 10: inner wall radius 8.
            (HalfSpace::Sphere { radius: 10.0 }, [0.0, 8.0, 0.0], [0.0, 10.0, 0.0]),
            // Torus major 10, minor 4: inner wall minor 2, on the tube top.
            (
                HalfSpace::Torus { major_radius: 10.0, minor_radius: 4.0 },
                [10.0, 2.0, 0.0],
                [10.0, 4.0, 0.0],
            ),
        ];
        for (hs, on_inner, on_outer) in cases {
            let inner = offset_halfspace_inward(&hs, t).expect("analytic inner wall");
            assert!(
                sdf_halfspace(&inner, on_inner).abs() < 1e-4,
                "{hs:?}: inner-wall point should be on the offset surface"
            );
            assert!(
                (sdf_halfspace(&inner, on_outer) - t).abs() < 1e-4,
                "{hs:?}: original surface is t outside the inner wall"
            );
        }

        // Degenerate + non-analytic cases yield no inner-wall surface.
        assert!(offset_halfspace_inward(&HalfSpace::Sphere { radius: 1.5 }, 2.0).is_none(), "wall thicker than radius vanishes");
        assert!(offset_halfspace_inward(&HalfSpace::Cone { half_angle: 0.5 }, 2.0).is_none(), "cone nappes separate under offset");
        assert!(offset_halfspace_inward(&HalfSpace::Gyroid { cell_size: 5.0, thickness: 0.3 }, 2.0).is_none(), "TPMS has no analytic offset");
    }

    #[test]
    fn inner_wall_branch_id_scheme() {
        // DL-75 Step 2: the inner-wall id is a distinct branch that preserves
        // `feature_id()` (so UI selection still resolves to the owning feature)
        // and never collides with the reserved extrude caps.
        let outer = BranchId::from_feature_sub(&FeatureId::from_u32(7), 3);
        let inner = outer.inner_wall();
        assert!(!outer.is_inner_wall());
        assert!(inner.is_inner_wall());
        assert_ne!(outer.as_u32(), inner.as_u32(), "inner wall is a distinct id");
        assert_eq!(inner.feature_id(), outer.feature_id(), "feature_id preserved");
        assert_eq!(inner.feature_id(), 7);
        assert_eq!(inner.inner_wall().as_u32(), inner.as_u32(), "idempotent");

        // The two extrude caps live below the inner-wall flag, so a cap and its
        // inner wall are distinct and the flag never collides with a cap.
        const { assert!(BranchId::EXTRUDE_CAP_TOP_SUB < BranchId::INNER_WALL_FLAG) };
        const { assert!(BranchId::EXTRUDE_CAP_BOTTOM_SUB < BranchId::INNER_WALL_FLAG) };
        let cap = BranchId::from_feature_sub(&FeatureId::from_u32(7), BranchId::EXTRUDE_CAP_TOP_SUB);
        assert!(!cap.is_inner_wall());
        assert_ne!(cap.as_u32(), cap.inner_wall().as_u32());
    }

    #[test]
    fn empty_tree_has_infinite_sdf_and_empty_aabb() {
        let tree = CsgNode::from_decoded(&[]);
        assert!(tree.sdf([0.0, 0.0, 0.0], EvalMode::Logical).is_infinite());
        assert!(tree.aabb().is_empty());
    }

    #[test]
    fn single_box_tree_matches_primitive_sdf() {
        let features = vec![box_feature(
            1,
            BoolOp::New,
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        )];
        let tree = CsgNode::from_decoded(&features);
        // Direct geometric SDF of a box with half-extents [10,10,10] at
        // the origin: negative inside, 0 on the +X face, positive outside.
        let expected = [
            ([0.0, 0.0, 0.0], -10.0),
            ([10.0, 0.0, 0.0], 0.0),
            ([20.0, 0.0, 0.0], 10.0),
        ];
        for &(p, want) in &expected {
            let a = tree.sdf(p, EvalMode::Logical);
            assert!((a - want).abs() < 1e-4, "{a} vs {want} at {p:?}");
        }
    }

    #[test]
    fn tree_sdf_matches_linear_fold_for_difference() {
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            DecodedFeature::from_parts(
                FeatureId::from_u32(2),
                "hole".into(),
                BoolOp::Remove,
                Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            ),
        ];
        let tree = CsgNode::from_decoded(&features);
        // Box half [10,10,10] at origin MINUS sphere radius 3 at origin.
        // Solid SDF = max(box_sdf, -sphere_sdf); values precomputed.
        let expected = [
            ([0.0, 0.0, 0.0], 3.0),
            ([5.0, 0.0, 0.0], -2.0),
            ([10.0, 0.0, 0.0], 0.0),
            ([-5.0, -5.0, -5.0], -5.0),
            ([50.0, 0.0, 0.0], 40.0),
        ];
        for (p, want) in expected {
            let a = tree.sdf(p, EvalMode::Logical);
            assert!(
                (a - want).abs() < 1e-4,
                "tree {a} vs expected {want} at {p:?}",
            );
        }
    }

    #[test]
    fn aabb_of_box_matches_half_extents() {
        let tree = CsgNode::from_decoded(&[box_feature(
            1,
            BoolOp::New,
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feature(2, BoolOp::New, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bb = CsgNode::from_decoded(&features).aabb();
        assert_eq!(bb.min, [-5.0, -5.0, -5.0]);
        assert_eq!(bb.max, [25.0, 5.0, 5.0]);
    }

    // ─── provenance / evaluate ─────────────────────────────────────

    fn cyl_feature(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Cyl {id}"),
            BoolOp::New,
            Primitive::Cylinder {
                center,
                radius,
                half_height,
            },
            Transform::IDENTITY,
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

    /// Tiny convenience used in the contributor-equality assertions:
    /// build a `Vec<BranchId>` from `(feature, sub)` pairs. Sorted so
    /// callers can compare against an order-independent expectation.
    fn branches(raw: &[(u32, u32)]) -> Vec<BranchId> {
        let mut v: Vec<BranchId> = raw
            .iter()
            .map(|&(f, sub)| BranchId::from_feature_sub(&FeatureId::from_u32(f), sub))
            .collect();
        v.sort_by_key(|b| b.as_u32());
        v
    }

    #[test]
    fn evaluate_on_solo_face_returns_single_contributor() {
        // Union(Box=1, Cylinder=2) with the cylinder poking out the
        // top of the box. Point on the box top, but outside the
        // cylinder footprint → only the box contributes.
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 50.0),
        ];
        let tree = CsgNode::from_decoded(&features);
        let (d, c) = tree.evaluate([15.0, 20.0, 0.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4, "point should sit on box top, got d={d}");
        assert_eq!(c.element(), ProxyElement::Face);
        // Box +Y face is sub-index 2 (see `primitive_halfspaces`).
        assert_eq!(c.as_slice(), branches(&[(1, 2)]).as_slice());
    }

    #[test]
    fn evaluate_on_emergent_edge_returns_two_contributors() {
        // The cylinder's lateral surface (x²+z²=100) meets the box's
        // top face (y=20) along the circle x²+z²=100 at y=20. At
        // (10, 20, 0) both leaves' individual SDFs are zero → the
        // point is an Edge in the B-Rep proxy.
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [20.0, 20.0, 20.0]),
            cyl_feature(2, [0.0, 0.0, 0.0], 10.0, 50.0),
        ];
        let tree = CsgNode::from_decoded(&features);
        let (d, c) = tree.evaluate([10.0, 20.0, 0.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4, "should lie on both surfaces, got d={d}");
        assert_eq!(c.element(), ProxyElement::Edge);
        let mut got: Vec<BranchId> = c.as_slice().to_vec();
        got.sort_by_key(|b| b.as_u32());
        // box +Y face (sub 2) meets cylinder lateral wall (sub 0).
        assert_eq!(got, branches(&[(1, 2), (2, 0)]));
    }

    #[test]
    fn evaluate_on_triple_intersection_returns_vertex() {
        // Three axis-aligned boxes meeting at a single corner (+x,+y,
        // +z). At the shared corner (10, 10, 10) all three box
        // surfaces pass through the same point → contributors = 3 →
        // classification = Vertex.
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(2, BoolOp::New, [20.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feature(3, BoolOp::New, [10.0, 20.0, 0.0], [10.0, 10.0, 10.0]),
        ];
        let tree = CsgNode::from_decoded(&features);
        // All three boxes meet at (10, 10, 10).
        let (d, c) = tree.evaluate([10.0, 10.0, 10.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4);
        // With per-face decomposition each box reports several of its
        // own faces at the shared corner, so the multiplicity is well
        // above three — still a Vertex. The provenance must name all
        // three feature ids (sub-index stripped).
        assert_eq!(c.element(), ProxyElement::Vertex);
        let mut feats: Vec<u32> = c.as_slice().iter().map(|b| b.feature_id()).collect();
        feats.sort_unstable();
        feats.dedup();
        assert_eq!(feats, vec![1, 2, 3]);
    }

    #[test]
    fn difference_preserves_both_contributors_on_hole_rim() {
        // Box (id=1) minus a sphere (id=2) punched out of its corner.
        // On the circle where the sphere exits the box's +x face the
        // point lies on *both* the box's +x face and the sphere's
        // lateral surface → the difference SDF is zero via the tie
        // `max(box_sdf, -sphere_sdf)`.
        let features = vec![
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            {
                let mut f = sphere_feature(2, [10.0, 0.0, 0.0], 4.0);
                f.op = BoolOp::Remove;
                f
            },
        ];
        let tree = CsgNode::from_decoded(&features);
        // (10, 0, 4) is on the box's +x face (x=10) AND on the
        // sphere's surface (distance from (10,0,0) = 4).
        let (d, c) = tree.evaluate([10.0, 0.0, 4.0], 0.5, EvalMode::Logical);
        assert!(d.abs() < 1e-4, "should lie on both boundaries, got d={d}");
        assert_eq!(c.element(), ProxyElement::Edge);
        let mut got: Vec<BranchId> = c.as_slice().to_vec();
        got.sort_by_key(|b| b.as_u32());
        // box +X face (sub 0) meets the sphere (un-split, sub 0).
        assert_eq!(got, branches(&[(1, 0), (2, 0)]));
    }

    #[test]
    fn expand_evaluate_leaf_box_face_and_edge_subfaces() {
        // The B-Rep substrate: a single Box leaf, evaluated directly
        // through `expand_evaluate_leaf`, must hand back the per-face
        // BranchId(s) of the half-space decomposition.
        let id = FeatureId::from_u32(1);
        let prim = Primitive::Box {
            center: [0.0, 0.0, 0.0],
            half_extents: [10.0, 10.0, 10.0],
        };
        let eps = 0.25;

        // +X face centre → exactly one contributor: the +X face (sub 0).
        let (d, c) = expand_evaluate_leaf(&id, &prim, [10.0, 0.0, 0.0], eps);
        assert!(d.abs() < 1e-4, "on +X face, got d={d}");
        assert_eq!(c.element(), ProxyElement::Face);
        assert_eq!(c.as_slice(), branches(&[(1, 0)]).as_slice());

        // A point on the +X/+Y edge → the two adjoining face sub-ids
        // (sub 0 = +X, sub 2 = +Y), in some order.
        let (d, c) = expand_evaluate_leaf(&id, &prim, [10.0, 10.0, 0.0], eps);
        assert!(d.abs() < 1e-4, "on +X/+Y edge, got d={d}");
        assert_eq!(c.element(), ProxyElement::Edge);
        let mut got: Vec<BranchId> = c.as_slice().to_vec();
        got.sort_by_key(|b| b.as_u32());
        assert_eq!(got, branches(&[(1, 0), (1, 2)]));
    }

    #[test]
    fn evaluate_far_from_surface_still_has_one_contributor() {
        // Inside a single leaf, nearer to one face than any other →
        // the dominant contributor is the unambiguous nearest face
        // (provenance still records which leaf owns that slab). A
        // point off-centre (closer to +Z) avoids the box-centre tie
        // where all six faces are equidistant.
        let features = vec![box_feature(
            1,
            BoolOp::New,
            [0.0, 0.0, 0.0],
            [10.0, 10.0, 10.0],
        )];
        let tree = CsgNode::from_decoded(&features);
        let (d, c) = tree.evaluate([0.0, 0.0, 5.0], 0.5, EvalMode::Logical);
        assert!(d < 0.0);
        // +Z face is sub-index 4.
        assert_eq!(c.as_slice(), branches(&[(1, 4)]).as_slice());
    }

    #[test]
    fn contributors_push_dedupes_and_caps() {
        let b = |n: u32| BranchId::from_feature(&FeatureId::from_u32(n));
        let mut c = Contributors::empty();
        // Eight distinct ids fill the set, with a dup and an overflow.
        for n in [1u32, 2, 1, 3, 4, 5, 6, 7, 8, 9] {
            c.push(b(n));
        }
        assert_eq!(c.len(), MAX_CONTRIBUTORS);
        let mut got: Vec<BranchId> = c.as_slice().to_vec();
        got.sort_by_key(|x| x.as_u32());
        assert_eq!(
            got,
            branches(&[(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0)])
        );
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

    /// Thread-like helix: rectangle profile r∈[8,10], y∈[−0.6,0.6],
    /// pitch 2mm, 5 turns, right-handed, no taper.
    fn test_helix() -> CsgNode {
        CsgNode::Helix {
            id: FeatureId::from_u32(1),
            sketch: Box::new(rect_sketch(9.0, 0.0, 1.0, 0.6)),
            pitch: 2.0,
            turns: 5.0,
            left_handed: false,
            taper: 0.0,
            start_angle: 0.0,
            y_anchor: 0.0,
            r_inner: 8.0,
        }
    }

    #[test]
    fn helix_sdf_winds_with_the_pitch() {
        let h = test_helix();
        let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);

        // The sweep starts AT t=0 (θ=0, y=0) — the ridge centre there
        // lies ON the start cap (d ≈ 0). Probe a quarter turn in
        // (t = π/2, y advanced by pitch/4): solidly inside.
        assert!(d([0.0, 0.5, 9.0]) < 0.0, "ridge centre must be inside");
        // Half a pitch later at the same angle: the groove between turns.
        assert!(d([0.0, 1.5, 9.0]) > 0.0, "between turns must be outside");
        // Helical invariance away from the ends: a quarter turn (t=π/2)
        // vs half a turn (t=π) sit at the same profile depth.
        let d90 = d([0.0, 0.5, 9.0]);
        let d180 = d([-9.0, 1.0, 0.0]);
        assert!(
            (d90 - d180).abs() < 1e-4,
            "helical invariance violated: {d90} vs {d180}"
        );
        // Without the y-advance the quarter-turn point is off the ridge.
        assert!(d([0.0, 0.0, 9.0]) > d90 + 0.1);
        // Turn 1 instance exists one pitch up (t=2π, interior).
        assert!(d([9.0, 2.0, 0.0]) < 0.0);
        // Beyond the last turn: outside.
        assert!(d([9.0, 12.0, 0.0]) > 0.0);
        // Radially off the profile: outside on both sides.
        assert!(d([11.5, 1.0, 0.0]) > 0.0);
        assert!(d([6.5, 1.0, 0.0]) > 0.0);
        // On the axis: well outside (bore), field positive and finite.
        let axis = d([0.0, 0.0, 0.0]);
        assert!(axis > 0.0 && axis.is_finite());
    }

    /// Ticket vtyh: the finite sweep must stay CONTINUOUS across the
    /// `atan2` branch seam (θ = ±π) in the end regions — the old
    /// per-winding clamp jumped `w` by a full pitch there, conjuring a
    /// phantom ribbon below the first winding (negative field where the
    /// real solid has ended).
    #[test]
    fn helix_ends_are_continuous_across_the_seam() {
        let h = test_helix();
        let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);
        let eps = 1e-3;
        // Pairs straddling the seam (z → ±0 at x < 0), below the start
        // (y = −1) and beyond the end (y = 11) at the profile radius.
        for y in [-1.0_f32, 11.0] {
            let a = d([-9.0, y, eps]);
            let b = d([-9.0, y, -eps]);
            assert!(
                (a - b).abs() < 1e-2,
                "seam discontinuity at y={y}: {a} vs {b}"
            );
            // …and both sides are OUTSIDE (no phantom material).
            assert!(a > 0.0 && b > 0.0, "phantom material at y={y}: {a}/{b}");
        }
        // Just behind the start cap (θ slightly negative, profile-deep
        // point): a thin positive gap, not phantom-inside.
        assert!(d([9.0, 0.0, -0.5]) > 0.0, "behind the start cap must be open");
        // Just past the start cap (θ slightly positive): inside.
        assert!(d([9.0, 0.05, 0.5]) < 0.0, "past the start cap must be solid");
    }

    /// vfqh: `start_angle` rotates the whole sweep around the axis —
    /// the start cap moves to the given angle (measured in the winding
    /// direction) and the ridge follows.
    #[test]
    fn helix_start_angle_rotates_the_sweep() {
        let mut h = test_helix();
        if let CsgNode::Helix { start_angle, .. } = &mut h {
            *start_angle = std::f32::consts::FRAC_PI_2;
        }
        let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);
        // A quarter turn AFTER the rotated start (θ=π, axial advance
        // pitch/4): on the ridge.
        assert!(d([-9.0, 0.5, 0.0]) < 0.0, "rotated ridge must be inside");
        // A quarter turn BEFORE the rotated start (θ=0): off the sweep.
        assert!(d([9.0, 0.0, 0.0]) > 0.0, "before the rotated start must be open");
    }

    #[test]
    fn helix_left_handed_flips_the_winding() {
        let mut h = test_helix();
        if let CsgNode::Helix { left_handed, .. } = &mut h {
            *left_handed = true;
        }
        let d = |p: [f32; 3]| h.sdf(p, EvalMode::Visual);
        // The right-handed quarter-turn ridge point is now off the
        // sweep entirely (its handed angle sits before the start)…
        assert!(d([0.0, 0.5, 9.0]) > 0.0);
        // …and the mirrored quarter turn (θ = −π/2 → handed t = π/2,
        // same +pitch/4 axial advance) is on the ridge.
        assert!(d([0.0, 0.5, -9.0]) < 0.0);
    }

    #[test]
    fn helix_aabb_covers_all_turns() {
        let h = test_helix();
        let bb = h.aabb();
        // Radial: the profile reaches r=10.
        assert!(bb.min[0] <= -10.0 && bb.max[0] >= 10.0);
        assert!(bb.min[2] <= -10.0 && bb.max[2] >= 10.0);
        // Axial: turn 4's instance tops out near y = 8.6 plus the
        // in-turn rise; the AABB must cover it.
        assert!(bb.min[1] <= -0.6);
        assert!(bb.max[1] >= 10.0);
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
        assert_eq!(c.as_slice(), branches(&[(42, 0)]).as_slice());
        assert_eq!(c.element(), ProxyElement::Face);
    }

    #[test]
    fn extrude_of_path_square_carries_per_segment_sub_ids() {
        use crate::sketch2d::SketchSegment;
        // Unit square as a 4-segment Path (CCW). Each side is its
        // own G1 group: bottom=0, right=1, top=2, left=3.
        let path = Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
                SketchSegment::Line { start: [1.0, -1.0], end: [1.0, 1.0] },
                SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
                SketchSegment::Line { start: [-1.0, 1.0], end: [-1.0, -1.0] },
            ],
            closed: true,
        });
        let fid = 9u32;
        let extrude = CsgNode::Extrude {
            id: FeatureId::from_u32(fid),
            sketch: Box::new(path),
            half_height: 3.0,
        };
        // Mid-wall probes with a tight band: each maps to exactly its own
        // wall group (the DL-65 contributor band only ties at a corner/rim, so
        // a clearly-nearest wall stays a single contributor). +x wall → group 1.
        let (_, c) = extrude.evaluate([4.0, 0.0, 0.0], 0.05, EvalMode::Visual);
        assert_eq!(c.as_slice(), branches(&[(fid, 1)]).as_slice());
        // Bottom (−y) wall → group 0.
        let (_, c) = extrude.evaluate([0.0, -4.0, 0.0], 0.05, EvalMode::Visual);
        assert_eq!(c.as_slice(), branches(&[(fid, 0)]).as_slice());
        // Top (+y) wall → group 2 — distinct sub-id.
        let (_, c) = extrude.evaluate([0.0, 4.0, 0.0], 0.05, EvalMode::Visual);
        assert_eq!(c.as_slice(), branches(&[(fid, 2)]).as_slice());
    }

    #[test]
    fn extrude_corner_reports_both_walls_and_rim_reports_wall_and_cap() {
        use crate::sketch2d::SketchSegment;
        // Unit square Path (bottom=0, right=1, top=2, left=3), extruded ±3.
        let path = Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                SketchSegment::Line { start: [-1.0, -1.0], end: [1.0, -1.0] },
                SketchSegment::Line { start: [1.0, -1.0], end: [1.0, 1.0] },
                SketchSegment::Line { start: [1.0, 1.0], end: [-1.0, 1.0] },
                SketchSegment::Line { start: [-1.0, 1.0], end: [-1.0, -1.0] },
            ],
            closed: true,
        });
        let fid = 9u32;
        let extrude = CsgNode::Extrude {
            id: FeatureId::from_u32(fid),
            sketch: Box::new(path),
            half_height: 3.0,
        };

        // DL-65: at the (+x,+y) sketch corner the band ties the right (1) and
        // top (2) walls → a vertical wall∩wall edge reports BOTH branches, so
        // the both-branches gate sees a real edge instead of a singleton.
        let (_, c) = extrude.evaluate([1.0, 1.0, 0.0], 0.25, EvalMode::Visual);
        let s: std::collections::HashSet<u32> =
            c.as_slice().iter().map(|b| b.as_u32()).collect();
        assert!(
            s.contains(&BranchId::from_feature_sub(&FeatureId::from_u32(fid), 1).as_u32())
                && s.contains(
                    &BranchId::from_feature_sub(&FeatureId::from_u32(fid), 2).as_u32()
                ),
            "corner must report both adjoining walls, got {s:?}"
        );
        assert_eq!(c.element(), ProxyElement::Edge);

        // At the top rim (mid-wall, z = +half_height) the wall group and the
        // top cap tie → a wall∩cap edge reports both branches.
        let (_, c) = extrude.evaluate([1.0, 0.0, 3.0], 0.25, EvalMode::Visual);
        let s: std::collections::HashSet<u32> =
            c.as_slice().iter().map(|b| b.as_u32()).collect();
        assert!(
            s.contains(&BranchId::from_feature_sub(&FeatureId::from_u32(fid), 1).as_u32())
                && s.contains(
                    &BranchId::from_feature_sub(
                        &FeatureId::from_u32(fid),
                        BranchId::EXTRUDE_CAP_TOP_SUB
                    )
                    .as_u32()
                ),
            "rim must report the wall group and the top cap, got {s:?}"
        );
        assert_eq!(c.element(), ProxyElement::Edge);
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
            box_feature(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            DecodedFeature::from_parts(
                FeatureId::from_u32(2),
                "hole".into(),
                BoolOp::Remove,
                Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            ),
        ];
        let bb = CsgNode::from_decoded(&features).aabb();
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
            mask_polyline: vec![[10.0, -15.0, 0.0], [10.0, 15.0, 0.0]],
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
    fn polyline_tube_two_points_equals_single_capsule() {
        // The reduction invariant: a 2-point polyline tube is exactly the
        // single-capsule SDF over those endpoints.
        let p0 = [10.0, -15.0, 0.0];
        let p1 = [10.0, 15.0, 0.0];
        let r = 5.0;
        let poly = [p0, p1];
        for p in [
            [10.0, 0.0, 8.0],
            [0.0, 0.0, 0.0],
            [12.0, 3.0, -4.0],
            [10.0, 20.0, 0.0],
            [-30.0, 0.0, 0.0],
        ] {
            let tube = sdf_polyline_tube(p, &poly, r);
            let cap = sdf_capsule_3d(p, p0, p1, r);
            assert!(
                (tube - cap).abs() < 1e-6,
                "tube {tube} != capsule {cap} at {p:?}",
            );
        }
    }

    #[test]
    fn polyline_tube_degenerate_cases() {
        let r = 4.0;
        // Single point → sphere of radius r at that point.
        let one = [[2.0, 0.0, 0.0]];
        assert!((sdf_polyline_tube([2.0, 0.0, 0.0], &one, r) - (-r)).abs() < 1e-6);
        assert!((sdf_polyline_tube([2.0 + r, 0.0, 0.0], &one, r)).abs() < 1e-6);
        // Empty → never active (mask falls through to the hard combine).
        assert_eq!(sdf_polyline_tube([0.0, 0.0, 0.0], &[], r), f32::INFINITY);
        // A 3-point polyline tube is the min over both segments.
        let three = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
        let p = [10.0, 5.0, 0.0]; // closest to the 2nd segment
        let expect = sdf_capsule_3d(p, three[1], three[2], r)
            .min(sdf_capsule_3d(p, three[0], three[1], r));
        assert!((sdf_polyline_tube(p, &three, r) - expect).abs() < 1e-6);
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
            mask_polyline: vec![[5.0, -15.0, 0.0], [5.0, 15.0, 0.0]],
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
            mask_polyline: vec![[0.0; 3], [0.0, 10.0, 0.0]],
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
                mask_polyline: vec![
                    [(i as f32 - 3.5) * 20.0, -55.0, 0.0],
                    [(i as f32 - 3.5) * 20.0, 55.0, 0.0],
                ],
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

    #[test]
    fn targeted_intersect_only_wraps_target_subtree() {
        let sphere = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Sphere".into(),
            BoolOp::New,
            Primitive::Sphere { center: [0.0; 3], radius: 10.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        let cube = DecodedFeature::from_parts(
            FeatureId::from_u32(2),
            "Box".into(),
            BoolOp::New,
            Primitive::Box {
                center: [15.0, 0.0, 0.0],
                half_extents: [5.0; 3],
            },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        let gyroid = DecodedFeature::from_parts(
            FeatureId::from_u32(3),
            "Gyroid".into(),
            BoolOp::Intersect,
            Primitive::Gyroid { cell_size: 5.0, thickness: 1.0 },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![FeatureId::from_u32(1)],
        );

        let csg = CsgNode::from_decoded(&[sphere, cube, gyroid]);

        // Centre of the cube (15,0,0) must remain inside —
        // the Gyroid targets only the Sphere, not the Box.
        let cube_centre = csg.sdf([15.0, 0.0, 0.0], EvalMode::Visual);
        assert!(
            cube_centre < 0.0,
            "cube centre must stay solid, got sdf = {cube_centre}",
        );
    }
}
