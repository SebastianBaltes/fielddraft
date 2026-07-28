//! Persistent topological naming and reference resolution (step D.1).
//!
//! Parametric CAD needs **stable referenceability**: a feature placed on "this
//! edge" must still mean the same edge after the model's parameters change and
//! the B-Rep is rebuilt. Classic kernels hand out arbitrary IDs (`Edge_42`)
//! that are re-issued on every rebuild, which makes re-finding the right entity
//! — the *Topological Naming Problem* — notoriously hard.
//!
//! FieldDraft's half-space architecture has a structural advantage: an edge is
//! not an arbitrary entity but the deterministic collision of two half-spaces,
//! and a [`BranchId`] is derived purely from a feature's UUID plus a local
//! half-space role — *not* from geometry. So for a pure parameter change
//! (resize a box, nudge a radius) the branch signatures are byte-for-byte
//! identical and an entity's semantic ID is automatically stable, with no
//! heuristic. The geometric fingerprint (Stufe 2) only earns its keep for the
//! harder cases (topology split/merge, spatial reordering).
//!
//! This module implements both stages described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Topologische Stabilität (Persistent
//! Naming)"):
//!
//! - **Stufe 1 — semantic IDs:** [`face_id`] / [`edge_id`] / [`vertex_id`]
//!   hash the *sorted* parent branch signature (plus a spatial index for
//!   multiplicity) with a fixed-construction [`fnv1a_64`] hash, so an ID minted
//!   today reproduces on another machine / Rust version.
//! - **Stufe 2 — geometric fingerprint + state machine:** [`Fingerprint`],
//!   [`TopoRef`], [`ResolutionState`] and [`resolve`] re-find an entity in a new
//!   B-Rep, blending a hash match with geometric similarity into a confidence
//!   score.
//!
//! **Scope (D.1).** This is the *naming + reference-resolution infrastructure*.
//! Embedding a [`TopoRef`] into an actual `Feature` (e.g. a fillet) is deferred
//! until a consuming feature exists — see DL-35. The concept's acceptance ("a
//! fillet on a cube edge survives a parameter change") is realised here at the
//! kernel level: a [`TopoRef`] built against one cube still [`resolve`]s to the
//! geometrically-corresponding edge/face of a resized cube.

use serde::{Deserialize, Serialize};

use fielddraft_protocol::Transform;

use crate::brep::{BRep, EdgeGeometry, EdgeId, FaceId, SurfaceGeometry, VertexId};

/// Version marker of the branch-sub-id layout the semantic IDs hash over.
/// Bump on any change that re-keys existing entities — persisted element
/// references (`qFace(hash @ fingerprint)`) then re-bind through the
/// fingerprint-healing resolver instead of the exact hash.
///
/// - **1** — positional wall subs: a sketch-sweep wall's sub-id was the
///   G1 boundary-group INDEX of the profile tree (shifted when earlier
///   sub-calls were added/removed).
/// - **2** — stable wall subs (Onshape-Import Schritt 2): a wall's sub-id
///   is the sketch-entity-derived ordinal from the sweep primitive's
///   `wall_subs` table (min solver curve-entity id of the G1 group);
///   positional numbering remains the fallback for unattributable
///   profiles (SVG `sketch_path`, mirror copies, replay failure).
/// - **3** — pattern instance ordinal (#429/E1): the branch word grew from
///   32 to 64 bits so a pattern copy carries its ordinal *in the name*
///   instead of being told apart by a positional `spatial_index`. Every
///   `id_hash` in every document changed with this bump — that is the point
///   of the marker.
pub const NAMING_SCHEMA_VERSION: u32 = 3;
use crate::csg::BranchId;

// ─── deterministic hash ───────────────────────────────────────────────────

/// 64-bit FNV-1a, fixed-construction (offset basis & prime per the canonical
/// spec). We deliberately do **not** use [`std::collections::hash_map::DefaultHasher`]:
/// its algorithm (SipHash) is randomly keyed / version-unstable and would mint
/// a *different* ID for the same input across runs, machines and Rust releases,
/// which is fatal for IDs that a feature persists to disk. FNV-1a is small,
/// allocation-free and reproduces byte-for-byte forever (see DL-31).
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Fold a list of branch words plus a trailing `spatial_index` into a stable
/// hash. The words must already be in canonical (sorted) order. Each word is
/// hashed in fixed little-endian byte order so the result is platform stable.
///
/// Schema v3 (#429/E1) widened the branch word from 4 to 8 bytes to make room
/// for the pattern instance ordinal. Every `id_hash` in the document therefore
/// changed — see [`NAMING_SCHEMA_VERSION`].
fn hash_signature(words: &[u64], spatial_index: u32) -> u64 {
    let mut buf = Vec::with_capacity(words.len() * 8 + 4);
    for &w in words {
        buf.extend_from_slice(&w.to_le_bytes());
    }
    buf.extend_from_slice(&spatial_index.to_le_bytes());
    fnv1a_64(&buf)
}

// ─── Stufe 1: semantic IDs ────────────────────────────────────────────────

/// `Face_ID = Branch_ID`, optionally disambiguated by `spatial_index` when one
/// branch surface splits into several disjoint faces. A face that owns its
/// branch uniquely uses `spatial_index = 0`, which by construction hashes the
/// branch word alone with a 0 tail.
pub fn face_id(branch: BranchId, spatial_index: u32) -> u64 {
    hash_signature(&[branch.as_u64()], spatial_index)
}

/// `Edge_ID = hash(sort(branch_a, branch_b) [, spatial_index])`. The signature
/// pair is canonicalised (smaller first) so the ID is independent of which
/// half-space the edge walk met first.
pub fn edge_id(a: BranchId, b: BranchId, spatial_index: u32) -> u64 {
    let (lo, hi) = if a.as_u64() <= b.as_u64() {
        (a.as_u64(), b.as_u64())
    } else {
        (b.as_u64(), a.as_u64())
    };
    hash_signature(&[lo, hi], spatial_index)
}

/// `Vertex_ID = hash(sort(branch u32s) [, spatial_index])`. `branches` is
/// sorted here defensively; the B-Rep already stores them canonically.
pub fn vertex_id(branches: &[BranchId], spatial_index: u32) -> u64 {
    let mut words: Vec<u64> = branches.iter().map(|b| b.as_u64()).collect();
    words.sort_unstable();
    hash_signature(&words, spatial_index)
}

/// What a [`TopoRef`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Vertex,
    Edge,
    Face,
}

