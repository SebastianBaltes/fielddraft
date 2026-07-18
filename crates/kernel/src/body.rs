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

use fielddraft_protocol::{BoolOp, FeatureId, Transform};

use crate::brep::octree::Octree;
use crate::brep::weaving::{weave, weave_parametric};
use crate::brep::BRep;
use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, CsgNode, DecodedFeature, EdgeFillet};
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
            // NaN/∞-Guard (v0.6.355): ein nicht-endlicher Float in der
            // Eingabe — z.B. ein NaN aus dem Web-Sketch-Solver — würde die
            // Weave-Numerik weit von der Quelle entfernt vergiften und die
            // WASM-Instanz per Panik abreißen (`catch_unwind` fängt auf
            // wasm32 nichts). Stattdessen: leeres B-Rep unter dem aktuellen
            // Key cachen; sobald ein Edit die Geometrie repariert, ändert
            // sich der Key und es wird normal neu gewoben.
            if !self.feature_indices.iter().all(|&i| features[i].all_finite()) {
                eprintln!(
                    "weave: body {:?} carries non-finite geometry (NaN/inf); \
                     serving an empty B-Rep instead of weaving",
                    self.id
                );
                self.brep = Some((key, BRep::new()));
                return &self.brep.as_ref().unwrap().1;
            }
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
            let brep = if let Some(parametric_brep) =
                parametric.then(|| weave_parametric(&bounded)).flatten()
            {
                // Constructive/analytic weave — no octree at all. Tried FIRST so
                // a Helix/Sweep, whose surface is weave-excluded from the octree
                // path (non-Lipschitz, DL-71), still gets its edges traced
                // constructively from the profile (ticket ifog). Cheap: it bails
                // to `None` immediately for anything it can't lift.
                parametric_brep
            } else if bounded_contains_space_filling(&bounded) {
                // Space-filling (gyroid TPMS) or a non-constructible Helix/Sweep
                // profile: the OCTREE stays excluded outright (it would refine
                // on tens of thousands of cells — the DL-71 wasm stall). But the
                // octree-free graceful seed path CAN weave a gyroid's Boolean
                // rim seams (SSI seeds the gyroid as implicit operand, the dual
                // field traces them) — so try it; Helix/Sweep and other
                // non-gyroid non-Lipschitz bodies fail its scope gate and stay
                // edgeless as before, the renderer covers the shape.
                parametric
                    .then(|| crate::brep::seed_weaver::weave_branch_seeds_partial(&bounded))
                    .flatten()
                    .unwrap_or_default()
            } else if let Some(hybrid) = parametric
                .then(|| {
                    crate::brep::seed_weaver::weave_branch_seeds_hybrid(
                        &bounded,
                        topology_min_cell_size(&self.aabb),
                    )
                })
                .flatten()
            {
                // Hybrid branch-seed weave (Stufe A+B+C, tickets 7vbe/qmvn +
                // PARAMETRIC-WEAVING-PLAN Stufe C): the strict `weave_parametric`
                // above bailed on an unresolvable transition seam. Every
                // analytically resolvable edge is kept at parametric precision —
                // each operand's own edges AND the analytic Boolean boundary
                // (SSI cross-pair) edges — and ONLY the genuinely unresolvable
                // seams are filled from the octree wireframe (their pairs' edges
                // consumed, endpoint vertices welded into the seed corners). The
                // result is a complete B-Rep instead of the earlier partial open
                // shell. Returns `None` for out-of-scope bodies and when nothing
                // resolved → whole-body octree below.
                hybrid
            } else {
                let min_cell = topology_min_cell_size(&self.aabb);
                let octree = Octree::build(&bounded, min_cell);
                weave(&bounded, &octree)
            };
            self.brep = Some((key, brep));
        }
        &self.brep.as_ref().unwrap().1
    }

    /// Refresh [`Body::placement`] from the anchor feature without touching
    /// the weave. Cheap (O(features)) — placement never depends on the
    /// B-Rep, so callers that only need the current world pose can skip
    /// [`Body::ensure_brep`] entirely.
    pub fn refresh_placement(&mut self, features: &[DecodedFeature]) {
        self.placement = compute_body_placement(features, &self.feature_indices);
    }

    /// `true` when the cached B-Rep still matches the body's
    /// placement-invariant content key — i.e. [`Body::ensure_brep`] would be
    /// a pure cache hit. Refreshes `placement` as a side effect (the key is
    /// computed relative to it). Never weaves; used by the kernel's B-Rep
    /// suppression gate (gizmo drags) to serve cache-or-empty.
    pub fn brep_is_current(&mut self, features: &[DecodedFeature]) -> bool {
        self.refresh_placement(features);
        let key = hash_body_local_payload(features, &self.feature_indices, &self.placement);
        matches!(&self.brep, Some((k, _)) if *k == key)
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

/// Does `node`'s CSG tree contain a **weave-excluded** primitive (a gyroid
/// TPMS, [`Primitive::is_space_filling`], or a user CustomSdf —
/// [`Primitive::is_brep_excluded`])? Such a body skips the B-Rep weave
/// entirely (DL-71): its surface threads through nearly the whole volume (or
/// has arbitrary user-defined topology), so the octree would refine on tens
/// of thousands of cells to produce a wireframe of non-Lipschitz seams we
/// discard anyway.
fn bounded_contains_space_filling(node: &BoundedCsg) -> bool {
    match node {
        BoundedCsg::Leaf { primitive, .. } => primitive.is_brep_excluded(),
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
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => bounded_contains_space_filling(inner),
        // A helical sweep counts as weave-excluded too (DL-71 family):
        // the thread surface would make the octree refine on thousands
        // of cells for non-Lipschitz seams the weaver discards anyway.
        BoundedCsg::Helix { .. } => true,
        // A general sweep is weave-excluded for the same reason (non-analytic
        // surface; the weaver would refine on it pointlessly).
        BoundedCsg::Sweep { .. } => true,
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
                // An Add docks ONLY through an explicit link (`target_ids`):
                // a face-extrude carries its owner body, a merge-scoped
                // feature carries the chosen targets, and a Boolean union
                // routes its non-seed tools in via the seed link. An
                // *untagged* Add never merges by mere contact — touching or
                // overlapping solids stay separate Parts unless an explicit
                // Boolean joins them (hard product rule: no implicit union).
                // So an untagged Add seeds its own body.
                if feature.target_ids.is_empty() {
                    groups.push(BodyPartition {
                        feature_indices: vec![i],
                        aabb: feat_aabb,
                    });
                    continue;
                }
                // Targeted Add: dock into the bodies that hold one of its
                // targets. It can bridge *several* bodies at once — the
                // sphere that unions a left and a right solid fuses all
                // three into one Part, because union is transitive. The
                // link expresses intent, contact decides the merge — a
                // targeted Add of two *disjoint* solids stays two Parts.
                let connected: Vec<usize> = (0..groups.len())
                    .filter(|&idx| {
                        let targeted = groups[idx].feature_indices.iter().any(|&fi| {
                            let root = features[fi].id.root_ancestor();
                            feature.target_ids.iter().any(|tid| root == tid)
                        });
                        targeted && aabbs_overlap(groups[idx].aabb, feat_aabb)
                    })
                    .collect();
                match connected.split_first() {
                    // Targets exist but none touched: this Add seeds its own Part.
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
                    // Each feature keeps the affected body's AABB analytically
                    // current, just like the SDF (ticket f88u): an Intersect
                    // clips the body to the tool's bound; a Remove retreats the
                    // bound past the slab the tool provably caps. Both stay a
                    // conservative superset of the true result, so the meshing
                    // domain (rebuilt from the CSG tree) and docking stay sound.
                    let is_remove = matches!(feature.op, BoolOp::Remove);
                    let inner = if is_remove {
                        feature_inner_world_aabb(feature)
                    } else {
                        None
                    };
                    // indexed element is both read and mutated; iter_mut rewrite is risky
                    #[allow(clippy::needless_range_loop)]
                    for idx in 0..groups.len() {
                        let matches = groups[idx].feature_indices.iter().any(|&fi| {
                            let root = features[fi].id.root_ancestor();
                            feature.target_ids.iter().any(|tid| root == tid)
                        });
                        if matches {
                            groups[idx].feature_indices.push(i);
                            if is_remove {
                                if let Some(inner) = inner {
                                    groups[idx].aabb = subtract_cap(groups[idx].aabb, inner);
                                }
                            } else if !feat_aabb.is_empty() {
                                // A bounded Intersect tool clips the body to it.
                                // An UNBOUNDED / space-filling one (gyroid → an
                                // empty AABB sentinel, csg.rs) imposes no box
                                // constraint — keep the body's current bound
                                // rather than collapsing it to empty (ticket
                                // ipnx: a Sphere ∩ Gyroid ∩ Box otherwise lost
                                // its whole AABB, dropping the orbit pivot back
                                // to the origin).
                                groups[idx].aabb =
                                    Aabb::intersect(groups[idx].aabb, feat_aabb);
                            }
                        }
                    }
                } else {
                    // No explicit scope: a Remove/Intersect must name what it
                    // acts on — it does NOT dock by mere contact (that would
                    // hide ambiguity; Onshape requires a scope). Seed its own
                    // standalone body so it renders as a red warning preview;
                    // the feature itself is flagged with a merge-scope warning.
                    groups.push(BodyPartition {
                        feature_indices: vec![i],
                        aabb: feat_aabb,
                    });
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
    /// Insertion-ordered (key → woven breps) entries, oldest first. A small
    /// keyed cache instead of a single slot: one regeneration can `ensure`
    /// at several feature prefixes (every weave-needing query feature hits
    /// it with the features decoded *so far*), and the kernel re-installs
    /// the cache across regenerations — a single slot would thrash between
    /// those prefixes and re-weave on every regen.
    entries: Vec<(u64, Vec<(BRep, Transform)>)>,
}

/// Max retained [`BrepCache`] entries. Generous for the handful of distinct
/// feature prefixes a document's queries resolve against, small enough that
/// stale geometry never accumulates meaningfully.
const BREP_CACHE_CAPACITY: usize = 8;

impl BrepCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Weave (or reuse) the B-Reps for the current `features` spine and
    /// return them paired with their world placements. Re-weaves only
    /// when the content key is not cached.
    pub fn ensure(&mut self, features: &[DecodedFeature]) -> &[(BRep, Transform)] {
        let all: Vec<usize> = (0..features.len()).collect();
        let key = hash_body_payload(features, &all);
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            // Refresh recency so repeatedly-used prefixes outlive one-offs.
            let entry = self.entries.remove(i);
            self.entries.push(entry);
        } else {
            if self.entries.len() >= BREP_CACHE_CAPACITY {
                self.entries.remove(0);
            }
            self.entries.push((key, weave_all(features)));
        }
        &self.entries.last().unwrap().1
    }

    /// Drop the cache, forcing the next [`ensure`](BrepCache::ensure) to
    /// re-weave. The content key normally handles invalidation; explicit
    /// clearing is a belt-and-braces hook for callers (e.g. a commit) that
    /// prefer eager invalidation.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// World-space AABB of a single feature: its local geometry AABB
/// transformed by its placement.
fn feature_world_aabb(feature: &DecodedFeature) -> Aabb {
    feature.node.aabb().transformed(&feature.transform)
}

/// Does `rotation` (a quaternion) map every world axis onto ±another axis — a
/// "grid" rotation? Only then does transforming an axis-aligned box keep it
/// axis-aligned, so an *inner* (provably-inside) box stays inside after
/// placement. Any oblique rotation returns `false`.
fn rotation_is_axis_aligned(rotation: [f32; 4]) -> bool {
    const EPS: f32 = 1e-5;
    for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let r = crate::transform::quat_rotate(rotation, axis);
        let mut units = 0;
        for k in 0..3 {
            let a = r[k].abs();
            if a > 1.0 - EPS {
                units += 1;
            } else if a > EPS {
                return false;
            }
        }
        if units != 1 {
            return false;
        }
    }
    true
}

/// Largest world-space axis-aligned box PROVABLY inside a feature's solid, or
/// `None` when it can't be determined safely. Used to shrink a body's AABB when
/// this feature is *removed* (ticket f88u): everything of the body inside this
/// box is definitely gone, so the body's bound may retreat to the box's near
/// face.
///
/// Handles the bare primitives whose inscribed box is analytic — Box (itself),
/// Sphere (a `radius/√3` cube), Cylinder (`radius/√2` across, full height) —
/// and only under an axis-aligned (grid) placement, so the transformed inner
/// box stays axis-aligned AND stays inside (`Aabb::transformed` corner-bounds a
/// box exactly under a grid rotation, but overshoots under an oblique one).
/// Everything else — modifier-wrapped nodes, curved cutters we don't model,
/// oblique rotations — returns `None`, and the body keeps its conservative
/// bound. This NEVER over-shrinks, so the meshing domain (rebuilt from the CSG
/// tree, not this AABB) and Add-docking stay correct.
fn feature_inner_world_aabb(feature: &DecodedFeature) -> Option<Aabb> {
    use fielddraft_protocol::Primitive;
    const SQRT3_INV: f32 = 0.577_350_26; // 1/√3
    const SQRT2_INV: f32 = 0.707_106_77; // 1/√2
    let primitive = match &feature.node {
        CsgNode::Leaf { primitive, .. } => primitive,
        _ => return None,
    };
    let local = match primitive {
        Primitive::Box {
            center,
            half_extents,
        } => Aabb {
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
        Primitive::Sphere { center, radius } => {
            let h = radius * SQRT3_INV;
            Aabb {
                min: [center[0] - h, center[1] - h, center[2] - h],
                max: [center[0] + h, center[1] + h, center[2] + h],
            }
        }
        // Cylinder axis is local Y (see `Aabb::of_primitive`).
        Primitive::Cylinder {
            center,
            radius,
            half_height,
        } => {
            let r = radius * SQRT2_INV;
            Aabb {
                min: [center[0] - r, center[1] - half_height, center[2] - r],
                max: [center[0] + r, center[1] + half_height, center[2] + r],
            }
        }
        _ => return None,
    };
    if !rotation_is_axis_aligned(feature.transform.rotation) {
        return None;
    }
    Some(local.transformed(&feature.transform))
}

/// Shrink `body` by the axis-aligned slab that `inner` (a box provably inside a
/// removed tool) fully caps. `inner` caps `body` on axis `k` only when it spans
/// `body`'s entire cross-section in the other two axes; then everything of
/// `body` past `inner`'s near face on `k` is gone and the bound retreats to it.
/// Never grows `body` and never drops surviving material (`inner ⊆ tool`), so
/// the result stays a conservative superset of the true cut solid.
fn subtract_cap(body: Aabb, inner: Aabb) -> Aabb {
    if body.is_empty() || inner.is_empty() {
        return body;
    }
    let mut out = body;
    for k in 0..3 {
        let (u, v) = ((k + 1) % 3, (k + 2) % 3);
        // `inner` must cover `body`'s full cross-section perpendicular to `k`
        // (tested against the ORIGINAL body so multi-axis caps don't interact).
        let spans_cross = inner.min[u] <= body.min[u]
            && inner.max[u] >= body.max[u]
            && inner.min[v] <= body.min[v]
            && inner.max[v] >= body.max[v];
        if !spans_cross {
            continue;
        }
        // Cap the +k side: `inner` starts inside `body` and reaches past its +k
        // face → retreat the bound to `inner`'s near face.
        if inner.max[k] >= body.max[k] && inner.min[k] > body.min[k] {
            out.max[k] = out.max[k].min(inner.min[k]);
        }
        // Cap the −k side symmetrically.
        if inner.min[k] <= body.min[k] && inner.max[k] < body.max[k] {
            out.min[k] = out.min[k].max(inner.max[k]);
        }
    }
    out
}

fn aabbs_overlap(a: Aabb, b: Aabb) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    // A flush adjacency (a face-extrude grown from a body's face) meets its
    // parent at a shared plane, but that plane's coordinate is mesh-derived
    // (the centroid of a marching-cubes face) and lands a fraction of a micron
    // off the parent's exact bound. Expand by a tiny tolerance so genuinely
    // touching bodies still dock; bodies a real gap apart stay separate
    // (ticket px2o). Floor: 1 µm, far below any meaningful CAD feature size.
    // Scale-relative term: on metre-scale models the f32 coordinate noise of
    // the AABBs themselves exceeds a fixed micron, so the slack additionally
    // tracks the pair's extent (1e-6 of the larger span — epsilon policy,
    // ARCHITECTURE-ROADMAP Punkt 3).
    let span = (0..3)
        .map(|k| (a.max[k] - a.min[k]).max(b.max[k] - b.min[k]))
        .fold(0.0_f32, f32::max);
    let tol = (span * 1e-6).max(1e-3);
    a.min[0] <= b.max[0] + tol
        && a.max[0] + tol >= b.min[0]
        && a.min[1] <= b.max[1] + tol
        && a.max[1] + tol >= b.min[1]
        && a.min[2] <= b.max[2] + tol
        && a.max[2] + tol >= b.min[2]
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
        hash_feature_geometry(f, &mut geo);

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

/// Canonical geometry contribution of one feature to the body hash: the
/// rmp-serde byte serialisation of its local [`CsgNode`] tree plus the
/// junction blend, fed into the hasher as one length-prefixed byte string.
///
/// This replaces ~520 lines of hand-mirrored per-variant hashing
/// (`hash_node`/`hash_primitive`/`hash_sketch2d`/…): every `CsgNode` /
/// `Primitive` / `Sketch2d` variant and every future field now enters the
/// hash automatically through serde, so a forgotten variant can no longer
/// cause a silent stale cache. (The hand-walk had already accumulated a
/// real tag collision: `CsgNode::Offset` and `CsgNode::Extrude` both
/// hashed discriminant `14u8`.)
///
/// Properties relied on:
/// * rmp writes f32/f64 **bit-exactly** (`-0.0 ≠ 0.0`, NaN payloads kept),
///   matching the old `to_bits()` semantics.
/// * The hashed types contain no `HashMap`/`HashSet` (checked transitively:
///   only `Vec`s, arrays and scalars), so the byte form is deterministic.
/// * An imported mesh contributes its precomputed content fingerprint
///   instead of its triangle buffer (see
///   `csg::serialize_primitive_for_hash`), keeping this O(tree), not
///   O(mesh) — and *more* correct than the old AABB-only stand-in.
/// * Hashes are in-memory cache keys only (`Body` is never serialised),
///   so cross-run stability is not load-bearing; determinism within a
///   build is guaranteed (`DefaultHasher::new()` uses fixed keys).
fn hash_feature_geometry(f: &DecodedFeature, hasher: &mut DefaultHasher) {
    let bytes = rmp_serde::to_vec(&(&f.node, f.blend_radius.to_bits(), f.blend_mode))
        .expect("CsgNode hash serialisation is infallible");
    bytes.hash(hasher);
}

/// Build the `BoundedCsg` that the mesher and raycaster actually
/// query for a given body. Extracted as a free function so both
/// `Kernel::rebuild_bodies` and the raycaster's hot path can share
/// the logic: take the body's subset of features, fold them into a
/// `CsgNode` via `from_features`, then BVH-wrap.
pub fn build_bounded_for_body(features: &[DecodedFeature], indices: &[usize]) -> BoundedCsg {
    build_bounded_for_body_rounded(features, indices, None, &[])
}

/// Like [`build_bounded_for_body`] but rounds every Boolean junction by
/// `global_round` first (feature `round_all`, d). Used by the GPU-SDF compile
/// path so the ray-marched render shows the deburred edges.
///
/// `edge_fillets` are the per-edge fillets (`fillet_edge`) resolved during
/// regen: they must be applied HERE, not only in the pick tree, or the mesher
/// and the GPU-SDF renderer march the un-filleted field and the fillet is
/// invisible in every mode (#350). Edge fillets run BEFORE the global round —
/// the round then only fills the remaining hard junctions and leaves the
/// per-edge `MaskedBlend` nodes untouched.
pub fn build_bounded_for_body_rounded(
    features: &[DecodedFeature],
    indices: &[usize],
    global_round: Option<f32>,
    edge_fillets: &[EdgeFillet],
) -> BoundedCsg {
    let sub: Vec<DecodedFeature> = indices.iter().map(|&i| features[i].clone()).collect();
    let mut csg = CsgNode::from_decoded(&sub).apply_edge_fillets(edge_fillets);
    if let Some(r) = global_round {
        csg = csg.apply_global_round(r);
    }
    // #384 Stufe 3a: normalise Offsets down to the exactest subtrees LAST, so
    // the pass sees the final node shapes (fillets → MaskedBlend, round →
    // SmoothUnion) and correctly stops at them. Zero-set-preserving.
    // Phase A2: dann Erosions-Offsets an den Blättern analytisch einfalten
    // (`fold_offset_leaves`) — korrigiert die Nullmenge dort, wo das Blatt
    // vorher kein exaktes SDF war (Profil-Booleans, Sketch2d::Offset).
    csg = csg.push_down_offsets().fold_offset_leaves();
    // Phase B: Kanal-Korrekturkörper an den dualen Stopp-Knoten (läuft auf
    // dem BoundedCsg, Transforms bereits welt-gefaltet). Synchron zur
    // Regen-Zeit — die Naht-Kurven kommen aus SSI-Seeds + Feld-Tracing,
    // nicht aus dem (asynchronen) Produktions-Weave. Der Weave-Baupfad
    // (`build_bounded_for_body_local`) bleibt bewusst kanalfrei (Phase C).
    crate::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&csg),
        crate::offset_channel::DEFAULT_CHANNEL_TOL,
    )
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
    // offset_tree Phase A2 (#384), Audit-Punkt 1: der Weave-Baupfad MUSS
    // dieselbe Offset-Normalisierung fahren wie die Mesher-/Render-Pfade
    // (Pushdown + Blatt-Einfalten), sonst divergieren B-Rep-Kanten und
    // gemarchtes Feld überall dort, wo der Fold die Nullmenge korrigiert —
    // Feature-Snap-Pins fielen still weg (export_mesh, ON_SURFACE_TOL) und
    // CGAL-Schutzkanten lägen neben der Fläche (native_prepare). Edge-
    // Fillets/Global-Round bleiben hier wie bisher außen vor (der Weave
    // webt den harten Baum).
    let csg = CsgNode::from_decoded(&sub)
        .push_down_offsets()
        .fold_offset_leaves();
    BoundedCsg::from_csg_with_carry(&csg, &crate::inverse(placement))
}

