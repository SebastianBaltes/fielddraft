//! Multi-body partitioning of the linear feature spine.
//!
//! Each disjoint solid in the document owns its own CSG graph, BVH
//! and mesh cache. Mutations that only touch body `A` leave body
//! `B`'s mesh unchanged.
//!
//! Partitioning rules:
//!
//! * **Union** — always creates a new body. No implicit AABB-based
//!   merging; bodies stay separate unless an explicit Boolean
//!   operation connects them.
//! * **Difference / Intersection** — attached to existing bodies via
//!   `target_ids` (preferred) or AABB overlap (fallback for legacy
//!   features without explicit targets).
//! * Feature order inside a body is always the original document
//!   order.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

use crate::brep::octree::Octree;
use crate::brep::weaving::weave;
use crate::brep::BRep;
use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, CsgNode};
use crate::dc::Mesh;
use crate::sdf_compiler::CompiledSdf;
use crate::topology::BodyTopology;

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
    pub fn seed_id(&self, features: &[Feature]) -> FeatureId {
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
    /// Lazily computed B-Rep topology (faces + edges) from mesh
    /// classifications. Invalidated on mesh rebuild.
    pub topology: Option<BodyTopology>,
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
    /// not the mesh). Distinct from [`Body::topology`], which is a
    /// mesh-derived topology used by the viewport overlays.
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
    pub fn ensure_topology(&mut self) -> &BodyTopology {
        if self.topology.is_none() {
            self.topology = Some(crate::topology::extract_topology(&self.mesh));
        }
        self.topology.as_ref().unwrap()
    }

    /// Return the body's feature-based B-Rep, weaving it on first
    /// access and caching the result. Subsequent calls reuse the
    /// cache until the next remesh clears it (see [`Body::brep`]).
    ///
    /// The octree resolution is **body-relative**: `min_cell_size`
    /// scales with the body's largest AABB extent (DL-28), so a tiny
    /// or a huge body both get proportional sampling instead of an
    /// absolute default. The weave path is panic-safe — degenerate
    /// geometry yields a possibly-empty B-Rep rather than a crash.
    pub fn ensure_brep(&mut self, features: &[Feature]) -> &BRep {
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
            let min_cell = topology_min_cell_size(&self.aabb);
            let octree = Octree::build(&bounded, min_cell);
            self.brep = Some((key, weave(&bounded, &octree)));
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

/// Walk the feature spine once and return one [`BodyPartition`] per
/// resulting body.
///
/// Rules:
/// * **Union** — always starts a new body. No AABB-based merging;
///   bodies stay separate unless an explicit Boolean connects them.
/// * **Difference / Intersection** — attached to bodies via
///   `target_ids` (preferred) or AABB overlap (fallback for
///   features without explicit targets).
pub fn partition(features: &[Feature]) -> Vec<BodyPartition> {
    let mut groups: Vec<BodyPartition> = Vec::new();
    for (i, feature) in features.iter().enumerate() {
        let feat_aabb = feature_world_aabb(feature);

        match feature.op {
            BoolOp::Union => {
                groups.push(BodyPartition {
                    feature_indices: vec![i],
                    aabb: feat_aabb,
                });
            }
            BoolOp::Difference | BoolOp::Intersect => {
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

/// World-space AABB of a single feature's primitive, transformed by
/// its own `Transform`.
fn feature_world_aabb(feature: &Feature) -> Aabb {
    Aabb::of_primitive(&feature.primitive).transformed(&feature.transform)
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
    features: &[Feature],
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
        hash_primitive(&f.primitive, &mut geo);
        f.blend_radius.to_bits().hash(&mut geo);
        f.chamfer_size.to_bits().hash(&mut geo);
        f.twist_rate.to_bits().hash(&mut geo);
        f.bend_rate.to_bits().hash(&mut geo);
        f.displace_depth.to_bits().hash(&mut geo);
        f.displace_freq.to_bits().hash(&mut geo);
        f.shell_thickness.to_bits().hash(&mut geo);

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

pub fn hash_body_payload(features: &[Feature], indices: &[usize]) -> u64 {
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
pub fn compute_body_placement(features: &[Feature], indices: &[usize]) -> Transform {
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
    features: &[Feature],
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
pub fn build_bounded_for_body(features: &[Feature], indices: &[usize]) -> BoundedCsg {
    let sub: Vec<Feature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_features(&sub);
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
    features: &[Feature],
    indices: &[usize],
    placement: &Transform,
) -> BoundedCsg {
    let sub: Vec<Feature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_features(&sub);
    BoundedCsg::from_csg_with_carry(&csg, &crate::inverse(placement))
}

pub fn build_bounded_with_primitives(
    features: &[Feature],
    indices: &[usize],
) -> (BoundedCsg, std::collections::HashMap<u32, fielddraft_protocol::Primitive>) {
    let sub: Vec<Feature> = indices.iter().map(|&i| features[i].clone()).collect();
    let csg = CsgNode::from_features(&sub);
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
    use fielddraft_protocol::{Feature, Transform};

    fn box_feat(id: u32, op: BoolOp, center: [f32; 3], half: [f32; 3]) -> Feature {
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
            target_ids: vec![],
        }
    }

    fn sphere_feat(id: u32, op: BoolOp, center: [f32; 3], radius: f32) -> Feature {
        Feature {
            id: FeatureId::from_u32(id),
            name: format!("Sphere {id}"),
            op,
            primitive: Primitive::Sphere { center, radius },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
            target_ids: vec![],
        }
    }

    #[test]
    fn single_union_feature_produces_one_body() {
        let features = vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[0].seed_id(&features), FeatureId::from_u32(1));
    }

    #[test]
    fn two_disjoint_unions_produce_two_bodies() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [100.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn overlapping_unions_stay_separate() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [7.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 2, "overlapping unions must stay separate");
        assert_eq!(bodies[0].feature_indices, vec![0]);
        assert_eq!(bodies[1].feature_indices, vec![1]);
    }

    #[test]
    fn difference_attaches_to_overlapping_body() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            sphere_feat(2, BoolOp::Difference, [0.0, 0.0, 0.0], 3.0),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].feature_indices, vec![0, 1]);
    }

    #[test]
    fn orphan_difference_is_dropped() {
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere_feat(2, BoolOp::Difference, [100.0, 0.0, 0.0], 3.0),
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
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [30.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(3, BoolOp::Union, [15.0, 0.0, 0.0], [20.0, 5.0, 5.0]),
        ];
        let bodies = partition(&features);
        assert_eq!(bodies.len(), 3, "each union must be its own body");
    }

    #[test]
    fn targeted_intersect_attaches_only_to_target_body() {
        let mut gyroid = sphere_feat(3, BoolOp::Intersect, [0.0; 3], 1.0);
        gyroid.target_ids = vec![FeatureId::from_u32(1)];
        let features = vec![
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            box_feat(2, BoolOp::Union, [20.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
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
            box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]),
            sphere_feat(2, BoolOp::Difference, [0.0, 0.0, 0.0], 2.0),
        ];
        let a = hash_body_payload(&features, &[0, 1]);
        let b = hash_body_payload(&features, &[0, 1]);
        assert_eq!(a, b);
    }

    #[test]
    fn hash_changes_when_any_field_changes() {
        let baseline =
            vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        let h0 = hash_body_payload(&baseline, &[0]);

        let translated = vec![box_feat(1, BoolOp::Union, [0.5, 0.0, 0.0], [5.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&translated, &[0]));

        let bigger = vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [6.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&bigger, &[0]));

        let diff = vec![box_feat(1, BoolOp::Difference, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
        assert_ne!(h0, hash_body_payload(&diff, &[0]));
    }

    fn with_modifier(mut f: Feature, field: &str, val: f32) -> Feature {
        match field {
            "blend_radius" => f.blend_radius = val,
            "chamfer_size" => f.chamfer_size = val,
            "twist_rate" => f.twist_rate = val,
            "bend_rate" => f.bend_rate = val,
            "displace_depth" => f.displace_depth = val,
            "displace_freq" => f.displace_freq = val,
            "shell_thickness" => f.shell_thickness = val,
            _ => panic!("unknown field {field}"),
        }
        f
    }

    #[test]
    fn hash_split_geometry_vs_transform() {
        let base = vec![box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0])];
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
        let base = box_feat(1, BoolOp::Union, [0.0, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let h0 = hash_body_payload(std::slice::from_ref(&base), &[0]);

        for field in [
            "blend_radius",
            "chamfer_size",
            "twist_rate",
            "bend_rate",
            "displace_depth",
            "displace_freq",
            "shell_thickness",
        ] {
            let modified = with_modifier(base.clone(), field, 1.5);
            let h1 = hash_body_payload(&[modified], &[0]);
            assert_ne!(h0, h1, "hash must change when {field} changes");
        }
    }
}