/// A resolved handle into a [`BRep`] arena, tagged with its kind so the index
/// is interpreted against the right arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityHandle {
    Vertex(VertexId),
    Edge(EdgeId),
    Face(FaceId),
}

// ─── spatial disambiguation ───────────────────────────────────────────────

/// Assign a `spatial_index` to every entity of `kind` sharing a branch
/// signature — **across the whole body set**, not per B-Rep (#431).
///
/// Entities with a unique signature get index 0. A colliding group is numbered
/// in two levels, so that going multi-body cannot disturb a name that was
/// already unambiguous:
///
/// 1. **Between bodies.** The bodies that contribute to the group are ranked
///    by the mean **world** centroid of their own members (body-local centroid
///    folded through the body's `placement`), lexicographically X → Y → Z, with
///    the body index as the final tie-break. Each body's block of indices
///    starts after all the members of lower-ranked bodies.
/// 2. **Inside one body.** Members are ordered by their **body-local**
///    centroid on the same X → Y → Z sweep, arena index as tie-break — exactly
///    as before #431. Keeping this level local is what makes the stamp
///    invariant under a body move *or rotation*, and what leaves every
///    document whose bodies share no branch signature byte-identically named.
///
/// Why the split is needed at all: a pattern copy is its own body, and a body's
/// B-Rep is woven body-LOCAL (placement divided out — see `Body::ensure_brep`),
/// so all copies are byte-identical. Only the placement tells them apart.
///
/// `out[b]` is body `b`'s index vector, parallel to that body's `kind` arena.
/// See DL-32 for the axis order / tie-break rationale.
fn scope_spatial_indices(bodies: &[(&BRep, Transform)], kind: EntityKind) -> Vec<Vec<u32>> {
    use std::collections::HashMap;
    let count_of = |brep: &BRep| match kind {
        EntityKind::Vertex => brep.vertices.len(),
        EntityKind::Edge => brep.edges.len(),
        EntityKind::Face => brep.faces.len(),
    };
    let mut out: Vec<Vec<u32>> = bodies.iter().map(|(b, _)| vec![0u32; count_of(b)]).collect();

    // Group (body, arena slot) pairs by their semantic signature — the
    // *un-indexed* hash — over every body at once.
    let mut groups: HashMap<u64, Vec<(usize, usize)>> = HashMap::new();
    for (bi, (brep, _)) in bodies.iter().enumerate() {
        for i in 0..count_of(brep) {
            groups.entry(base_signature_hash(brep, kind, i)).or_default().push((bi, i));
        }
    }

    for members in groups.values() {
        if members.len() <= 1 {
            continue; // unique signature → spatial_index 0
        }
        // Level 2 first: per body, the local ordering of its own members.
        let mut per_body: HashMap<usize, Vec<usize>> = HashMap::new();
        for &(bi, i) in members {
            per_body.entry(bi).or_default().push(i);
        }
        let mut blocks: Vec<(usize, Vec<usize>, [f64; 3])> = Vec::with_capacity(per_body.len());
        for (bi, mut slots) in per_body {
            let (brep, placement) = bodies[bi];
            let local: Vec<[f64; 3]> =
                slots.iter().map(|&i| entity_centroid(brep, kind, i)).collect();
            let eps = sweep_eps(&local);
            let mut ordered: Vec<usize> = (0..slots.len()).collect();
            ordered.sort_by(|&a, &b| {
                cmp_axis_order(local[a], local[b], eps).then(slots[a].cmp(&slots[b]))
            });
            // Mean WORLD centroid of this body's members — the body's rank key.
            let mut mean = [0.0f64; 3];
            for c in &local {
                let w = to_world(*c, &placement);
                for k in 0..3 {
                    mean[k] += w[k];
                }
            }
            for m in mean.iter_mut() {
                *m /= local.len() as f64;
            }
            slots = ordered.iter().map(|&o| slots[o]).collect();
            blocks.push((bi, slots, mean));
        }
        // Level 1: rank the bodies by their world key, then number straight
        // through the blocks.
        let keys: Vec<[f64; 3]> = blocks.iter().map(|(_, _, k)| *k).collect();
        let eps = sweep_eps(&keys);
        blocks.sort_by(|a, b| cmp_axis_order(a.2, b.2, eps).then(a.0.cmp(&b.0)));
        let mut next = 0u32;
        for (bi, slots, _) in &blocks {
            for &i in slots {
                out[*bi][i] = next;
                next += 1;
            }
        }
    }
    out
}

/// Group-relative sweep epsilon: an axis difference below one millionth of the
/// group's own spread is float jitter, not geometry — without this, a nanometre
/// wobble on the leading axis flips the whole order (plan finding B1). Floored
/// at an absolute 1e-9 for degenerate (near-coincident) groups.
fn sweep_eps(centroids: &[[f64; 3]]) -> f64 {
    let spread = (0..3)
        .map(|k| {
            let (lo, hi) = centroids
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), c| (lo.min(c[k]), hi.max(c[k])));
            hi - lo
        })
        .fold(0.0_f64, f64::max);
    (spread * 1e-6).max(1e-9)
}

/// Fold a body-local point through a body `placement` (rigid + uniform scale,
/// exactly the form `compute_body_placement` produces) into world space.
fn to_world(p: [f64; 3], t: &Transform) -> [f64; 3] {
    let s = t.scale[0] as f64;
    let scaled = [p[0] * s, p[1] * s, p[2] * s];
    let (qx, qy, qz, qw) = (
        t.rotation[0] as f64,
        t.rotation[1] as f64,
        t.rotation[2] as f64,
        t.rotation[3] as f64,
    );
    let u = [
        2.0 * (qy * scaled[2] - qz * scaled[1]),
        2.0 * (qz * scaled[0] - qx * scaled[2]),
        2.0 * (qx * scaled[1] - qy * scaled[0]),
    ];
    [
        scaled[0] + qw * u[0] + qy * u[2] - qz * u[1] + t.translate[0] as f64,
        scaled[1] + qw * u[1] + qz * u[0] - qx * u[2] + t.translate[1] as f64,
        scaled[2] + qw * u[2] + qx * u[1] - qy * u[0] + t.translate[2] as f64,
    ]
}

// ─── the one writer of the naming stamp ───────────────────────────────────

