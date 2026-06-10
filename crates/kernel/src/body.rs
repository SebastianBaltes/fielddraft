//! Multi-body partitioning of the linear feature spine.
//!
//! Each disjoint solid in the document owns its own CSG graph, BVH
//! and mesh cache. Mutations that only touch body `A` leave body
//! `B`'s mesh unchanged.
//!
//! Partitioning rules:
//!
//! * **Union** — seeds a new body, unless it carries explicit
//!   `target_ids` (an explicit Boolean union) *and* actually touches
//!   the target body (AABB overlap), in which case it docks into it.
//!   An explicit union of two disjoint solids stays two bodies: the
//!   link is intent, contact decides the merge. Spatial overlap alone
//!   (no explicit Boolean) never merges.
//! * **Difference / Intersection** — attached to existing bodies via
//!   `target_ids` (preferred) or AABB overlap (fallback for legacy
//!   features without explicit targets).
//! * Feature order inside a body is always the original document
//!   order.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use fielddraft_protocol::{BoolOp, FeatureId, HalfSpace, Primitive, Transform};

use crate::brep::octree::Octree;
use crate::brep::weaving::{weave, weave_parametric};
use crate::brep::BRep;
use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, BlendMode, CsgNode, DecodedFeature};
use crate::dc::Mesh;
use crate::sdf_compiler::CompiledSdf;

/// One partitioned body as seen by the [`partition`] pass — just the
/// feature indices and the accumulated world AABB, without any
/// mesh/BVH state. Lives long enough for `lib.rs` to diff it against
/// the previous frame's bodies and decide what to remesh.
#[derive(Debug, Clone)]
pub struct BodyPartition {
    /// Indices into the original `Document::features` list, in
    /// original order. The first entry is always the seed Union that
    /// started the body (or the first Union that merged into it if
    /// the seed has since been removed — see the merge case in
    /// [`partition`]).
    pub feature_indices: Vec<usize>,
    /// Running world-space AABB of the body. Grows monotonically
    /// under Union, stays put under Difference / Intersection.
    pub aabb: Aabb,
}

impl BodyPartition {
    /// Stable id for this body. Derived from the lowest feature id
    /// among its members: that id survives across rebuilds as long
    /// as the feature lives in the document, so the frontend can
    /// keep the body's BufferGeometry around even after unrelated
    /// mutations reorder the body list.
    ///
    /// K8.0 accommodates hierarchical ids (linear_pattern
    /// clones): flat-numeric ids sort first by their u32
    /// value, hierarchical ids (clones) fall back to
    /// `u32::MAX` so a body that contains both a
    /// pre-existing primitive and some pattern clones still
    /// picks the primitive as its stable id.
    pub fn seed_id(&self, features: &[DecodedFeature]) -> FeatureId {
        self.feature_indices
            .iter()
            .map(|&i| features[i].id.clone())
            .min_by_key(|id| id.as_u32_if_flat().unwrap_or(u32::MAX))
            .expect("BodyPartition is always non-empty")
    }
}

/// A partitioned body plus its meshing state. Owned by `Kernel`.
#[derive(Debug, Clone)]
pub struct Body {
    /// Stable identifier — see [`BodyPartition::seed_id`].
    pub id: FeatureId,
    pub feature_indices: Vec<usize>,
    pub aabb: Aabb,
    /// Hash of the per-feature payload at the time of the last
    /// successful mesh rebuild. `rebuild_bodies` skips re-meshing
    /// when the hash still matches after a partition.
    pub payload_hash: u64,
    /// Geometry-only hash (primitives, booleans, modifiers — no
    /// transforms). Used by the affine fast-path: when this matches
    /// but `transform_hash` doesn't, a pure translation can be
    /// resolved as a GPU matrix instead of a full remesh.
    pub geometry_hash: u64,
    /// Transform-only hash (translate, rotation, scale per feature).
    pub transform_hash: u64,
    /// Voxel grid resolution this body's cached mesh was built at.
    /// Stored alongside the hash so a change from active to context
    /// LOD (or vice versa) correctly invalidates the cache even
    /// when the feature payload is unchanged.
    pub mesh_resolution: u32,
    pub mesh: Mesh,
    /// Per-body version counter. Bumps whenever the mesh is
    /// actually rebuilt (payload hash changed or LOD switched),
    /// not on bookkeeping updates like feature-index renumbering.
    pub version: u32,
    /// When set, the frontend applies this translation offset as a
    /// GPU matrix instead of uploading new geometry. Cleared on the
    /// next full remesh.
    pub transform_delta: Option<[f32; 3]>,
    /// Snapshot of per-feature transforms at the time of the last
    /// full remesh. Used to detect uniform translation deltas.
    pub last_mesh_transforms: Vec<fielddraft_protocol::Transform>,
    /// Compiled SDF opcode buffer for GPU raymarching. Recompiled
    /// whenever the body's geometry changes (payload_hash differs).
    pub compiled_sdf: Option<CompiledSdf>,
    /// Rigid + uniform-scale **placement** of this body, recomputed on
    /// every [`Body::ensure_brep`] from the body's anchor feature
    /// (`feature_indices[0]`, the seed Union that started the body —
    /// see [`partition`]). The woven [`Body::brep`] lives in the
    /// body-LOCAL frame (placement removed); consumers (STEP export,
    /// world picking) compose this placement back in to obtain world
    /// geometry. A pure move changes only `placement` and therefore
    /// reuses the cached local weave (DL-50 correction).
    ///
    /// Runtime-only — `Body` is not serialised (bodies are ephemeral
    /// runtime partitions, not document entities), so there is no
    /// serde concern here.
    pub placement: Transform,
    /// Lazily computed *feature-based* B-Rep (sharp face/edge/vertex
    /// structure woven from the analytic half-space decomposition,
    /// not the mesh).
    ///
    /// Woven in the **body-local** frame: the build carry is seeded
    /// with `inverse(placement)`, so octree vertex positions and
    /// analytic face `Transform`s come out placement-free (DL-50
    /// correction). Keyed on [`hash_body_local_payload`] — the
    /// geometry plus the *relative* arrangement of features, with
    /// placement divided out — so a pure move (which only shifts
    /// `placement`) leaves the key unchanged and reuses the cache.
    /// The key makes the cache self-healing: a stale hash forces a
    /// re-weave even when the lazy `brep = None` invalidation is
    /// bypassed (e.g. the affine pure-translation fast-path in
    /// `rebuild_bodies`).
    pub brep: Option<(u64, BRep)>,
}