pub fn build_bounded_with_primitives(
    features: &[DecodedFeature],
    indices: &[usize],
) -> (BoundedCsg, std::collections::HashMap<u32, fielddraft_protocol::Primitive>) {
    build_bounded_with_primitives_rounded(features, indices, None, &[])
}

/// Like [`build_bounded_with_primitives`] but rounds every Boolean junction by
/// `global_round` first (feature `round_all`, d), so the render mesh shows the
/// deburred edges. `None` is the plain build. The primitive map is unaffected —
/// the round only rewrites Boolean nodes, never leaves.
pub fn build_bounded_with_primitives_rounded(
    features: &[DecodedFeature],
    indices: &[usize],
    global_round: Option<f32>,
    edge_fillets: &[EdgeFillet],
) -> (BoundedCsg, std::collections::HashMap<u32, fielddraft_protocol::Primitive>) {
    let sub: Vec<DecodedFeature> = indices.iter().map(|&i| features[i].clone()).collect();
    let mut csg = CsgNode::from_decoded(&sub).apply_edge_fillets(edge_fillets);
    if let Some(r) = global_round {
        csg = csg.apply_global_round(r);
    }
    // #384 Stufe 3a + Phase A2 — same normalisation as
    // `build_bounded_for_body_rounded`. Runs BEFORE the primitive-map
    // extraction, and that order is now LOAD-BEARING: `fold_offset_leaves`
    // rewrites leaf primitive parameters, and the map must carry the
    // FOLDED parameters (they describe what the mesher marches —
    // docs/OFFSET-TREE-AUDIT.md, Konsument 1). Leaf FeatureIds are
    // fold-invariant, so the map keys are unchanged.
    csg = csg.push_down_offsets().fold_offset_leaves();
    let prims: std::collections::HashMap<u32, fielddraft_protocol::Primitive> = csg
        .leaf_primitives()
        .into_iter()
        .map(|(k, v)| (k, v.clone()))
        .collect();
    // Phase B: Kanal-Synthese wie in `build_bounded_for_body_rounded` —
    // NACH der Primitive-Map (Kanäle ändern keine Blatt-Parameter).
    let bounded = crate::offset_channel::synthesize_offset_channels(
        BoundedCsg::from_csg(&csg),
        crate::offset_channel::DEFAULT_CHANNEL_TOL,
    );
    (bounded, prims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::PrimitiveModifiers;
    use fielddraft_protocol::{Primitive, Transform};

    /// v0.6.355: Ein NaN aus dem Sketch-Solver darf den Weave nicht mehr
    /// paniken lassen (= WASM-Abort im Web). Der Weave-Einstieg erkennt die
    /// nicht-endliche Geometrie vorab und cached ein leeres B-Rep unter dem
    /// aktuellen Key — der nächste reparierende Edit webt normal neu.
    #[test]
    fn nan_sketch_weaves_to_empty_brep_instead_of_panicking() {
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [f32::NAN, 0.0], [10.0, 10.0]],
        });
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "NaN-Extrude".into(),
            BoolOp::New,
            Primitive::Extrude { sketch, half_depth_z: 5.0, center: [0.0; 3], wall_subs: None },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        assert!(!f.all_finite(), "NaN vertex must be detected in the decoded feature");

        let woven = weave_all(&[f]);
        assert_eq!(woven.len(), 1, "one body partition");
        let (brep, _) = &woven[0];
        assert_eq!(brep.face_count(), 0, "empty B-Rep instead of a NaN weave");
        assert_eq!(brep.edge_count(), 0);
        assert_eq!(brep.vertex_count(), 0);
    }

    /// Gegenprobe zum NaN-Guard: eine endliche Version desselben Features
    /// webt weiterhin ein nicht-leeres B-Rep (der Guard greift nur bei
    /// nicht-endlicher Geometrie).
    #[test]
    fn finite_sketch_still_weaves_normally() {
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [20.0, 0.0], [10.0, 10.0]],
        });
        let f = DecodedFeature::from_parts(
            FeatureId::from_u32(1),
            "Extrude".into(),
            BoolOp::New,
            Primitive::Extrude { sketch, half_depth_z: 5.0, center: [0.0; 3], wall_subs: None },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![],
        );
        assert!(f.all_finite());
        let woven = weave_all(&[f]);
        assert_eq!(woven.len(), 1);
        assert!(woven[0].0.face_count() > 0, "finite geometry must still weave");
    }

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

    fn torus_feat(
        id: u32,
        op: BoolOp,
        target: u32,
        center: [f32; 3],
        major: f32,
        minor: f32,
    ) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Torus {id}"),
            op,
            Primitive::Torus { center, major_radius: major, minor_radius: minor },
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![FeatureId::from_u32(target)],
        )
    }

    #[test]
    fn graceful_partial_seed_keeps_operand_edges_in_ensure_brep() {
        // Tickets 7vbe/qmvn (Stufe A+B): a Union of a fully seed-weavable box
        // with a torus whose tube wall is non-analytic. The strict branch-seed
        // path bails (every box×torus cross pair is unresolved), which used to
        // throw the WHOLE body onto the coarse octree. `ensure_brep` now falls
        // back to the GRACEFUL partial seed weave, so the box operand's exact
        // 12 analytic edges + 8 corners survive — driven through the real
        // user-facing `weave_all` → `ensure_brep` path, not the unit weaver.
        let bodies = weave_all(&[
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            // outer tube radius 8+3 = 11 > box half-extent 10 → real overlap;
            // a TARGETED Add (→ box id 1) docks them into one body (the product
            // rule forbids implicit merge by mere contact).
            torus_feat(2, BoolOp::Add, 1, [0.0, 0.0, 0.0], 8.0, 3.0),
        ]);
        assert_eq!(bodies.len(), 1, "box ∪ torus is one connected body");
        let brep = &bodies[0].0;

        // The box operand's full analytic wireframe is present.
        let box_edges = brep
            .edges
            .iter()
            .filter(|e| e.signature.0.feature_id() == 1 && e.signature.1.feature_id() == 1)
            .count();
        assert_eq!(box_edges, 12, "all 12 analytic box edges survive the graceful weave");

        // Every box corner lands on an exact ±10 box corner — analytic
        // precision the octree fallback would have lost.
        let box_corners: Vec<_> = brep
            .vertices
            .iter()
            .filter(|v| v.branches.len() == 3 && v.branches.iter().all(|b| b.feature_id() == 1))
            .collect();
        assert_eq!(box_corners.len(), 8, "all 8 analytic box corners survive");
        for v in &box_corners {
            for &c in &v.position {
                assert!(
                    (c.abs() - 10.0).abs() < 1e-3,
                    "box corner coord {c} not ±10 — analytic precision lost"
                );
            }
        }
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
    fn space_filling_body_weaves_rim_edges_octree_free() {
        // DL-71 (lifted for gyroids): the OCTREE stays excluded for a
        // space-filling body (it would refine over the whole volume — the 30s
        // wasm move-stall), but the octree-free graceful seed path now weaves
        // the gyroid's Boolean rim seams (SSI seeds the gyroid as implicit
        // operand, the dual field traces). A plain box still weaves its 12
        // edges through the strict path.
        let gyroid_body = weave_all(&[
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            gyroid_feat(2, BoolOp::Intersect, 1, 0.5, 0.15),
        ]);
        assert_eq!(gyroid_body.len(), 1);
        assert!(
            gyroid_body[0].0.edge_count() > 0,
            "a gyroid body must weave its Boolean rim edges via the seed path"
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

    /// A boolean tool (Remove/Intersect) targeting `target`.
    fn tool_feat(id: u32, op: BoolOp, target: u32, prim: Primitive) -> DecodedFeature {
        DecodedFeature::from_parts(
            FeatureId::from_u32(id),
            format!("Tool {id}"),
            op,
            prim,
            Transform::IDENTITY,
            PrimitiveModifiers::NONE,
            vec![FeatureId::from_u32(target)],
        )
    }

    #[test]
    fn remove_retreats_body_aabb_past_a_box_cut() {
        // f88u: a sphere (r=10) with its whole +x half carved by a box. The
        // body AABB must follow the visible LEFT half — x ∈ [−10, 0] — not stay
        // the full sphere, so the orbit pivot lands in the remaining material.
        let features = vec![
            sphere_feat(1, BoolOp::New, [0.0, 0.0, 0.0], 10.0),
            tool_feat(
                2,
                BoolOp::Remove,
                1,
                Primitive::Box {
                    center: [10.0, 0.0, 0.0],
                    half_extents: [10.0, 20.0, 20.0],
                },
            ),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        let bb = bodies[0].aabb;
        assert!((bb.min[0] + 10.0).abs() < 1e-4, "min x {}", bb.min[0]);
        assert!(
            bb.max[0].abs() < 1e-4,
            "max x should retreat to the cut plane (0), got {}",
            bb.max[0]
        );
        // y and z are untouched by the cut.
        assert!((bb.min[1] + 10.0).abs() < 1e-4 && (bb.max[1] - 10.0).abs() < 1e-4);
        let cx = 0.5 * (bb.min[0] + bb.max[0]);
        assert!((cx + 5.0).abs() < 1e-4, "centre x should be −5, got {cx}");
    }

    #[test]
    fn intersect_clips_body_aabb_to_the_tool_bound() {
        // Box(±10) ∩ Box(centre +5, ±10) → analytic body AABB = the AABB
        // intersection, x ∈ [−5, 10]; y, z stay ±10.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            tool_feat(
                2,
                BoolOp::Intersect,
                1,
                Primitive::Box {
                    center: [5.0, 0.0, 0.0],
                    half_extents: [10.0, 10.0, 10.0],
                },
            ),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        let bb = bodies[0].aabb;
        assert!((bb.min[0] + 5.0).abs() < 1e-4, "min x {}", bb.min[0]);
        assert!((bb.max[0] - 10.0).abs() < 1e-4, "max x {}", bb.max[0]);
        assert!((bb.min[1] + 10.0).abs() < 1e-4 && (bb.max[1] - 10.0).abs() < 1e-4);
    }

    #[test]
    fn remove_with_a_partial_cutter_keeps_the_conservative_bound() {
        // A small sphere carved from a big box removes interior material but
        // spans NO full cross-section → the body AABB must stay the full box.
        // Over-shrinking here would clip live geometry (docking / pivot).
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            tool_feat(
                2,
                BoolOp::Remove,
                1,
                Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                },
            ),
        ];
        let bodies = partition(&features);
        let bb = bodies[0].aabb;
        for k in 0..3 {
            assert!(
                (bb.min[k] + 10.0).abs() < 1e-4 && (bb.max[k] - 10.0).abs() < 1e-4,
                "axis {k} shrank but the cutter does not span the cross-section"
            );
        }
    }

    #[test]
    fn intersect_with_a_space_filling_tool_keeps_the_body_aabb() {
        // Ticket ipnx: Sphere ∩ Gyroid ∩ small offset Box. The gyroid is
        // space-filling and reports an EMPTY AABB sentinel — it must NOT
        // collapse the body's bound to empty (which dropped the orbit pivot to
        // the origin). The final box then clips the AABB to its own extent.
        let features = vec![
            sphere_feat(1, BoolOp::New, [0.0, 0.0, 0.0], 50.0),
            gyroid_feat(2, BoolOp::Intersect, 1, 10.0, 1.0),
            DecodedFeature::from_parts(
                FeatureId::from_u32(3),
                "Box".into(),
                BoolOp::Intersect,
                Primitive::Box {
                    center: [0.0, 0.0, 0.0],
                    half_extents: [3.0, 4.0, 3.0],
                },
                Transform {
                    translate: [8.0, 22.0, 30.0],
                    ..Transform::IDENTITY
                },
                PrimitiveModifiers::NONE,
                vec![FeatureId::from_u32(1)],
            ),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        let bb = bodies[0].aabb;
        assert!(!bb.is_empty(), "gyroid intersect wrongly emptied the body AABB");
        // Clipped to the translated box → centre ≈ (8, 22, 30).
        let c = [
            0.5 * (bb.min[0] + bb.max[0]),
            0.5 * (bb.min[1] + bb.max[1]),
            0.5 * (bb.min[2] + bb.max[2]),
        ];
        assert!(
            (c[0] - 8.0).abs() < 0.5 && (c[1] - 22.0).abs() < 0.5 && (c[2] - 30.0).abs() < 0.5,
            "AABB centre should follow the box intersect, got {c:?}"
        );
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
    fn untagged_add_overlapping_body_stays_separate() {
        // Hard product rule: touching/overlapping solids NEVER auto-merge.
        // An untagged Add (no explicit target link) seeds its own body —
        // merging requires an explicit Boolean feature.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Add, [7.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "an untagged Add never merges by contact");
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn untagged_add_spanning_two_bodies_stays_separate() {
        // No implicit union, even when an untagged Add overlaps two bodies:
        // all three stay distinct Parts.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::New, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(3, BoolOp::Add, [15.0, 0.0, 0.0], [12.0, 2.0, 2.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 3, "an untagged Add never fuses touched bodies");
    }

    #[test]
    fn tagged_add_docks_into_target_body() {
        // The legit merge path: an Add carrying an explicit `target_ids`
        // link (face-extrude owner, Boolean union seed, merge scope) docks
        // into the body holding that target — this is how face-extrudes
        // grow their parent body.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            DecodedFeature::from_parts(
                FeatureId::from_u32(2),
                "Add 2".into(),
                BoolOp::Add,
                Primitive::Box {
                    center: [7.0, 0.0, 0.0],
                    half_extents: [5.0, 5.0, 5.0],
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![FeatureId::from_u32(1)],
            ),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1, "a targeted Add docks into its target body");
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
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
    fn untargeted_difference_is_a_standalone_preview() {
        // Scope is mandatory: a Remove with no explicit target must NOT carve
        // by contact — it seeds its own (red warning-preview) body. The
        // feature is flagged with a merge-scope warning at the feature layer.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            sphere_feat(2, BoolOp::Remove, [0.0, 0.0, 0.0], 3.0),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "an unscoped Remove never carves by contact");
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn targeted_difference_attaches_to_its_target() {
        // With an explicit target the Remove docks into that body and carves it.
        let features = vec![
            box_feat(1, BoolOp::New, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            DecodedFeature::from_parts(
                FeatureId::from_u32(2),
                "Remove 2".into(),
                BoolOp::Remove,
                Primitive::Sphere {
                    center: [0.0, 0.0, 0.0],
                    radius: 3.0,
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![FeatureId::from_u32(1)],
            ),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
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

    /// Serde-Hashing (B8): ein strukturell identisch aufgebauter Baum —
    /// inklusive Sketch-Profil und Junction-Blend — ergibt denselben Hash,
    /// unabhängig davon, ob er dieselbe oder eine frisch konstruierte
    /// Instanz ist.
    #[test]
    fn serde_hash_identical_tree_gives_identical_hash() {
        use fielddraft_protocol::{Sketch2d, Sketch2dPrimitive};
        let build = || {
            let sketch = Sketch2d::Prim(Sketch2dPrimitive::Polygon {
                vertices: vec![[0.0, 0.0], [20.0, 0.0], [10.0, 10.0]],
            });
            let mut f = DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Extrude".into(),
                BoolOp::New,
                Primitive::Extrude { sketch, half_depth_z: 5.0, center: [1.0, 2.0, 3.0], wall_subs: None },
                Transform::IDENTITY,
                PrimitiveModifiers { blend_radius: 1.5, ..PrimitiveModifiers::NONE },
                vec![],
            );
            f.blend_radius = 1.5;
            f
        };
        let a = hash_body_payload(&[build()], &[0]);
        let b = hash_body_payload(&[build()], &[0]);
        assert_eq!(a, b, "identical trees must hash identically");
    }

    /// Serde-Hashing (B8): Feld-Änderungen in exemplarischen Varianten
    /// (Extrude-Sketch-Vertex, Helix-Taper, Shell-Open-Face-HalfSpace)
    /// müssen den Hash bewegen — genau die Fälle, in denen ein vergessenes
    /// Feld im alten Hand-Hashing einen stillen Stale-Cache erzeugt hätte.
    #[test]
    fn serde_hash_field_changes_change_hash() {
        use fielddraft_protocol::{HalfSpace, Sketch2d, Sketch2dPrimitive};

        // 1) Extrude: ein einzelner Sketch-Vertex bewegt sich.
        let extrude = |x: f32| {
            let sketch = Sketch2d::Prim(Sketch2dPrimitive::Polygon {
                vertices: vec![[0.0, 0.0], [x, 0.0], [10.0, 10.0]],
            });
            DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Extrude".into(),
                BoolOp::New,
                Primitive::Extrude { sketch, half_depth_z: 5.0, center: [0.0; 3], wall_subs: None },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            )
        };
        assert_ne!(
            hash_body_payload(&[extrude(20.0)], &[0]),
            hash_body_payload(&[extrude(20.5)], &[0]),
            "a moved sketch vertex must change the hash"
        );

        // 2) Helix: der Taper-Parameter ändert sich.
        let helix = |taper: f32| {
            let sketch = Sketch2d::Prim(Sketch2dPrimitive::Circle {
                center: [8.0, 0.0],
                radius: 1.0,
            });
            DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Helix".into(),
                BoolOp::New,
                Primitive::Helix {
                    sketch,
                    center: [0.0; 3],
                    pitch: 2.0,
                    turns: 5.0,
                    left_handed: false,
                    taper,
                    start_angle: 0.0,
                    wall_subs: None,
                },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            )
        };
        assert_ne!(
            hash_body_payload(&[helix(0.0)], &[0]),
            hash_body_payload(&[helix(0.1)], &[0]),
            "a changed helix taper must change the hash"
        );

        // 3) Shell mit Open-Face: der analytische HalfSpace der Öffnung
        //    ändert sich (Plane-Offset). Baut den Knoten direkt, da
        //    Open-Faces nicht über `from_parts` laufen.
        let shell = |offset: f32| {
            let open = crate::csg::OpenSurface {
                branch: 42,
                surface: HalfSpace::Plane { normal: [0.0, 1.0, 0.0], offset },
                placement: Transform::IDENTITY,
            };
            DecodedFeature {
                id: FeatureId::from_u32(1),
                name: "Shell".into(),
                op: BoolOp::New,
                blend_radius: 0.0,
                blend_mode: crate::csg::BlendMode::Fillet,
                target_ids: vec![],
                transform: Transform::IDENTITY,
                node: CsgNode::Shell(
                    Box::new(CsgNode::Leaf {
                        id: FeatureId::from_u32(1),
                        primitive: Primitive::Box { center: [0.0; 3], half_extents: [5.0; 3] },
                    }),
                    2.0,
                    vec![open],
                ),
            }
        };
        assert_ne!(
            hash_body_payload(&[shell(5.0)], &[0]),
            hash_body_payload(&[shell(6.0)], &[0]),
            "a changed open-face half-space must change the hash"
        );
    }

    /// Serde-Hashing (B8): der Mesh-INHALT eines ImportedMesh geht über den
    /// Content-Fingerprint in den Hash ein. Zwei Meshes mit identischer AABB,
    /// aber verschiedenen Dreiecken, kollidieren nicht mehr (das alte
    /// Hand-Hashing hashte nur die AABB — stiller Stale-Cache).
    #[test]
    fn serde_hash_imported_mesh_content_enters_hash() {
        use fielddraft_protocol::MeshSdf;
        // Ein Dreieck spannt die AABB auf; ein zweites, inneres Dreieck
        // unterscheidet die beiden Meshes, ohne die AABB zu bewegen.
        let mesh = |inner_x: f32| {
            #[rustfmt::skip]
            let positions = vec![
                0.0, 0.0, 0.0,  10.0, 0.0, 0.0,  0.0, 10.0, 10.0, // AABB-Spanner
                1.0, 1.0, 1.0,  inner_x, 1.0, 1.0,  1.0, 2.0, 1.0, // inneres Dreieck
            ];
            let indices = vec![0u32, 1, 2, 3, 4, 5];
            let m = MeshSdf::from_buffers(&positions, &indices);
            DecodedFeature::from_parts(
                FeatureId::from_u32(1),
                "Import".into(),
                BoolOp::New,
                Primitive::ImportedMesh { mesh: Box::new(m) },
                Transform::IDENTITY,
                PrimitiveModifiers::NONE,
                vec![],
            )
        };
        let a = mesh(2.0);
        let b = mesh(3.0);
        // Gleiche AABB — der alte AABB-only-Hash hätte kollidiert.
        assert_eq!(feature_world_aabb(&a).min, feature_world_aabb(&b).min);
        assert_eq!(feature_world_aabb(&a).max, feature_world_aabb(&b).max);
        assert_ne!(
            hash_body_payload(&[a], &[0]),
            hash_body_payload(&[b], &[0]),
            "differing mesh content behind an identical AABB must change the hash"
        );
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