/// Stamp every body's [`SpatialIndices`] from the whole set's point of view
/// (#431). The **only** writer of the stamp; [`BRep::spatial_index`] is the
/// only reader.
///
/// Call this wherever a set of bodies is assembled — `BrepCache::ensure`,
/// `Kernel::ensure_named_breps` — and after any single weave (`finish_weave` stamps
/// its result as a one-body scope, so a standalone `weave()` is never
/// unstamped).
pub fn stamp_scope(bodies: &mut [(&mut BRep, Transform)]) {
    let plan = {
        let view: Vec<(&BRep, Transform)> = bodies.iter().map(|(b, t)| (&**b, *t)).collect();
        [
            scope_spatial_indices(&view, EntityKind::Vertex),
            scope_spatial_indices(&view, EntityKind::Edge),
            scope_spatial_indices(&view, EntityKind::Face),
        ]
    };
    for (bi, (brep, _)) in bodies.iter_mut().enumerate() {
        brep.spatial.vertices = plan[0][bi].clone();
        brep.spatial.edges = plan[1][bi].clone();
        brep.spatial.faces = plan[2][bi].clone();
    }
}

/// Stamp a standalone B-Rep as its own one-body scope.
pub fn stamp_single(brep: &mut BRep) {
    stamp_scope(&mut [(brep, Transform::IDENTITY)]);
}

/// [`stamp_scope`] for an owned body set — the shape `BrepCache::ensure`
/// assembles and `Kernel::bodies` exposes.
pub fn stamp_owned_scope(bodies: &mut [(BRep, Transform)]) {
    let mut view: Vec<(&mut BRep, Transform)> =
        bodies.iter_mut().map(|(b, t)| (&mut *b, *t)).collect();
    stamp_scope(&mut view);
}

/// The signature hash of arena slot `i` *without* a spatial index — used purely
/// to group colliding entities before assigning indices.
fn base_signature_hash(brep: &BRep, kind: EntityKind, i: usize) -> u64 {
    match kind {
        EntityKind::Vertex => vertex_id(&brep.vertices[i].branches, 0),
        EntityKind::Edge => {
            let e = &brep.edges[i];
            edge_id(e.signature.0, e.signature.1, 0)
        }
        EntityKind::Face => face_id(brep.faces[i].branch_id, 0),
    }
}

/// Lexicographic comparison of two centroids in X → Y → Z order. `eps` is the
/// caller's group-relative jitter floor (see [`sweep_eps`]): differences
/// at or below it don't decide the order, the next axis does.
fn cmp_axis_order(a: [f64; 3], b: [f64; 3], eps: f64) -> std::cmp::Ordering {
    for k in 0..3 {
        if (a[k] - b[k]).abs() > eps {
            return a[k].partial_cmp(&b[k]).unwrap_or(std::cmp::Ordering::Equal);
        }
    }
    std::cmp::Ordering::Equal
}

// ─── geometry helpers ─────────────────────────────────────────────────────
// Shared algebra lives in brep::vec3; the naming pass wants a zero axis
// (not the input) for a degenerate vector, hence `normalize_or_zero`.
use super::vec3::{add, dot, norm, normalize_or_zero as normalize, scale, sub};

/// The ordered point chain of an edge, whatever its geometry representation.
fn edge_points(geom: &EdgeGeometry) -> Vec<[f64; 3]> {
    match geom {
        EdgeGeometry::Polyline(pts) => pts.clone(),
        EdgeGeometry::Hermite(samples) => samples.iter().map(|s| s.point).collect(),
        // For a NURBS edge we use the control polygon, which for the cubic
        // Hermite fits FieldDraft produces interpolates the endpoints and
        // tracks the curve closely enough for fingerprinting (see DL-33).
        EdgeGeometry::Nurbs(c) => c.control_points.clone(),
    }
}

/// Centroid + chord length of an edge's point chain.
fn edge_centroid_length(pts: &[[f64; 3]]) -> ([f64; 3], f64) {
    if pts.is_empty() {
        return ([0.0; 3], 0.0);
    }
    if pts.len() == 1 {
        return (pts[0], 0.0);
    }
    // Length-weighted centroid ∫ p ds / ∫ ds over the polyline — invariant to how
    // densely or evenly the curve is sampled. The plain point mean drifts toward
    // the denser end once edges are curvature-adaptively traced (DL-81), where a
    // straight run carries a few unevenly spaced points instead of an even step;
    // that drift then biased the face centroid off true (mate-connector origin).
    let mut c = [0.0; 3];
    let mut len = 0.0;
    for w in pts.windows(2) {
        let seg = norm(sub(w[1], w[0]));
        let mid = scale(add(w[0], w[1]), 0.5);
        c = add(c, scale(mid, seg));
        len += seg;
    }
    if len > 0.0 {
        (scale(c, 1.0 / len), len)
    } else {
        (pts[0], 0.0)
    }
}

/// Unit tangent at the midpoint of an edge's point chain (direction of the
/// central chord; falls back to the overall end-to-end direction).
fn edge_mid_tangent(pts: &[[f64; 3]]) -> [f64; 3] {
    match pts.len() {
        0 | 1 => [0.0, 0.0, 0.0],
        2 => normalize(sub(pts[1], pts[0])),
        n => {
            let mid = n / 2;
            let lo = mid.saturating_sub(1);
            let hi = (mid + 1).min(n - 1);
            normalize(sub(pts[hi], pts[lo]))
        }
    }
}

/// Centroid of an entity's arena slot, by kind.
fn entity_centroid(brep: &BRep, kind: EntityKind, i: usize) -> [f64; 3] {
    match kind {
        EntityKind::Vertex => brep.vertices[i].position,
        EntityKind::Edge => edge_centroid_length(&edge_points(&brep.edges[i].geometry)).0,
        EntityKind::Face => face_centroid_size(brep, i).0,
    }
}

/// Centroid + a size scalar for a face. The centroid is the mean of the outer
/// loop's edge centroids; the size is the mean radius of those centroids about
/// the face centroid (a cheap, rotation-invariant extent proxy).
fn face_centroid_size(brep: &BRep, i: usize) -> ([f64; 3], f64) {
    let face = &brep.faces[i];
    let mut pts: Vec<[f64; 3]> = Vec::new();
    for &(eid, _) in &face.outer.edges {
        let (c, _) = edge_centroid_length(&edge_points(&brep.edge(eid).geometry));
        pts.push(c);
    }
    // Tessellated faces may have no analytic loop centroids to fall back on;
    // use the mesh vertices in that case.
    if pts.is_empty() {
        if let SurfaceGeometry::Tessellated { positions, .. } = &face.surface {
            pts = positions.clone();
        }
    }
    if pts.is_empty() {
        return ([0.0; 3], 0.0);
    }
    let mut c = [0.0; 3];
    for p in &pts {
        c = add(c, *p);
    }
    c = scale(c, 1.0 / pts.len() as f64);
    let size = pts.iter().map(|p| norm(sub(*p, c))).sum::<f64>() / pts.len() as f64;
    (c, size)
}