impl Body {
    /// Return the body's feature-based B-Rep, weaving it on first
    /// access and caching the result. Subsequent calls reuse the
    /// cache until the next remesh clears it (see [`Body::brep`]).
    ///
    /// The octree resolution is **body-relative**: `min_cell_size`
    /// scales with the body's largest AABB extent (DL-28), so a tiny
    /// or a huge body both get proportional sampling instead of an
    /// absolute default. The weave path is panic-safe — degenerate
    /// geometry yields a possibly-empty B-Rep rather than a crash.
    /// `parametric` selects milestone-1 parametric weaving: pure grundforms
    /// (currently the Box) get an exact analytic B-Rep via
    /// [`weave_parametric`], skipping the octree entirely; anything it does not
    /// yet handle transparently falls back to the octree [`weave`]. The flag
    /// does not enter the cache key — the woven `id_hash`es are identical on
    /// both paths, and the kernel clears the cache when the flag is toggled.
    pub fn ensure_brep(&mut self, features: &[DecodedFeature], parametric: bool) -> &BRep {
        // Placement is cheap (O(features)) and recomputed on EVERY call,
        // OUTSIDE the stale-gated weave: a move changes only `placement`,
        // so consumers always see the current world pose without forcing
        // a re-weave (DL-50 correction).
        self.placement = compute_body_placement(features, &self.feature_indices);

        // Re-weave only when the placement-INVARIANT local key changes —
        // i.e. geometry or the *relative* arrangement of features moved.
        // A pure body move leaves this key untouched and reuses the cache.
        let key = hash_body_local_payload(features, &self.feature_indices, &self.placement);
        let stale = !matches!(&self.brep, Some((k, _)) if *k == key);
        if stale {
            // Weave in the body-local frame: seed the build carry with
            // `inverse(placement)` so vertices + analytic face transforms
            // come out placement-free. The octree's `min_cell` is derived
            // from the body AABB extent, which is placement-invariant
            // under rigid + uniform scale, so the local sampling matches.
            let bounded =
                build_bounded_for_body_local(features, &self.feature_indices, &self.placement);
            // A body containing a space-filling primitive (a gyroid TPMS) gets
            // NO B-Rep wireframe (DL-71): the surface threads through nearly the
            // whole volume, so the octree refines on tens of thousands of cells
            // — a 30s move-stall in wasm — to weave a wireframe whose every seam
            // is non-Lipschitz garbage we discard anyway. Skip the octree+weave
            // outright; the renderer covers the shape. A *separate* clean body
            // (union → its own partition) still weaves normally.
            let brep = if bounded_contains_space_filling(&bounded) {
                BRep::new()
            } else if let Some(parametric_brep) =
                parametric.then(|| weave_parametric(&bounded)).flatten()
            {
                // Exact analytic weave for a pure grundform — no octree at all.
                parametric_brep
            } else {
                let min_cell = topology_min_cell_size(&self.aabb);
                let octree = Octree::build(&bounded, min_cell);
                weave(&bounded, &octree)
            };
            self.brep = Some((key, brep));
        }
        &self.brep.as_ref().unwrap().1
    }

    /// Borrow the already-woven B-Rep without re-weaving, or `None` if
    /// [`Body::ensure_brep`] has never run (or the cache was invalidated).
    /// Lets a caller weave a batch of bodies first, then borrow them all
    /// immutably at once (e.g. multi-body STEP export).
    pub fn brep_cached(&self) -> Option<&BRep> {
        self.brep.as_ref().map(|(_, b)| b)
    }

    /// **Debug only.** Rebuild this body's topology octree and return its
    /// surviving leaf cells in the **body-LOCAL** frame as
    /// `(min, max, class)` triples, where `class` is `0`=Face, `1`=Edge,
    /// `2`=Vertex. Mirrors the octree path in [`Body::ensure_brep`] (same
    /// body-local bounded build, same body-relative `min_cell`), but is
    /// never cached — it re-runs the build on each call, so it is meant for
    /// the viewport's octree-cell debug overlay, not the hot path. The octree
    /// is built unconditionally here (ignoring parametric weaving) so the
    /// overlay can show the cell grid even for a grundform that the analytic
    /// weave would otherwise skip. A space-filling body yields no cells, just
    /// like the real weave (DL-71).
    pub fn debug_octree_cells(
        &self,
        features: &[DecodedFeature],
    ) -> Vec<([f32; 3], [f32; 3], u8)> {
        use crate::brep::octree::CellClass;
        let placement = compute_body_placement(features, &self.feature_indices);
        let bounded = build_bounded_for_body_local(features, &self.feature_indices, &placement);
        if bounded_contains_space_filling(&bounded) {
            return Vec::new();
        }
        let min_cell = topology_min_cell_size(&self.aabb);
        let octree = Octree::build(&bounded, min_cell);
        octree
            .leaves()
            .iter()
            .map(|c| {
                let class = match c.class {
                    CellClass::Face => 0u8,
                    CellClass::Edge => 1,
                    CellClass::Vertex => 2,
                };
                (c.aabb.min, c.aabb.max, class)
            })
            .collect()
    }
}

/// Octree `min_cell_size` for a body's B-Rep weave, derived from its
/// world AABB (DL-28). We target ~32 cells across the body's largest
/// extent, then clamp to `[1e-3, 10.0]` mm so a degenerate (zero-size)
/// or astronomically large body can never drive the octree to its
/// `max_depth` cap and hang. An empty AABB falls back to the floor.
fn topology_min_cell_size(aabb: &Aabb) -> f32 {
    const DIVISOR: f32 = 32.0;
    const FLOOR: f32 = 1e-3;
    const CEIL: f32 = 10.0;
    if aabb.is_empty() {
        return FLOOR;
    }
    let size = aabb.size();
    let largest = size[0].max(size[1]).max(size[2]);
    (largest / DIVISOR).clamp(FLOOR, CEIL)
}

/// Does `node`'s CSG tree contain a **space-filling** primitive (a gyroid
/// TPMS, [`Primitive::is_space_filling`])? Such a body skips the B-Rep weave
/// entirely (DL-71): its surface threads through nearly the whole volume, so
/// the octree would refine on tens of thousands of cells to produce a wireframe
/// of non-Lipschitz seams we discard anyway.
fn bounded_contains_space_filling(node: &BoundedCsg) -> bool {
    match node {
        BoundedCsg::Leaf { primitive, .. } => primitive.is_space_filling(),
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            bounded_contains_space_filling(left) || bounded_contains_space_filling(right)
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::Shell { inner, .. } => bounded_contains_space_filling(inner),
        BoundedCsg::Extrude { .. } | BoundedCsg::Revolve { .. } | BoundedCsg::Empty => false,
    }
}

