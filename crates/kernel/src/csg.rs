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
/// Packs a `FeatureId`'s flat numeric id, a **pattern instance ordinal**
/// and a local sub-index into a single `u64`, so one word names "the +X
/// face of the 2nd copy of feature 7" without a separate side table. The
/// B-Rep substrate keys faces/edges off these ids; the sub-index lets a
/// primitive's six faces (or a sketch's many segments) each get a
/// distinct, Copy, hashable identity.
///
/// Layout (#429/E1, boundary loop #424, positional flag #445) — 51 significant
/// bits, low to high:
///
/// | bits | field | meaning |
/// | --- | --- | --- |
/// | 0..13 | `sub` | per-feature sub-face (sketch entity ordinal, positional flag, cap, inner-wall flag) |
/// | 14..18 | `occurrence` | which wall of this sweep the sketch entity in `sub` produced |
/// | 19..30 | `instance` | pattern instance ordinal; 0 = the original |
/// | 31..50 | `feature` | flat feature id (root ancestor) |
/// | 51..63 | — | reserved, always 0 |
///
/// The sub field has its own ordering rule, and it is load-bearing too: every
/// WALL sub sorts BELOW the reserved cap ids. The weave's signature
/// canonicalisation and face-loop ordering read that order, and putting the
/// positional wall band ABOVE the caps (the first shape #445 took) shattered a
/// swept extrude's rim into 104 fragments — measured in
/// `swept_extrude_rim_consolidates_to_clean_loops`. The reserved band therefore
/// lives at the TOP of the widened field and everything nameable stays under it.
///
/// **Field ORDER is load-bearing, not cosmetic.** Branch words are sorted in
/// several places that then act on the order — the coincidence classes of
/// `weaving::canonicalize`, whose greedy first-occurrence-survives dedup
/// decides which of two coplanar walls keeps its face. Sorting must therefore
/// stay feature-major: `(feature, instance, occurrence, sub)` reads as the
/// nesting it is — a feature has copies, a copy has walls, a wall may be one
/// of several the same sketch curve produced. Putting the new field ABOVE the
/// feature would have saved the `id_hash` of every body that never repeats an
/// entity (ordinal 0 → unchanged word) but interleaved the branches of
/// DIFFERENT features by it, which measurably moved unrelated topology
/// (`onshape_419_revolve_remove_stays_bounded` gained two coplanar duplicate
/// faces). Cheap ids are not worth a reshuffled weave.
///
/// **Why 51 bits and not 64:** the packed id crosses the wasm boundary as
/// a plain JSON number (`branchId` in `bodyBrepFaces`, `a`/`b` in
/// `bodyBrepEdges`). JavaScript integers are exact only below 2^53, so the
/// layout deliberately stays inside that range — 2^51−1 ≈ 2.3e15, a factor 4
/// of headroom. Anything that widens a field further has to move the wire to
/// strings first.
///
/// Being a plain `u32` newtype makes [`Contributors`] `Copy`, which the
/// hot evaluate path relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BranchId(pub u64);

impl BranchId {
    pub const SENTINEL: u64 = u64::MAX;
    /// Bits reserved for the per-feature sub-index. 14 bits → 16384 sub-ids
    /// per feature, laid out as
    /// [`INNER_WALL_FLAG`](Self::INNER_WALL_FLAG) (bit 13) over an outer sub of
    /// 13 bits: 4096 sketch-entity ordinals, then the
    /// [`POSITIONAL_WALL_FLAG`](Self::POSITIONAL_WALL_FLAG) band (#445), then
    /// the reserved cap / blend / cutter ids at the very top.
    pub const SUB_BITS: u32 = 14;
    const SUB_MASK: u64 = 0x3FFF;
    /// The sub field's mask, as a `u32`, for callers that carry a packed wall
    /// name (loop ordinal above the sub) and need the sub alone.
    pub const SUB_MASK_U32: u32 = Self::SUB_MASK as u32;
    /// Bits reserved for the wall-occurrence ordinal (#424). 5 bits → one
    /// sketch curve may fence up to 32 walls of one sweep;
    /// [`crate::sketch::wall_sub_table`] refuses a profile that needs more
    /// rather than aliasing occurrence 32 back onto occurrence 0.
    pub const OCCURRENCE_BITS: u32 = 5;
    const OCCURRENCE_MASK: u64 = 0x1F;
    pub const OCCURRENCE_SHIFT: u32 = Self::SUB_BITS;
    /// Highest occurrence ordinal the field can hold.
    pub const OCCURRENCE_MAX: u32 = Self::OCCURRENCE_MASK as u32;
    /// Width of a packed **wall name** — occurrence plus sub, the pair
    /// [`from_feature_sub`](Self::from_feature_sub) takes and
    /// [`crate::sketch::wall_sub_table`] hands out.
    pub const NAME_BITS: u32 = Self::OCCURRENCE_BITS + Self::SUB_BITS;
    /// Bits reserved for the pattern instance ordinal (#429/E1). 12 bits →
    /// 4095 copies plus the original.
    pub const INSTANCE_BITS: u32 = 12;
    const INSTANCE_MASK: u64 = 0xFFF;
    pub const INSTANCE_SHIFT: u32 = Self::NAME_BITS;
    /// Where the flat feature id starts. 20 bits ≈ 1M feature ids.
    pub const FEATURE_SHIFT: u32 = Self::INSTANCE_SHIFT + Self::INSTANCE_BITS;
    pub const FEATURE_BITS: u32 = 20;
    const FEATURE_MASK: u64 = 0xF_FFFF;
    /// Total significant width of a packed branch word.
    pub const LAYOUT_BITS: u32 = Self::FEATURE_SHIFT + Self::FEATURE_BITS;

    /// Bit 13 of the 14-bit sub field, reserved as the inner-wall flag (DL-75).
    /// A shelled body's inner wall is the outer face offset inward by the wall
    /// thickness — the *same* feature and analytic surface family, so it keeps
    /// the outer face's `feature_id()` (selection still resolves to the owning
    /// feature) but needs a *distinct* branch id so the weave classifies it as
    /// its own face. Setting this bit is that distinct id. All outer sub-ids
    /// (sketch boundary groups + the two extrude caps) live strictly below it
    /// (`< 0x2000`), so the flag never collides; `feature_id()` (the top 20
    /// bits) is untouched.
    pub const INNER_WALL_FLAG: u32 = 0x2000;

    /// Bit 12 of the sub field, reserved for a wall whose sub is a POSITIONAL
    /// boundary-group index rather than a sketch-entity ordinal (#445).
    ///
    /// [`crate::sketch::wall_sub_table`] refuses a profile it cannot attribute
    /// (an SVG `sketch_path`, a mirror copy, a replay failure) and the sweep
    /// then numbers its walls 0, 1, 2, … in contour order. Those numbers used
    /// to be indistinguishable from entity ordinals, and `qWall(op, entity)`
    /// compares `sub == entity` — so `qWall(3, 13)` bound „the 13th wall in
    /// contour order" and called it the wall of curve 13. In #419 that never
    /// showed because the entity ordinals there are three-digit and simply
    /// missed; in #416 they are small and COLLIDED. A silently wrong face is
    /// worse than none.
    ///
    /// Marking it HERE rather than in the query is what makes the guarantee
    /// structural: `wall_sub_table` refuses an entity ordinal that reaches
    /// this flag, so a flagged sub can never equal one, and every present and
    /// future reader that matches an entity against a sub misses it without
    /// having to know the rule. The flag is set in the one funnel
    /// [`from_feature_sub`](Self::from_feature_sub) reaches through
    /// [`map_wall_sub`], so no wall can acquire it — or miss it — on its own.
    ///
    /// A positional wall still gets its OWN branch (the group index rides
    /// below the flag), so faces stay apart and `qCreatedBy(op, WALL)` — which
    /// asks for the role, not for a name — keeps binding all of them. The band
    /// sits BELOW the reserved cap ids, where every other wall sub sits: the
    /// weave sorts branch words and acts on the order (see the type doc).
    pub const POSITIONAL_WALL_FLAG: u32 = 0x1000;

    /// Reserved extrude-cap sub-ids (DL-50). The top/bottom caps of an
    /// extrude are not sketch boundary groups, so they get distinct sub-ids at
    /// the top of the *outer* sub range (just below [`INNER_WALL_FLAG`]). Wall
    /// groups are always small (`< edge_count ≪ 0x7FE`), so there is no
    /// collision.
    pub const EXTRUDE_CAP_TOP_SUB: u32 = 0x1FFF;
    pub const EXTRUDE_CAP_BOTTOM_SUB: u32 = 0x1FFE;

    /// The rounding/bevel surface of a hard edge cutter — Onshape's
    /// `BLEND_FACE` (DERIVED-NAMING-PLAN E2). A fillet's tangent cylinder and a
    /// chamfer's bevel plane are the *one* face the user asked for; everything
    /// else the cutter carries is scaffolding ([`CUTTER_AUX_ROUND`] …). The
    /// sub-id is what [`crate::queries::face_role`] reads, so the role comes
    /// from the provenance, never from the surface class.
    pub const BLEND_SUB: u32 = 0x1FFD;

    /// Scaffolding bands of a hard edge cutter, one per sub-solid. Both
    /// sub-solids of a fillet cutter are `FeatureId::child`ren of the SAME
    /// fillet — `branch_identity()` folds them together — so without separate
    /// bands the tangent cylinder's mantle (sub 0) would overwrite the corner
    /// box's +X plane (sub 0). Measured 2026-07-28 and pinned in
    /// `schritt2_hard_cutter_fillet_face_exists_but_has_no_blend_role`.
    pub const CUTTER_AUX_ROUND: u32 = 0x1FE0;
    pub const CUTTER_AUX_BOX: u32 = 0x1FE8;
    pub const CUTTER_AUX_BEVEL: u32 = 0x1FF0;

    /// Branch id for a whole feature (sub-index 0).
    pub fn from_feature(id: &FeatureId) -> Self {
        Self::from_feature_sub(id, 0)
    }

    /// Branch id for sub-face `name` of feature `id`.
    ///
    /// `name` is a packed **wall name**: the low [`SUB_BITS`](Self::SUB_BITS)
    /// bits are the sub-index, anything above them is the occurrence ordinal
    /// (#424). A plain sub — a cap, a primitive face, a positional group — is
    /// simply a name whose occurrence is 0, so every existing caller passes
    /// one unchanged.
    ///
    /// The instance ordinal comes from the *id*, not from the geometry:
    /// [`FeatureId::instance_ordinal`] reads it off the hierarchical clone
    /// id a pattern mints (`«pattern».«source»~«i»`). That is what makes a
    /// copy's name survive a rearrangement — flipping the pattern direction
    /// moves the body, not the ordinal (#429/E1).
    ///
    /// The sub-index is re-keyed for the sub-solids of a hard edge cutter
    /// ([`cutter_sub`]). Doing the split and the re-key *here* is what keeps
    /// the mapping single — every reader of a branch id (weave, seed pairs,
    /// hot-SDF attribution, queries) goes through this one function.
    pub fn from_feature_sub(id: &FeatureId, name: u32) -> Self {
        let (feature, instance) = id.branch_identity();
        let sub = name & Self::SUB_MASK_U32;
        let occurrence = name >> Self::SUB_BITS;
        Self::from_parts_occ(feature, instance, occurrence, cutter_sub(id, sub))
    }