// ─── Stufe 2: fingerprint ─────────────────────────────────────────────────

/// A geometric fingerprint of an entity, captured at reference time and
/// compared against rebuilt entities to drive the resolution state machine.
///
/// All three kinds share the same fields with kind-specific meaning:
/// - **Edge:** `centroid` = point-chain centroid, `size` = chord length,
///   `tangent` = unit tangent at the midpoint.
/// - **Face:** `centroid` = mean of outer-loop edge centroids, `size` = mean
///   radius (extent proxy), `tangent` = `[0;3]` (unused).
/// - **Vertex:** `centroid` = position, `size` = 0, `tangent` = `[0;3]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub centroid: [f64; 3],
    /// Edge length / face extent / 0 for a vertex.
    pub size: f64,
    /// Edge midpoint tangent (unit); `[0;3]` for face/vertex.
    pub tangent: [f64; 3],
}

impl Fingerprint {
    pub fn of_edge(brep: &BRep, id: EdgeId) -> Self {
        let pts = edge_points(&brep.edge(id).geometry);
        let (centroid, size) = edge_centroid_length(&pts);
        Self {
            centroid,
            size,
            tangent: edge_mid_tangent(&pts),
        }
    }

    pub fn of_face(brep: &BRep, id: FaceId) -> Self {
        let (centroid, size) = face_centroid_size(brep, id.0 as usize);
        Self {
            centroid,
            size,
            tangent: [0.0; 3],
        }
    }

    pub fn of_vertex(brep: &BRep, id: VertexId) -> Self {
        Self {
            centroid: brep.vertex(id).position,
            size: 0.0,
            tangent: [0.0; 3],
        }
    }
}

// ─── Stufe 2: TopoRef + state machine ─────────────────────────────────────

/// A persistent reference to a topological entity: its kind, its deterministic
/// semantic [`fnv1a_64`] id, and a geometric [`Fingerprint`] for healing. Fully
/// serde-serialisable so a feature *could* persist it (embedding into a real
/// feature is deferred — DL-35).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TopoRef {
    pub kind: EntityKind,
    pub id_hash: u64,
    pub fingerprint: Fingerprint,
}

impl TopoRef {
    /// Build a reference to a specific edge of `brep`, with the correct spatial
    /// index for that slot.
    pub fn from_edge(brep: &BRep, id: EdgeId) -> Self {
        let si = brep.spatial_index(EntityKind::Edge, id.0 as usize);
        let e = brep.edge(id);
        Self {
            kind: EntityKind::Edge,
            id_hash: edge_id(e.signature.0, e.signature.1, si),
            fingerprint: Fingerprint::of_edge(brep, id),
        }
    }

    /// Build a reference to a specific face of `brep`.
    pub fn from_face(brep: &BRep, id: FaceId) -> Self {
        let si = brep.spatial_index(EntityKind::Face, id.0 as usize);
        Self {
            kind: EntityKind::Face,
            id_hash: face_id(brep.face(id).branch_id, si),
            fingerprint: Fingerprint::of_face(brep, id),
        }
    }

    /// Build a reference to a specific vertex of `brep`.
    pub fn from_vertex(brep: &BRep, id: VertexId) -> Self {
        let si = brep.spatial_index(EntityKind::Vertex, id.0 as usize);
        Self {
            kind: EntityKind::Vertex,
            id_hash: vertex_id(&brep.vertex(id).branches, si),
            fingerprint: Fingerprint::of_vertex(brep, id),
        }
    }
}

/// The state of a reference after [`resolve`], driven by the confidence score
/// and the concept's thresholds (DL-34):
/// - `> 0.95` → [`ResolutionState::Resolved`]
/// - `0.7..=0.95` → [`ResolutionState::Healed`]
/// - `0.4..0.7` → [`ResolutionState::Ambiguous`]
/// - `< 0.4` → [`ResolutionState::Missing`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionState {
    Resolved,
    Healed,
    Ambiguous,
    Missing,
}

impl ResolutionState {
    /// Map a confidence score to a state using the concept's thresholds.
    pub fn from_score(score: f64) -> Self {
        if score > 0.95 {
            ResolutionState::Resolved
        } else if score >= 0.7 {
            ResolutionState::Healed
        } else if score >= 0.4 {
            ResolutionState::Ambiguous
        } else {
            ResolutionState::Missing
        }
    }
}

/// The outcome of resolving one [`TopoRef`] against a new B-Rep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolution {
    pub state: ResolutionState,
    pub score: f64,
    /// The best-matching entity, or `None` when nothing scored above the
    /// `Missing` floor (e.g. no entity of the right kind exists at all).
    pub handle: Option<EntityHandle>,
}

impl Resolution {
    fn missing() -> Self {
        Self {
            state: ResolutionState::Missing,
            score: 0.0,
            handle: None,
        }
    }
}

/// Normalised geometric similarity in `[0, 1]` between a stored fingerprint and
/// a candidate, blending three terms (DL-34):
///
/// - **centroid distance**, exponentially decayed by a length scale derived
///   from the larger of the two sizes (so a uniform resize keeps the term high
///   — the centroid moves *proportionally* to the entity it belongs to);
/// - **size ratio** (`min/max`), capturing length / extent agreement;
/// - **tangent alignment** (`|cos θ|`, orientation-insensitive), only for
///   edges that actually carry a tangent.
///
/// Weights: centroid 0.4, size 0.4, tangent 0.2 for edges; for faces/vertices
/// (no tangent) the tangent weight is folded into centroid+size (0.5/0.5).
fn fingerprint_similarity(kind: EntityKind, a: &Fingerprint, b: &Fingerprint) -> f64 {
    // Length scale: the bigger entity's size, floored so a zero-size entity
    // (vertex) still has a sane decay distance.
    let scale_len = a.size.abs().max(b.size.abs()).max(1.0);
    let cdist = norm(sub(a.centroid, b.centroid));
    let centroid_term = (-cdist / scale_len).exp();

    let size_term = if a.size.abs() < 1e-12 && b.size.abs() < 1e-12 {
        1.0 // both sizeless (vertices) → perfect agreement on this axis
    } else {
        let lo = a.size.abs().min(b.size.abs());
        let hi = a.size.abs().max(b.size.abs()).max(1e-12);
        lo / hi
    };

    let has_tangent = norm(a.tangent) > 1e-9 && norm(b.tangent) > 1e-9;
    if kind == EntityKind::Edge && has_tangent {
        let align = dot(a.tangent, b.tangent).abs().clamp(0.0, 1.0);
        0.4 * centroid_term + 0.4 * size_term + 0.2 * align
    } else {
        0.5 * centroid_term + 0.5 * size_term
    }
}