/// Walk the feature spine once and return one [`BodyPartition`] per
/// resulting body.
///
/// Rules (Onshape result-body vocabulary):
/// * **New** — always starts a fresh body. No AABB-based merging;
///   spatial overlap alone never folds two `New` bodies together.
/// * **Add** — a geometric union that merges into every body it
///   touches (AABB overlap), bridging several into one Part when it
///   spans them. Explicit `target_ids` narrow the candidates to the
///   targeted bodies (contact still decides). An `Add` that touches
///   nothing falls back to seeding its own body, so geometry never
///   vanishes.
/// * **Remove / Intersect** — attached to bodies via `target_ids`
///   (preferred) or AABB overlap (fallback for features without
///   explicit targets).
pub fn partition(features: &[DecodedFeature]) -> Vec<BodyPartition> {
    let mut groups: Vec<BodyPartition> = Vec::new();
    for (i, feature) in features.iter().enumerate() {
        let feat_aabb = feature_world_aabb(feature);

        match feature.op {
            BoolOp::New => {
                groups.push(BodyPartition {
                    feature_indices: vec![i],
                    aabb: feat_aabb,
                });
            }
            BoolOp::Add => {
                // An untagged Add docks into every body it touches (AABB
                // overlap); one carrying explicit `target_ids` only into the
                // touching bodies that hold one of its targets. An Add can
                // bridge *several* bodies at once: the sphere that joins a
                // left and a right solid fuses all three into a single Part,
                // because union is transitive. The link expresses intent,
                // contact decides the merge — a targeted Add of two
                // *disjoint* solids stays two Parts.
                let connected: Vec<usize> = (0..groups.len())
                    .filter(|&idx| {
                        let targeted = feature.target_ids.is_empty()
                            || groups[idx].feature_indices.iter().any(|&fi| {
                                let root = features[fi].id.root_ancestor();
                                feature.target_ids.iter().any(|tid| root == tid)
                            });
                        targeted && aabbs_overlap(groups[idx].aabb, feat_aabb)
                    })
                    .collect();
                match connected.split_first() {
                    // Nothing touched: this Add seeds its own Part.
                    None => groups.push(BodyPartition {
                        feature_indices: vec![i],
                        aabb: feat_aabb,
                    }),
                    // Dock into the lowest-index bridged body and fold every
                    // other one into it (descending order keeps `keep` and
                    // the not-yet-removed indices valid). `connected` is
                    // ascending, so `keep` is below all of `rest`.
                    Some((&keep, rest)) => {
                        groups[keep].feature_indices.push(i);
                        groups[keep].aabb = Aabb::union(groups[keep].aabb, feat_aabb);
                        for &idx in rest.iter().rev() {
                            let merged = groups.remove(idx);
                            groups[keep]
                                .feature_indices
                                .extend(merged.feature_indices);
                            groups[keep].aabb = Aabb::union(groups[keep].aabb, merged.aabb);
                        }
                        // Keep members in original document order (the
                        // `feature_indices` invariant downstream relies on).
                        groups[keep].feature_indices.sort_unstable();
                    }
                }
            }
            BoolOp::Remove | BoolOp::Intersect => {
                if !feature.target_ids.is_empty() {
                    // indexed element is both read and mutated; iter_mut rewrite is risky
                    #[allow(clippy::needless_range_loop)]
                    for idx in 0..groups.len() {
                        let matches = groups[idx].feature_indices.iter().any(|&fi| {
                            let root = features[fi].id.root_ancestor();
                            feature.target_ids.iter().any(|tid| root == tid)
                        });
                        if matches {
                            groups[idx].feature_indices.push(i);
                        }
                    }
                } else {
                    let target = groups
                        .iter()
                        .enumerate()
                        .find(|(_, g)| aabbs_overlap(g.aabb, feat_aabb))
                        .map(|(idx, _)| idx);
                    if let Some(idx) = target {
                        groups[idx].feature_indices.push(i);
                    }
                }
            }
        }
    }
    groups
}

/// Weave every body partition of `features` into one body-local
/// [`BRep`], returning the woven solids paired with the rigid +
/// uniform-scale **placement** that maps each one into world space.
///
/// This is the single canonical body-construction recipe shared by
/// every mid-loop / live-context consumer: the regeneration
/// [`Context`](crate::features::Context), the FieldSkript
/// `RegenScratch`, and any other caller that needs the element-level
/// topology of the in-flight geometry. The BReps come out body-local
/// (placement divided out, matching [`Body::ensure_brep`]); apply the
/// paired placement to lift a face centroid / normal into world space.
pub fn weave_all(features: &[DecodedFeature]) -> Vec<(BRep, Transform)> {
    let parts = partition(features);
    let mut breps = Vec::with_capacity(parts.len());
    for part in &parts {
        let mut body = Body {
            id: part.seed_id(features),
            feature_indices: part.feature_indices.clone(),
            aabb: part.aabb,
            payload_hash: 0,
            geometry_hash: 0,
            transform_hash: 0,
            mesh_resolution: 0,
            mesh: Mesh::default(),
            version: 1,
            transform_delta: None,
            last_mesh_transforms: Vec::new(),
            compiled_sdf: None,
            placement: Transform::IDENTITY,
            brep: None,
        };
        // Parametric (branch-seed) weave, octree fallback — the same route the
        // renderer uses. Query resolution previously forced the octree path,
        // but that path collapses on stacked coplanar extrudes at ≳100 mm scale
        // (ticket vyee: truncated edges, missing faces → garbage face-extrude
        // profiles); `id_hash`es are toggle-invariant, so handles resolve
        // identically on both routes.
        body.ensure_brep(features, true);
        let placement = body.placement;
        breps.push((body.brep.expect("ensure_brep populates the cache").1, placement));
    }
    breps
}

/// Content-keyed lazy cache of the woven B-Reps of an in-flight feature
/// spine, one entry per body partition.
///
/// The key is a content hash of the whole spine
/// ([`hash_body_payload`] over every feature). When a feature is
/// appended or mutated the key changes and [`BrepCache::ensure`]
/// re-weaves; a pure read re-uses the cache. This is the shared
/// laziness layer behind both the regeneration
/// [`Context`](crate::features::Context) (mid-loop element queries) and
/// the FieldSkript `RegenScratch` live context — one weave
/// implementation, one invalidation rule.
#[derive(Debug, Default)]
pub struct BrepCache {
    cached: Option<(u64, Vec<(BRep, Transform)>)>,
}

impl BrepCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Weave (or reuse) the B-Reps for the current `features` spine and
    /// return them paired with their world placements. Re-weaves only
    /// when the content key changes.
    pub fn ensure(&mut self, features: &[DecodedFeature]) -> &[(BRep, Transform)] {
        let all: Vec<usize> = (0..features.len()).collect();
        let key = hash_body_payload(features, &all);
        let fresh = !matches!(&self.cached, Some((k, _)) if *k == key);
        if fresh {
            self.cached = Some((key, weave_all(features)));
        }
        &self.cached.as_ref().unwrap().1
    }

    /// Drop the cache, forcing the next [`ensure`](BrepCache::ensure) to
    /// re-weave. The content key normally handles invalidation; explicit
    /// clearing is a belt-and-braces hook for callers (e.g. a commit) that
    /// prefer eager invalidation.
    pub fn clear(&mut self) {
        self.cached = None;
    }
}