    /// Pack the three fields (occurrence 0). Each is masked to its width —
    /// a feature id ≥ 2^20, an ordinal ≥ 4096 or a sub ≥ 4096 aliases rather
    /// than corrupting a neighbouring field.
    pub fn from_parts(feature: u32, instance: u32, sub: u32) -> Self {
        Self::from_parts_occ(feature, instance, 0, sub)
    }

    /// Pack all four fields, occurrence ordinal included (#424). The argument
    /// order mirrors the bit layout, outermost field first.
    pub fn from_parts_occ(
        feature: u32,
        instance: u32,
        occurrence: u32,
        sub: u32,
    ) -> Self {
        Self(
            ((feature as u64 & Self::FEATURE_MASK) << Self::FEATURE_SHIFT)
                | ((instance as u64 & Self::INSTANCE_MASK) << Self::INSTANCE_SHIFT)
                | ((occurrence as u64 & Self::OCCURRENCE_MASK) << Self::OCCURRENCE_SHIFT)
                | (sub as u64 & Self::SUB_MASK),
        )
    }

    /// The flat feature id (instance and sub-index stripped).
    pub fn feature_id(self) -> u32 {
        ((self.0 >> Self::FEATURE_SHIFT) & Self::FEATURE_MASK) as u32
    }

    /// The pattern instance ordinal (#429/E1). `0` is the original body —
    /// every branch that a pattern did not copy reads 0, so the field is
    /// invisible to models without patterns.
    pub fn instance(self) -> u32 {
        ((self.0 >> Self::INSTANCE_SHIFT) & Self::INSTANCE_MASK) as u32
    }

    /// WHICH of the walls its sketch entity produced this branch is (#424).
    /// `0` for everything the entity fences exactly once — which is nearly
    /// everything, so the field is invisible to ordinary models.
    ///
    /// One sketch curve can legitimately sweep SEVERAL disjoint walls: a
    /// profile of two separate rails fenced by the same two long lines
    /// (#409's `Extrude 4`), or one contour a long line bounds along two
    /// separate stretches (#419's `Extrude 20`). Ordering is the profile's
    /// canonical DFS — contours in `sketch_arrangement`'s geometric order,
    /// each starting at a G1 break — so the ordinal is a property of the
    /// SHAPE, not of the traversal.
    ///
    /// `qWall(op, entity)` deliberately says nothing about it and binds every
    /// occurrence: Onshape's own `SWEPT_FACE` query carries no such
    /// distinction either (measured on #409's `Mirror 2`). The ordinal exists
    /// to keep two disjoint walls off ONE branch, exactly as
    /// [`INNER_WALL_FLAG`](Self::INNER_WALL_FLAG) keeps a shell's inner wall
    /// off its outer one.
    pub fn occurrence(self) -> u32 {
        ((self.0 >> Self::OCCURRENCE_SHIFT) & Self::OCCURRENCE_MASK) as u32
    }

    /// Same sub-face and occurrence, but on instance `instance` of a
    /// pattern whose feature id is `feature`. This is the whole of
    /// `qInstance`'s resolution rule: a copy's branch differs from the
    /// original's only in the feature and instance fields.
    pub fn on_instance(self, feature: u32, instance: u32) -> Self {
        Self::from_parts_occ(feature, instance, self.occurrence(), self.sub())
    }

    /// The per-feature sub-index (the low [`SUB_BITS`](Self::SUB_BITS) bits,
    /// inner-wall flag included). For a sketch-sweep wall this is the STABLE
    /// sketch-entity ordinal from the sweep's `wall_subs` table (naming
    /// schema v2) — or the positional G1 boundary-group index when the
    /// profile has no table (fallback / legacy). For a cap it is one of the
    /// reserved cap sub-ids.
    pub fn sub(self) -> u32 {
        (self.0 & Self::SUB_MASK) as u32
    }

    /// Map this (outer) branch to its shell inner-wall counterpart (DL-75) by
    /// setting [`INNER_WALL_FLAG`](Self::INNER_WALL_FLAG). Idempotent; leaves
    /// `feature_id()` unchanged.
    pub fn inner_wall(self) -> Self {
        Self(self.0 | Self::INNER_WALL_FLAG as u64)
    }

