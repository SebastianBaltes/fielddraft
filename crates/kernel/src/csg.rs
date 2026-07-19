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
    /// inner-wall flag included). For a sketch-sweep wall this is the STABLE
    /// sketch-entity ordinal from the sweep's `wall_subs` table (naming
    /// schema v2) — or the positional G1 boundary-group index when the
    /// profile has no table (fallback / legacy). For a cap it is one of the
    /// reserved cap sub-ids.
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
    /// [`expand_evaluate_leaf`]; the mesh-level proxy (DC/MDC
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
/// EXCEPTION (blend-carrier work): a [`CsgNode::MaskedBlend`] with a
/// [`BlendProvenance`] is mode-INDEPENDENT — the fillet surface owns a
/// branch identity of its own (the fillet feature), so its contributor
/// sets are well-defined and the B-Rep weaves the real (blended) surface
/// instead of the phantom hard edge.
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
                wall_subs: _,
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
            Primitive::Revolve { sketch, center, wall_subs: _ } => {
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
            // The user field is clipped to its declared local box.
            Primitive::CustomSdf { half_extents, .. } => Aabb {
                min: [-half_extents[0], -half_extents[1], -half_extents[2]],
                max: [half_extents[0], half_extents[1], half_extents[2]],
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
/// operands. Fillet uses the circular smin ([`smin_poly`], zero set a true
/// circular arc of radius `r` between planar walls); Chamfer uses a linear
/// 45° cut.
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
#[derive(Debug, Clone, PartialEq, Serialize)]
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

/// `Serialize` exists for **content hashing only** (the body cache keys in
/// `body::hash_body_payload_split` feed the rmp-serde bytes of this tree into
/// the hasher) — it is NOT a persistence format and there is deliberately no
/// `Deserialize`: the `Leaf.primitive` field substitutes an imported mesh's
/// precomputed content fingerprint for its full triangle buffer (see
/// [`serialize_primitive_for_hash`]), so the byte form does not round-trip.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum CsgNode {
    Empty,
    Leaf {
        id: FeatureId,
        #[serde(serialize_with = "serialize_primitive_for_hash")]
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
        /// Branch identity of the blend face + the two junction walls —
        /// feeds the procedural [`fielddraft_protocol::HalfSpace::Blend`]
        /// carrier in the B-Rep surface registry.
        provenance: BlendProvenance,
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
        /// Positional wall group → stable sketch-entity sub (see
        /// [`Primitive::Extrude`]'s `wall_subs`); `None` = positional.
        /// `Arc` keeps node clones cheap.
        wall_subs: Option<std::sync::Arc<Vec<u32>>>,
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
        /// See [`CsgNode::Extrude::wall_subs`].
        wall_subs: Option<std::sync::Arc<Vec<u32>>>,
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
        /// See [`CsgNode::Extrude::wall_subs`].
        wall_subs: Option<std::sync::Arc<Vec<u32>>>,
    },
    /// General sweep of a 2D sketch profile along an arbitrary spine
    /// polyline. The path is pre-sampled (a
    /// [`crate::path_provider::SampledPath`], source-agnostic); `build`
    /// derives framing, arclength and the BVH into the shared
    /// [`crate::sweep_path::SweepPath`]. Single [`FeatureId`] contributor,
    /// same rationale as Extrude/Revolve/Helix.
    Sweep {
        id: FeatureId,
        sketch: Box<Sketch2d>,
        path: Vec<[f32; 3]>,
        closed: bool,
        /// Total profile twist (radians) about the spine, linear in arclength.
        twist: f32,
    },
}

/// Hashing-only stand-in serialisation for a leaf's [`Primitive`]: an
/// imported mesh contributes its precomputed content fingerprint (plus its
/// AABB) instead of the full triangle buffer — [`MeshSdf`] is immutable
/// after construction, so the fingerprint is exact, and hashing stays O(1)
/// in the mesh size (a raw serialisation would re-emit megabytes of
/// triangles on every cache-key check). Every other primitive passes
/// through its canonical serde form, so new variants and new fields enter
/// the hash automatically.
///
/// [`MeshSdf`]: fielddraft_protocol::MeshSdf
fn serialize_primitive_for_hash<S: serde::Serializer>(
    p: &Primitive,
    s: S,
) -> Result<S::Ok, S::Error> {
    match p {
        Primitive::ImportedMesh { mesh } => (
            "imported_mesh_fp",
            mesh.content_fingerprint(),
            mesh.aabb_min,
            mesh.aabb_max,
        )
            .serialize(s),
        other => other.serialize(s),
    }
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
    /// `true` genau dann, wenn Placement, Blend-Radius und der gesamte
    /// Geometrie-Teilbaum ([`CsgNode::all_finite`]) nur endliche Floats
    /// tragen. Vorab-Guard des Weave-Einstiegs gegen NaN/∞ aus dem Web.
    pub fn all_finite(&self) -> bool {
        self.blend_radius.is_finite()
            && crate::finite::all_finite(&self.transform)
            && self.node.all_finite()
    }

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
                ref wall_subs,
            } => {
                let centered = CsgNode::Extrude {
                    id: id.clone(),
                    sketch: Box::new(sketch.clone()),
                    half_height: half_depth_z,
                    wall_subs: wall_subs.clone().map(std::sync::Arc::new),
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
            Primitive::Revolve { ref sketch, center, ref wall_subs } => {
                // #390 invariant: a Revolve node's profile must carry NO
                // boundary on the revolve axis — an on-axis edge is
                // interior after revolution, and its zero-set would be a
                // phantom "needle" along the whole axis. The features
                // pipeline strips before wall-sub extraction already;
                // re-running here is idempotent and covers every direct
                // `Primitive::Revolve` constructor (tests, imports). The
                // strip never bends the meridian map, so Revolve's
                // 1-Lipschitz `march_relax = 1.0` proof is untouched.
                let centered = CsgNode::Revolve {
                    id: id.clone(),
                    sketch: Box::new(crate::sketch::strip_revolve_axis_segments(
                        sketch.clone(),
                    )),
                    wall_subs: wall_subs.clone().map(std::sync::Arc::new),
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
                ref wall_subs,
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
                    wall_subs: wall_subs.clone().map(std::sync::Arc::new),
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
                    wall_subs,
                    ..
                } => {
                    return Some(Primitive::Extrude {
                        sketch: (**sketch).clone(),
                        half_depth_z: *half_height,
                        center: [0.0, 0.0, 0.0],
                        wall_subs: wall_subs.as_deref().cloned(),
                    })
                }
                CsgNode::Revolve { sketch, wall_subs, .. } => {
                    return Some(Primitive::Revolve {
                        sketch: (**sketch).clone(),
                        center: [0.0, 0.0, 0.0],
                        wall_subs: wall_subs.as_deref().cloned(),
                    })
                }
                CsgNode::Helix {
                    sketch, pitch, turns, left_handed, taper, start_angle, wall_subs, ..
                } => {
                    return Some(Primitive::Helix {
                        sketch: (**sketch).clone(),
                        center: [0.0, 0.0, 0.0],
                        pitch: *pitch,
                        turns: *turns,
                        left_handed: *left_handed,
                        taper: *taper,
                        start_angle: *start_angle,
                        wall_subs: wall_subs.as_deref().cloned(),
                    })
                }
                // The extrude/revolve self-centering Transform: restore
                // its translate as the primitive's `center`.
                CsgNode::Transform(t, inner) => match &**inner {
                    CsgNode::Extrude {
                        sketch,
                        half_height,
                        wall_subs,
                        ..
                    } => {
                        return Some(Primitive::Extrude {
                            sketch: (**sketch).clone(),
                            half_depth_z: *half_height,
                            center: t.translate,
                            wall_subs: wall_subs.as_deref().cloned(),
                        })
                    }
                    CsgNode::Revolve { sketch, wall_subs, .. } => {
                        return Some(Primitive::Revolve {
                            sketch: (**sketch).clone(),
                            center: t.translate,
                            wall_subs: wall_subs.as_deref().cloned(),
                        })
                    }
                    CsgNode::Helix {
                        sketch, pitch, turns, left_handed, taper, start_angle, wall_subs, ..
                    } => {
                        return Some(Primitive::Helix {
                            sketch: (**sketch).clone(),
                            center: t.translate,
                            pitch: *pitch,
                            turns: *turns,
                            left_handed: *left_handed,
                            taper: *taper,
                            start_angle: *start_angle,
                            wall_subs: wall_subs.as_deref().cloned(),
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
    /// `true` genau dann, wenn jeder Float in diesem Teilbaum endlich ist —
    /// Primitiv-Parameter, Sketch-Geometrie, Transforms, Modifier-Skalare.
    /// Ein NaN/∞ (z.B. aus einer degenerierten Web-Sketch-Solver-Lösung)
    /// würde sonst weit von der Quelle entfernt die Weave-/Mesh-Numerik
    /// vergiften; der Weave-Einstieg ([`crate::body::Body::ensure_brep`])
    /// prüft das vorab und liefert stattdessen ein leeres B-Rep.
    pub fn all_finite(&self) -> bool {
        use crate::finite::all_finite;
        let finite3 = |p: &[f32; 3]| p.iter().all(|c| c.is_finite());
        match self {
            CsgNode::Empty => true,
            CsgNode::Leaf { primitive, .. } => all_finite(primitive),
            CsgNode::Transform(t, inner) => all_finite(t) && inner.all_finite(),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b) => a.all_finite() && b.all_finite(),
            CsgNode::SmoothUnion(a, b, r, _) | CsgNode::SmoothDifference(a, b, r, _) => {
                r.is_finite() && a.all_finite() && b.all_finite()
            }
            CsgNode::Twist(inner, rate)
            | CsgNode::Bend(inner, rate)
            | CsgNode::Offset(inner, rate) => rate.is_finite() && inner.all_finite(),
            CsgNode::Displace(inner, depth, freq) => {
                depth.is_finite() && freq.is_finite() && inner.all_finite()
            }
            CsgNode::Shell(inner, thickness, open_faces) => {
                thickness.is_finite()
                    && open_faces
                        .iter()
                        .all(|f| all_finite(&f.surface) && all_finite(&f.placement))
                    && inner.all_finite()
            }
            CsgNode::Morph(a, b, t) => t.is_finite() && a.all_finite() && b.all_finite(),
            CsgNode::MaskedBlend {
                a,
                b,
                mask_polyline,
                mask_radius,
                blend_radius,
                ..
            } => {
                mask_radius.is_finite()
                    && blend_radius.is_finite()
                    && mask_polyline.iter().all(finite3)
                    && a.all_finite()
                    && b.all_finite()
            }
            CsgNode::Extrude { sketch, half_height, .. } => {
                half_height.is_finite() && all_finite(sketch.as_ref())
            }
            CsgNode::Revolve { sketch, .. } => all_finite(sketch.as_ref()),
            CsgNode::Helix {
                sketch,
                pitch,
                turns,
                taper,
                start_angle,
                y_anchor,
                r_inner,
                ..
            } => {
                [pitch, turns, taper, start_angle, y_anchor, r_inner]
                    .into_iter()
                    .all(|v| v.is_finite())
                    && all_finite(sketch.as_ref())
            }
            CsgNode::Sweep { sketch, path, twist, .. } => {
                twist.is_finite() && path.iter().all(finite3) && all_finite(sketch.as_ref())
            }
        }
    }

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
            // The `offset` feature is the same kind of post-process: the sentinel
            // `Offset(Empty, δ)` wraps the accumulated subtree of its target body
            // in a real `CsgNode::Offset`, which the `push_down_offsets` /
            // `fold_offset_leaves` / channel passes then expand at build time
            // (#384 offset_tree, docs/OFFSET-TREE-PLAN.md §8). Internal offsets
            // (thicken's `Offset(leaf, …)`) have a non-`Empty` inner and are left
            // untouched here.
            if let CsgNode::Offset(inner, delta) = &f.node {
                if matches!(**inner, CsgNode::Empty) {
                    acc = wrap_offset(acc, &f.target_ids, *delta);
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
                if let Some((f, provenance)) = find_matching_fillet(&a, &b, fillets) {
                    CsgNode::MaskedBlend {
                        a: Box::new(a),
                        b: Box::new(b),
                        mask_polyline: f.mask_polyline.clone(),
                        mask_radius: f.mask_radius,
                        blend_radius: f.blend_radius,
                        blend_mode: f.blend_mode,
                        is_difference: false,
                        provenance,
                    }
                } else {
                    CsgNode::Union(Box::new(a), Box::new(b))
                }
            }
            CsgNode::Difference(a, b) => {
                let a = a.apply_edge_fillets(fillets);
                let b = b.apply_edge_fillets(fillets);
                if let Some((f, provenance)) = find_matching_fillet(&a, &b, fillets) {
                    CsgNode::MaskedBlend {
                        a: Box::new(a),
                        b: Box::new(b),
                        mask_polyline: f.mask_polyline.clone(),
                        mask_radius: f.mask_radius,
                        blend_radius: f.blend_radius,
                        blend_mode: f.blend_mode,
                        is_difference: true,
                        provenance,
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

    /// d / "Kanten runden (global)": round EVERY remaining hard Boolean
    /// junction with radius `r`, reusing the exact-circular smin. A plain
    /// `Union` becomes `SmoothUnion` (convex edge rounded), `Difference`
    /// becomes `SmoothDifference` (concave edge rounded). Nodes already
    /// blended per-edge (`MaskedBlend`) or per-feature (`SmoothUnion`/
    /// `SmoothDifference`, i.e. a fillet the user placed by hand) are left
    /// untouched — the global round only fills the gaps.
    ///
    /// This is the SDF-native "deburr everything" operator: robust (a field
    /// blend never invalidates the solid) and exact between planar walls. It
    /// rounds FEATURE-JUNCTION edges; a lone primitive's own edges are inside
    /// its SDF, not a Boolean node, so they are out of scope (that needs a
    /// per-primitive round radius or an eikonal offset — neither built).
    pub fn apply_global_round(self, r: f32) -> Self {
        if r <= 0.0 {
            return self;
        }
        match self {
            CsgNode::Union(a, b) => CsgNode::SmoothUnion(
                Box::new(a.apply_global_round(r)),
                Box::new(b.apply_global_round(r)),
                r,
                BlendMode::Fillet,
            ),
            CsgNode::Difference(a, b) => CsgNode::SmoothDifference(
                Box::new(a.apply_global_round(r)),
                Box::new(b.apply_global_round(r)),
                r,
                BlendMode::Fillet,
            ),
            CsgNode::Intersection(a, b) => CsgNode::Intersection(
                Box::new(a.apply_global_round(r)),
                Box::new(b.apply_global_round(r)),
            ),
            // Recurse through existing blends/morphs without re-rounding them.
            CsgNode::SmoothUnion(a, b, br, m) => CsgNode::SmoothUnion(
                Box::new(a.apply_global_round(r)),
                Box::new(b.apply_global_round(r)),
                br,
                m,
            ),
            CsgNode::SmoothDifference(a, b, br, m) => CsgNode::SmoothDifference(
                Box::new(a.apply_global_round(r)),
                Box::new(b.apply_global_round(r)),
                br,
                m,
            ),
            CsgNode::Morph(a, b, t) => CsgNode::Morph(
                Box::new(a.apply_global_round(r)),
                Box::new(b.apply_global_round(r)),
                t,
            ),
            CsgNode::MaskedBlend {
                a,
                b,
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            } => CsgNode::MaskedBlend {
                a: Box::new(a.apply_global_round(r)),
                b: Box::new(b.apply_global_round(r)),
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            },
            CsgNode::Transform(t, inner) => {
                CsgNode::Transform(t, Box::new(inner.apply_global_round(r)))
            }
            CsgNode::Twist(inner, rate) => CsgNode::Twist(Box::new(inner.apply_global_round(r)), rate),
            CsgNode::Bend(inner, rate) => CsgNode::Bend(Box::new(inner.apply_global_round(r)), rate),
            CsgNode::Shell(inner, t, open) => {
                CsgNode::Shell(Box::new(inner.apply_global_round(r)), t, open)
            }
            CsgNode::Offset(inner, delta) => {
                CsgNode::Offset(Box::new(inner.apply_global_round(r)), delta)
            }
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.apply_global_round(r)), d, f)
            }
            other => other,
        }
    }

    /// #384 Stufe 3a: push every [`CsgNode::Offset`] down through the Boolean
    /// nodes where the Minkowski algebra is EXACT (measured in
    /// `docs/research/sdf-metricity/SYNTHESIS.md`, 2×2-Klassifikation):
    ///
    /// * **outward** (`δ > 0`, dilation) distributes over **Union**:
    ///   `offset(A ∪ B, +δ) = offset(A, +δ) ∪ offset(B, +δ)`;
    /// * **inward** (`δ < 0`, erosion) distributes over **Intersection**
    ///   (`erode(A ∩ B) = erode(A) ∩ erode(B)`) and over **Difference**
    ///   with the subtrahend taking the *opposite* offset:
    ///   `offset(A \ B, −δ) = offset(A, −δ) \ offset(B, +δ)`.
    ///
    /// Rigid `Transform`s (unit scale) commute with the offset and are
    /// traversed; stacked `Offset`s merge their deltas (today's evaluator
    /// already computes `f − δ₁ − δ₂`, so the sum is behaviour-preserving).
    /// Everywhere else the pushdown STOPS and the node keeps today's `f − δ`
    /// fallback: the dual Boolean cases (Intersection/Difference at `+δ`,
    /// Union at `−δ`) differ from the true offset by channel bodies along
    /// the intersection curves (Rossignac & Requicha 1986 — the future
    /// `offset_tree` correction pass, NOT this step), and smin/MaskedBlend/
    /// Morph/warp/Shell nodes have no exact offset commutation at all.
    ///
    /// The rewrite is zero-set-preserving by construction (in the exact
    /// cases `min`/`max` are translation-equivariant, so the field is even
    /// pointwise identical) — its value is structural: after the pass the
    /// offset sits on leaves / deeper subtrees whose fields are exact or
    /// exacter, which is the entry point for the analytic per-leaf folding
    /// (sphere/torus `r ± δ`, 2D-profile offsetting) of the `offset_tree`
    /// product plan. That folding is deliberately NOT done here: it changes
    /// leaf `Primitive` parameters, which the feature-snap / B-Rep-carrier
    /// paths consume — it lives in the separate follow-up pass
    /// [`Self::fold_offset_leaves`] (Phase A2, consumers audit in
    /// docs/OFFSET-TREE-AUDIT.md), which every build path runs right after
    /// this one.
    pub fn push_down_offsets(self) -> Self {
        match self {
            CsgNode::Offset(inner, delta) => inner.push_down_offset(delta),
            CsgNode::Union(a, b) => CsgNode::Union(
                Box::new(a.push_down_offsets()),
                Box::new(b.push_down_offsets()),
            ),
            CsgNode::Difference(a, b) => CsgNode::Difference(
                Box::new(a.push_down_offsets()),
                Box::new(b.push_down_offsets()),
            ),
            CsgNode::Intersection(a, b) => CsgNode::Intersection(
                Box::new(a.push_down_offsets()),
                Box::new(b.push_down_offsets()),
            ),
            CsgNode::SmoothUnion(a, b, r, m) => CsgNode::SmoothUnion(
                Box::new(a.push_down_offsets()),
                Box::new(b.push_down_offsets()),
                r,
                m,
            ),
            CsgNode::SmoothDifference(a, b, r, m) => CsgNode::SmoothDifference(
                Box::new(a.push_down_offsets()),
                Box::new(b.push_down_offsets()),
                r,
                m,
            ),
            CsgNode::Morph(a, b, t) => CsgNode::Morph(
                Box::new(a.push_down_offsets()),
                Box::new(b.push_down_offsets()),
                t,
            ),
            CsgNode::MaskedBlend {
                a,
                b,
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            } => CsgNode::MaskedBlend {
                a: Box::new(a.push_down_offsets()),
                b: Box::new(b.push_down_offsets()),
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            },
            CsgNode::Transform(t, inner) => {
                CsgNode::Transform(t, Box::new(inner.push_down_offsets()))
            }
            CsgNode::Twist(inner, rate) => {
                CsgNode::Twist(Box::new(inner.push_down_offsets()), rate)
            }
            CsgNode::Bend(inner, rate) => {
                CsgNode::Bend(Box::new(inner.push_down_offsets()), rate)
            }
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.push_down_offsets()), d, f)
            }
            CsgNode::Shell(inner, t, open) => {
                CsgNode::Shell(Box::new(inner.push_down_offsets()), t, open)
            }
            other => other,
        }
    }

    /// Push one pending offset `delta` into `self` — the recursive worker of
    /// [`Self::push_down_offsets`]. Returns a tree whose field is pointwise
    /// identical to `Offset(self, delta)`.
    fn push_down_offset(self, delta: f32) -> Self {
        if delta == 0.0 {
            // `f − 0` is the identity — drop the node, keep normalising below.
            return self.push_down_offsets();
        }
        match self {
            // Stacked offsets evaluate as `f − δ₂ − δ` today; fold the sum and
            // let its sign pick the exact rules.
            CsgNode::Offset(inner, d2) => inner.push_down_offset(delta + d2),
            CsgNode::Union(a, b) if delta > 0.0 => CsgNode::Union(
                Box::new(a.push_down_offset(delta)),
                Box::new(b.push_down_offset(delta)),
            ),
            CsgNode::Intersection(a, b) if delta < 0.0 => CsgNode::Intersection(
                Box::new(a.push_down_offset(delta)),
                Box::new(b.push_down_offset(delta)),
            ),
            // Erosion of a difference erodes the minuend and DILATES the
            // subtrahend: `erode(A \ B) = erode(A) \ dilate(B)`.
            CsgNode::Difference(a, b) if delta < 0.0 => CsgNode::Difference(
                Box::new(a.push_down_offset(delta)),
                Box::new(b.push_down_offset(-delta)),
            ),
            // A rigid placement (unit scale) is an isometry — offset commutes.
            // Any non-unit scale breaks the metric (the evaluator multiplies by
            // `min_scale`), so the pushdown stops above it.
            CsgNode::Transform(t, inner) if t.scale == [1.0, 1.0, 1.0] => {
                CsgNode::Transform(t, Box::new(inner.push_down_offset(delta)))
            }
            // Everything else (dual Boolean cases, smin/MaskedBlend/Morph,
            // warp ops, Shell, leaves): keep today's `f − δ` right here. Leaf
            // fields are exact SDFs, so `f − δ` on a leaf IS the exact offset.
            other => CsgNode::Offset(Box::new(other.push_down_offsets()), delta),
        }
    }

    /// offset_tree Phase A2 (#384): faltet **Erosions**-Offsets
    /// (`Offset(blatt, δ)` mit `δ < 0`) in ein neues Blatt derselben
    /// Familie mit geschrumpften Parametern ein — läuft direkt NACH
    /// [`Self::push_down_offsets`] (der die Offsets an die Blätter
    /// geschoben hat) in ALLEN Baupfaden, auch dem Weave-Baupfad
    /// (`body::build_bounded_for_body_local`), damit Mesher-Feld,
    /// B-Rep-Carrier und hs_map dieselbe Nullmenge beschreiben
    /// (docs/OFFSET-TREE-AUDIT.md, Punkt 1).
    ///
    /// Regeln, Identitätsvertrag (FeatureId + Sub-Face-Raum bleiben) und
    /// Verweigerungs-Doktrin: [`crate::offset_fold`]. Dilation (`δ > 0`)
    /// bleibt unangetastet — `f − δ` ist auf einem exakten Blatt bereits
    /// das exakte Dilations-SDF.
    pub fn fold_offset_leaves(self) -> Self {
        match self {
            CsgNode::Offset(inner, delta) => {
                let inner = inner.fold_offset_leaves();
                if delta < 0.0 {
                    match crate::offset_fold::try_fold_erosion(&inner, -delta) {
                        Some(crate::offset_fold::FoldOutcome::Folded(folded)) => return folded,
                        // Phase C1 (#384, Plan §7.1): parametrischer Kollaps —
                        // die Erosion verschluckt das Blatt vollständig. Das
                        // leere Blatt ist die WAHRE Erosionsmenge; `f − δ` auf
                        // einem inexakten Profil-Feld ließe hier Geisterkörper
                        // stehen (Innen-Ridge bis 1.53 mm, a_summary.csv).
                        Some(crate::offset_fold::FoldOutcome::Collapsed) => {
                            return CsgNode::Empty;
                        }
                        None => {}
                    }
                }
                CsgNode::Offset(Box::new(inner), delta)
            }
            CsgNode::Union(a, b) => CsgNode::Union(
                Box::new(a.fold_offset_leaves()),
                Box::new(b.fold_offset_leaves()),
            ),
            CsgNode::Difference(a, b) => CsgNode::Difference(
                Box::new(a.fold_offset_leaves()),
                Box::new(b.fold_offset_leaves()),
            ),
            CsgNode::Intersection(a, b) => CsgNode::Intersection(
                Box::new(a.fold_offset_leaves()),
                Box::new(b.fold_offset_leaves()),
            ),
            CsgNode::SmoothUnion(a, b, r, m) => CsgNode::SmoothUnion(
                Box::new(a.fold_offset_leaves()),
                Box::new(b.fold_offset_leaves()),
                r,
                m,
            ),
            CsgNode::SmoothDifference(a, b, r, m) => CsgNode::SmoothDifference(
                Box::new(a.fold_offset_leaves()),
                Box::new(b.fold_offset_leaves()),
                r,
                m,
            ),
            CsgNode::Morph(a, b, t) => CsgNode::Morph(
                Box::new(a.fold_offset_leaves()),
                Box::new(b.fold_offset_leaves()),
                t,
            ),
            CsgNode::MaskedBlend {
                a,
                b,
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            } => CsgNode::MaskedBlend {
                a: Box::new(a.fold_offset_leaves()),
                b: Box::new(b.fold_offset_leaves()),
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            },
            CsgNode::Transform(t, inner) => {
                CsgNode::Transform(t, Box::new(inner.fold_offset_leaves()))
            }
            CsgNode::Twist(inner, rate) => {
                CsgNode::Twist(Box::new(inner.fold_offset_leaves()), rate)
            }
            CsgNode::Bend(inner, rate) => {
                CsgNode::Bend(Box::new(inner.fold_offset_leaves()), rate)
            }
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.fold_offset_leaves()), d, f)
            }
            CsgNode::Shell(inner, t, open) => {
                CsgNode::Shell(Box::new(inner.fold_offset_leaves()), t, open)
            }
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
            CsgNode::Extrude { .. } | CsgNode::Revolve { .. } | CsgNode::Helix { .. } | CsgNode::Sweep { .. } | CsgNode::Empty => {}
        }
    }

    /// Collect all leaf FeatureIds reachable from this node.
    fn contains_any_target(&self, target_ids: &[FeatureId]) -> bool {
        match self {
            CsgNode::Leaf { id, .. } | CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } | CsgNode::Helix { id, .. } | CsgNode::Sweep { id, .. } => {
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
            CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } | CsgNode::Helix { id, .. } | CsgNode::Sweep { id, .. } => {
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
                ..
            } => {
                // Geometry is mode-INDEPENDENT: the fillet is the body's real
                // surface, and the B-Rep must weave it (its contributor
                // identity is the blend's provenance branch, see
                // `evaluate_open`). Logical == Visual since the blend-carrier
                // work; before it, Logical degraded to the hard combine.
                // #384: ONE field for all paths — the same cross-faded combine
                // the compiled tape / TSL shader evaluates (the former hard
                // switch at the tube boundary jumped by up to (1−1/√2)·r
                // under the Circular-CD smin).
                let da = a.sdf_open(p, mode, open);
                let db_raw = b.sdf_open(p, mode, open);
                let mask_d = sdf_polyline_tube(p, mask_polyline, *mask_radius);
                let hard = hard_combine(*is_difference, da, db_raw);
                if mask_d >= 0.0 {
                    return hard;
                }
                let blended = if *is_difference {
                    -blend_min(-da, db_raw, *blend_radius, *blend_mode)
                } else {
                    blend_min(da, db_raw, *blend_radius, *blend_mode)
                };
                masked_blend_combine(mask_d, blended, hard, *blend_radius)
            }
            CsgNode::Extrude {
                id,
                sketch,
                half_height,
                wall_subs,
            } => {
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let ws = wall_subs.as_deref().map(|v| v.as_slice());
                    let d2 = sketch.sdf_retained([p[0], p[1]], &mut |g| {
                        open.contains_sub(id, map_wall_sub(ws, g))
                    });
                    extrude_retained_combine(id, d2, p[2], *half_height, open)
                } else {
                    sdf_extrude(sketch.sdf([p[0], p[1]]), p[2], *half_height)
                }
            }
            CsgNode::Revolve { id, sketch, wall_subs } => {
                let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let ws = wall_subs.as_deref().map(|v| v.as_slice());
                    sketch.sdf_retained([r, p[1]], &mut |g| {
                        open.contains_sub(id, map_wall_sub(ws, g))
                    })
                } else {
                    sketch.sdf([r, p[1]])
                }
            }
            CsgNode::Helix {
                id, sketch, pitch, turns, left_handed, taper, start_angle,
                y_anchor, r_inner, wall_subs,
            } => {
                let (q, ell, scale, _cap_side) = crate::helix_profile_point(
                    p, *pitch, *turns, *left_handed, *taper, *y_anchor, *r_inner,
                    *start_angle,
                );
                let d = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let ws = wall_subs.as_deref().map(|v| v.as_slice());
                    sketch.sdf_retained(q, &mut |g| {
                        open.contains_sub(id, map_wall_sub(ws, g))
                    })
                } else {
                    sketch.sdf(q)
                };
                crate::helix_compose(d, ell) * scale
            }
            CsgNode::Sweep { id, sketch, path, closed, twist, .. } => {
                // Unbounded picking-path eval: build the SweepPath on the fly
                // (the hot render path uses BoundedCsg::Sweep's cached one).
                // Shell-retained eval mirrors the Helix arm via the profile
                // plane (see the BoundedCsg::Sweep counterpart).
                let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(sketch);
                let Some(sp) = crate::sweep_path::SweepPath::new(path.clone(), *closed, *twist)
                else {
                    return f32::INFINITY;
                };
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let cs = sp.cross_section(p);
                    let d2 = bounded
                        .sdf_retained([cs.u, cs.v], &mut |g| open.contains_sub(id, g));
                    crate::sweep_path::sweep_compose(d2, cs.w)
                } else {
                    sp.sdf(&bounded, p)
                }
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
                provenance,
            } => {
                let (da, ca) = a.evaluate_open(p, eps, mode, open);
                let (db_raw, cb) = b.evaluate_open(p, eps, mode, open);
                let db = if *is_difference { -db_raw } else { db_raw };
                let hard = |da, ca, db, cb| {
                    if *is_difference {
                        combine_max(da, ca, db, cb, eps)
                    } else {
                        combine_min(da, ca, db, cb, eps)
                    }
                };
                let mask_d = sdf_polyline_tube(p, mask_polyline, *mask_radius);
                if mask_d >= 0.0 {
                    return hard(da, ca, db, cb);
                }
                // Inside the mask tube: geometry is the blend (mode-
                // independent — the fillet is the body's real surface).
                // #384: the VALUE is the same cross-faded field as everywhere
                // else (`masked_blend_combine`); only the contributor logic
                // below keeps its own gating.
                let (ba, bb) = if *is_difference { (-da, db_raw) } else { (da, db_raw) };
                let blended = if *is_difference {
                    -blend_min(-da, db_raw, *blend_radius, *blend_mode)
                } else {
                    blend_min(da, db_raw, *blend_radius, *blend_mode)
                };
                let hard_d = hard_combine(*is_difference, da, db_raw);
                let d = masked_blend_combine(mask_d, blended, hard_d, *blend_radius);
                if provenance.branch == 0 {
                    // No B-Rep identity (hand-built node): legacy semantics,
                    // both operand sets merged.
                    let mut c = ca;
                    c.extend(cb);
                    return (d, c);
                }
                // Deviation zone (`h > 0`): the surface here belongs to the
                // fillet branch. The operand walls join the set only within
                // the provenance eps of their OWN surface, so the fillet⇄wall
                // skirt seams carry the pair signature like any other edge.
                let deviates = *blend_radius > 0.0 && (ba - bb).abs() < *blend_radius;
                if !deviates {
                    return hard(da, ca, db, cb);
                }
                let mut c = Contributors::single(BranchId(provenance.branch));
                if da.abs() <= eps {
                    c.extend(ca);
                }
                if db_raw.abs() <= eps {
                    c.extend(cb);
                }
                (d, c)
            }
            CsgNode::Extrude {
                id,
                sketch,
                half_height,
                wall_subs,
            } => {
                // Per-segment identity (DL-46) + cap split (DL-50) +
                // eps-banded multi-contributors (DL-65): the sketch band
                // reports every boundary group within `eps`, and the cap
                // term `|z| − hh` joins when it ties — so a genuine extrude
                // edge names both colliding branches (uniform with analytic
                // CSG; the both-branches gate no longer needs to special-
                // case sweeps).
                let ws = wall_subs.as_deref().map(|v| v.as_slice());
                if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    let band = sketch.sdf_with_edges_retained([p[0], p[1]], eps, &mut |g| {
                        open.contains_sub(id, map_wall_sub(ws, g))
                    });
                    let d = extrude_retained_combine(id, band.distance, p[2], *half_height, open);
                    let c = extrude_contributors_retained(
                        id,
                        &band,
                        p[2],
                        *half_height,
                        eps,
                        open,
                        ws,
                    );
                    (d, c)
                } else {
                    let band = sketch.sdf_with_edges([p[0], p[1]], eps);
                    let d = sdf_extrude(band.distance, p[2], *half_height);
                    let c = extrude_contributors(id, &band, p[2], *half_height, eps, ws);
                    (d, c)
                }
            }
            CsgNode::Revolve { id, sketch, wall_subs } => {
                let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
                let ws = wall_subs.as_deref().map(|v| v.as_slice());
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained([r, p[1]], eps, &mut |g| {
                        open.contains_sub(id, map_wall_sub(ws, g))
                    })
                } else {
                    sketch.sdf_with_edges([r, p[1]], eps)
                };
                let c = revolve_contributors(id, &band, ws);
                (band.distance, c)
            }
            CsgNode::Helix {
                id, sketch, pitch, turns, left_handed, taper, start_angle,
                y_anchor, r_inner, wall_subs,
            } => {
                let (q, ell, scale, cap_side) = crate::helix_profile_point(
                    p, *pitch, *turns, *left_handed, *taper, *y_anchor, *r_inner,
                    *start_angle,
                );
                // The contributor band epsilon lives in WORLD units; widen
                // it by the inverse Lipschitz scale so profile-space edge
                // matching stays comparable.
                let beps = eps / scale.max(1e-6);
                let ws = wall_subs.as_deref().map(|v| v.as_slice());
                let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                    sketch.sdf_with_edges_retained(q, beps, &mut |g| {
                        open.contains_sub(id, map_wall_sub(ws, g))
                    })
                } else {
                    sketch.sdf_with_edges(q, beps)
                };
                let c = helix_contributors(id, &band, ell, cap_side, beps, ws);
                (crate::helix_compose(band.distance, ell) * scale, c)
            }
            CsgNode::Sweep { id, sketch, path, closed, twist, .. } => {
                let bounded = crate::sketch2d::BoundedSketch2d::from_sketch(sketch);
                match crate::sweep_path::SweepPath::new(path.clone(), *closed, *twist) {
                    Some(sp) => {
                        let cs = sp.cross_section(p);
                        let band = if open.has_feature(id.as_u32_or_root().unwrap_or(0)) {
                            bounded.sdf_with_edges_retained([cs.u, cs.v], eps, &mut |g| {
                                open.contains_sub(id, g)
                            })
                        } else {
                            bounded.sdf_with_edges([cs.u, cs.v], eps)
                        };
                        // General sweeps keep positional wall subs (no
                        // stable-ordinal table yet — Onshape import doesn't
                        // emit them and the numbering is self-consistent).
                        let c = revolve_contributors(id, &band, None);
                        (crate::sweep_path::sweep_compose(band.distance, cs.w), c)
                    }
                    None => (f32::INFINITY, Contributors::empty()),
                }
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
            CsgNode::Sweep { sketch, path, .. } => {
                let s = sketch.aabb();
                if s.is_empty() || path.len() < 2 {
                    Aabb::EMPTY
                } else {
                    // Conservative: each spine sample ± the profile radius.
                    let r_max = (s.max[0].abs().max(s.min[0].abs()))
                        .hypot(s.max[1].abs().max(s.min[1].abs()));
                    let mut bb = Aabb::EMPTY;
                    for q in path {
                        bb = Aabb::union(
                            bb,
                            Aabb {
                                min: [q[0] - r_max, q[1] - r_max, q[2] - r_max],
                                max: [q[0] + r_max, q[1] + r_max, q[2] + r_max],
                            },
                        );
                    }
                    bb
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
/// Map a positional wall boundary group to the branch sub-id that names
/// the swept wall face. With a `wall_subs` table (stable sketch-entity
/// ordinals, see [`Primitive::Extrude`]) the group indexes the table; an
/// out-of-range group keeps its positional id (defensive — the table is
/// built for exactly the profile's group count). Without a table the
/// positional id IS the sub (legacy numbering).
#[inline]
pub(crate) fn map_wall_sub(wall_subs: Option<&[u32]>, g: u32) -> u32 {
    match wall_subs {
        Some(t) => t.get(g as usize).copied().unwrap_or(g),
        None => g,
    }
}

pub(crate) fn extrude_contributors(
    id: &FeatureId,
    band: &SketchEdgeBand,
    z: f32,
    half_height: f32,
    eps: f32,
    wall_subs: Option<&[u32]>,
) -> Contributors {
    let wall = band.distance;
    let cap = z.abs() - half_height;
    let d_max = wall.max(cap);
    let mut c = Contributors::empty();
    if (wall - d_max).abs() <= eps {
        for &g in band.groups() {
            c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
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
        c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
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
    wall_subs: Option<&[u32]>,
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
            c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
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
        c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
    }
    c
}

/// Contributor band for a **revolve** point (DL-65). A revolve has no
/// caps, so every banded sketch boundary group is a wall contributor; a
/// sketch corner therefore reports both adjoining walls and the
/// both-branches gate sees the revolved edge.
/// Provenance for a helix point: the profile wall group(s) plus the END-CAP
/// sub when the cap term `ell` ties the dominating distance (ticket exus —
/// makes the two planar end caps pickable). Mirrors [`extrude_contributors`],
/// with `ell` as the cap coordinate and `cap_side` choosing which end.
/// `beps` is the band epsilon already widened by the inverse Lipschitz scale.
pub(crate) fn helix_contributors(
    id: &FeatureId,
    band: &SketchEdgeBand,
    ell: f32,
    cap_side: f32,
    beps: f32,
    wall_subs: Option<&[u32]>,
) -> Contributors {
    let wall = band.distance;
    let d_max = wall.max(ell);
    let mut c = Contributors::empty();
    if (wall - d_max).abs() <= beps {
        for &g in band.groups() {
            c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
        }
    }
    if (ell - d_max).abs() <= beps {
        let sub = if cap_side >= 0.0 {
            BranchId::EXTRUDE_CAP_TOP_SUB
        } else {
            BranchId::EXTRUDE_CAP_BOTTOM_SUB
        };
        c.push(BranchId::from_feature_sub(id, sub));
    }
    if c.is_empty() {
        let g = band.groups().first().copied().unwrap_or(0);
        c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
    }
    c
}

pub(crate) fn revolve_contributors(
    id: &FeatureId,
    band: &SketchEdgeBand,
    wall_subs: Option<&[u32]>,
) -> Contributors {
    let mut c = Contributors::empty();
    for &g in band.groups() {
        c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, g)));
    }
    if c.is_empty() {
        c.push(BranchId::from_feature_sub(id, map_wall_sub(wall_subs, 0)));
    }
    c
}

/// K10.0 / c-M-b / #384: **circular** smooth minimum, IQ's Circular (CD)
/// variant. `r` is the blend radius. Between two half-spaces the zero level
/// set is a TRUE circular arc of radius `r` (centre at `(r, r)` in the two
/// distances' plane) — bit-for-bit the SAME zero set as the previous
/// circular-geometrical form (`max(r, min) − ‖(relu(r−da), relu(r−db))‖`),
/// so a filleted planar edge keeps its measurable radius. When the inputs
/// are far apart (`|da − db| ≥ r`) it degenerates to `min(da, db)`.
///
/// `f = min(da,db) − (r/2)·(1 + h − √(1 − h·(h−2)))` with
/// `h = max(r − |da−db|, 0)/r`. Unlike the old form this is a CONVEX
/// combination of the inputs (`∂f/∂a + ∂f/∂b = 1`, both in `[0, 1]`, see
/// [`blend_min_weights`]): for 1-Lipschitz inputs the result is 1-Lipschitz
/// everywhere and NEVER overestimates distance — the old form was √2-Lipschitz
/// perpendicular to the seam and inflated interior values without bound
/// (measured: real tunneling on smin chains, ‖∇f‖ 1.22 → 1.0001 after the
/// swap; docs/research/sdf-metricity/SYNTHESIS.md §5 Exp 1). Trade-off: the
/// under-estimation is non-local — on the seam bisector (`da = db`) the value
/// sits `(1 − 1/√2)·r ≈ 0.293·r` below `min` even far from the corner. Max
/// undershoot below `min` is that same `0.293·r`; the AABB and cell-prune
/// margins carry `r`, which covers it.
///
/// Reference: <https://iquilezles.org/articles/smin/> (circular, CD family).
pub(crate) fn smin_poly(da: f32, db: f32, r: f32) -> f32 {
    if r <= 0.0 {
        return da.min(db);
    }
    let h = (r - (da - db).abs()).max(0.0) / r;
    da.min(db) - r * 0.5 * (1.0 + h - (1.0 - h * (h - 2.0)).sqrt())
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

/// Wrap the target body's accumulated subtree in a `CsgNode::Offset(_, δ)`
/// (the `offset` feature sentinel; mirrors [`wrap_shell`]). Recurses through
/// unions so only the branch that carries a target feature is offset, never a
/// sibling body; any body-preserving node (Difference/Intersection/…) is
/// wrapped whole.
fn wrap_offset(tree: CsgNode, target_ids: &[FeatureId], delta: f32) -> CsgNode {
    if !tree.contains_any_target(target_ids) {
        return tree;
    }
    match tree {
        CsgNode::Union(a, b) => {
            let a_has = a.contains_any_target(target_ids);
            let b_has = b.contains_any_target(target_ids);
            match (a_has, b_has) {
                (true, false) => {
                    CsgNode::Union(Box::new(wrap_offset(*a, target_ids, delta)), b)
                }
                (false, true) => {
                    CsgNode::Union(a, Box::new(wrap_offset(*b, target_ids, delta)))
                }
                _ => CsgNode::Offset(Box::new(CsgNode::Union(a, b)), delta),
            }
        }
        CsgNode::SmoothUnion(a, b, r, m) => {
            let a_has = a.contains_any_target(target_ids);
            let b_has = b.contains_any_target(target_ids);
            match (a_has, b_has) {
                (true, false) => {
                    CsgNode::SmoothUnion(Box::new(wrap_offset(*a, target_ids, delta)), b, r, m)
                }
                (false, true) => {
                    CsgNode::SmoothUnion(a, Box::new(wrap_offset(*b, target_ids, delta)), r, m)
                }
                _ => CsgNode::Offset(Box::new(CsgNode::SmoothUnion(a, b, r, m)), delta),
            }
        }
        // Difference / Intersection / etc. keep a body together — wrap whole.
        other => CsgNode::Offset(Box::new(other), delta),
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

/// K10.4: find the first EdgeFillet that matches a junction between two
/// subtrees. A fillet matches when one subtree contains feature_a and the
/// other contains feature_b; the returned [`BlendProvenance`] carries the
/// wall branches oriented to the (left, right) operand order.
fn find_matching_fillet<'a>(
    left: &CsgNode,
    right: &CsgNode,
    fillets: &'a [EdgeFillet],
) -> Option<(&'a EdgeFillet, BlendProvenance)> {
    let left_ids = left.leaf_ids();
    let right_ids = right.leaf_ids();
    fillets.iter().find_map(|f| {
        let a_in_left = left_ids.contains(&f.feature_a) && right_ids.contains(&f.feature_b);
        let a_in_right = right_ids.contains(&f.feature_a) && left_ids.contains(&f.feature_b);
        if !a_in_left && !a_in_right {
            return None;
        }
        let branch = BranchId::from_feature(&f.fillet_id).as_u32();
        let (wall_a, wall_b) =
            if a_in_left { (f.branch_a, f.branch_b) } else { (f.branch_b, f.branch_a) };
        Some((f, BlendProvenance { branch, wall_a, wall_b }))
    })
}

/// Branch identity of a masked blend's fillet surface and its two junction
/// walls (full [`BranchId`] u32s from the filleted edge's signature).
/// `NONE` (all zero) marks a blend with no B-Rep identity — the registry
/// skips it and its face degrades to Tessellated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlendProvenance {
    /// The blend face's own branch: the fillet feature's id, sub 0.
    pub branch: u32,
    /// The junction wall inside operand `a` (MaskedBlend's left subtree).
    pub wall_a: u32,
    /// The junction wall inside operand `b`.
    pub wall_b: u32,
}

impl BlendProvenance {
    pub const NONE: Self = Self { branch: 0, wall_a: 0, wall_b: 0 };
}

/// K10.4 / DL-61: per-edge fillet specification. Identifies a Boolean
/// junction by the feature IDs of its two operands (derived from the
/// queried edge's branch-pair signature) and places a polyline-tube mask
/// along the edge's actual geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeFillet {
    /// The fillet feature's own id — the blend face's branch identity.
    pub fillet_id: FeatureId,
    /// Feature id of the "left" operand at the junction.
    pub feature_a: FeatureId,
    /// Feature id of the "right" operand.
    pub feature_b: FeatureId,
    /// Full branch id (with sub) of the junction wall on `feature_a`'s side.
    pub branch_a: u32,
    /// Full branch id (with sub) of the junction wall on `feature_b`'s side.
    pub branch_b: u32,
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

/// #382/#384: THE masked-blend combine — ONE field for every path (CPU
/// `csg::sdf`, weave `evaluate_open`, compiled opcode tape, dual numbers,
/// TSL/WGSL shader). Cross-fades `hard → blended` with a C1 smoothstep across
/// the tube's inner shell band (`mask_d ∈ [−band, 0]`): a HARD switch at the
/// tube boundary would make the field DISCONTINUOUS — under the Circular-CD
/// smin the blend sits up to `(1 − 1/√2)·blend_r` below `hard` on the seam
/// bisector even at the tube boundary, so the jump is real (#382 grey wall
/// symptomatics). Since the CD smin is itself 1-Lipschitz, the old global ÷√2
/// Lipschitz normalisation (`masked_blend_render_fade`) is GONE — the fade
/// keeps `f ≤ hard` pointwise (blended ≤ hard always), so the field never
/// overestimates distance and the sphere-trace cannot overstep.
#[inline]
pub(crate) fn masked_blend_combine(mask_d: f32, blended: f32, hard: f32, blend_r: f32) -> f32 {
    let band = masked_blend_fade_band(blend_r);
    // w: 1 deep inside (mask_d ≤ −band → fully blended), 0 at/outside the
    // boundary (mask_d ≥ 0 → hard). C1 via smoothstep.
    let x = ((-mask_d) / band).clamp(0.0, 1.0);
    let w = x * x * (3.0 - 2.0 * x);
    hard + (blended - hard) * w
}

/// Cross-fade band width (mm) for [`masked_blend_combine`]. A WIDE band (one
/// `blend_r`) keeps `∇w ≤ 1.5/band` small so the fade adds little slope.
/// Mirrored in the TSL shader (`sdfVmInterpreter.ts`, `OP_MASKED_BLEND`).
#[inline]
pub(crate) fn masked_blend_fade_band(blend_r: f32) -> f32 {
    blend_r.max(1e-3)
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
            // Wall half-angle of the truncated cone. ALWAYS positive: the
            // `HalfSpace::Cone` SDF (`r·cos α − |y|·sin α`) is a symmetric
            // double cone about the apex — the OPENING direction lives in the
            // apex offset below, not in the angle's sign. A signed angle (the
            // old `atan` of the raw slope) silently negated the slope term
            // for upward-widening frustums (`r_top > r_bottom`): the wall
            // field read ~2·|y|·sin α instead of 0 on the wall, every rim
            // seed "projected" nowhere, and the seed route emitted an
            // edgeless frustum.
            let half_angle = ((r_bottom - r_top) / (2.0 * *half_height)).atan().abs();
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
        HalfSpace::Blend {
            a,
            a_transform,
            a_flip,
            b,
            b_transform,
            b_flip,
            radius,
            chamfer,
            is_difference,
        } => {
            let fa = blend_wall_value(a, a_transform, *a_flip, p);
            let fb = blend_wall_value(b, b_transform, *b_flip, p);
            let mode = if *chamfer { BlendMode::Chamfer } else { BlendMode::Fillet };
            if *is_difference {
                -blend_min(-fa, fb, *radius, mode)
            } else {
                blend_min(fa, fb, *radius, mode)
            }
        }
    }
}

/// One embedded wall field of a [`HalfSpace::Blend`], evaluated at a
/// body-frame point: local carrier value lifted to the body metric via the
/// placement's uniform scale, complemented when the wall's feature sits under
/// an odd number of subtractions within its blend operand's subtree.
fn blend_wall_value(hs: &HalfSpace, xf: &Transform, flip: bool, p: [f32; 3]) -> f32 {
    let local = crate::transform::world_to_local(xf, p);
    let v = sdf_halfspace(hs, local) * xf.min_scale();
    if flip { -v } else { v }
}

/// Value + body-frame unit gradient of one embedded [`HalfSpace::Blend`] wall.
/// Under a rotation + uniform scale the world gradient of `s·f(T⁻¹p)` is the
/// rotated local gradient (the value scale and the coordinate compression
/// cancel), so unit-norm carriers stay unit-norm in the body frame.
fn blend_wall_value_grad(hs: &HalfSpace, xf: &Transform, flip: bool, p: [f32; 3]) -> (f32, [f32; 3]) {
    let local = crate::transform::world_to_local(xf, p);
    let v = sdf_halfspace(hs, local) * xf.min_scale();
    let g = crate::transform::quat_rotate(xf.rotation, grad_halfspace(hs, local));
    if flip { (-v, [-g[0], -g[1], -g[2]]) } else { (v, g) }
}

/// Partial derivatives `(∂f/∂a, ∂f/∂b)` of [`blend_min`]. For the #384
/// Circular-CD smin they are the convex weights `(1 − s, s)` toward the
/// nearer input, with `s = ½·(1 − (1−h)/√(1 + h·(2−h)))` and
/// `h = max(r − |a−b|, 0)/r` (they sum to 1; `(½, ½)` on the bisector,
/// `(1, 0)` outside the blend strip — on the fillet's zero set the combined
/// direction matches the old arc normal). For the chamfer they are the active
/// plane's coefficients. Used to combine the two wall gradients of a
/// [`HalfSpace::Blend`] carrier.
fn blend_min_weights(da: f32, db: f32, r: f32, mode: BlendMode) -> (f32, f32) {
    match mode {
        BlendMode::Fillet => {
            let pick = if da <= db { (1.0, 0.0) } else { (0.0, 1.0) };
            if r <= 0.0 {
                return pick;
            }
            let h = (r - (da - db).abs()).max(0.0) / r;
            if h <= 0.0 {
                return pick;
            }
            // s = s'(h)/r — the far input's share; h→0 gives the hard pick,
            // h = 1 (bisector) gives (½, ½). Continuous at both ends.
            let s = 0.5 * (1.0 - (1.0 - h) / (1.0 + h * (2.0 - h)).sqrt());
            if da <= db { (1.0 - s, s) } else { (s, 1.0 - s) }
        }
        BlendMode::Chamfer => {
            let chamfer = (da + db - r) * std::f32::consts::FRAC_1_SQRT_2;
            if chamfer < da.min(db) {
                (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2)
            } else if da <= db {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            }
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
        HalfSpace::Blend {
            a,
            a_transform,
            a_flip,
            b,
            b_transform,
            b_flip,
            radius,
            chamfer,
            is_difference,
        } => {
            let (fa, ga) = blend_wall_value_grad(a, a_transform, *a_flip, p);
            let (fb, gb) = blend_wall_value_grad(b, b_transform, *b_flip, p);
            let mode = if *chamfer { BlendMode::Chamfer } else { BlendMode::Fillet };
            // Difference form `f = -blend(-fa, fb)`: ∂f/∂fa = w_a(-fa, fb),
            // ∂f/∂fb = -w_b(-fa, fb).
            let (wa, wb) = if *is_difference {
                let (wa, wb) = blend_min_weights(-fa, fb, *radius, mode);
                (wa, -wb)
            } else {
                blend_min_weights(fa, fb, *radius, mode)
            };
            [
                wa * ga[0] + wb * gb[0],
                wa * ga[1] + wb * gb[1],
                wa * ga[2] + wb * gb[2],
            ]
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
    let shrink = |r: f32| shrink_param(r, thickness);
    match hs {
        HalfSpace::Plane { normal, offset } => Some(HalfSpace::Plane {
            normal: *normal,
            offset: offset_scalar_inward(*offset, thickness),
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
        // Cone nappes separate under offset; Gyroid/TPMS have no analytic
        // offset; a Blend's offset is not a blend of the offset walls (the
        // smin level sets are not parallel surfaces).
        HalfSpace::Cone { .. } | HalfSpace::Gyroid { .. } | HalfSpace::Blend { .. } => None,
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

/// SSOT der Offset-Skalar-Arithmetik: der um `t` nach innen verschobene
/// Flächenparameter ist IMMER der eine Ausdruck `v − t`. Sowohl
/// [`offset_halfspace_inward`] (Carrier-Verschiebung an Stopp-Knoten /
/// Shell-Innenwand) als auch das Blatt-Einfalten
/// ([`crate::offset_fold`], Phase A2 #384) rechnen hierüber, damit ein
/// gefaltetes `r − a` **bit-exakt** gleich dem verschobenen Carrier eines
/// Nachbar-Knotens ist — die analytische Koinzidenz-Klassifikation des
/// Weave vergleicht Parameter exakt (Leitregel: Koinzidenz analytisch,
/// nie Sampling-Epsilon).
#[inline]
pub(crate) fn offset_scalar_inward(v: f32, t: f32) -> f32 {
    v - t
}

/// Gated Variante von [`offset_scalar_inward`] für Parameter, die positiv
/// bleiben müssen (Radien, Halbhöhen, Halbausdehnungen): `None`, wenn der
/// geschrumpfte Wert ≤ 1e-4 mm — dieselbe Degenerationsgrenze, die
/// [`offset_halfspace_inward`] seit jeher verwendet.
#[inline]
pub(crate) fn shrink_param(v: f32, t: f32) -> Option<f32> {
    let s = offset_scalar_inward(v, t);
    (s > 1e-4).then_some(s)
}

#[cfg(test)]
#[path = "csg_tests.rs"]
mod tests;