/// World-space AABB of a single feature: its local geometry AABB
/// transformed by its placement.
fn feature_world_aabb(feature: &DecodedFeature) -> Aabb {
    feature.node.aabb().transformed(&feature.transform)
}

fn aabbs_overlap(a: Aabb, b: Aabb) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.min[0] <= b.max[0]
        && a.max[0] >= b.min[0]
        && a.min[1] <= b.max[1]
        && a.max[1] >= b.min[1]
        && a.min[2] <= b.max[2]
        && a.max[2] >= b.min[2]
}

/// Split hash of a body's feature payload: geometry vs. transform.
///
/// `geometry_hash` covers everything that affects the CSG tree shape
/// (primitive params, boolean op, modifier fields). `transform_hash`
/// covers the per-feature affine placement (translate, rotation,
/// scale). The combined `payload_hash` is a simple XOR of both —
/// identical to the old single hash for cache-invalidation purposes,
/// but the split lets `rebuild_mesh_internal` detect pure-translation
/// changes and apply them as a GPU matrix without a full remesh.
pub fn hash_body_payload_split(
    features: &[DecodedFeature],
    indices: &[usize],
) -> (u64, u64) {
    let mut geo = DefaultHasher::new();
    let mut xform = DefaultHasher::new();
    indices.len().hash(&mut geo);
    indices.len().hash(&mut xform);
    for &i in indices {
        let f = &features[i];
        f.id.hash(&mut geo);
        f.op.tag().hash(&mut geo);
        // The local geometry node already encodes the primitive and
        // every domain-warp/shell modifier, placement-free.
        hash_node(&f.node, &mut geo);
        f.blend_radius.to_bits().hash(&mut geo);
        hash_blend_mode(&f.blend_mode, &mut geo);

        f.id.hash(&mut xform);
        for v in f.transform.translate {
            v.to_bits().hash(&mut xform);
        }
        for v in f.transform.rotation {
            v.to_bits().hash(&mut xform);
        }
        f.transform.scale[0].to_bits().hash(&mut xform);
        f.transform.scale[1].to_bits().hash(&mut xform);
        f.transform.scale[2].to_bits().hash(&mut xform);
    }
    (geo.finish(), xform.finish())
}

pub fn hash_body_payload(features: &[DecodedFeature], indices: &[usize]) -> u64 {
    let (g, t) = hash_body_payload_split(features, indices);
    combine_hashes(g, t)
}

/// Compute a body's rigid + uniform-scale **placement** from its anchor
/// feature (`indices[0]`, the seed Union that started the body — see
/// [`partition`]). The placement is the anchor transform's rotation +
/// translation + a single uniform scale (taken as [`Transform::min_scale`]).
///
/// If the anchor carries a non-uniform scale, only its uniform component
/// is folded into the placement; the residual non-uniform part stays in
/// the body-local weave (where arbitrary affine is fine for the SDF). The
/// woven analytic surfaces (cylinder/cone/sphere/torus) are only closed
/// under rigid + uniform scale, which is exactly the class extracted here,
/// so the local frame keeps them analytic. An empty body falls back to
/// [`Transform::IDENTITY`] (world weave).
pub fn compute_body_placement(features: &[DecodedFeature], indices: &[usize]) -> Transform {
    let Some(&anchor) = indices.first() else {
        return Transform::IDENTITY;
    };
    let t = &features[anchor].transform;
    let s = t.min_scale();
    Transform {
        translate: t.translate,
        rotation: t.rotation,
        scale: [s, s, s],
    }
}

/// Placement-invariant cache key for a body's local B-Rep weave.
///
/// Hashes the geometry portion (primitive params, boolean op, modifiers —
/// identical to [`hash_body_payload_split`]'s geometry hash) plus the
/// *relative* arrangement of features: each feature's transform
/// re-expressed as `inverse(placement) ∘ feature.transform`. A pure body
/// move shifts `placement` and every `feature.transform` by the same
/// delta, so the composed local transforms — and therefore this key —
/// stay constant, letting [`Body::ensure_brep`] reuse the cached weave.
pub fn hash_body_local_payload(
    features: &[DecodedFeature],
    indices: &[usize],
    placement: &Transform,
) -> u64 {
    let (geo, _) = hash_body_payload_split(features, indices);
    let inv = crate::inverse(placement);
    let mut local = DefaultHasher::new();
    indices.len().hash(&mut local);
    for &i in indices {
        let f = &features[i];
        f.id.hash(&mut local);
        let lt = crate::compose_transforms(&inv, &f.transform);
        for v in lt.translate {
            v.to_bits().hash(&mut local);
        }
        for v in lt.rotation {
            v.to_bits().hash(&mut local);
        }
        for v in lt.scale {
            v.to_bits().hash(&mut local);
        }
    }
    combine_hashes(geo, local.finish())
}

fn combine_hashes(geometry: u64, transform: u64) -> u64 {
    let mut h = DefaultHasher::new();
    geometry.hash(&mut h);
    transform.hash(&mut h);
    h.finish()
}

/// Returns `Some([dx, dy, dz])` when every transform pair differs by
/// the same pure translation (rotation + scale unchanged). Returns
/// `None` on rotation/scale changes, mixed translations, or empty
/// input.
pub fn detect_uniform_translation(
    old: &[fielddraft_protocol::Transform],
    new: &[fielddraft_protocol::Transform],
) -> Option<[f32; 3]> {
    if old.len() != new.len() || old.is_empty() {
        return None;
    }
    let mut delta: Option<[f32; 3]> = None;
    for (o, n) in old.iter().zip(new.iter()) {
        if o.rotation != n.rotation || o.scale != n.scale {
            return None;
        }
        let d = [
            n.translate[0] - o.translate[0],
            n.translate[1] - o.translate[1],
            n.translate[2] - o.translate[2],
        ];
        match delta {
            None => delta = Some(d),
            Some(prev) => {
                if (d[0] - prev[0]).abs() > 1e-7
                    || (d[1] - prev[1]).abs() > 1e-7
                    || (d[2] - prev[2]).abs() > 1e-7
                {
                    return None;
                }
            }
        }
    }
    let d = delta?;
    if d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0 {
        return None;
    }
    Some(d)
}

fn hash_blend_mode(mode: &BlendMode, hasher: &mut DefaultHasher) {
    (match mode {
        BlendMode::Fillet => 0u8,
        BlendMode::Chamfer => 1u8,
    })
    .hash(hasher);
}