    /// True if this branch is a shell inner-wall face (DL-75).
    pub fn is_inner_wall(self) -> bool {
        self.0 & Self::INNER_WALL_FLAG as u64 != 0
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Local id segment of each sub-solid a hard edge cutter is built from —
/// minted by [`CsgNode::build_edge_cutter`] / [`CsgNode::build_chamfer_cutter`]
/// and read back by [`cutter_sub`]. They are constants rather than literals so
/// the two ends cannot drift apart.
pub const CUTTER_LOCAL_ROUND: &str = "cut_cyl";
pub const CUTTER_LOCAL_BOX: &str = "cut_box";
pub const CUTTER_LOCAL_BEVEL: &str = "cut_bevel";

/// Re-key the sub-index of a hard edge cutter's sub-solid into its reserved
/// band, and lift the one face the user actually asked for onto
/// [`BranchId::BLEND_SUB`].
///
/// A cutter's sub-solids are `FeatureId::child`ren of the fillet/chamfer, so
/// they share its `branch_identity()`; the bands are what keeps their faces
/// apart within that shared identity. Which face is the blend follows from the
/// construction, not from geometry:
///
/// - **Fillet** — `sub 0` of the tangent cylinder is its mantle
///   ([`primitive_halfspaces`]), and the mantle IS the rolling-ball surface.
///   Its two caps sit outside the corner box by design and never reach the
///   surface, but they get band slots anyway rather than an exception.
/// - **Chamfer** — the bevel box is authored in the frame `(m, s, w)` with `m`
///   the outward bisector, so the plane facing the body is `−X` = `sub 1`.
///
/// Anything that is not a cutter sub-solid passes through untouched.
pub fn cutter_sub(id: &FeatureId, sub: u32) -> u32 {
    match id.local.as_str() {
        CUTTER_LOCAL_ROUND if sub == 0 => BranchId::BLEND_SUB,
        CUTTER_LOCAL_ROUND => BranchId::CUTTER_AUX_ROUND + sub,
        CUTTER_LOCAL_BEVEL if sub == 1 => BranchId::BLEND_SUB,
        CUTTER_LOCAL_BEVEL => BranchId::CUTTER_AUX_BEVEL + sub,
        CUTTER_LOCAL_BOX => BranchId::CUTTER_AUX_BOX + sub,
        _ => sub,
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

    /// Collapse to per-*feature-instance* granularity: every contained
    /// branch is reduced to its owning feature and pattern instance
    /// (sub-index dropped) and duplicates
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
            out.push(BranchId::from_parts(b.feature_id(), b.instance(), 0));
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
            // #385 P1: the loft stacks its profiles along local Z; the honest
            // bound is the union of each profile's 2D AABB lifted onto its own
            // offset plane (lateral profile shifts already live inside each
            // sketch's XY extent). The P1 placeholder lowering (a first-profile
            // extrude) never routes through here — it becomes a `CsgNode::Extrude`
            // in `from_parts` — but keeping the arm exact means a direct
            // `Primitive::Loft` AABB query is correct for P2.
            Primitive::Loft { profiles, center, .. } => {
                let mut acc = Self::EMPTY;
                for s in profiles {
                    let bb2 = s.sketch.aabb();
                    if bb2.is_empty() {
                        continue;
                    }
                    // Re-express the profile's own 2D AABB in the shared loft
                    // frame (own → common via `frame_map` forward), then lift
                    // onto its offset plane and shift by the stack `center`.
                    let corners = [
                        [bb2.min[0], bb2.min[1]],
                        [bb2.max[0], bb2.min[1]],
                        [bb2.min[0], bb2.max[1]],
                        [bb2.max[0], bb2.max[1]],
                    ];
                    for c in corners {
                        let scaled = [
                            c[0] * s.frame_map.scale[0],
                            c[1] * s.frame_map.scale[1],
                            0.0,
                        ];
                        let r = quat_rotate(s.frame_map.rotation, scaled);
                        let pt = [
                            center[0] + r[0] + s.frame_map.translate[0],
                            center[1] + r[1] + s.frame_map.translate[1],
                            center[2] + s.offset,
                        ];
                        let p = Self { min: pt, max: pt };
                        acc = if acc.is_empty() { p } else { Self::union(acc, p) };
                    }
                }
                acc
            }
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
    pub branch: u64,
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
    pub(crate) fn contains(&self, branch: u64) -> bool {
        self.faces.iter().any(|f| f.branch == branch)
            || self.parent.is_some_and(|p| p.contains(branch))
    }

    pub(crate) fn contains_sub(&self, feature: &FeatureId, sub: u32) -> bool {
        self.contains(BranchId::from_feature_sub(feature, sub).as_u64())
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
    /// #384 Phase D2: a transparent FIELD pass-through that remaps its child's
    /// contributor set onto the DL-75 shell inner-wall band. Its distance is
    /// *identical* to the child (`d` unchanged); only the provenance is
    /// rewritten via [`Contributors::mapped_inner`] so the copied inner wall
    /// gets distinct, selectable branch ids — exactly what the field-based
    /// closed shell did inline at [`CsgNode::Shell`]'s `evaluate_open` arm.
    ///
    /// A dualised closed shell expands to
    /// `Difference(body, InnerWall(Offset(body, −t)))` (see
    /// [`CsgNode::expand_shells`]): the subtracted eroded copy IS the inner
    /// wall, so wrapping it in `InnerWall` reproduces the old
    /// `c_ret.mapped_inner()` classification while the erosion inherits the
    /// exact, already-validated offset path (push_down_offsets +
    /// fold_offset_leaves + ArcChannel) instead of the inexact
    /// `max(d, −(d+t))` field modifier.
    ///
    /// Because [`BranchId::INNER_WALL_FLAG`] is a single bit, only ONE level
    /// of nesting is meaningful (like shell-of-shell) — `mapped_inner` is
    /// idempotent. Every non-provenance traversal recurses transparently into
    /// the child.
    InnerWall(Box<CsgNode>),
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
    /// #385 P2: N-profile loft — a z-bracketed 2D cross-section morph
    /// (`docs/LOFT-FEATURE-PLAN.md` Option a). At loft-local height `z` the 2D
    /// field is the piecewise-linear (bracket) interpolation of the neighbouring
    /// profile fields, then hard-clamped in `z` to the profile slab exactly like
    /// [`CsgNode::Extrude`]'s caps (flat, pickable lids). The `sections` are
    /// stored with **centred** `z` (the slab spans `[-half_span, +half_span]`,
    /// so the outer placement puts local `z = 0` at mid-span) and ascending;
    /// each carries the rigid `map` reframing loft-local `(x, y)` into that
    /// section's own sketch coordinates. Single [`FeatureId`] contributor, same
    /// rationale as Extrude/Revolve/Helix/Sweep. `Arc` keeps node clones cheap.
    Loft {
        id: FeatureId,
        sections: std::sync::Arc<Vec<LoftNodeSection>>,
        /// Half the axial span `(offset_last − offset_0) / 2`; the z-clamp is
        /// the symmetric `|z| − half_span` (IQ extrude cap composition).
        half_span: f32,
        /// See [`CsgNode::Extrude::wall_subs`].
        wall_subs: Option<std::sync::Arc<Vec<u32>>>,
    },
}

/// #385 P2: one lowered loft cross-section (see [`CsgNode::Loft`]). Carries the
/// 2D profile, its **centred** z-coordinate along the loft axis, and the rigid
/// map from loft-local `(x, y)` into this section's own sketch coordinates
/// ([`Transform::world_to_local`] of `map`; a Z-rotation + XY-translation, so
/// distance-preserving).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoftNodeSection {
    pub sketch: Box<Sketch2d>,
    pub z: f32,
    pub map: Transform,
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
            // #385 P2: the real z-bracketed cross-section morph. The profiles
            // are stored with CENTRED z (slab `[-half_span, +half_span]`), so an
            // inner placement translate of `center + mid·axis` puts loft-local
            // z = 0 at mid-span (letting the eval / opcode reuse the symmetric
            // IQ extrude cap composition). Each section keeps its resolver-baked
            // `frame_map` so the morph reads every profile in the shared frame.
            Primitive::Loft { profiles, center, wall_subs } => {
                let z0 = profiles.first().map(|s| s.offset).unwrap_or(0.0);
                let z1 = profiles.last().map(|s| s.offset).unwrap_or(0.0);
                let mid = (z0 + z1) * 0.5;
                let half_span = ((z1 - z0) * 0.5).abs();
                let sections: Vec<LoftNodeSection> = profiles
                    .into_iter()
                    .map(|s| LoftNodeSection {
                        sketch: Box::new(s.sketch),
                        z: s.offset - mid,
                        map: s.frame_map,
                    })
                    .collect();
                let node = CsgNode::Loft {
                    id: id.clone(),
                    sections: std::sync::Arc::new(sections),
                    half_span,
                    wall_subs: wall_subs.map(std::sync::Arc::new),
                };
                let tz = center[2] + mid;
                if center[0] == 0.0 && center[1] == 0.0 && tz == 0.0 {
                    node
                } else {
                    CsgNode::Transform(
                        Transform {
                            translate: [center[0], center[1], tz],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0],
                        },
                        Box::new(node),
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
                | CsgNode::InnerWall(inner)
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
                CsgNode::Loft { sections, half_span, wall_subs, .. } => {
                    return Some(loft_export_primitive(
                        sections,
                        *half_span,
                        [0.0, 0.0, -*half_span],
                        wall_subs.as_deref().cloned(),
                    ))
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
                    CsgNode::Loft { sections, half_span, wall_subs, .. } => {
                        // The inner placement is `center + mid·axis` with
                        // `mid == half_span`; peel the `mid` back out of z.
                        return Some(loft_export_primitive(
                            sections,
                            *half_span,
                            [
                                t.translate[0],
                                t.translate[1],
                                t.translate[2] - *half_span,
                            ],
                            wall_subs.as_deref().cloned(),
                        ))
                    }
                    _ => cur = inner,
                },
                _ => return None,
            }
        }
    }
}

impl CsgNode {
    /// Re-brand every embedded identity in this subtree from the `from`
    /// id-subtree onto `to`.
    ///
    /// **Why this exists (#429/E1).** A pattern copy is
    /// `source_feature.clone()` with a fresh `DecodedFeature::id`. But the
    /// geometry lives in `DecodedFeature::node`, and every id-carrying node
    /// variant stores its **own** `FeatureId` — the source's. Measured
    /// 2026-07-28 on a 3-copy linear pattern of a box: the clones' decoded
    /// ids were `2.1~1` / `2.1~2`, yet all three bodies wove the branch words
    /// `1/0/0 … 1/0/5`. The new id never reached the branch, so all three
    /// copies carried one name and only the positional `spatial_index`
    /// (#431) told them apart. Renaming the feature without renaming its node
    /// is the actual root cause — not the `as_u32_or_root()` collapse the
    /// plan suspected (that one is real but downstream of this).
    ///
    /// Branch words embedded in `Shell`'s open faces and `MaskedBlend`'s
    /// provenance are re-branded too, so an opened or filleted body patterns
    /// correctly.
    ///
    /// The match is exhaustive on purpose: a new id-carrying variant must not
    /// be able to slip through silently.
    pub fn rebrand(&mut self, from: &FeatureId, to: &FeatureId) {
        let rebrand_id = |id: &mut FeatureId| {
            if let Some(next) = id.rebase(from, to) {
                *id = next;
            }
        };
        // A branch word carries the *flat* feature id, so re-branding one is
        // a field swap: keep the sub, take the target's feature + instance.
        let (from_feat, _) = from.branch_identity();
        let (to_feat, to_inst) = to.branch_identity();
        let rebrand_branch = |w: &mut u64| {
            let b = BranchId(*w);
            if b.feature_id() == from_feat {
                *w = BranchId::from_parts(to_feat, to_inst, b.sub()).as_u64();
            }
        };
        match self {
            Self::Empty => {}
            Self::Leaf { id, .. }
            | Self::Extrude { id, .. }
            | Self::Revolve { id, .. }
            | Self::Helix { id, .. }
            | Self::Sweep { id, .. }
            | Self::Loft { id, .. } => rebrand_id(id),
            Self::Transform(_, inner)
            | Self::Twist(inner, _)
            | Self::Bend(inner, _)
            | Self::Displace(inner, _, _)
            | Self::InnerWall(inner)
            | Self::Offset(inner, _) => inner.rebrand(from, to),
            Self::Union(a, b)
            | Self::Difference(a, b)
            | Self::Intersection(a, b)
            | Self::SmoothUnion(a, b, _, _)
            | Self::SmoothDifference(a, b, _, _)
            | Self::Morph(a, b, _) => {
                a.rebrand(from, to);
                b.rebrand(from, to);
            }
            Self::Shell(inner, _, open) => {
                inner.rebrand(from, to);
                for f in open {
                    rebrand_branch(&mut f.branch);
                }
            }
            Self::MaskedBlend { a, b, provenance, .. } => {
                a.rebrand(from, to);
                b.rebrand(from, to);
                rebrand_branch(&mut provenance.branch);
                rebrand_branch(&mut provenance.wall_a);
                rebrand_branch(&mut provenance.wall_b);
            }
        }
    }

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
            CsgNode::InnerWall(inner) => inner.all_finite(),
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
            CsgNode::Loft { sections, half_span, .. } => {
                half_span.is_finite()
                    && sections.iter().all(|s| {
                        s.z.is_finite()
                            && all_finite(&s.map)
                            && all_finite(s.sketch.as_ref())
                    })
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

    /// Apply per-edge fillets to the body tree.
    ///
    /// #383/#406: fillets whose edge is straight between two planar faces carry
    /// analytic [`FilletCutter`] geometry. Those are realised as HARD CUTTERS —
    /// an exact Box−Cylinder body subtracted from the whole tree (Onshape
    /// rolling-ball semantics: cylindrical rounding along the edge, sharp
    /// corners). This is the only mechanism that can round a LONE primitive's
    /// own edge (a cube edge, `feature_a == feature_b`, has no Boolean junction
    /// to rewrite — the historic #406 bug). Every remaining fillet (curved
    /// edges, non-planar faces, or a cutter that fails the convexity/orthogonality
    /// gate) falls through to the legacy `smin` mask-blend path
    /// ([`Self::apply_mask_fillets`]).
    pub fn apply_edge_fillets(self, fillets: &[EdgeFillet]) -> Self {
        if fillets.is_empty() {
            return self;
        }
        // Fillets with analytic cutter geometry try the hard path; the rest go
        // straight to the mask path.
        let mask_only: Vec<EdgeFillet> =
            fillets.iter().filter(|f| f.cutter.is_none()).cloned().collect();
        let with_cutter: Vec<EdgeFillet> =
            fillets.iter().filter(|f| f.cutter.is_some()).cloned().collect();

        // 1. Legacy smin mask blend for the non-cutter fillets.
        let mut node = self.apply_mask_fillets(&mask_only);

        // 2. Hard cutters, subtracted at body level. Convexity + orthogonality
        //    are decided against the live field of the (already mask-filleted)
        //    body; a cutter that does not apply falls back to the mask path so
        //    a convex Boolean-junction edge still rounds.
        let mut fallback: Vec<EdgeFillet> = Vec::new();
        for f in &with_cutter {
            match node.build_edge_cutter(f) {
                Some(cutter) => {
                    node = CsgNode::Difference(Box::new(node), Box::new(cutter));
                }
                None => fallback.push(f.clone()),
            }
        }
        if !fallback.is_empty() {
            node = node.apply_mask_fillets(&fallback);
        }
        node
    }

    /// #383/#406: build the exact Box−Cylinder cutter for one straight, convex,
    /// orthogonal fillet edge, ready to subtract from `self`. Returns `None`
    /// when the edge fails a gate (degenerate, non-orthogonal dihedral, or
    /// concave — all deferred to later fillet phases, see
    /// `docs/FILLET-CUTTER-PLAN.md`). Uses only [`Primitive::Box`] /
    /// [`Primitive::Cylinder`] + [`CsgNode::Difference`], so it flows through
    /// every SDF evaluator without a new opcode.
    fn build_edge_cutter(&self, f: &EdgeFillet) -> Option<CsgNode> {
        use fielddraft_protocol::{Primitive, Transform};
        let c = f.cutter.as_ref()?;
        let r = c.radius;
        let fr =
            edge_cutter_frame(self, c.edge_start, c.edge_end, c.normal_a, c.normal_b, r).ok()?;
        let (mid, u, v, w, len, cen) = (fr.mid, fr.u, fr.v, fr.w, fr.len, fr.inner);

        let box_id = f.fillet_id.child(CUTTER_LOCAL_BOX);
        let cyl_id = f.fillet_id.child(CUTTER_LOCAL_ROUND);

        // Corner box: r×r cross-section on the edge, full edge length along w.
        let box_node = CsgNode::Transform(
            Transform { translate: mid, rotation: quat_from_cols3(u, v, w), scale: [1.0; 3] },
            Box::new(CsgNode::Leaf {
                id: box_id,
                primitive: Primitive::Box {
                    center: [0.0; 3],
                    half_extents: [r, r, len * 0.5],
                },
            }),
        );
        // Tangent cylinder: axis along w (local Y → w), radius r, slightly
        // longer than the edge so its flat caps sit outside the box → the
        // rounding runs crisply to the sharp ends.
        let cyl_node = CsgNode::Transform(
            Transform {
                translate: cen,
                rotation: quat_from_cols3(u, w, cross3(u, w)),
                scale: [1.0; 3],
            },
            Box::new(CsgNode::Leaf {
                id: cyl_id,
                primitive: Primitive::Cylinder {
                    center: [0.0; 3],
                    radius: r,
                    half_height: len * 0.5 + r,
                },
            }),
        );
        Some(CsgNode::Difference(Box::new(box_node), Box::new(cyl_node)))
    }

    /// #414: apply per-edge chamfers as hard cutters, subtracted at body level.
    ///
    /// The structurally simpler brother of [`Self::apply_edge_fillets`]: where a
    /// fillet subtracts `corner box − tangent cylinder`, a chamfer subtracts a
    /// single oriented box whose inner face IS the bevel plane. There is NO
    /// `smin` fallback — a chamfer has no field-blend analogue, and rounding an
    /// edge the user asked to bevel would be silently wrong. An edge whose
    /// dihedral fails the gate is therefore skipped here; the `chamfer_edge`
    /// feature runs the same gate at execute time and reports it as a hard
    /// feature error, so the skip is never the user's first signal.
    pub fn apply_edge_chamfers(self, chamfers: &[EdgeChamfer]) -> Self {
        if chamfers.is_empty() {
            return self;
        }
        let mut node = self;
        for c in chamfers {
            if let Some(cutter) = node.build_chamfer_cutter(c) {
                node = CsgNode::Difference(Box::new(node), Box::new(cutter));
            }
        }
        node
    }

    /// #414: the exact bevel cutter for one straight, convex, orthogonal edge.
    ///
    /// With EQUAL_OFFSETS the bevel plane cuts `d` back from the edge on BOTH
    /// adjacent faces, i.e. it passes through `mid − d·n_b` and `mid − d·n_a`
    /// and its outward normal is the face bisector `m = normalize(n_a + n_b)`
    /// (a 45° cut at an orthogonal dihedral). In the local frame `(u, v)` of the
    /// two face normals the material near the edge is `{u ≤ 0, v ≤ 0}` and the
    /// wedge to remove is `{u + v ≥ −d}` — so ONE oriented box in the frame
    /// `(m, s, w)` suffices:
    ///
    /// - along `m`: from the bevel plane (`−d/√2`) outward by a margin `d`,
    /// - along `s = w × m`: `±d`, provably non-binding on material
    ///   (`u, v ∈ [−d, 0]` on the wedge ⟹ `|u − v| ≤ d < d·√2`),
    /// - along `w`: exactly the edge length, so the ends stay sharp (#383).
    ///
    /// No intersection with a corner box is needed, and no new opcode: a single
    /// [`Primitive::Box`] under a [`CsgNode::Transform`] flows through every
    /// evaluator (CPU, WGSL, sdf-vm, CGAL) like any other leaf.
    fn build_chamfer_cutter(&self, c: &EdgeChamfer) -> Option<CsgNode> {
        use fielddraft_protocol::{Primitive, Transform};
        let d = c.distance;
        let fr =
            edge_cutter_frame(self, c.edge_start, c.edge_end, c.normal_a, c.normal_b, d).ok()?;
        // Outward bisector of the two faces = the bevel plane's normal.
        let m = normalize3([
            fr.na[0] + fr.nb[0],
            fr.na[1] + fr.nb[1],
            fr.na[2] + fr.nb[2],
        ])?;
        let s = cross3(fr.w, m); // unit, ⟂ m and w; (m, s, w) is right-handed
        // Plane offset from the edge along m, and the outward margin past it.
        let inset = d / 2.0f32.sqrt();
        let margin = d;
        let m_half = 0.5 * (inset + margin);
        let m_off = 0.5 * (margin - inset);
        let centre = [
            fr.mid[0] + m_off * m[0],
            fr.mid[1] + m_off * m[1],
            fr.mid[2] + m_off * m[2],
        ];
        Some(CsgNode::Transform(
            Transform {
                translate: centre,
                rotation: quat_from_cols3(m, s, fr.w),
                scale: [1.0; 3],
            },
            Box::new(CsgNode::Leaf {
                id: c.chamfer_id.child(CUTTER_LOCAL_BEVEL),
                primitive: Primitive::Box {
                    center: [0.0; 3],
                    half_extents: [m_half, d, fr.len * 0.5],
                },
            }),
        ))
    }

    /// Legacy per-edge fillet: rewrite the matching Boolean junction into a
    /// spatially-masked `smin` blend ([`CsgNode::MaskedBlend`]). Used for
    /// fillets the analytic hard cutter does not cover (curved edges, non-planar
    /// faces, non-orthogonal/concave junctions).
    fn apply_mask_fillets(self, fillets: &[EdgeFillet]) -> Self {
        if fillets.is_empty() {
            return self;
        }
        match self {
            CsgNode::Union(a, b) => {
                let a = a.apply_mask_fillets(fillets);
                let b = b.apply_mask_fillets(fillets);
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
                let a = a.apply_mask_fillets(fillets);
                let b = b.apply_mask_fillets(fillets);
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
                CsgNode::Transform(t, Box::new(inner.apply_mask_fillets(fillets)))
            }
            CsgNode::Twist(inner, r) => {
                CsgNode::Twist(Box::new(inner.apply_mask_fillets(fillets)), r)
            }
            CsgNode::Bend(inner, r) => {
                CsgNode::Bend(Box::new(inner.apply_mask_fillets(fillets)), r)
            }
            CsgNode::Shell(inner, t, open) => {
                CsgNode::Shell(Box::new(inner.apply_mask_fillets(fillets)), t, open)
            }
            CsgNode::Offset(inner, delta) => {
                CsgNode::Offset(Box::new(inner.apply_mask_fillets(fillets)), delta)
            }
            CsgNode::InnerWall(inner) => {
                CsgNode::InnerWall(Box::new(inner.apply_mask_fillets(fillets)))
            }
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.apply_mask_fillets(fillets)), d, f)
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
            CsgNode::InnerWall(inner) => {
                CsgNode::InnerWall(Box::new(inner.apply_global_round(r)))
            }
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.apply_global_round(r)), d, f)
            }
            other => other,
        }
    }

    /// #384 Phase D2: structurally expand every CLOSED shell (empty
    /// `open_faces`) into its complement duality so it inherits the exact,
    /// already-validated offset erosion path instead of the metrically
    /// inexact `max(d, −(d+t))` field modifier.
    ///
    /// Runs recursively over the whole tree, IMMEDIATELY BEFORE
    /// [`Self::push_down_offsets`] in every build path (so the synthesised
    /// `Offset(body, −t)` is picked up by the pushdown / `fold_offset_leaves`
    /// / ArcChannel passes) and AFTER `apply_edge_fillets`/`apply_global_round`
    /// (so those never see the artificial inner-wall `Difference`).
    ///
    /// Rewrite for a closed shell:
    ///
    ///   `Shell(body, t, [])  →  Difference(body, InnerWall(Offset(body, −t)))`
    ///
    /// Field check (before folding): `Difference(A, B)` evaluates to
    /// `max(dA, −dB)`; here `B = Offset(body, −t)` evaluates to
    /// `d − (−t) = d + t` (the evaluator computes `inner.sdf − δ`, so
    /// `δ = −t` gives `d + t`). Thus the field is `max(d, −(d + t))` —
    /// bit-identical to today's closed [`CsgNode::Shell`] modifier
    /// ([`shell_wall_sdf`] with `d_ret = d`) BEFORE any folding. AFTER
    /// `fold_offset_leaves` the erosion becomes an exact leaf offset, so the
    /// inner wall is metrically exact: strictly ≥ today's quality, with no
    /// regression on non-foldable leaves (they keep the same `f − δ` fallback,
    /// hence the same field).
    ///
    /// OPEN shells (non-empty `open_faces`) are left UNCHANGED on the
    /// [`CsgNode::Shell`] field path: their retained-face masking (`d_ret` is a
    /// per-sub-face field mask) has no clean complement-duality expression.
    ///
    /// The [`InnerWall`](CsgNode::InnerWall) wrapper reproduces the DL-75
    /// inner-wall contributor remap the closed shell did inline
    /// (`c_ret.mapped_inner()`), so selection/picking of the cavity surface is
    /// unchanged.
    pub fn expand_shells(self) -> Self {
        match self {
            CsgNode::Shell(inner, t, open) => {
                let e = inner.expand_shells();
                if open.is_empty() {
                    CsgNode::Difference(
                        Box::new(e.clone()),
                        Box::new(CsgNode::InnerWall(Box::new(CsgNode::Offset(
                            Box::new(e),
                            -t,
                        )))),
                    )
                } else {
                    CsgNode::Shell(Box::new(e), t, open)
                }
            }
            CsgNode::InnerWall(inner) => CsgNode::InnerWall(Box::new(inner.expand_shells())),
            CsgNode::Offset(inner, delta) => {
                CsgNode::Offset(Box::new(inner.expand_shells()), delta)
            }
            CsgNode::Union(a, b) => {
                CsgNode::Union(Box::new(a.expand_shells()), Box::new(b.expand_shells()))
            }
            CsgNode::Difference(a, b) => {
                CsgNode::Difference(Box::new(a.expand_shells()), Box::new(b.expand_shells()))
            }
            CsgNode::Intersection(a, b) => {
                CsgNode::Intersection(Box::new(a.expand_shells()), Box::new(b.expand_shells()))
            }
            CsgNode::SmoothUnion(a, b, r, m) => CsgNode::SmoothUnion(
                Box::new(a.expand_shells()),
                Box::new(b.expand_shells()),
                r,
                m,
            ),
            CsgNode::SmoothDifference(a, b, r, m) => CsgNode::SmoothDifference(
                Box::new(a.expand_shells()),
                Box::new(b.expand_shells()),
                r,
                m,
            ),
            CsgNode::Morph(a, b, t) => {
                CsgNode::Morph(Box::new(a.expand_shells()), Box::new(b.expand_shells()), t)
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
            } => CsgNode::MaskedBlend {
                a: Box::new(a.expand_shells()),
                b: Box::new(b.expand_shells()),
                mask_polyline,
                mask_radius,
                blend_radius,
                blend_mode,
                is_difference,
                provenance,
            },
            CsgNode::Transform(t, inner) => {
                CsgNode::Transform(t, Box::new(inner.expand_shells()))
            }
            CsgNode::Twist(inner, rate) => CsgNode::Twist(Box::new(inner.expand_shells()), rate),
            CsgNode::Bend(inner, rate) => CsgNode::Bend(Box::new(inner.expand_shells()), rate),
            CsgNode::Displace(inner, d, f) => {
                CsgNode::Displace(Box::new(inner.expand_shells()), d, f)
            }
            // No CsgNode children (leaves, sketch lifts, empty).
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
            // #384 D2: transparent — the eroded copy inside must have its own
            // `Offset` pushed down to the leaves (this pass runs right AFTER
            // `expand_shells`, so `InnerWall(Offset(body, −t))` is present here).
            CsgNode::InnerWall(inner) => {
                CsgNode::InnerWall(Box::new(inner.push_down_offsets()))
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
        // Single source of truth for the pushdown decision, shared verbatim
        // with the post-pass invariant checker
        // ([`crate::offset_invariants::check_offsets_pushed`]): if the offset
        // cannot be pushed through this node, it STOPS here and keeps today's
        // `f − δ` fallback. The match below then only ever destructures a
        // variant the predicate has already blessed as pushable.
        if !offset_can_push_through(&self, delta) {
            // Everything the predicate rejects (dual Boolean cases,
            // smin/MaskedBlend/Morph, warp ops, Shell, scaled transforms,
            // leaves): keep today's `f − δ` right here. Leaf fields are exact
            // SDFs, so `f − δ` on a leaf IS the exact offset.
            return CsgNode::Offset(Box::new(self.push_down_offsets()), delta);
        }
        match self {
            // Stacked offsets evaluate as `f − δ₂ − δ` today; fold the sum and
            // let its sign pick the exact rules.
            CsgNode::Offset(inner, d2) => inner.push_down_offset(delta + d2),
            // Dilation (`δ > 0`) distributes over Union.
            CsgNode::Union(a, b) => CsgNode::Union(
                Box::new(a.push_down_offset(delta)),
                Box::new(b.push_down_offset(delta)),
            ),
            // Erosion (`δ < 0`) distributes over Intersection.
            CsgNode::Intersection(a, b) => CsgNode::Intersection(
                Box::new(a.push_down_offset(delta)),
                Box::new(b.push_down_offset(delta)),
            ),
            // Erosion of a difference erodes the minuend and DILATES the
            // subtrahend: `erode(A \ B) = erode(A) \ dilate(B)`.
            CsgNode::Difference(a, b) => CsgNode::Difference(
                Box::new(a.push_down_offset(delta)),
                Box::new(b.push_down_offset(-delta)),
            ),
            // A rigid placement (unit scale) is an isometry — offset commutes.
            CsgNode::Transform(t, inner) => {
                CsgNode::Transform(t, Box::new(inner.push_down_offset(delta)))
            }
            // InnerWall is field-transparent (it only remaps contributor ids),
            // so an offset commutes: `Offset(InnerWall(X), δ)` ≡
            // `InnerWall(Offset(X, δ))`. Keeping the wrapper OUTSIDE preserves
            // the pipeline invariant that InnerWall stays the direct right
            // child of its Difference (F1 aus dem #384-Fuzz-Harness: erosion
            // over a closed shell dilates the cavity subtrahend — without this
            // arm the offset wrapped the InnerWall and broke `collect_parity`'s
            // structural assumption, and the cavity never folded).
            CsgNode::InnerWall(inner) => {
                CsgNode::InnerWall(Box::new(inner.push_down_offset(delta)))
            }
            // Unreachable: `offset_can_push_through` returned true, so `self`
            // is one of the variants above. Keep the `f − δ` fallback as a
            // belt-and-braces net rather than a panic.
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
            // #384 D2: transparent — the eroded inner-wall copy's leaf offsets
            // (`Offset(leaf, −t)`, produced by the preceding `push_down_offsets`)
            // must be folded here too, or the inner wall keeps the inexact
            // `f − δ` field and the metric gain is lost.
            CsgNode::InnerWall(inner) => {
                CsgNode::InnerWall(Box::new(inner.fold_offset_leaves()))
            }
            other => other,
        }
    }

    /// #384 offset_capability: read-only walk that records the capability
    /// downgrades of every residual `Offset(inner, δ)` still standing after
    /// `push_down_offsets` + `fold_offset_leaves`. This handles the **non-dual**
    /// stops only — the dual Boolean seams (`Union` at δ<0, `Intersection` /
    /// `Difference` at δ>0) are classified at channel-synthesis time
    /// (`offset_channel::synthesize_offset_channels_reported`), so they are
    /// deliberately skipped here to avoid double counting.
    ///
    /// Purely additive: never mutates the tree, so the field is unaffected.
    pub fn collect_offset_capability(
        &self,
        report: &mut crate::offset_capability::CapabilityReport,
    ) {
        match self {
            CsgNode::Offset(inner, delta) => {
                let d = *delta;
                if d != 0.0 && d.is_finite() && !offset_is_dual_stop(inner, d) {
                    classify_residual_offset(inner, d, report);
                }
                inner.collect_offset_capability(report);
            }
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b)
            | CsgNode::SmoothUnion(a, b, _, _)
            | CsgNode::SmoothDifference(a, b, _, _)
            | CsgNode::Morph(a, b, _) => {
                a.collect_offset_capability(report);
                b.collect_offset_capability(report);
            }
            CsgNode::MaskedBlend { a, b, .. } => {
                a.collect_offset_capability(report);
                b.collect_offset_capability(report);
            }
            CsgNode::Shell(inner, _, _) => {
                // Nach `expand_shells` sind geschlossene Shells wegkompiliert —
                // ein residualer Shell-Knoten ist eine OFFENE Shell auf dem
                // Feld-Pfad. Ihre Innenwand ist die Nullmenge von `d + t` des
                // Kind-Felds: maßhaltig nur, wenn das Kind-Feld eine exakte
                // Distanz ist (einzelnes exaktes Blatt unter höchstens rigiden
                // Transforms). Über Booleans/Blends drift die Wand wie f−δ.
                if !subtree_field_is_exact_distance(inner) {
                    report.note_conservative(
                        "Offene Shell auf einem zusammengesetzten Körper — die Wand folgt dem Feld-Pfad, an Boolean-Nähten ist die Wandstärke nur näherungsweise maßhaltig.",
                    );
                }
                inner.collect_offset_capability(report);
            }
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Displace(inner, _, _)
            | CsgNode::InnerWall(inner) => inner.collect_offset_capability(report),
            CsgNode::Empty
            | CsgNode::Leaf { .. }
            | CsgNode::Extrude { .. }
            | CsgNode::Revolve { .. }
            | CsgNode::Helix { .. }
            | CsgNode::Sweep { .. }
            | CsgNode::Loft { .. } => {}
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
            // #384 D2: do NOT descend into the dualised inner wall. Its eroded
            // copy shares the outer body's FeatureId, and this map is keyed by
            // feature id (the mesh contributor buffer is feature-collapsed, so
            // inner/outer verts cannot be told apart in developable-face
            // simplification anyway). Recursing would let the eroded primitive
            // overwrite the OUTER one and snap the visible outer surface inward.
            // Skipping keeps the outer primitive, matching the pre-D2 closed
            // shell (where the inner wall was never a separate leaf).
            CsgNode::InnerWall(_) => {}
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
            CsgNode::Extrude { .. } | CsgNode::Revolve { .. } | CsgNode::Helix { .. } | CsgNode::Sweep { .. } | CsgNode::Loft { .. } | CsgNode::Empty => {}
        }
    }

    /// Collect all leaf FeatureIds reachable from this node.
    fn contains_any_target(&self, target_ids: &[FeatureId]) -> bool {
        match self {
            CsgNode::Leaf { id, .. } | CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } | CsgNode::Helix { id, .. } | CsgNode::Sweep { id, .. } | CsgNode::Loft { id, .. } => {
                let root = id.root_ancestor();
                target_ids.iter().any(|tid| root == tid)
            }
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Shell(inner, _, _)
            | CsgNode::InnerWall(inner)
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

    /// True if the subtree contains a domain warp (Twist/Bend). An offset over
    /// a warp has no exact expansion — the warp is not an isometry, so no
    /// offset-closed class survives it (docs/OFFSET-TREE-PLAN.md §2). The
    /// `offset` feature uses this to refuse with a clear error rather than
    /// leave a silent `f − δ` approximation.
    pub(crate) fn contains_warp(&self) -> bool {
        match self {
            CsgNode::Twist(..) | CsgNode::Bend(..) => true,
            CsgNode::Transform(_, inner)
            | CsgNode::Shell(inner, _, _)
            | CsgNode::InnerWall(inner)
            | CsgNode::Offset(inner, _)
            | CsgNode::Displace(inner, _, _) => inner.contains_warp(),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b)
            | CsgNode::SmoothUnion(a, b, _, _)
            | CsgNode::SmoothDifference(a, b, _, _)
            | CsgNode::Morph(a, b, _)
            | CsgNode::MaskedBlend { a, b, .. } => a.contains_warp() || b.contains_warp(),
            CsgNode::Leaf { .. }
            | CsgNode::Extrude { .. }
            | CsgNode::Revolve { .. }
            | CsgNode::Helix { .. }
            | CsgNode::Sweep { .. }
            | CsgNode::Loft { .. }
            | CsgNode::Empty => false,
        }
    }

    pub fn leaf_ids(&self) -> Vec<FeatureId> {
        let mut out = Vec::new();
        self.collect_leaf_ids(&mut out);
        out
    }

    /// Rewrite EVERY leaf id in the subtree to `new_id`.
    ///
    /// Needed whenever a decoded feature is COPIED onto the spine under a
    /// fresh id (the Boolean's `keep_tools` working copies): the geometry has
    /// to answer to the copy's own id, or the two id-based passes disagree —
    /// [`crate::body::partition`] matches on the DECODED feature id while
    /// [`wrap_targeted`] matches on the LEAF id, so a copy whose two ids
    /// differ would dock into one body and carve another. Re-tagging also
    /// keeps `qCreatedBy(tool)` naming the tool's own body alone instead of
    /// additionally naming the cut faces the copy leaves in the target.
    pub fn retag_leaf_ids(&mut self, new_id: &FeatureId) {
        match self {
            CsgNode::Leaf { id, .. }
            | CsgNode::Extrude { id, .. }
            | CsgNode::Revolve { id, .. }
            | CsgNode::Helix { id, .. }
            | CsgNode::Sweep { id, .. }
            | CsgNode::Loft { id, .. } => *id = new_id.clone(),
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Shell(inner, _, _)
            | CsgNode::InnerWall(inner)
            | CsgNode::Offset(inner, _)
            | CsgNode::Displace(inner, _, _) => inner.retag_leaf_ids(new_id),
            CsgNode::Union(a, b)
            | CsgNode::Difference(a, b)
            | CsgNode::Intersection(a, b)
            | CsgNode::SmoothUnion(a, b, _, _)
            | CsgNode::SmoothDifference(a, b, _, _)
            | CsgNode::Morph(a, b, _) => {
                a.retag_leaf_ids(new_id);
                b.retag_leaf_ids(new_id);
            }
            CsgNode::MaskedBlend { a, b, .. } => {
                a.retag_leaf_ids(new_id);
                b.retag_leaf_ids(new_id);
            }
            CsgNode::Empty => {}
        }
    }

    fn collect_leaf_ids(&self, out: &mut Vec<FeatureId>) {
        match self {
            CsgNode::Leaf { id, .. } => out.push(id.clone()),
            CsgNode::Transform(_, inner)
            | CsgNode::Twist(inner, _)
            | CsgNode::Bend(inner, _)
            | CsgNode::Shell(inner, _, _)
            | CsgNode::InnerWall(inner)
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
            CsgNode::Extrude { id, .. } | CsgNode::Revolve { id, .. } | CsgNode::Helix { id, .. } | CsgNode::Sweep { id, .. } | CsgNode::Loft { id, .. } => {
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
            // #384 D2: field-transparent — the inner-wall remap only rewrites
            // provenance (see `evaluate_open`); the distance is the child's.
            CsgNode::InnerWall(inner) => inner.sdf_open(p, mode, open),
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
            // #385 P2: loft field (see `sdf_loft`). Shell-retained opening of a
            // loft is not modelled yet (P3) — the plain morph field is returned
            // even under an open-branch set.
            CsgNode::Loft { sections, half_span, .. } => {
                sdf_loft(sections, *half_span, p)
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
            // #384 D2: field-identical to the child, contributors remapped to the
            // DL-75 inner-wall band — the provenance half of the dualised closed
            // shell (mirror of the old `Shell` arm's `c_ret.mapped_inner()`).
            CsgNode::InnerWall(inner) => {
                let (d, c) = inner.evaluate_open(p, eps, mode, open);
                (d, c.mapped_inner())
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
            // #385 P3: cap-vs-wall contributor split. `d2` (the morphed 2D
            // cross-section) is compared against the cap term inside
            // `loft_contributors`, so a point on the planar lid resolves to its
            // cap sub-id (pickable, retained analytic Plane) and a point on the
            // morph side wall to the whole-feature branch (sub 0, one traced
            // face). Field value is unchanged (`sdf_extrude(d2, …)`).
            CsgNode::Loft { id, sections, half_span, .. } => {
                let d2 = loft_morph_2d(sections, p);
                let d = sdf_extrude(d2, p[2], *half_span);
                let c = loft_contributors(id, d2, p[2], *half_span, eps);
                (d, c)
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
            // #384 D2: field-transparent → bounds are the child's.
            CsgNode::InnerWall(inner) => inner.aabb(),
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
            CsgNode::Loft { sections, half_span, .. } => {
                loft_local_aabb(
                    sections.iter().map(|s| (s.sketch.aabb(), &s.map)),
                    *half_span,
                )
            }
        }
    }
}

/// #385 P2: the loft-local AABB — union of every section's 2D profile AABB
/// re-expressed in the shared loft frame (own → common via
/// [`Transform::quat_rotate`] on `map`; the maps are rigid so the four mapped
/// corners re-bound it), spread over the axial slab `[-half_span, +half_span]`.
/// Conservative: the interpolated cross-section at any interior height lies
/// within the union of the neighbouring profile extents.
pub(crate) fn loft_local_aabb<'a>(
    sections: impl Iterator<Item = (crate::sketch2d::Aabb2, &'a Transform)>,
    half_span: f32,
) -> Aabb {
    let mut acc = Aabb::EMPTY;
    for (bb2, map) in sections {
        if bb2.is_empty() {
            continue;
        }
        // own → common: invert the loft-local → own `world_to_local(map, ·)`,
        // i.e. apply `map` forward (rotate + translate, unit scale).
        let corners = [
            [bb2.min[0], bb2.min[1]],
            [bb2.max[0], bb2.min[1]],
            [bb2.min[0], bb2.max[1]],
            [bb2.max[0], bb2.max[1]],
        ];
        for c in corners {
            let scaled = [c[0] * map.scale[0], c[1] * map.scale[1], 0.0];
            let r = quat_rotate(map.rotation, scaled);
            let x = r[0] + map.translate[0];
            let y = r[1] + map.translate[1];
            acc = Aabb::union(
                acc,
                Aabb {
                    min: [x, y, -half_span],
                    max: [x, y, half_span],
                },
            );
        }
    }
    acc
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

/// #385 P2: one loft cross-section as seen by the shared field math — its
/// centred z, its rigid loft-local → own-sketch [`Transform`], and its 2D
/// distance at a point already mapped into the section's own frame. Implemented
/// by both the unbounded [`LoftNodeSection`] (naive [`Sketch2d`]) and the BVH
/// [`crate::bvh::BoundedCsg::Loft`]'s pruned section, so the loft field has a
/// single source of truth ([`sdf_loft`]) on both eval paths.
pub(crate) trait LoftProfileField {
    fn z(&self) -> f32;
    fn map(&self) -> &Transform;
    fn sdf2(&self, uv: [f32; 2]) -> f32;
}

impl LoftProfileField for LoftNodeSection {
    fn z(&self) -> f32 {
        self.z
    }
    fn map(&self) -> &Transform {
        &self.map
    }
    fn sdf2(&self, uv: [f32; 2]) -> f32 {
        self.sketch.sdf(uv)
    }
}

/// #385 P2: the loft distance field in loft-local (centred) coordinates. The
/// slab spans `[-half_span, +half_span]`; `sections` are ascending in `z`.
/// Value = the piecewise-linear (bracket) morph of the section 2D fields
/// (partition-of-unity [`loft_hat`] weights), hard-clamped in `z` by the
/// symmetric IQ extrude cap composition ([`sdf_extrude`]). This is the CPU
/// reference the compiled per-section `OP_LOFT_MORPH` accumulator + trailing
/// `OP_EXTRUDE` tape mirrors 1:1.
///
/// TODO(#385 P5): the morph field is NOT 1-Lipschitz in `z` in general — the
/// worst-case axial gradient is
///   `L_z ≈ 1 + max_k( max_lateral_shift(profile_k, profile_{k+1})
///          / (z_{k+1} − z_k) )`,
/// i.e. the lateral rate at which the level sets slide between profiles. The
/// sphere-trace `march_relax` damping that keeps the renderer from tunnelling
/// (grey wall, #382 mode) is P5 scope, not evaluated here.
pub(crate) fn sdf_loft<S: LoftProfileField>(sections: &[S], half_span: f32, p: [f32; 3]) -> f32 {
    sdf_extrude(loft_morph_2d(sections, p), p[2], half_span)
}

/// #385 P3: the loft's *un-clamped* morphed 2D cross-section value at `p` — the
/// tent-weighted lerp of the section profile fields, WITHOUT the z-slab clamp.
/// This is the `d2` term of [`sdf_loft`] (`sdf_loft = sdf_extrude(d2, z,
/// half_span)`), split out so the contributor logic ([`loft_contributors`]) can
/// compare it against the cap term `|z| − half_span` to decide wall-vs-cap
/// provenance — exactly as the extrude band's `distance` is compared against its
/// cap term. Field value is untouched: `sdf_loft` still returns the identical
/// composition, so P3 is feld-neutral.
pub(crate) fn loft_morph_2d<S: LoftProfileField>(sections: &[S], p: [f32; 3]) -> f32 {
    let z = p[2];
    let n = sections.len();
    let mut d2 = 0.0f32;
    for j in 0..n {
        let zc = sections[j].z();
        let z_prev = if j == 0 { zc } else { sections[j - 1].z() };
        let z_next = if j + 1 == n { zc } else { sections[j + 1].z() };
        let w = loft_hat(z, z_prev, zc, z_next);
        // `w == 0` sections contribute exactly `0 · d = 0` (profiles are
        // non-empty ⇒ `d` finite), matching the static opcode tape which
        // evaluates every section unconditionally.
        if w != 0.0 {
            let local = world_to_local(sections[j].map(), p);
            d2 += w * sections[j].sdf2([local[0], local[1]]);
        }
    }
    d2
}

/// #385 P3: contributor band for a **loft** point. The loft SDF is
/// `max(d2, |z| − half_span)` (the morphed cross-section [`loft_morph_2d`]
/// hard-clamped to the profile slab, exactly like [`CsgNode::Extrude`]'s caps),
/// so the nearest surface is whichever term ties the max within `eps`:
///
/// * the **cap** term names the reserved top/bottom cap sub-id
///   ([`BranchId::EXTRUDE_CAP_TOP_SUB`] / `…_BOTTOM_SUB`, the SAME scheme as
///   extrude/helix caps) — a retained planar lid registered as an analytic
///   `Plane` carrier ([`crate::brep::weaving::branch_halfspace_map`]);
/// * the **wall** term names the whole-feature branch (sub 0) — the loft's
///   morph side wall is one non-analytic surface (an implicit field mean of the
///   profiles, no per-segment analytic carrier — plan §1 correspondence-free
///   morph), so it is a single traced/marched face, not per-boundary walls like
///   an extrude.
///
/// At the rim (wall∩cap) BOTH branches are returned so the B-Rep both-branches
/// gate sees a real edge, uniform with the extrude cap band. `z`/`half_span`
/// are in the centred loft-local frame, so the cap split is placement-
/// independent.
pub(crate) fn loft_contributors(
    id: &FeatureId,
    d2: f32,
    z: f32,
    half_span: f32,
    eps: f32,
) -> Contributors {
    let wall = d2;
    let cap = z.abs() - half_span;
    let d_max = wall.max(cap);
    let mut c = Contributors::empty();
    if (wall - d_max).abs() <= eps {
        c.push(BranchId::from_feature(id));
    }
    if (cap - d_max).abs() <= eps {
        let sub = if z >= 0.0 {
            BranchId::EXTRUDE_CAP_TOP_SUB
        } else {
            BranchId::EXTRUDE_CAP_BOTTOM_SUB
        };
        c.push(BranchId::from_feature_sub(id, sub));
    }
    // Degenerate guard: never return an empty set — name the whole feature so
    // the point still has a provenance.
    if c.is_empty() {
        c.push(BranchId::from_feature(id));
    }
    c
}

/// #385 P2: the piecewise-linear "hat" (tent) basis weight of a section at
/// height `z`. Rises 0→1 over `[z_prev, z_self]`, falls 1→0 over
/// `[z_self, z_next]`; the first/last section (sentinel `z_prev == z_self` resp.
/// `z_next == z_self`) saturates to 1 beyond its end, so `z` outside the slab
/// takes the nearest end profile. Over any interior bracket `[z_k, z_{k+1}]`
/// the two active weights are exactly `(1 − t)` and `t`, so `Σ_j hat_j ≡ 1`
/// (partition of unity) and the weighted sum is the bracket lerp.
pub(crate) fn loft_hat(z: f32, z_prev: f32, z_self: f32, z_next: f32) -> f32 {
    let rise = if z_self <= z_prev {
        1.0
    } else {
        ((z - z_prev) / (z_self - z_prev)).clamp(0.0, 1.0)
    };
    let fall = if z_next <= z_self {
        1.0
    } else {
        ((z_next - z) / (z_next - z_self)).clamp(0.0, 1.0)
    };
    rise.min(fall)
}

/// #385 P2: reconstruct a [`Primitive::Loft`] from a lowered [`CsgNode::Loft`]'s
/// centred sections (raw `offset = z + half_span`, `mid == half_span`).
fn loft_export_primitive(
    sections: &[LoftNodeSection],
    half_span: f32,
    center: [f32; 3],
    wall_subs: Option<Vec<u32>>,
) -> Primitive {
    Primitive::Loft {
        profiles: sections
            .iter()
            .map(|s| fielddraft_protocol::LoftSection {
                sketch: (*s.sketch).clone(),
                offset: s.z + half_span,
                frame_map: s.map,
            })
            .collect(),
        center,
        wall_subs,
    }
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
/// Map a positional wall boundary group to the **wall name** of the swept
/// wall face. With a `wall_subs` table (stable sketch-entity ordinals, see
/// [`Primitive::Extrude`]) the group indexes the table.
///
/// Without a table — and for a group the table does not reach, which is
/// defensive, the table is built for exactly the profile's group count — the
/// positional index becomes the sub, MARKED with
/// [`BranchId::POSITIONAL_WALL_FLAG`] (#445). The mark is the whole point:
/// a positional id is not a name, and `qWall(op, entity)` must not be able to
/// mistake one for the other. Every reader that goes through
/// [`BranchId::from_feature_sub`] gets it for free.
#[inline]
pub(crate) fn map_wall_sub(wall_subs: Option<&[u32]>, g: u32) -> u32 {
    let positional = || g | BranchId::POSITIONAL_WALL_FLAG;
    match wall_subs {
        Some(t) => t.get(g as usize).copied().unwrap_or_else(positional),
        None => positional(),
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

// ─── #384 offset pipeline decision predicates ────────────────────────────

/// The single pushdown decision of [`CsgNode::push_down_offset`]: may a pending
/// offset of `delta` be pushed *through* `inner` (rather than left standing as a
/// residual `Offset(inner, delta)`)?
///
/// This is the SSOT the pass and its post-pass invariant checker
/// ([`crate::offset_invariants::check_offsets_pushed`]) both consult, so the
/// contract can never drift from the transform. The exact 2×2 Minkowski algebra
/// (docs/research/sdf-metricity/SYNTHESIS.md): dilation distributes over Union,
/// erosion over Intersection/Difference; rigid transforms and stacked offsets
/// always commute; everything else stops.
pub(crate) fn offset_can_push_through(inner: &CsgNode, delta: f32) -> bool {
    if delta == 0.0 {
        // `f − 0` is the identity — `push_down_offset` drops it entirely, so a
        // δ=0 offset is always eliminable and must never survive the pass.
        return true;
    }
    match inner {
        // Stacked offsets always merge (`f − δ₂ − δ`).
        CsgNode::Offset(..) => true,
        CsgNode::Union(..) => delta > 0.0,
        CsgNode::Intersection(..) | CsgNode::Difference(..) => delta < 0.0,
        // A rigid placement (unit scale) is an isometry; a non-unit scale
        // breaks the metric (the evaluator multiplies by `min_scale`).
        CsgNode::Transform(t, _) => t.scale == [1.0, 1.0, 1.0],
        // Field-transparent contributor-id wrapper — offset commutes through.
        CsgNode::InnerWall(_) => true,
        _ => false,
    }
}

// ─── #384 offset_capability helpers ──────────────────────────────────────

/// The dual Boolean offset stops that [`offset_channel::synthesize_offset_channels`]
/// tries to correct with arc-spline channels (mirror of its `dual_stop`
/// predicate): `Union` under erosion, `Intersection`/`Difference` under
/// dilation. These are classified at synthesis time, NOT by
/// [`CsgNode::collect_offset_capability`].
fn offset_is_dual_stop(inner: &CsgNode, delta: f32) -> bool {
    match inner {
        CsgNode::Union(..) => delta < 0.0,
        CsgNode::Intersection(..) | CsgNode::Difference(..) => delta > 0.0,
        _ => false,
    }
}

/// Records the capability reason of a NON-dual residual `Offset(inner, δ)`.
/// Dilation (`δ > 0`) `f − δ` on an exact leaf carrier IS the exact offset
/// SDF, so it is left silent (exact); only inexact carriers and refused
/// erosion folds are recorded conservative.
fn classify_residual_offset(
    inner: &CsgNode,
    delta: f32,
    report: &mut crate::offset_capability::CapabilityReport,
) {
    let erosion = delta < 0.0;
    match inner {
        // Exact-SDF leaf under dilation → exact (silent). A residual EROSION
        // on a leaf means `fold_offset_leaves` refused it (a foldable leaf
        // would have been folded away or collapsed). A leaf whose own field is
        // NOT an exact distance (Gyroid, CustomSdf, Thread, non-spherical
        // Ellipsoid) makes even the dilation `f − δ` only approximate.
        CsgNode::Leaf { primitive, .. } => {
            if !leaf_sdf_is_exact_distance(primitive) {
                report.note_conservative(format!(
                    "Offset auf einem {} — dessen Feld ist selbst keine exakte Distanz, f−δ trifft die Offset-Fläche nur näherungsweise.",
                    primitive_family_name(primitive)
                ));
            } else if erosion {
                report.note_conservative(format!(
                    "Offset über einem {}-Primitiv ist nicht analytisch faltbar — Näherung f−δ (Nullmenge nur näherungsweise maßhaltig).",
                    primitive_family_name(primitive)
                ));
            }
        }
        CsgNode::Extrude { sketch, .. } => {
            if erosion {
                if sketch_has_quadbezier(sketch) {
                    report.note_conservative(
                        "Extrude-Profil enthält ein Bézier-Segment — analytisches Falten nicht möglich, Näherung f−δ.",
                    );
                } else {
                    report.note_conservative(
                        "Extrude-Profil lässt sich nicht exakt erodieren (offen, selbstschneidend oder zu tief verschachtelt) — Näherung f−δ.",
                    );
                }
            }
        }
        CsgNode::Revolve { sketch, .. } => {
            if erosion {
                if sketch_has_quadbezier(sketch) {
                    report.note_conservative(
                        "Revolve-Profil enthält ein Bézier-Segment — analytisches Falten nicht möglich, Näherung f−δ.",
                    );
                } else if revolve_touches_axis(sketch) {
                    report.note_conservative(
                        "Revolve-Profil berührt die Rotationsachse (#390) — kein exakter Meridian-Offset, Näherung f−δ.",
                    );
                } else {
                    report.note_conservative(
                        "Revolve-Profil lässt sich nicht exakt erodieren — Näherung f−δ.",
                    );
                }
            }
        }
        CsgNode::Helix { .. } | CsgNode::Sweep { .. } | CsgNode::Loft { .. } => {
            if erosion {
                report.note_conservative(
                    "Offset über einem Sweep/Helix/Loft ist nicht analytisch faltbar — Näherung f−δ.",
                );
            }
        }
        CsgNode::SmoothUnion(..) | CsgNode::SmoothDifference(..) => {
            report.note_conservative(
                "Offset über einer gerundeten Verrundungsfläche (Smooth-Union/-Difference) — keine offset-abgeschlossene Fläche, Näherung f−δ.",
            );
        }
        CsgNode::MaskedBlend { .. } => {
            report.note_conservative(
                "Offset über einer Kanten-Fillet-Fläche (MaskedBlend) — keine offset-abgeschlossene Fläche, Näherung f−δ.",
            );
        }
        CsgNode::Morph(..) => {
            report.note_conservative(
                "Offset über einem Morph — keine offset-abgeschlossene Fläche, Näherung f−δ.",
            );
        }
        CsgNode::Twist(..) | CsgNode::Bend(..) | CsgNode::Displace(..) => {
            report.note_conservative(
                "Offset über einer Verformung (Twist/Bend/Displace) — kein exakter Offset, Näherung f−δ.",
            );
        }
        CsgNode::Transform(..) => {
            // push_down_offsets already commutes rigid transforms; a residual
            // here is a non-uniformly scaled transform.
            report.note_conservative(
                "Offset über einem ungleichmäßig skalierten Transform — kein exakter Offset, Näherung f−δ.",
            );
        }
        // Non-dual booleans (e.g. dilation of a union) are exact under the
        // pushdown and would have been pushed to the leaves; empty / nested
        // wrappers are handled by the recursion. Nothing to record.
        CsgNode::Union(..)
        | CsgNode::Intersection(..)
        | CsgNode::Difference(..)
        | CsgNode::InnerWall(_)
        | CsgNode::Shell(..)
        | CsgNode::Offset(..)
        | CsgNode::Empty => {}
    }
}

/// Short human name of a primitive family for capability reasons.
fn primitive_family_name(p: &Primitive) -> &'static str {
    match p {
        Primitive::Sphere { .. } => "Kugel",
        Primitive::Box { .. } => "Quader",
        Primitive::Cylinder { .. } => "Zylinder",
        Primitive::Torus { .. } => "Torus",
        Primitive::Cone { .. } => "Kegel",
        Primitive::Capsule { .. } => "Kapsel",
        Primitive::Ellipsoid { .. } => "Ellipsoid",
        Primitive::HexPrism { .. } => "Sechskant-Prisma",
        Primitive::Thread { .. } => "Gewinde",
        Primitive::Gyroid { .. } => "Gyroid",
        Primitive::ImportedMesh { .. } => "importiertes Mesh",
        Primitive::CustomSdf { .. } => "benutzerdefiniertes SDF",
        Primitive::Extrude { .. } => "Extrude",
        Primitive::Revolve { .. } => "Revolve",
        Primitive::Helix { .. } => "Helix",
        Primitive::Loft { .. } => "Loft",
    }
}

/// Whether a 2D sweep profile contains a quadratic Bézier segment (the family
/// `fold_offset_leaves` refuses because cavalier_contours has no arc-exact
/// Bézier offset).
fn sketch_has_quadbezier(sketch: &Sketch2d) -> bool {
    use crate::sketch2d::{Sketch2dPrimitive, SketchSegment};
    match sketch {
        Sketch2d::Empty => false,
        Sketch2d::Prim(Sketch2dPrimitive::Path { segments, .. }) => segments
            .iter()
            .any(|s| matches!(s, SketchSegment::QuadBezier { .. })),
        Sketch2d::Prim(_) => false,
        Sketch2d::Union(a, b)
        | Sketch2d::Difference(a, b)
        | Sketch2d::Intersection(a, b) => sketch_has_quadbezier(a) || sketch_has_quadbezier(b),
        Sketch2d::Offset { inner, .. } => sketch_has_quadbezier(inner),
    }
}

/// Whether a revolve meridian profile reaches (or crosses) the rotation axis
/// (`x ≤ ε`), the #390 phantom-null-set gate that makes the erosion fold
/// refuse.
fn revolve_touches_axis(sketch: &Sketch2d) -> bool {
    let bb = sketch.aabb();
    bb.min[0] <= 1e-4
}

/// Whether a leaf primitive's field is an exact Euclidean distance, so that
/// `f − δ`'s zero set IS the true offset surface. NOT exact: Gyroid
/// (`|trig| − t`), CustomSdf (only Lipschitz-bounded, arbitrary user tape),
/// Thread (groove carved via max into a cylinder — non-distance at the carve
/// seam), and the non-spherical Ellipsoid (scale-normalised bound
/// `(len(p/r)−1)·r_min`, exact only when all radii coincide).
fn leaf_sdf_is_exact_distance(p: &Primitive) -> bool {
    match p {
        Primitive::Gyroid { .. } | Primitive::CustomSdf { .. } | Primitive::Thread { .. } => false,
        Primitive::Ellipsoid { radii } => radii[0] == radii[1] && radii[1] == radii[2],
        _ => true,
    }
}

/// Whether a subtree's field is an exact Euclidean distance: a single exact
/// leaf (or exact-2D-distance Extrude/Revolve) under at most RIGID transforms.
/// Everything composite (Booleans, Blends, Warps, Offsets, Shells) is not —
/// used by the open-shell capability check.
fn subtree_field_is_exact_distance(node: &CsgNode) -> bool {
    match node {
        CsgNode::Leaf { primitive, .. } => leaf_sdf_is_exact_distance(primitive),
        // Extrude/Revolve leaves evaluate an exact 2D distance → exact 3D SDF
        // (offset_tree doctrine, docs/OFFSET-TREE-PLAN.md).
        CsgNode::Extrude { .. } | CsgNode::Revolve { .. } => true,
        CsgNode::Transform(t, inner) => {
            t.scale == [1.0; 3] && subtree_field_is_exact_distance(inner)
        }
        _ => false,
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

// ─── #383/#406 hard-cutter vector helpers ──────────────────────────────────

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Normalise, or `None` if the vector is (near-)degenerate.
fn normalize3(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot3(v, v).sqrt();
    if l < 1e-6 {
        None
    } else {
        Some([v[0] / l, v[1] / l, v[2] / l])
    }
}

/// Return `n` flipped so it points OUTWARD from the solid at `mid` (an edge
/// point, field ≈ 0): the outward side is where the field grows. `None` if the
/// probe is inconclusive (the two sides are indistinguishable).
/// Why a straight edge cannot carry an analytic edge cutter (#383/#406/#414).
///
/// Shared by the fillet and the chamfer, which differ only in the CUT PROFILE
/// (rolling-ball cylinder vs. bevel plane) and not in the dihedral they need.
/// The fillet swallows a rejection and falls back to the `smin` mask path; the
/// chamfer has no such fallback and reports the reason to the user instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeCutterReject {
    /// Zero-length edge, a normal that is not perpendicular to it, or a face
    /// whose outward side could not be determined against the field.
    Degenerate,
    /// The dihedral is not a right angle — P1 covers cube-style edges only
    /// (P2 in `docs/FILLET-CUTTER-PLAN.md` generalises to skew prisms).
    NonOrthogonal,
    /// The edge is concave: material would have to be ADDED, not removed.
    Concave,
}

/// The local frame an analytic edge cutter is built in (#383/#406/#414).
///
/// `(u, v, w)` is right-handed and orthonormal with `w` along the edge and
/// `u` the in-plane projection of `n_a`; `inner` is the rolling-ball centre at
/// `reach` inside both faces, which doubles as the convexity probe point.
pub struct EdgeCutterFrame {
    pub mid: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub w: [f32; 3],
    pub len: f32,
    /// Face normals re-oriented OUTWARD against the live field.
    pub na: [f32; 3],
    pub nb: [f32; 3],
    pub cdot: f32,
    /// `mid − reach/(1+cdot)·(n_a + n_b)` — deep inside the material.
    pub inner: [f32; 3],
}

/// Classify a straight edge's dihedral against the live field of `node` and
/// return the frame an analytic cutter is built in.
///
/// `reach` is the cutter's local size (the fillet radius / the chamfer
/// distance); it scales the outward probe and the convexity margin, so a
/// cutter is only accepted when the edge is convex on ITS OWN length scale.
/// The normals may point either way — their sign is re-fixed here against the
/// field, so no caller has to guarantee the weaver's orientation convention.
pub fn edge_cutter_frame(
    node: &CsgNode,
    edge_start: [f32; 3],
    edge_end: [f32; 3],
    normal_a: [f32; 3],
    normal_b: [f32; 3],
    reach: f32,
) -> Result<EdgeCutterFrame, EdgeCutterReject> {
    use EdgeCutterReject as R;
    if !(reach > 0.0) || !reach.is_finite() {
        return Err(R::Degenerate);
    }
    let dir = sub3(edge_end, edge_start);
    let len = dot3(dir, dir).sqrt();
    if len <= 1e-4 {
        return Err(R::Degenerate);
    }
    let w = scale3(dir, 1.0 / len);
    let mid = scale3(
        [
            edge_start[0] + edge_end[0],
            edge_start[1] + edge_end[1],
            edge_start[2] + edge_end[2],
        ],
        0.5,
    );

    // Orient both face normals OUTWARD against the live field (independent
    // of the weaver's sign convention): along the true outward direction
    // the field grows from ~0 at the edge to positive outside.
    let na = orient_outward(node, mid, normal_a, reach).ok_or(R::Degenerate)?;
    let nb = orient_outward(node, mid, normal_b, reach).ok_or(R::Degenerate)?;

    // P1 gate: orthogonal dihedral only (cube-style right-angle edges).
    let cdot = dot3(na, nb);
    if cdot.abs() > 2e-2 {
        return Err(R::NonOrthogonal);
    }
    // Both face normals must be perpendicular to the edge (planar straight
    // edge). Project the in-plane axis u ⟂ w and rebuild a clean right-handed
    // orthonormal frame (u, v, w).
    let u = normalize3(sub3(na, scale3(w, dot3(na, w)))).ok_or(R::Degenerate)?;
    let v = cross3(w, u); // unit (u ⟂ w), right-handed

    // Rolling-ball centre: distance `reach` inside from both faces. cdot≈0 here
    // so 1/(1+cdot) ≈ 1; keep the general form for robustness.
    let k = reach / (1.0 + cdot);
    let inner = [
        mid[0] - k * (na[0] + nb[0]),
        mid[1] - k * (na[1] + nb[1]),
        mid[2] - k * (na[2] + nb[2]),
    ];
    // The cutter's own anchor point must sit comfortably INSIDE the solid,
    // otherwise there is nothing here to cut.
    if node.sdf(inner, EvalMode::Visual) > -0.5 * reach {
        return Err(R::Concave);
    }
    // CONVEXITY. Note that `inner` alone cannot decide it: at `u = v = −reach`
    // the material is present for a convex edge (`u ≤ 0 AND v ≤ 0`) *and* for a
    // concave one (`u ≤ 0 OR v ≤ 0`) — the original P1 gate therefore accepted
    // reflex edges and would have cut material out of an inside corner.
    //
    // The discriminating probes are the two MIXED quadrants (`u > 0, v < 0` and
    // `u < 0, v > 0`): both are empty at a convex edge and both are solid at a
    // concave one. Requiring BOTH to be outside also rejects the half-open
    // (tangential/degenerate) configurations.
    let eps = (0.25 * reach).clamp(1e-3, 0.5);
    let mixed = |sa: f32| {
        let q = [
            mid[0] + eps * (sa * na[0] - sa * nb[0]),
            mid[1] + eps * (sa * na[1] - sa * nb[1]),
            mid[2] + eps * (sa * na[2] - sa * nb[2]),
        ];
        node.sdf(q, EvalMode::Visual)
    };
    if mixed(1.0) < 0.2 * eps || mixed(-1.0) < 0.2 * eps {
        return Err(R::Concave);
    }
    Ok(EdgeCutterFrame { mid, u, v, w, len, na, nb, cdot, inner })
}

fn orient_outward(node: &CsgNode, mid: [f32; 3], n: [f32; 3], r: f32) -> Option<[f32; 3]> {
    let n = normalize3(n)?;
    let eps = (0.1 * r).clamp(1e-3, 0.5);
    let plus = node.sdf([mid[0] + eps * n[0], mid[1] + eps * n[1], mid[2] + eps * n[2]], EvalMode::Visual);
    let minus = node.sdf([mid[0] - eps * n[0], mid[1] - eps * n[1], mid[2] - eps * n[2]], EvalMode::Visual);
    if (plus - minus).abs() < 1e-5 {
        return None;
    }
    Some(if plus >= minus { n } else { [-n[0], -n[1], -n[2]] })
}

/// Unit quaternion `(x,y,z,w)` for the rotation whose matrix COLUMNS are the
/// orthonormal right-handed basis `(e1, e2, e3)` (i.e. it maps local X→e1,
/// Y→e2, Z→e3). Mirror of `offset_channel::quat_from_cols` in f32.
fn quat_from_cols3(e1: [f32; 3], e2: [f32; 3], e3: [f32; 3]) -> [f32; 4] {
    let m = [
        [e1[0], e2[0], e3[0]],
        [e1[1], e2[1], e3[1]],
        [e1[2], e2[2], e3[2]],
    ];
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    let l = (x * x + y * y + z * z + w * w).sqrt().max(1e-30);
    [x / l, y / l, z / l, w / l]
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
        let branch = BranchId::from_feature(&f.fillet_id).as_u64();
        let (wall_a, wall_b) =
            if a_in_left { (f.branch_a, f.branch_b) } else { (f.branch_b, f.branch_a) };
        Some((f, BlendProvenance { branch, wall_a, wall_b }))
    })
}

/// Branch identity of a masked blend's fillet surface and its two junction
/// walls (full [`BranchId`] words from the filleted edge's signature).
/// `NONE` (all zero) marks a blend with no B-Rep identity — the registry
/// skips it and its face degrades to Tessellated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlendProvenance {
    /// The blend face's own branch: the fillet feature's id, sub 0.
    pub branch: u64,
    /// The junction wall inside operand `a` (MaskedBlend's left subtree).
    pub wall_a: u64,
    /// The junction wall inside operand `b`.
    pub wall_b: u64,
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
    pub branch_a: u64,
    /// Full branch id (with sub) of the junction wall on `feature_b`'s side.
    pub branch_b: u64,
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
    /// #383/#406: analytic hard-cutter inputs, present when both adjacent faces
    /// are planes and the queried edge is straight. When set, the fillet is
    /// realised as a boolean subtraction of an exact Box−Cylinder cutter
    /// (Onshape rolling-ball semantics) instead of the `smin` mask blend — see
    /// [`CsgNode::apply_edge_fillets`]. `None` keeps the fillet on the mask path
    /// (curved edges, non-planar faces).
    pub cutter: Option<FilletCutter>,
}

/// #383/#406: geometric inputs for an analytic hard fillet cutter on a straight
/// edge between two planar faces. All in world/CSG eval space. The two normals
/// are the faces' outward normals up to sign — the sign is re-fixed against the
/// live field at apply time (see [`CsgNode::build_edge_cutter`]), so the caller
/// need not guarantee an orientation convention.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FilletCutter {
    /// Outward normal of adjacent face A (world, sign re-fixed at apply time).
    pub normal_a: [f32; 3],
    /// Outward normal of adjacent face B.
    pub normal_b: [f32; 3],
    /// Straight edge endpoints (world). The cutter spans this segment; ends stay
    /// sharp (no corner blend — Onshape default).
    pub edge_start: [f32; 3],
    pub edge_end: [f32; 3],
    /// Rolling-ball radius (== the fillet's `blend_radius`).
    pub radius: f32,
}

/// #414: per-edge CHAMFER specification — a planar bevel along one straight
/// edge, Onshape's `EQUAL_OFFSETS` mode (one distance, applied to both faces).
///
/// Deliberately NOT an [`EdgeFillet`] with a different profile: a chamfer has
/// no `smin` mask path, so it needs none of the junction bookkeeping
/// (`feature_a`/`branch_a`/`mask_polyline`/`blend_mode`) that only the fallback
/// consumes. What the two DO share — the dihedral gate and the local frame —
/// is factored into [`edge_cutter_frame`], not into a common record.
///
/// Documented gaps (P1): Onshape's `OPPOSITE_OFFSETS` (two distinct distances)
/// and `ANGLE` (distance + angle) modes are unsupported, as are concave edges
/// and non-orthogonal dihedrals — see `docs/FILLET-CUTTER-PLAN.md` §5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeChamfer {
    /// The chamfer feature's own id — names the cutter leaf.
    pub chamfer_id: FeatureId,
    /// Outward normal of adjacent face A (world, sign re-fixed at apply time).
    pub normal_a: [f32; 3],
    /// Outward normal of adjacent face B.
    pub normal_b: [f32; 3],
    /// Straight edge endpoints (world). The bevel spans exactly this segment;
    /// its ends stay sharp (no corner blend — Onshape default).
    pub edge_start: [f32; 3],
    pub edge_end: [f32; 3],
    /// EQUAL_OFFSETS leg length `d`: the bevel cuts `d` back from the edge
    /// along BOTH adjacent faces.
    pub distance: f32,
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