/// Base confidence granted to a semantic-id (hash) match, refined upward by
/// fingerprint similarity. A hash match is *strong* evidence in FieldDraft's
/// architecture: branch IDs are derived from feature UUIDs, not geometry, so a
/// pure parameter change (resize a box) keeps the hash byte-for-byte identical
/// even though the geometry — and thus the fingerprint — moved. We therefore
/// floor a hash hit at 0.9 and let `0.1·similarity` push it into the `Resolved`
/// band. The blended score is clamped to `[0, 1]`. See DL-34.
const HASH_MATCH_FLOOR: f64 = 0.9;
const HASH_MATCH_SIM_WEIGHT: f64 = 0.1;

/// Two candidates whose fingerprint similarities lie within this margin are
/// geometrically indistinguishable *to the fingerprint* — any pick between
/// them rests on other evidence (the hash) or is arbitrary.
const CLOSE_MARGIN: f64 = 0.05;

/// Above this similarity the fingerprint alone is considered decisive
/// evidence of "same element". Below it, a candidate merely *resembles* the
/// stored reference — good enough to heal towards when unrivalled, not good
/// enough to override a tie (see the split-wall case in `naming_gate`).
const STRONG_SIM: f64 = 0.8;

/// Resolve a stored [`TopoRef`] against a freshly woven B-Rep.
///
/// Every same-kind entity is scored on **two axes**: geometric fingerprint
/// similarity, and whether its freshly computed semantic id equals the
/// stored `id_hash`. The two axes are then reconciled (plan
/// `PERSISTENT-NAMING-PLAN.md`, Leitentscheidung 3 — no silent misbinding):
///
/// 1. **Hash and fingerprint agree** (the hash carrier is also the most
///    fingerprint-similar candidate, or ties with it within
///    [`CLOSE_MARGIN`]): bind to it with the boosted score
///    `HASH_MATCH_FLOOR + HASH_MATCH_SIM_WEIGHT·sim`. If the runner-up ties
///    within [`CLOSE_MARGIN`] *and* the winner's similarity is below
///    [`STRONG_SIM`], the geometric evidence is a coin toss between weak
///    candidates (a split region inheriting index 0 by accident) — the state
///    demotes to `Ambiguous`, keeping the hash carrier as the reported
///    handle.
/// 2. **They conflict** (some other entity is clearly more similar than the
///    hash carrier): the spatial index has migrated onto a different element
///    — bind to the fingerprint's choice with its raw similarity as score
///    (no hash bonus). This is the anti-misbinding override.
/// 3. **No hash hit**: bind to the most similar candidate; a runner-up
///    within [`CLOSE_MARGIN`] demotes to `Ambiguous` (with no hash there is
///    nothing to break a tie, however similar the twins are).
///
/// Never panics.
pub fn resolve(old: &TopoRef, new_brep: &BRep) -> Resolution {
    let kind = old.kind;
    let si = |i: usize| new_brep.spatial_index(kind, i);
    let count = match kind {
        EntityKind::Vertex => new_brep.vertices.len(),
        EntityKind::Edge => new_brep.edges.len(),
        EntityKind::Face => new_brep.faces.len(),
    };
    if count == 0 {
        return Resolution::missing();
    }

    let handle_of = |i: usize| -> EntityHandle {
        match kind {
            EntityKind::Vertex => EntityHandle::Vertex(VertexId(i as u32)),
            EntityKind::Edge => EntityHandle::Edge(EdgeId(i as u32)),
            EntityKind::Face => EntityHandle::Face(FaceId(i as u32)),
        }
    };
    let fingerprint_of = |i: usize| -> Fingerprint {
        match kind {
            EntityKind::Vertex => Fingerprint::of_vertex(new_brep, VertexId(i as u32)),
            EntityKind::Edge => Fingerprint::of_edge(new_brep, EdgeId(i as u32)),
            EntityKind::Face => Fingerprint::of_face(new_brep, FaceId(i as u32)),
        }
    };
    let id_of = |i: usize| -> u64 {
        match kind {
            EntityKind::Vertex => vertex_id(&new_brep.vertices[i].branches, si(i)),
            EntityKind::Edge => {
                let e = &new_brep.edges[i];
                edge_id(e.signature.0, e.signature.1, si(i))
            }
            EntityKind::Face => face_id(new_brep.faces[i].branch_id, si(i)),
        }
    };

    // Score both axes for every candidate in one pass.
    let sims: Vec<f64> = (0..count)
        .map(|i| fingerprint_similarity(kind, &old.fingerprint, &fingerprint_of(i)))
        .collect();
    let max_sim_of = |pool: &mut dyn Iterator<Item = usize>| -> Option<usize> {
        pool.max_by(|&a, &b| sims[a].partial_cmp(&sims[b]).unwrap_or(std::cmp::Ordering::Equal))
    };
    // The best hash carrier (most similar among possibly several hits).
    let hash_best = max_sim_of(&mut (0..count).filter(|&i| id_of(i) == old.id_hash));

    let (best, score, demote) = match hash_best {
        Some(h) => {
            // The hash carrier reveals the stored reference's branch
            // signature — identity is provenance-first, so the override and
            // ambiguity comparisons run over the SAME-SIGNATURE group only.
            // A merely similar face of a *different* feature is not a
            // candidate (it would be cross-branch misbinding), and it must
            // not manufacture false ambiguity either.
            let sig = base_signature_hash(new_brep, kind, h);
            let group: Vec<usize> = (0..count)
                .filter(|&i| base_signature_hash(new_brep, kind, i) == sig)
                .collect();
            let g_best = max_sim_of(&mut group.iter().copied()).unwrap();
            let s1 = sims[g_best];
            let s2 = group
                .iter()
                .filter(|&&i| i != g_best)
                .map(|&i| sims[i])
                .fold(0.0_f64, f64::max);
            if h == g_best || sims[h] >= s1 - CLOSE_MARGIN {
                // Hash and fingerprint agree (or tie within the margin).
                let score =
                    (HASH_MATCH_FLOOR + HASH_MATCH_SIM_WEIGHT * sims[h]).clamp(0.0, 1.0);
                // Weak-evidence tie: the sibling looks just as (dis)similar
                // and even the winner doesn't really resemble the stored
                // reference — the hash pick is arbitrary (split-region case).
                let demote = s1 - s2 < CLOSE_MARGIN && s1 < STRONG_SIM && group.len() > 1;
                (h, score, demote)
            } else {
                // Conflict: within the signature group the fingerprint
                // clearly prefers a sibling — the spatial index migrated;
                // binding to the hash carrier would be the silent misbinding
                // of plan finding B3. Geometric override, no hash bonus.
                (g_best, s1, false)
            }
        }
        // No hash hit — the branch may have been renamed entirely; geometry
        // alone decides over all candidates, ties are ambiguous.
        None => {
            let sim_best = max_sim_of(&mut (0..count)).unwrap();
            let s1 = sims[sim_best];
            let s2 = (0..count)
                .filter(|&i| i != sim_best)
                .map(|i| sims[i])
                .fold(0.0_f64, f64::max);
            (sim_best, s1, s1 - s2 < CLOSE_MARGIN && count > 1)
        }
    };

    // Demotion marks an *actionable but arbitrary* pick; a score already
    // below the Ambiguous floor stays what it is (Missing must stay Missing).
    let state = if demote && score >= 0.4 {
        ResolutionState::Ambiguous
    } else {
        ResolutionState::from_score(score)
    };
    Resolution {
        state,
        score,
        // Below the Missing floor we report no handle: the match is too weak to
        // act on, but the caller still learns the best candidate's score is low.
        handle: if score >= 0.4 {
            Some(handle_of(best))
        } else {
            None
        },
    }
}