/// Structural hash of a feature's local geometry subtree. Placement-free
/// (the outer transform is hashed separately in
/// [`hash_body_payload_split`]), so a pure move leaves this unchanged.
/// Covers primitives, the extrude/revolve self-centering, and every
/// domain-warp/shell modifier.
/// Structural hash of an analytic [`HalfSpace`]: a kind tag plus its scalar
/// parameters. Used to fold a shell's curved/planar open faces into the body
/// hash so an opening change invalidates the cache.
fn hash_halfspace(hs: &HalfSpace, hasher: &mut DefaultHasher) {
    match hs {
        HalfSpace::Plane { normal, offset } => {
            0u8.hash(hasher);
            for v in normal {
                v.to_bits().hash(hasher);
            }
            offset.to_bits().hash(hasher);
        }
        HalfSpace::InfiniteCylinder { radius } => {
            1u8.hash(hasher);
            radius.to_bits().hash(hasher);
        }
        HalfSpace::Sphere { radius } => {
            2u8.hash(hasher);
            radius.to_bits().hash(hasher);
        }
        HalfSpace::Cone { half_angle } => {
            3u8.hash(hasher);
            half_angle.to_bits().hash(hasher);
        }
        HalfSpace::Torus { major_radius, minor_radius } => {
            4u8.hash(hasher);
            major_radius.to_bits().hash(hasher);
            minor_radius.to_bits().hash(hasher);
        }
        HalfSpace::Gyroid { cell_size, thickness } => {
            5u8.hash(hasher);
            cell_size.to_bits().hash(hasher);
            thickness.to_bits().hash(hasher);
        }
    }
}

fn hash_node(node: &CsgNode, hasher: &mut DefaultHasher) {
    match node {
        CsgNode::Empty => 0u8.hash(hasher),
        CsgNode::Leaf { id, primitive } => {
            1u8.hash(hasher);
            id.hash(hasher);
            hash_primitive(primitive, hasher);
        }
        CsgNode::Transform(t, inner) => {
            2u8.hash(hasher);
            for v in t.translate {
                v.to_bits().hash(hasher);
            }
            for v in t.rotation {
                v.to_bits().hash(hasher);
            }
            for v in t.scale {
                v.to_bits().hash(hasher);
            }
            hash_node(inner, hasher);
        }
        CsgNode::Union(a, b) => {
            3u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
        }
        CsgNode::Difference(a, b) => {
            4u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
        }
        CsgNode::Intersection(a, b) => {
            5u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
        }
        CsgNode::SmoothUnion(a, b, r, m) => {
            6u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
            r.to_bits().hash(hasher);
            hash_blend_mode(m, hasher);
        }
        CsgNode::SmoothDifference(a, b, r, m) => {
            7u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
            r.to_bits().hash(hasher);
            hash_blend_mode(m, hasher);
        }
        CsgNode::Twist(a, rate) => {
            8u8.hash(hasher);
            hash_node(a, hasher);
            rate.to_bits().hash(hasher);
        }
        CsgNode::Bend(a, rate) => {
            9u8.hash(hasher);
            hash_node(a, hasher);
            rate.to_bits().hash(hasher);
        }
        CsgNode::Displace(a, depth, freq) => {
            10u8.hash(hasher);
            hash_node(a, hasher);
            depth.to_bits().hash(hasher);
            freq.to_bits().hash(hasher);
        }
        CsgNode::Shell(a, thickness, open) => {
            11u8.hash(hasher);
            hash_node(a, hasher);
            thickness.to_bits().hash(hasher);
            for f in open {
                hash_halfspace(&f.surface, hasher);
                for v in f.placement.translate {
                    v.to_bits().hash(hasher);
                }
                for v in f.placement.rotation {
                    v.to_bits().hash(hasher);
                }
                for v in f.placement.scale {
                    v.to_bits().hash(hasher);
                }
            }
        }
        CsgNode::Morph(a, b, t) => {
            12u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
            t.to_bits().hash(hasher);
        }
        CsgNode::Offset(a, delta) => {
            14u8.hash(hasher);
            hash_node(a, hasher);
            delta.to_bits().hash(hasher);
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
            13u8.hash(hasher);
            hash_node(a, hasher);
            hash_node(b, hasher);
            mask_polyline.len().hash(hasher);
            for pt in mask_polyline {
                for v in pt {
                    v.to_bits().hash(hasher);
                }
            }
            mask_radius.to_bits().hash(hasher);
            blend_radius.to_bits().hash(hasher);
            hash_blend_mode(blend_mode, hasher);
            is_difference.hash(hasher);
        }
        CsgNode::Extrude {
            id,
            sketch,
            half_height,
        } => {
            14u8.hash(hasher);
            id.hash(hasher);
            hash_sketch2d(sketch, hasher);
            half_height.to_bits().hash(hasher);
        }
        CsgNode::Revolve { id, sketch } => {
            15u8.hash(hasher);
            id.hash(hasher);
            hash_sketch2d(sketch, hasher);
        }
    }
}

fn hash_primitive(primitive: &Primitive, hasher: &mut DefaultHasher) {
    match primitive {
        Primitive::Box { center, half_extents } => {
            0u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            for v in half_extents {
                v.to_bits().hash(hasher);
            }
        }
        Primitive::Sphere { center, radius } => {
            1u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            radius.to_bits().hash(hasher);
        }
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        } => {
            2u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            radius.to_bits().hash(hasher);
            half_height.to_bits().hash(hasher);
        }
        // K7.3: hash the full sketch payload so a profile
        // edit bumps the body cache and triggers a remesh.
        Primitive::Extrude {
            sketch,
            half_depth_z,
            center,
        } => {
            3u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            half_depth_z.to_bits().hash(hasher);
            hash_sketch2d(sketch, hasher);
        }
        // K8.4: same reasoning as Extrude — any profile edit
        // must bump the body-cache hash so the remesh fires.
        Primitive::Revolve { sketch, center } => {
            4u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            hash_sketch2d(sketch, hasher);
        }
        Primitive::Torus { center, major_radius, minor_radius } => {
            6u8.hash(hasher);
            for v in center {
                v.to_bits().hash(hasher);
            }
            major_radius.to_bits().hash(hasher);
            minor_radius.to_bits().hash(hasher);
        }
        Primitive::Gyroid { cell_size, thickness } => {
            5u8.hash(hasher);
            cell_size.to_bits().hash(hasher);
            thickness.to_bits().hash(hasher);
        }
        Primitive::Thread { nominal_radius, pitch, depth, half_height } => {
            7u8.hash(hasher);
            nominal_radius.to_bits().hash(hasher);
            pitch.to_bits().hash(hasher);
            depth.to_bits().hash(hasher);
            half_height.to_bits().hash(hasher);
        }
        Primitive::Cone { r_bottom, r_top, half_height } => {
            8u8.hash(hasher);
            r_bottom.to_bits().hash(hasher);
            r_top.to_bits().hash(hasher);
            half_height.to_bits().hash(hasher);
        }
        Primitive::Capsule { radius, half_height } => {
            9u8.hash(hasher);
            radius.to_bits().hash(hasher);
            half_height.to_bits().hash(hasher);
        }
        Primitive::HexPrism { apothem, half_height } => {
            10u8.hash(hasher);
            apothem.to_bits().hash(hasher);
            half_height.to_bits().hash(hasher);
        }
        Primitive::ImportedMesh { mesh } => {
            11u8.hash(hasher);
            for v in &mesh.aabb_min {
                v.to_bits().hash(hasher);
            }
            for v in &mesh.aabb_max {
                v.to_bits().hash(hasher);
            }
        }
        Primitive::Ellipsoid { radii } => {
            12u8.hash(hasher);
            for v in radii {
                v.to_bits().hash(hasher);
            }
        }
    }
}

/// K7.3: walk a [`Sketch2d`] tree and push every leaf's
/// coordinates into the hasher. Used by [`hash_primitive`] so
/// the body-cache reuse check notices profile edits.
fn hash_sketch2d(
    sketch: &fielddraft_protocol::Sketch2d,
    hasher: &mut DefaultHasher,
) {
    use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
    match sketch {
        Sketch2d::Empty => 0u8.hash(hasher),
        Sketch2d::Prim(prim) => {
            1u8.hash(hasher);
            match prim {
                Sketch2dPrimitive::Circle { center, radius } => {
                    0u8.hash(hasher);
                    for v in center {
                        v.to_bits().hash(hasher);
                    }
                    radius.to_bits().hash(hasher);
                }
                Sketch2dPrimitive::Rectangle {
                    center,
                    half_extents,
                } => {
                    1u8.hash(hasher);
                    for v in center {
                        v.to_bits().hash(hasher);
                    }
                    for v in half_extents {
                        v.to_bits().hash(hasher);
                    }
                }
                Sketch2dPrimitive::Polygon { vertices } => {
                    2u8.hash(hasher);
                    vertices.len().hash(hasher);
                    for vert in vertices {
                        for v in vert {
                            v.to_bits().hash(hasher);
                        }
                    }
                }
                Sketch2dPrimitive::Sector {
                    center,
                    radius,
                    start_angle,
                    sweep_angle,
                } => {
                    3u8.hash(hasher);
                    for v in center {
                        v.to_bits().hash(hasher);
                    }
                    radius.to_bits().hash(hasher);
                    start_angle.to_bits().hash(hasher);
                    sweep_angle.to_bits().hash(hasher);
                }
                Sketch2dPrimitive::Path { segments, closed } => {
                    4u8.hash(hasher);
                    closed.hash(hasher);
                    segments.len().hash(hasher);
                    for seg in segments {
                        hash_sketch_segment(seg, hasher);
                    }
                }
            }
        }
        Sketch2d::Union(a, b) => {
            2u8.hash(hasher);
            hash_sketch2d(a, hasher);
            hash_sketch2d(b, hasher);
        }
        Sketch2d::Difference(a, b) => {
            3u8.hash(hasher);
            hash_sketch2d(a, hasher);
            hash_sketch2d(b, hasher);
        }
        Sketch2d::Intersection(a, b) => {
            4u8.hash(hasher);
            hash_sketch2d(a, hasher);
            hash_sketch2d(b, hasher);
        }
        Sketch2d::Offset { inner, distance } => {
            5u8.hash(hasher);
            hash_sketch2d(inner, hasher);
            distance.to_bits().hash(hasher);
        }
    }
}

fn hash_sketch_segment(
    seg: &fielddraft_protocol::SketchSegment,
    hasher: &mut DefaultHasher,
) {
    use fielddraft_protocol::SketchSegment;
    let hash_pt = |pt: &[f32; 2], h: &mut DefaultHasher| {
        for v in pt {
            v.to_bits().hash(h);
        }
    };
    match seg {
        SketchSegment::Line { start, end } => {
            0u8.hash(hasher);
            hash_pt(start, hasher);
            hash_pt(end, hasher);
        }
        SketchSegment::QuadBezier { p0, p1, p2 } => {
            1u8.hash(hasher);
            hash_pt(p0, hasher);
            hash_pt(p1, hasher);
            hash_pt(p2, hasher);
        }
        SketchSegment::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            2u8.hash(hasher);
            hash_pt(center, hasher);
            radius.to_bits().hash(hasher);
            start_angle.to_bits().hash(hasher);
            sweep_angle.to_bits().hash(hasher);
        }
    }
}

/// Build the `BoundedCsg` that the mesher and raycaster actually
/// query for a given body. Extracted as a free function so both
/// `Kernel::rebuild_bodies` and the raycaster's hot path can share
/// the logic: take the body's subset of features, fold them into a
/// `CsgNode` via `from_features`, then BVH-wrap.
pub fn build_bounded_for_body(features: &[DecodedFeature], indices: &[usize]) -> BoundedCsg {
    let sub: Vec<DecodedFeature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_decoded(&sub);
    BoundedCsg::from_csg(&csg)
}

/// Like [`build_bounded_for_body`], but builds the `BoundedCsg` in the
/// body-LOCAL frame by seeding the transform carry with
/// `inverse(placement)`. Used only by the B-Rep weave (DL-50): every
/// leaf's `world_transform` becomes `placement⁻¹ ∘ feature.transform`,
/// so the octree samples the local SDF and analytic faces + vertices come
/// out placement-free. The mesh / raycast paths keep using the world-frame
/// [`build_bounded_for_body`].
pub fn build_bounded_for_body_local(
    features: &[DecodedFeature],
    indices: &[usize],
    placement: &Transform,
) -> BoundedCsg {
    // Include the body-shell modifier so the weave sees the hollow and can give
    // the inner wall / opening selectable B-Rep faces (DL-75). The shell's open
    // faces are world-space (`body_placement ∘ face_transform`); the build carry
    // `inverse(placement)` folds into them too (see `build`'s Shell arm), so they
    // land in the same body-local frame as the leaves. Earlier this modifier was
    // skipped precisely because that frame correction was missing.
    let sub: Vec<DecodedFeature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_decoded(&sub);
    BoundedCsg::from_csg_with_carry(&csg, &crate::inverse(placement))
}