/// Resolve a *set* of references in one call, returning a parallel vector of
/// [`Resolution`]s. Lets a multi-edge feature (e.g. a fillet on 4 edges) degrade
/// gracefully: each ref resolves independently, so partial success is
/// expressible (some `Resolved`, some `Missing`) instead of all-or-nothing.
pub fn resolve_set(refs: &[TopoRef], new_brep: &BRep) -> Vec<Resolution> {
    refs.iter().map(|r| resolve(r, new_brep)).collect()
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brep::octree::Octree;
    use crate::brep::weaving::weave;
    use crate::brep::{Edge, EdgeGeometry};
    use crate::bvh::BoundedCsg;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};

    fn box_body(half: [f32; 3]) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: half,
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    fn cube_brep(half: f32) -> BRep {
        let body = box_body([half, half, half]);
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    /// Weave a rectangle-sketch extrude body (the SA-1…SA-3 sketch-SDF
    /// pipeline), so naming can be validated against the *real* extrude path
    /// rather than a primitive cube.
    fn rect_extrude_brep(half_x: f32, half_y: f32, half_z: f32) -> BRep {
        use fielddraft_protocol::{Sketch2d as ProtoSketch2d, Sketch2dPrimitive as ProtoSketch2dPrim};
        let sketch = ProtoSketch2d::Prim(ProtoSketch2dPrim::Rectangle {
            center: [0.0, 0.0],
            half_extents: [half_x, half_y],
        });
        let csg = CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(1),
            sketch: Box::new(sketch),
            half_height: half_z,
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    // ── 1. Determinism ──────────────────────────────────────────────────

    #[test]
    fn fnv1a_known_constant() {
        // FNV-1a/64 of the ASCII string "fielddraft" — a fixed regression
        // constant. If this ever changes the hash algorithm has drifted and
        // every persisted ID would be invalidated.
        assert_eq!(fnv1a_64(b"fielddraft"), 0x3ee2_6228_6177_cce2);
        // Empty input → the bare offset basis.
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn same_cube_yields_identical_ids() {
        let a = cube_brep(1.0);
        let b = cube_brep(1.0);
        assert_eq!(a.edge_count(), 12);
        assert_eq!(b.edge_count(), 12);

        let ids = |brep: &BRep| -> (Vec<u64>, Vec<u64>, Vec<u64>) {
            let si_e = &brep.spatial.edges;
            let si_f = &brep.spatial.faces;
            let si_v = &brep.spatial.vertices;
            let mut e: Vec<u64> = brep
                .edges
                .iter()
                .enumerate()
                .map(|(i, ed)| edge_id(ed.signature.0, ed.signature.1, si_e[i]))
                .collect();
            let mut f: Vec<u64> = brep
                .faces
                .iter()
                .enumerate()
                .map(|(i, fc)| face_id(fc.branch_id, si_f[i]))
                .collect();
            let mut v: Vec<u64> = brep
                .vertices
                .iter()
                .enumerate()
                .map(|(i, vt)| vertex_id(&vt.branches, si_v[i]))
                .collect();
            e.sort_unstable();
            f.sort_unstable();
            v.sort_unstable();
            (e, f, v)
        };
        assert_eq!(
            ids(&a),
            ids(&b),
            "two identical cubes must hash identically"
        );

        // No accidental ID collisions within a plain cube (12/6/8 unique).
        let (e, f, v) = ids(&a);
        let uniq = |mut x: Vec<u64>| {
            x.dedup();
            x.len()
        };
        assert_eq!(uniq(e), 12, "12 distinct edge ids");
        assert_eq!(uniq(f), 6, "6 distinct face ids");
        assert_eq!(uniq(v), 8, "8 distinct vertex ids");
    }

    // ── 2. Reference survives a parameter change (D.1 acceptance) ────────

    #[test]
    fn edge_reference_survives_resize() {
        let old = cube_brep(1.0); // half-extents 1 → corners at ±1
        let new = cube_brep(1.5); // resized → corners at ±1.5

        // Pick a specific edge and capture which branch signature / axis it is.
        let pick = EdgeId(0);
        let sig = old.edge(pick).signature;
        let topo = TopoRef::from_edge(&old, pick);

        let res = resolve(&topo, &new);
        assert_eq!(
            res.state,
            ResolutionState::Resolved,
            "edge must resolve after resize, score {}",
            res.score
        );
        assert!(res.score > 0.95, "score {} not > 0.95", res.score);

        // The matched edge has the same branch signature (the geometrically
        // corresponding edge).
        match res.handle.unwrap() {
            EntityHandle::Edge(eid) => {
                assert_eq!(
                    new.edge(eid).signature,
                    sig,
                    "resolved to an edge of a different branch pair"
                );
            }
            other => panic!("expected an edge handle, got {other:?}"),
        }

        // Sanity: the new edge's centroid moved outward (the cube grew). The
        // old centroid magnitude must be no larger than the new one.
        let old_c = Fingerprint::of_edge(&old, pick).centroid;
        let new_c = match res.handle.unwrap() {
            EntityHandle::Edge(eid) => Fingerprint::of_edge(&new, eid).centroid,
            _ => unreachable!(),
        };
        assert!(
            norm(new_c) >= norm(old_c),
            "resized edge centroid should be at least as far from origin"
        );
    }

    #[test]
    fn face_reference_survives_resize() {
        let old = cube_brep(1.0);
        let new = cube_brep(1.5);

        let pick = FaceId(0);
        let branch = old.face(pick).branch_id;
        let topo = TopoRef::from_face(&old, pick);

        let res = resolve(&topo, &new);
        assert_eq!(
            res.state,
            ResolutionState::Resolved,
            "face must resolve after resize, score {}",
            res.score
        );
        assert!(res.score > 0.95, "score {} not > 0.95", res.score);
        match res.handle.unwrap() {
            EntityHandle::Face(fid) => {
                assert_eq!(new.face(fid).branch_id, branch, "resolved to wrong branch");
            }
            other => panic!("expected a face handle, got {other:?}"),
        }
    }

    #[test]
    fn vertex_reference_survives_resize() {
        let old = cube_brep(1.0);
        let new = cube_brep(1.5);
        let topo = TopoRef::from_vertex(&old, VertexId(0));
        let res = resolve(&topo, &new);
        assert_eq!(res.state, ResolutionState::Resolved);
        assert!(res.handle.is_some());
    }

    // ── 2b. Reference survives a SKETCH param change (SA-5 Task C) ────────

    /// A face TopoRef on an analytic side wall of a rectangle-extrude body must
    /// survive a change to the sketch width: the branch signature of each wall
    /// is derived from the feature UUID + the wall's sub-id, not its geometry,
    /// so a pure parameter change keeps the semantic id byte-for-byte identical
    /// and `resolve` lands in `Resolved` (>0.95).
    #[test]
    fn extrude_face_reference_survives_sketch_width_change() {
        let old = rect_extrude_brep(1.0, 2.0, 1.0);
        let new = rect_extrude_brep(1.7, 2.0, 1.0); // wider rectangle

        assert_eq!(old.face_count(), 6, "4 walls + 2 caps");
        assert_eq!(new.face_count(), 6);

        // Pick a +x side wall (an analytic plane, not a cap).
        let pick = (0..old.face_count() as u32)
            .map(FaceId)
            .find(|&fid| {
                matches!(
                    &old.face(fid).surface,
                    SurfaceGeometry::Analytic {
                        surface: fielddraft_protocol::HalfSpace::Plane { normal, .. },
                        ..
                    } if normal[0].abs() > 0.5
                )
            })
            .expect("a ±x side wall exists");
        let branch = old.face(pick).branch_id;
        let topo = TopoRef::from_face(&old, pick);

        let res = resolve(&topo, &new);
        assert_eq!(
            res.state,
            ResolutionState::Resolved,
            "extrude side face must resolve after a sketch-width change, score {}",
            res.score
        );
        assert!(res.score > 0.95, "score {} not > 0.95", res.score);
        match res.handle.unwrap() {
            EntityHandle::Face(fid) => assert_eq!(
                new.face(fid).branch_id,
                branch,
                "resolved to a different wall branch"
            ),
            other => panic!("expected a face handle, got {other:?}"),
        }
    }

    /// An edge TopoRef on a rectangle-extrude body survives a change to the
    /// extrude **depth** *for the edges whose topology is stable*. This test
    /// also pins an honest, observed limitation of the current sketch-extrude
    /// weaver (recorded rather than papered over):
    ///
    /// Re-weaving the same rectangle at a different depth is NOT perfectly
    /// topology-stable: at half-depth 1.0 the body has 12 straight edges (the
    /// ideal box), but at half-depth 2.5 the weaver consolidates two of the
    /// wall–cap rims into *closed-loop* edges and drops two wall–wall verticals
    /// (12 → 10 edges). For the edges that DO persist with their branch-pair
    /// signature (e.g. a vertical wall–wall edge), the semantic id is
    /// byte-for-byte identical, so `resolve` lands in `Resolved` (>0.95). The
    /// branch-pairs that get restructured (involving the consolidated wall)
    /// genuinely cannot resolve via the hash and only reach the `Ambiguous`
    /// fingerprint band — that is a weaver topology-stability limitation, not a
    /// naming defect, and is asserted here as the real behaviour.
    #[test]
    fn extrude_edge_reference_survives_depth_change() {
        let old = rect_extrude_brep(1.0, 1.0, 1.0);
        let new = rect_extrude_brep(1.0, 1.0, 2.5); // taller extrude

        assert!(old.edge_count() > 0, "extrude has edges");
        assert!(new.edge_count() > 0);

        // Pick a vertical wall–wall edge: its branch pair are two adjacent side
        // walls, which both survive a pure depth change with identical
        // signatures. (A wall–cap rim, by contrast, may get consolidated.)
        // Side walls are the sketch-derived sub-ids; caps are the reserved
        // EXTRUDE_CAP_TOP/BOTTOM_SUB. A wall–wall edge touches neither cap.
        let cap_top = crate::csg::BranchId::from_feature_sub(
            &FeatureId::from_u32(1),
            crate::csg::BranchId::EXTRUDE_CAP_TOP_SUB,
        )
        .as_u64();
        let cap_bottom = crate::csg::BranchId::from_feature_sub(
            &FeatureId::from_u32(1),
            crate::csg::BranchId::EXTRUDE_CAP_BOTTOM_SUB,
        )
        .as_u64();
        let is_wall_wall = |e: &crate::brep::Edge| {
            let (a, b) = (e.signature.0.as_u64(), e.signature.1.as_u64());
            a != cap_top && a != cap_bottom && b != cap_top && b != cap_bottom
        };
        // Require the picked wall–wall edge to also persist (same branch pair)
        // in the rebuilt body — only the wall–cap rims get restructured, and at
        // least one vertical wall–wall edge always survives a depth change.
        let pick = (0..old.edge_count() as u32)
            .map(EdgeId)
            .find(|&eid| {
                let e = old.edge(eid);
                is_wall_wall(e) && new.edges.iter().any(|n| n.signature == e.signature)
            })
            .expect("a topology-stable vertical wall–wall edge exists");
        let sig = old.edge(pick).signature;
        let topo = TopoRef::from_edge(&old, pick);

        let res = resolve(&topo, &new);
        assert_eq!(
            res.state,
            ResolutionState::Resolved,
            "a topology-stable extrude edge must resolve after a depth change, score {}",
            res.score
        );
        assert!(res.score > 0.95, "score {} not > 0.95", res.score);
        match res.handle.unwrap() {
            EntityHandle::Edge(eid) => assert_eq!(
                new.edge(eid).signature,
                sig,
                "resolved to an edge of a different branch pair"
            ),
            other => panic!("expected an edge handle, got {other:?}"),
        }
    }

    // ── 3. Spatial disambiguation ────────────────────────────────────────

    /// Hand-build a tiny B-Rep with **two edges of the identical branch
    /// signature** but different centroids (the "plane meets both legs of a
    /// U-profile" multiplicity case). They must receive distinct, centroid-
    /// ordered spatial indices and therefore distinct IDs.
    #[test]
    fn spatial_disambiguation_distinguishes_same_signature_edges() {
        let mut brep = BRep::new();
        let a = BranchId(1 << 4);
        let b = BranchId(2 << 4);

        // Edge far from the origin (added first, deliberately the "higher" one
        // so we also prove ordering is by centroid, not insertion order).
        let e_far = brep.add_edge(Edge::new(
            EdgeGeometry::Polyline(vec![[5.0, 0.0, 0.0], [5.0, 1.0, 0.0]]),
            a,
            b,
            None,
            None,
        ));
        let e_near = brep.add_edge(Edge::new(
            EdgeGeometry::Polyline(vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            a,
            b,
            None,
            None,
        ));

        stamp_single(&mut brep);
        let si = brep.spatial.edges.clone();
        // Ordered by centroid X (0.0 before 5.0), independent of insertion.
        assert_eq!(si[e_near.0 as usize], 0, "near edge → spatial index 0");
        assert_eq!(si[e_far.0 as usize], 1, "far edge → spatial index 1");

        let id_near = edge_id(a, b, si[e_near.0 as usize]);
        let id_far = edge_id(a, b, si[e_far.0 as usize]);
        assert_ne!(
            id_near, id_far,
            "same-signature edges must get distinct IDs"
        );

        // A TopoRef to the near edge resolves back to the near edge, not the far
        // one — the spatial index plus fingerprint pin it down.
        let topo = TopoRef::from_edge(&brep, e_near);
        let res = resolve(&topo, &brep);
        assert_eq!(res.state, ResolutionState::Resolved);
        assert_eq!(res.handle, Some(EntityHandle::Edge(e_near)));
    }

    #[test]
    fn unique_signatures_get_spatial_index_zero() {
        // A plain cube has 12 unique edge signatures and 8 unique vertex sets.
        let brep = cube_brep(1.0);
        assert!(brep.spatial.edges.iter().all(|&i| i == 0));
        assert!(brep.spatial.vertices.iter().all(|&i| i == 0));
        assert!(brep.spatial.faces.iter().all(|&i| i == 0));
    }

    // ── 4. Missing ───────────────────────────────────────────────────────

    #[test]
    fn missing_signature_resolves_to_missing_not_panic() {
        let new = cube_brep(1.0);
        // A TopoRef whose branch signature does not exist in the cube and whose
        // fingerprint is geometrically far away from any cube edge.
        let bogus = TopoRef {
            kind: EntityKind::Edge,
            id_hash: edge_id(BranchId(0xDEAD << 4), BranchId(0xBEEF << 4), 0),
            fingerprint: Fingerprint {
                centroid: [1000.0, 1000.0, 1000.0],
                size: 0.001,
                tangent: [1.0, 0.0, 0.0],
            },
        };
        let res = resolve(&bogus, &new);
        assert_eq!(res.state, ResolutionState::Missing);
        assert!(
            res.score < 0.4,
            "score {} should be below the floor",
            res.score
        );
        assert!(res.handle.is_none());
    }

    #[test]
    fn resolve_against_empty_brep_is_missing() {
        let empty = BRep::new();
        let topo = TopoRef {
            kind: EntityKind::Face,
            id_hash: 123,
            fingerprint: Fingerprint {
                centroid: [0.0; 3],
                size: 1.0,
                tangent: [0.0; 3],
            },
        };
        let res = resolve(&topo, &empty);
        assert_eq!(res.state, ResolutionState::Missing);
        assert!(res.handle.is_none());
    }

    // ── graceful degradation ──────────────────────────────────────────────

    #[test]
    fn resolve_set_expresses_partial_success() {
        let old = cube_brep(1.0);
        let new = cube_brep(1.5);
        // A real edge ref (will resolve) plus a bogus one (will be Missing).
        let good = TopoRef::from_edge(&old, EdgeId(0));
        let bad = TopoRef {
            kind: EntityKind::Edge,
            id_hash: edge_id(BranchId(0xAAAA << 4), BranchId(0xBBBB << 4), 0),
            fingerprint: Fingerprint {
                centroid: [9999.0, 9999.0, 9999.0],
                size: 0.0001,
                tangent: [0.0, 0.0, 1.0],
            },
        };
        let results = resolve_set(&[good, bad], &new);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].state, ResolutionState::Resolved);
        assert_eq!(results[1].state, ResolutionState::Missing);
    }

    #[test]
    fn resolution_state_thresholds() {
        assert_eq!(ResolutionState::from_score(0.99), ResolutionState::Resolved);
        assert_eq!(ResolutionState::from_score(0.95), ResolutionState::Healed);
        assert_eq!(ResolutionState::from_score(0.8), ResolutionState::Healed);
        assert_eq!(ResolutionState::from_score(0.7), ResolutionState::Healed);
        assert_eq!(ResolutionState::from_score(0.5), ResolutionState::Ambiguous);
        assert_eq!(ResolutionState::from_score(0.4), ResolutionState::Ambiguous);
        assert_eq!(ResolutionState::from_score(0.3), ResolutionState::Missing);
    }

    #[test]
    fn topo_ref_serde_round_trips() {
        let brep = cube_brep(1.0);
        let topo = TopoRef::from_edge(&brep, EdgeId(0));
        let json = serde_json::to_string(&topo).expect("serialise");
        let back: TopoRef = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(topo, back);
    }
}