pub fn build_bounded_with_primitives(
    features: &[DecodedFeature],
    indices: &[usize],
) -> (BoundedCsg, std::collections::HashMap<u32, fielddraft_protocol::Primitive>) {
    let sub: Vec<DecodedFeature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_decoded(&sub);
    let prims: std::collections::HashMap<u32, fielddraft_protocol::Primitive> = csg
        .leaf_primitives()
        .into_iter()
        .map(|(k, v)| (k, v.clone()))
        .collect();
    (BoundedCsg::from_csg(&csg), prims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::PrimitiveModifiers;
    use fielddraft_protocol::Transform;

    fn box_feat(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> DecodedFeature {
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

    fn sphere_feat(id: u32, op: BoolOp, center: [f32; 3], radius: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Sphere {id}"),
            op,
            Primitive::Sphere { center, radius },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        )
    }

    fn gyroid_feat(id: u32, op: BoolOp, target: u32, cell_size: f32, thickness: f32) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Gyroid {id}"),
            op,
            Primitive::Gyroid { cell_size, thickness },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![FeatureId::from_u32(target)],
        )
    }

    #[test]
    fn space_filling_body_skips_weave() {
        // DL-71: a body containing a gyroid (space-filling) gets NO B-Rep — the
        // octree would refine over the whole volume (a 30s wasm move-stall) to
        // weave a wireframe of non-Lipschitz seams we discard anyway. A plain
        // box, by contrast, still weaves its 12 edges.
        let gyroid_body = weave_all(&[
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            gyroid_feat(2, BoolOp::Intersect, 1, 0.5, 0.15),
        ]);
        assert_eq!(gyroid_body.len(), 1);
        assert_eq!(
            gyroid_body[0].0.edge_count(),
            0,
            "a space-filling body must weave an empty B-Rep"
        );

        let plain_box = weave_all(&[box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0])]);
        assert_eq!(plain_box[0].0.edge_count(), 12, "a plain box keeps its edges");
    }

    #[test]
    fn single_union_feature_produces_one_body() {
        let features = vec![box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
    }

    #[test]
    fn two_disjoint_unions_produce_two_bodies() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [100.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn overlapping_unions_stay_separate() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [7.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "overlapping unions must stay separate");
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn untagged_add_merges_into_touching_body() {
        // Onshape semantics: an Add without explicit targets docks into
        // the body it touches — extruding a box face yields ONE part.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Add, [7.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1, "an overlapping Add merges into the body");
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
    }

    #[test]
    fn untagged_add_bridges_all_touching_bodies() {
        // Union is transitive: an Add spanning two separate bodies fuses
        // all three into a single Part.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(3, BoolOp::Add, [15.0, 0.0, 0.0], [12.0, 2.0, 2.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1, "a spanning Add fuses every touched body");
        assert_eq!(bodies[0].feature_indices, vec![0, 1, 2]);
    }

    #[test]
    fn untagged_add_without_contact_seeds_its_own_body() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Add, [100.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "a contact-free Add falls back to seeding");
    }

    #[test]
    fn difference_attaches_to_overlapping_body() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            sphere_feat(2, BoolOp::Remove, [0.0, 0.0, 0.0], 3.0),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
    }

    #[test]
    fn orphan_difference_is_dropped() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere_feat(2, BoolOp::Remove, [100.0, 0.0, 0.0], 3.0),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(
            bodies[0].feature_indices,
            vec![0],
            "orphan difference must not attach"
        );
    }

    #[test]
    fn each_union_is_its_own_body() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(3, BoolOp::New, [15.0, 0.0, 0.0], [20.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 3, "each union must be its own body");
    }

    #[test]
    fn bridging_sphere_weaves_clean_faces_on_both_boxes() {
        // DL-74: once the bridging union fuses the two boxes and the sphere into
        // ONE body (DL-73), the sphere overhangs each box's inner top edge and
        // crosses it at two points. Those two corners share a branch-set
        // {inner-face, top-face, sphere}; the old branch-set-keyed vertex dedup
        // merged them into one phantom corner, leaving the touched faces
        // unorderable (the "left box's edges are incomplete" report). The fix
        // keeps the two corners distinct and trims the buried edge stretch, so
        // every face orders into a connected ring and the solid stays watertight.
        let mut bridge = sphere_feat(3, BoolOp::Add, [0.0, 0.0, 10.0], 8.0);
        bridge.target_ids = vec![FeatureId::from_u32(1), FeatureId::from_u32(2)];
        let features = vec![
            box_feat(1, BoolOp::New, [-12.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            box_feat(2, BoolOp::New, [12.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            bridge,
        ];
        let breps = weave_all(&features);
        assert_eq!(breps.len(), 1, "the bridging union is one body");
        let brep = &breps[0].0;
        assert_eq!(brep.validate(), Ok(()), "woven B-Rep must be well-formed");

        // No face fell back to the unordered (degenerate) loop: every outer and
        // inner loop is a properly connected ring.
        for f in &brep.faces {
            assert!(
                f.outer.is_connected_ring(&brep.edges),
                "every face outer loop must order into a connected ring"
            );
            assert!(
                f.inner.iter().all(|lp| lp.is_connected_ring(&brep.edges)),
                "every hole loop must order into a connected ring"
            );
        }

        // The fused solid is watertight: every edge is shared by exactly two
        // face loops.
        let mut uses = vec![0u32; brep.edge_count()];
        for f in &brep.faces {
            for &(crate::brep::EdgeId(idx), _) in &f.outer.edges {
                uses[idx as usize] += 1;
            }
            for lp in &f.inner {
                for &(crate::brep::EdgeId(idx), _) in &lp.edges {
                    uses[idx as usize] += 1;
                }
            }
        }
        assert!(
            uses.iter().all(|&u| u == 2),
            "every edge used by exactly two face loops (watertight)"
        );

        // Each box's doubly-crossed inner-top edge yields exactly two
        // triple-vertices with the sphere (one per crossing), not one merged
        // phantom — the heart of the DL-74 fix.
        let sphere_branch = crate::csg::BranchId::from_feature(&FeatureId::from_u32(3));
        for box_id in [1u32, 2] {
            let n = brep
                .vertices
                .iter()
                .filter(|v| {
                    v.branches
                        .iter()
                        .any(|b| b.feature_id() == sphere_branch.feature_id())
                        && v.branches.iter().filter(|b| b.feature_id() == box_id).count() == 2
                })
                .count();
            assert_eq!(n, 2, "box {box_id} keeps both sphere-crossing corners");
        }
    }

    #[test]
    fn bridging_union_fuses_both_targets_into_one_body() {
        // Two disjoint boxes each seed their own Part. A sphere that
        // unions BOTH (target_ids = [1, 2]) and physically overlaps each
        // must fuse all three into a single Part — union is transitive,
        // so it cannot dock into only the first body and orphan the
        // second (the "2 Parts that should be 1" bug).
        let mut bridge = sphere_feat(3, BoolOp::Add, [15.0, 0.0, 0.0], 16.0);
        bridge.target_ids = vec![FeatureId::from_u32(1), FeatureId::from_u32(2)];
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            bridge,
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1, "a bridging union fuses both targets");
        assert_eq!(
            bodies[0].feature_indices,
            vec![0, 1, 2],
            "all three features in original order"
        );
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
    }

    #[test]
    fn bridging_union_skips_a_target_it_does_not_touch() {
        // A bridge that overlaps only one of its two targets fuses just
        // that one; the untouched target stays a separate Part (contact
        // still decides each merge).
        let mut bridge = sphere_feat(3, BoolOp::Add, [7.0, 0.0, 0.0], 5.0);
        bridge.target_ids = vec![FeatureId::from_u32(1), FeatureId::from_u32(2)];
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [100.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            bridge,
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "the far target stays separate");
        assert_eq!(bodies[0].feature_indices, vec![0, 2]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn targeted_union_docks_into_seed_body() {
        // An explicit Boolean union tags the second tool with the first
        // tool's id; the two must collapse into one body even though a
        // plain pair of unions would stay separate.
        let mut sphere = sphere_feat(2, BoolOp::Add, [7.0, 0.0, 0.0], 5.0);
        sphere.target_ids = vec![FeatureId::from_u32(1)];
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere,
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1, "an explicit union is one body");
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
    }

    #[test]
    fn untouching_targeted_union_stays_separate() {
        // An explicit union of two solids that do NOT touch must stay
        // two bodies — the target link expresses intent, but contact
        // decides the merge.
        let mut sphere = sphere_feat(2, BoolOp::Add, [100.0, 0.0, 0.0], 5.0);
        sphere.target_ids = vec![FeatureId::from_u32(1)];
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere,
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "disjoint explicit union stays two bodies");
    }

    #[test]
    fn orphan_targeted_union_seeds_its_own_body() {
        // A tagged union whose target is absent must not vanish — it
        // falls back to seeding a fresh body rather than being dropped.
        let mut sphere = sphere_feat(2, BoolOp::Add, [7.0, 0.0, 0.0], 5.0);
        sphere.target_ids = vec![FeatureId::from_u32(99)];
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere,
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "an orphan union still produces a body");
    }

    #[test]
    fn targeted_intersect_attaches_only_to_target_body() {
        let mut gyroid = sphere_feat(3, BoolOp::Intersect, [0.0; 3], 1.0);
        gyroid.target_ids = vec![FeatureId::from_u32(1)];
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            gyroid,
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].feature_indices, vec![0, 2], "intersect attaches to target");
        assert_eq!(bodies[1].feature_indices, vec![1], "non-target stays clean");
    }

    #[test]
    fn hash_is_stable_across_equivalent_feature_sets() {
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere_feat(2, BoolOp::Remove, [0.0, 0.0, 0.0], 2.0),
        ];
        let a = hash_body_payload(&features, &[0, 1]);
        let b = hash_body_payload(&features, &[0, 1]);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_any_field_changes() {
        let baseline =
            vec![box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let h0 = hash_body_payload(&baseline, &[0]);

        let translated = vec![box_feat(1, BoolOp::New, [0.5, 0.0, 0.0], [5.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&translated, &[0]));

        let bigger = vec![box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [6.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&bigger, &[0]));

        let diff = vec![box_feat(1, BoolOp::Remove, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&diff, &[0]));
    }

    /// A unit box decoded with explicit modifiers, so the modifier-field
    /// tests can exercise each warp/blend without a fat builder struct.
    fn box_feat_mods(id: u32, mods: PrimitiveModifiers) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Box {id}"),
            BoolOp::New,
            Primitive::Box {
                center: [0.0; 3],
                half_extents: [5.0; 3],
            },
            Transform::IDENTITY,
            mods,
            vec![],
        )
    }

    #[test]
    fn hash_split_geometry_vs_transform() {
        let base = vec![box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let (g0, t0) = hash_body_payload_split(&base, &[0]);

        let mut translated = base.clone();
        translated[0].transform.translate = [1.0, 0.0, 0.0];
        let (g1, t1) = hash_body_payload_split(&translated, &[0]);
        assert_eq!(g0, g1, "geometry hash must not change on translation");
        assert_ne!(t0, t1, "transform hash must change on translation");

        let mut rotated = base.clone();
        rotated[0].transform.rotation = [0.0, 0.707, 0.0, 0.707];
        let (g2, t2) = hash_body_payload_split(&rotated, &[0]);
        assert_eq!(g0, g2, "geometry hash must not change on rotation");
        assert_ne!(t0, t2, "transform hash must change on rotation");

        let mut blended = base.clone();
        blended[0].blend_radius = 2.0;
        let (g3, t3) = hash_body_payload_split(&blended, &[0]);
        assert_ne!(g0, g3, "geometry hash must change on blend_radius");
        assert_eq!(t0, t3, "transform hash must not change on blend_radius");
    }

    #[test]
    fn detect_uniform_translation_pure_shift() {
        use fielddraft_protocol::Transform;
        let old = vec![Transform { translate: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }];
        let new = vec![Transform { translate: [5.0, 3.0, -1.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }];
        assert_eq!(
            detect_uniform_translation(&old, &new),
            Some([5.0, 3.0, -1.0])
        );
    }

    #[test]
    fn detect_uniform_translation_rejects_rotation_change() {
        use fielddraft_protocol::Transform;
        let old = vec![Transform { translate: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }];
        let new = vec![Transform { translate: [5.0, 0.0, 0.0], rotation: [0.0, 0.707, 0.0, 0.707], scale: [1.0, 1.0, 1.0] }];
        assert_eq!(detect_uniform_translation(&old, &new), None);
    }

    #[test]
    fn detect_uniform_translation_rejects_mixed_deltas() {
        use fielddraft_protocol::Transform;
        let old = vec![
            Transform { translate: [0.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] },
            Transform { translate: [10.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] },
        ];
        let new = vec![
            Transform { translate: [5.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] },
            Transform { translate: [12.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] },
        ];
        assert_eq!(detect_uniform_translation(&old, &new), None);
    }

    #[test]
    fn detect_uniform_translation_returns_none_on_zero_delta() {
        use fielddraft_protocol::Transform;
        let t = vec![Transform { translate: [1.0, 2.0, 3.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0, 1.0, 1.0] }];
        assert_eq!(detect_uniform_translation(&t, &t), None);
    }

    #[test]
    fn hash_changes_when_modifier_fields_change() {
        let base = box_feat_mods(1, PrimitiveModifiers::NONE);
        let h0 = hash_body_payload(std::slice::from_ref(&base), &[0]);

        // Each variant alters the decoded geometry node (or the junction
        // blend), so the payload hash must move. Displacement only
        // registers a node when BOTH depth and freq are non-zero, so it
        // is exercised as a pair rather than per-field.
        let variants = vec![
            PrimitiveModifiers { blend_radius: 1.5, ..PrimitiveModifiers::NONE },
            PrimitiveModifiers { chamfer_size: 1.5, ..PrimitiveModifiers::NONE },
            PrimitiveModifiers { twist_rate: 1.5, ..PrimitiveModifiers::NONE },
            PrimitiveModifiers { bend_rate: 1.5, ..PrimitiveModifiers::NONE },
            PrimitiveModifiers {
                displace_depth: 1.5,
                displace_freq: 1.5,
                ..PrimitiveModifiers::NONE
            },
            PrimitiveModifiers { shell_thickness: 1.5, ..PrimitiveModifiers::NONE },
        ];

        for mods in variants {
            let modified = box_feat_mods(1, mods);
            let h1 = hash_body_payload(&[modified], &[0]);
            assert_ne!(h0, h1, "hash must change when a modifier changes");
        }
    }
}
