//! Boundary-Representation (B-Rep) data structures.
//!
//! This module owns the *output* shape of the B-Rep extraction pipeline
//! described in `docs/GEOMETRY-KERNEL-CONCEPT.md` ("B-Rep-Extraktion: Der
//! Algorithmus", Phases 1–4). Step C.1 defines only the data structures
//! plus their construction/accessors — no extraction algorithms. The later
//! phases produce, and store into, exactly these arenas:
//!
//! - **Phase 1 (octree / collision detection, C.2):** finds where ≥3 branch
//!   surfaces collide → seeds [`Vertex`]es; where exactly 2 collide → seeds
//!   [`Edge`]s with a `(Branch_A, Branch_B)` [`Edge::signature`].
//! - **Phase 2 (edge walking, C.3/C.4):** marches each 2-branch collision
//!   into an ordered chain of `(point, tangent)` samples → [`EdgeGeometry::Hermite`],
//!   bounded by the vertices found in Phase 1 (`start` / `end`).
//! - **Phase 3 (spline fitting, C.5):** fits the Hermite samples to cubic
//!   Hermite splines convertible to NURBS → [`EdgeGeometry::Nurbs`].
//! - **Phase 4 (face/shell/solid weaving, C.6):** groups edges that bound a
//!   common branch surface into trimmed [`Face`]s ([`SurfaceGeometry::Analytic`]
//!   for the NURBS-able half-spaces, [`SurfaceGeometry::Tessellated`] for
//!   Gyroid/TPMS), assembles faces into [`Shell`]s and shells into [`Solid`]s.
//!
//! The container [`BRep`] is an **index-arena** B-Rep: entities live in flat
//! `Vec`s and reference each other through newtype index handles
//! ([`VertexId`], [`EdgeId`], …). This is the borrow-checker-friendly idiom —
//! no `Rc`/`RefCell`, cheap to clone, and trivially serialisable.
//!
//! ## Glossary
//!
//! Terms used throughout the pipeline (introduced by the coincidence work,
//! `docs/BREP-CLASSIFICATION-PLAN.md`):
//!
//! - **Carrier**: the infinite analytic surface a branch's face lives on
//!   (plane / cylinder / sphere …), independent of trimming. Coincidence is
//!   decided between carriers, symbolically ([`coincidence`]).
//! - **Coincidence class**: an oriented equivalence class of branches whose
//!   carriers are the same surface — ON-same (stacked walls, seam) or
//!   ON-opposite (flush contact, membrane).
//! - **Membrane**: a measure-zero `f = 0` sheet with material on BOTH sides
//!   (flush-add contact plane, flush-remove zero-thickness wall). Not part
//!   of the body boundary — culled before face materialisation.
//! - **Seam**: the edge where two ON-same coincident faces meet at a 180°
//!   dihedral. Valid manifold topology (faces are deliberately NOT merged).
//! - **Fin**: an edge whose adjoining face regions were culled (it lay on a
//!   membrane, not on `∂body`) — dropped with an interior proof, never to
//!   fake watertightness.
//! - **Rim**: a closed-circle boundary edge without bounding vertices (a
//!   cylinder cap circle, a full-revolution trim).
//! - **Canonicalisation**: the post-trace pass
//!   (`weaving::canonicalize`) that rewrites per-branch trace output into
//!   manifold topology on coincident geometry: weld seam vertices, merge
//!   duplicate seam edges per class, cull membranes, drop fins.

pub mod branch_pairs;
pub mod branch_seed;
#[cfg(test)]
mod classification_gate;
#[cfg(test)]
mod naming_gate;
pub mod coincidence;
pub mod diff;
pub mod edge_sampling;
pub mod edge_trace;
pub mod edge_walker;
pub mod extraction;
pub mod face_export;
pub mod face_offset;
pub mod face_lateral_trim;
pub mod face_profile;
pub mod face_thicken;
pub mod naming;
pub mod octree;
pub mod projection;
pub mod seed_weaver;
pub mod ssi;
pub mod tangential_seams;
pub mod step_export;
pub mod tessellate;
pub mod tol;
mod vec3;
pub mod weaving;

use serde::{Deserialize, Serialize};

use crate::csg::BranchId;
use fielddraft_protocol::{HalfSpace, Transform};

/// The body's symbolic surface table: branch id → analytic carrier
/// half-space in its world transform, walked off the CSG tree by
/// [`weaving::branch_halfspace_map`]. The shared input of SSI seeding, the
/// coincidence registry, face materialisation and vertex refinement.
pub type BranchSurfaceMap = std::collections::HashMap<u64, (HalfSpace, Transform)>;

/// serde adapter for [`BranchId`], which is a protocol-adjacent newtype that
/// deliberately does not derive `Serialize`/`Deserialize` itself. We round-trip
/// it through its public `u64` payload so B-Rep entities stay serialisable
/// without reaching outside this module.
mod branch_id_serde {
    use super::BranchId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &BranchId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(id.as_u64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BranchId, D::Error> {
        Ok(BranchId(u64::deserialize(d)?))
    }
}

/// serde adapter for a canonical `(BranchId, BranchId)` edge signature.
mod branch_pair_serde {
    use super::BranchId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        pair: &(BranchId, BranchId),
        s: S,
    ) -> Result<S::Ok, S::Error> {
        [pair.0.as_u64(), pair.1.as_u64()].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<(BranchId, BranchId), D::Error> {
        let [a, b] = <[u64; 2]>::deserialize(d)?;
        Ok((BranchId(a), BranchId(b)))
    }
}

/// serde adapter for `Vec<BranchId>` (a vertex's colliding branches).
mod branch_vec_serde {
    use super::BranchId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(ids: &[BranchId], s: S) -> Result<S::Ok, S::Error> {
        let raw: Vec<u64> = ids.iter().map(|b| b.as_u64()).collect();
        raw.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<BranchId>, D::Error> {
        let raw = Vec::<u64>::deserialize(d)?;
        Ok(raw.into_iter().map(BranchId).collect())
    }
}

// ─── index handles ────────────────────────────────────────────────────────

/// Index handle into [`BRep::vertices`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VertexId(pub u32);

/// Index handle into [`BRep::edges`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EdgeId(pub u32);

/// Index handle into [`BRep::faces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaceId(pub u32);

/// Index handle into [`BRep::shells`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShellId(pub u32);

/// Index handle into [`BRep::solids`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SolidId(pub u32);

// ─── vertices ───────────────────────────────────────────────────────────────

/// A topological vertex: a 0-cell where **≥3 branch surfaces collide**
/// (Phase 1). The colliding branches are kept in canonical sorted order so
/// that two vertices built from the same set of branches compare equal and
/// hash identically regardless of discovery order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vertex {
    pub position: [f64; 3],
    /// The ≥3 colliding [`BranchId`]s, canonically sorted & deduplicated.
    #[serde(with = "branch_vec_serde")]
    pub branches: Vec<BranchId>,
}

impl Vertex {
    /// Build a vertex, canonicalising `branches` (sort + dedup) so equality
    /// is independent of insertion order.
    pub fn new(position: [f64; 3], mut branches: Vec<BranchId>) -> Self {
        branches.sort_unstable_by_key(|b| b.as_u64());
        branches.dedup();
        Self { position, branches }
    }
}

// ─── curve geometry (edges) ──────────────────────────────────────────────────

/// A rational B-spline curve. The eventual NURBS form an [`Edge`] carries
/// after Phase 3 spline fitting (C.5), directly exportable to STEP/IGES.
///
/// Invariants the fitter must uphold (not enforced here): `weights.len() ==
/// control_points.len()` and `knots.len() == control_points.len() + degree + 1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NurbsCurve {
    pub degree: usize,
    pub control_points: Vec<[f64; 3]>,
    pub weights: Vec<f64>,
    pub knots: Vec<f64>,
}

/// One `(point, tangent)` sample along an edge, the raw product of the
/// Phase 2 edge-walk (C.4) before spline fitting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HermiteSample {
    pub point: [f64; 3],
    pub tangent: [f64; 3],
}

/// The geometric realisation of an [`Edge`].
///
/// - [`EdgeGeometry::Hermite`] — ordered `(point, tangent)` chain straight
///   from the edge-walk (C.4).
/// - [`EdgeGeometry::Nurbs`] — the fitted analytic curve (C.5).
/// - [`EdgeGeometry::Polyline`] — a dumb point chain; the simple fallback used
///   for hand-built test geometry and degenerate edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeGeometry {
    Hermite(Vec<HermiteSample>),
    Nurbs(NurbsCurve),
    Polyline(Vec<[f64; 3]>),
}

/// A topological edge: a 1-cell formed where **exactly two branch surfaces
/// collide**. Its `signature` is the canonical (smaller-first) pair of those
/// two branches — the stable identity that lets Phase 4 weave faces sharing
/// an edge. `start`/`end` reference the bounding vertices; both `None` marks a
/// closed-loop edge (e.g. a circular cut with no corner vertex).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub geometry: EdgeGeometry,
    /// The two colliding branch surfaces, canonically sorted (`.0 <= .1`).
    #[serde(with = "branch_pair_serde")]
    pub signature: (BranchId, BranchId),
    pub start: Option<VertexId>,
    pub end: Option<VertexId>,
}

impl Edge {
    /// Build an edge, canonicalising the `(a, b)` signature so that the
    /// smaller [`BranchId`] always comes first. Two edges discovered from the
    /// same surface pair therefore share an identical signature regardless of
    /// which surface the walk encountered first.
    pub fn new(
        geometry: EdgeGeometry,
        a: BranchId,
        b: BranchId,
        start: Option<VertexId>,
        end: Option<VertexId>,
    ) -> Self {
        let signature = if a.as_u64() <= b.as_u64() {
            (a, b)
        } else {
            (b, a)
        };
        Self { geometry, signature, start, end }
    }
}

// ─── surface geometry (faces) ────────────────────────────────────────────────

/// The geometric realisation of a [`Face`].
///
/// - [`SurfaceGeometry::Analytic`] — a [`HalfSpace`] surface placed by a
///   [`Transform`]. The common case (Plane/Cylinder/Sphere/Cone/Torus), all of
///   which convert cleanly to NURBS surfaces for export.
/// - [`SurfaceGeometry::Tessellated`] — a triangle mesh fallback for surfaces
///   with no closed-form NURBS, i.e. Gyroid / other TPMS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceGeometry {
    Analytic {
        surface: HalfSpace,
        transform: Transform,
    },
    Tessellated {
        positions: Vec<[f64; 3]>,
        indices: Vec<[u32; 3]>,
    },
}

/// An ordered, oriented boundary loop of edges. Each entry pairs an [`EdgeId`]
/// with a `reversed` flag: `false` traverses the edge from `start` to `end`,
/// `true` from `end` to `start`. A face's outer loop runs counter-clockwise
/// seen from outside the solid; inner loops (holes) run the opposite way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Loop {
    /// `(edge, reversed)` in boundary order.
    pub edges: Vec<(EdgeId, bool)>,
}

impl Loop {
    pub fn new(edges: Vec<(EdgeId, bool)>) -> Self {
        Self { edges }
    }

    /// The oriented `(start, end)` vertices of one loop entry: with `reversed`
    /// the edge is walked `end → start`, so the head-to-tail endpoints swap.
    /// Returns `None` for an edge missing a bounding vertex (a closed-loop edge,
    /// e.g. a full circle), since such an edge cannot be chained by vertex.
    fn oriented_endpoints(
        edges: &[Edge],
        (EdgeId(idx), reversed): (EdgeId, bool),
    ) -> Option<(VertexId, VertexId)> {
        let edge = &edges[idx as usize];
        let (s, e) = (edge.start?, edge.end?);
        Some(if reversed { (e, s) } else { (s, e) })
    }

    /// Check that this loop is a **connected ring**: every entry's oriented end
    /// vertex equals the next entry's oriented start vertex, and the last entry
    /// closes back onto the first. An empty loop is vacuously connected (a fully
    /// closed analytic surface, e.g. a lone sphere). A single-entry loop whose
    /// one edge is a closed-loop edge (no bounding vertices, e.g. a trim circle)
    /// is also considered connected — it is its own ring.
    ///
    /// Returns `false` as soon as a head-to-tail break is found, so a wrongly
    /// ordered or disconnected loop never silently passes as a valid ring.
    pub fn is_connected_ring(&self, edges: &[Edge]) -> bool {
        match self.edges.len() {
            0 => return true,
            1 => {
                let (EdgeId(idx), _) = self.edges[0];
                let edge = &edges[idx as usize];
                // A self-closing edge (no vertices) is a ring on its own;
                // a single vertex-bounded edge cannot close into a ring.
                return edge.start.is_none() && edge.end.is_none();
            }
            _ => {}
        }
        let mut prev_end = match Self::oriented_endpoints(edges, self.edges[0]) {
            Some((_, e)) => e,
            None => return false,
        };
        let first_start = match Self::oriented_endpoints(edges, self.edges[0]) {
            Some((s, _)) => s,
            None => return false,
        };
        for &entry in &self.edges[1..] {
            let Some((s, e)) = Self::oriented_endpoints(edges, entry) else {
                return false;
            };
            if s != prev_end {
                return false;
            }
            prev_end = e;
        }
        prev_end == first_start
    }
}

/// A trimmed analytic (or tessellated) surface — a 2-cell of the B-Rep.
/// It lies on a single branch surface (`branch_id`), carries that surface's
/// geometry, and is trimmed by an `outer` loop plus zero or more `inner` loops
/// (holes / internal trim boundaries). Produced by Phase 4 face weaving (C.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Face {
    /// The half-space branch this face lies on.
    #[serde(with = "branch_id_serde")]
    pub branch_id: BranchId,
    pub surface: SurfaceGeometry,
    pub outer: Loop,
    pub inner: Vec<Loop>,
}

// ─── shells & solids ─────────────────────────────────────────────────────────

/// A connected set of faces. `is_closed` records whether the shell forms a
/// watertight manifold (computed/set by Phase 4); it defaults to `false`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Shell {
    pub faces: Vec<FaceId>,
    pub is_closed: bool,
}

impl Shell {
    pub fn new(faces: Vec<FaceId>, is_closed: bool) -> Self {
        Self { faces, is_closed }
    }
}

/// A solid body: one `outer` shell bounding the material, plus zero or more
/// `inner` shells bounding internal voids (cavities).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solid {
    pub outer: ShellId,
    pub inner: Vec<ShellId>,
}

impl Solid {
    pub fn new(outer: ShellId, inner: Vec<ShellId>) -> Self {
        Self { outer, inner }
    }
}

// ─── container ───────────────────────────────────────────────────────────────

/// Per-entity spatial disambiguation indices, parallel to the three arenas
/// (#431). Written by exactly one authority —
/// [`naming::stamp_scope`](crate::brep::naming::stamp_scope) — and read back
/// through [`BRep::spatial_index`] when a semantic id is minted.
///
/// The index is what separates several entities that share one branch
/// signature (a wall split by a boolean, and — since #431 — the same wall in
/// several copies of a pattern). It is assigned across the whole **body set**,
/// not per B-Rep: pattern copies are separate bodies whose B-Reps are woven
/// body-LOCAL and are therefore byte-identical, so a per-B-Rep index left all
/// copies at 0 and made `qFace(hash)` ambiguous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SpatialIndices {
    pub vertices: Vec<u32>,
    pub edges: Vec<u32>,
    pub faces: Vec<u32>,
}

/// The top-level index-arena B-Rep. All entities live in flat `Vec`s and
/// reference each other through newtype handles. Later extraction phases push
/// entities through `add_*` and read them back through the accessors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BRep {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub faces: Vec<Face>,
    pub shells: Vec<Shell>,
    pub solids: Vec<Solid>,
    /// Naming stamp — see [`SpatialIndices`]. Derived state: every weave
    /// route ends in `finish_weave`, which stamps the B-Rep as its own
    /// one-body scope; assembling several bodies re-stamps them together.
    #[serde(default)]
    pub spatial: SpatialIndices,
}

impl BRep {
    /// An empty B-Rep.
    pub fn new() -> Self {
        Self::default()
    }

    // ── arena insertion ──────────────────────────────────────────────────

    pub fn add_vertex(&mut self, vertex: Vertex) -> VertexId {
        let id = VertexId(self.vertices.len() as u32);
        self.vertices.push(vertex);
        id
    }

    pub fn add_edge(&mut self, edge: Edge) -> EdgeId {
        let id = EdgeId(self.edges.len() as u32);
        self.edges.push(edge);
        id
    }

    pub fn add_face(&mut self, face: Face) -> FaceId {
        let id = FaceId(self.faces.len() as u32);
        self.faces.push(face);
        id
    }

    pub fn add_shell(&mut self, shell: Shell) -> ShellId {
        let id = ShellId(self.shells.len() as u32);
        self.shells.push(shell);
        id
    }

    pub fn add_solid(&mut self, solid: Solid) -> SolidId {
        let id = SolidId(self.solids.len() as u32);
        self.solids.push(solid);
        id
    }

    // ── accessors ─────────────────────────────────────────────────────────

    pub fn vertex(&self, id: VertexId) -> &Vertex {
        &self.vertices[id.0 as usize]
    }

    pub fn edge(&self, id: EdgeId) -> &Edge {
        &self.edges[id.0 as usize]
    }

    pub fn face(&self, id: FaceId) -> &Face {
        &self.faces[id.0 as usize]
    }

    pub fn shell(&self, id: ShellId) -> &Shell {
        &self.shells[id.0 as usize]
    }

    pub fn solid(&self, id: SolidId) -> &Solid {
        &self.solids[id.0 as usize]
    }

    /// Arena slot `i`'s naming stamp (see [`SpatialIndices`]).
    ///
    /// An unstamped B-Rep reports 0, which is correct for every entity with a
    /// unique branch signature and wrong only for a colliding group — so the
    /// debug build asserts the stamp is present rather than minting a
    /// silently-colliding name.
    pub fn spatial_index(&self, kind: naming::EntityKind, i: usize) -> u32 {
        let (stamp, arena) = match kind {
            naming::EntityKind::Vertex => (&self.spatial.vertices, self.vertices.len()),
            naming::EntityKind::Edge => (&self.spatial.edges, self.edges.len()),
            naming::EntityKind::Face => (&self.spatial.faces, self.faces.len()),
        };
        debug_assert_eq!(
            stamp.len(),
            arena,
            "B-Rep was never stamped by naming::stamp_scope — {kind:?} ids would collide"
        );
        stamp.get(i).copied().unwrap_or(0)
    }

    // ── counts ──────────────────────────────────────────────────────────

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// Faces whose boundary is NOT a set of connected rings — the faces the
    /// weaver could not order and emitted through the unordered last-resort
    /// fallback (`materialize_faces`, "could not order face loops"). Such a
    /// face has no traversable boundary: it cannot be exported as valid STEP,
    /// and its presence means the topology arriving at the weaver was
    /// incomplete (an edge the tracer never found) or over-complete (an
    /// interior seam that is not part of any ring).
    ///
    /// This is the DEFECT itself rather than the log line that announces it:
    /// the warning is printed once per `materialize_faces` pass and a body can
    /// be re-woven several times per document, so counting messages counts
    /// re-runs; counting faces counts geometry (#436).
    pub fn open_face_count(&self) -> usize {
        self.faces
            .iter()
            .filter(|f| {
                !f.outer.is_connected_ring(&self.edges)
                    || !f.inner.iter().all(|lp| lp.is_connected_ring(&self.edges))
            })
            .count()
    }

    pub fn shell_count(&self) -> usize {
        self.shells.len()
    }

    pub fn solid_count(&self) -> usize {
        self.solids.len()
    }

    // ── validation ──────────────────────────────────────────────────────

    /// Basic referential-integrity check. Returns the list of problems found
    /// (empty `Ok(())` when the B-Rep is well-formed). A stub that the later
    /// phases extend with manifoldness / loop-closure checks; for C.1 it
    /// verifies that:
    ///
    /// - every referenced handle is within its arena's bounds, and
    /// - every edge signature is canonically sorted (`.0 <= .1`).
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();

        let nv = self.vertices.len() as u32;
        let ne = self.edges.len() as u32;
        let nf = self.faces.len() as u32;
        let ns = self.shells.len() as u32;

        for (i, edge) in self.edges.iter().enumerate() {
            if edge.signature.0.as_u64() > edge.signature.1.as_u64() {
                problems.push(format!(
                    "edge {i}: signature not canonically sorted ({} > {})",
                    edge.signature.0.as_u64(),
                    edge.signature.1.as_u64()
                ));
            }
            for (label, v) in [("start", edge.start), ("end", edge.end)] {
                if let Some(VertexId(idx)) = v {
                    if idx >= nv {
                        problems.push(format!(
                            "edge {i}: {label} VertexId({idx}) out of range (have {nv})"
                        ));
                    }
                }
            }
        }

        let check_loop = |problems: &mut Vec<String>, ctx: &str, lp: &Loop| {
            for &(EdgeId(idx), _) in &lp.edges {
                if idx >= ne {
                    problems.push(format!("{ctx}: EdgeId({idx}) out of range (have {ne})"));
                }
            }
        };

        for (i, face) in self.faces.iter().enumerate() {
            check_loop(&mut problems, &format!("face {i} outer"), &face.outer);
            for (j, lp) in face.inner.iter().enumerate() {
                check_loop(&mut problems, &format!("face {i} inner[{j}]"), lp);
            }
        }

        for (i, shell) in self.shells.iter().enumerate() {
            for &FaceId(idx) in &shell.faces {
                if idx >= nf {
                    problems.push(format!(
                        "shell {i}: FaceId({idx}) out of range (have {nf})"
                    ));
                }
            }
        }

        for (i, solid) in self.solids.iter().enumerate() {
            if solid.outer.0 >= ns {
                problems.push(format!(
                    "solid {i}: outer ShellId({}) out of range (have {ns})",
                    solid.outer.0
                ));
            }
            for &ShellId(idx) in &solid.inner {
                if idx >= ns {
                    problems.push(format!(
                        "solid {i}: inner ShellId({idx}) out of range (have {ns})"
                    ));
                }
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// Check the **orientable-manifold / watertight invariant** (DL-24).
    ///
    /// Iterates over every loop-use of every face (each face's `outer` loop
    /// plus all its `inner` loops) and tallies, per [`EdgeId`], how often the
    /// edge is referenced and with which `reversed` flag. A closed orientable
    /// manifold requires that **every edge is referenced by exactly two
    /// loop-uses — once `reversed = false` and once `reversed = true`**.
    ///
    /// Returns the list of violations as human-readable strings (empty when
    /// the B-Rep is a watertight orientable manifold). Reported cases:
    ///
    /// - edges with a use-count `!= 2` (count 1 = open boundary / phantom,
    ///   count > 2 = non-manifold),
    /// - edges referenced exactly twice but with matching orientation (both
    ///   `reversed` equal) → orientation/consistency error,
    /// - edges present in the arena but used by no loop at all (count 0,
    ///   dangling edge).
    ///
    /// An empty B-Rep (no faces) is vacuously watertight and yields an empty
    /// `Vec`. Unlike [`BRep::validate`] this is a *geometric/topological*
    /// invariant, kept separate so the referential-integrity stub's semantics
    /// (and test output) stay stable.
    pub fn manifold_violations(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.faces.is_empty() {
            return problems;
        }

        // Per edge: (count of reversed=false uses, count of reversed=true uses).
        let mut uses = vec![(0u32, 0u32); self.edges.len()];

        let tally = |uses: &mut Vec<(u32, u32)>, lp: &Loop| {
            for &(EdgeId(idx), reversed) in &lp.edges {
                if let Some(slot) = uses.get_mut(idx as usize) {
                    if reversed {
                        slot.1 += 1;
                    } else {
                        slot.0 += 1;
                    }
                }
                // out-of-range EdgeIds are a referential-integrity problem and
                // are reported by `validate()`; we silently skip them here.
            }
        };

        for face in &self.faces {
            tally(&mut uses, &face.outer);
            for lp in &face.inner {
                tally(&mut uses, lp);
            }
        }

        for (i, &(fwd, rev)) in uses.iter().enumerate() {
            let total = fwd + rev;
            match total {
                2 if fwd == 1 && rev == 1 => { /* watertight & consistent */ }
                2 => problems.push(format!(
                    "edge {i}: used twice but with matching orientation \
                     ({fwd} forward, {rev} reversed) — orientation error"
                )),
                0 => problems.push(format!(
                    "edge {i}: used by no loop (count 0) — dangling edge"
                )),
                1 => problems.push(format!(
                    "edge {i}: used once (count 1) — open boundary / phantom edge"
                )),
                n => problems.push(format!(
                    "edge {i}: used {n} times (count > 2) — non-manifold edge"
                )),
            }
        }

        problems
    }

    /// The Euler characteristic `V − E + F` of this B-Rep.
    ///
    /// For a single closed genus-0 solid this equals `2`; for genus `g` it is
    /// `2 − 2g`. Because the value depends on topology (and on how a model is
    /// split across multiple shells), there is **no** fixed absolute assert in
    /// [`BRep::validate`] — this is exposed purely as a metric for curated
    /// tests and diagnostics.
    pub fn euler_characteristic(&self) -> i64 {
        self.vertices.len() as i64 - self.edges.len() as i64 + self.faces.len() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-build the canonical axis-aligned unit cube `[-1, 1]³` as a B-Rep:
    /// 8 vertices, 12 edges, 6 faces, 1 shell, 1 solid. Each of the 6 cube
    /// faces is one branch (`bx0, bx1, by0, by1, bz0, bz1`). Mirrors the
    /// eventual C.5 cube target; here it just proves the structures hold it.
    fn build_cube() -> BRep {
        let mut brep = BRep::new();

        // Six face branches, one per cube side.
        let bx0 = BranchId(1 << 4); // -X
        let bx1 = BranchId(2 << 4); // +X
        let by0 = BranchId(3 << 4); // -Y
        let by1 = BranchId(4 << 4); // +Y
        let bz0 = BranchId(5 << 4); // -Z
        let bz1 = BranchId(6 << 4); // +Z

        // 8 corner vertices. Index bits = (x, y, z) sign, 0 → -1, 1 → +1.
        // Each corner is where its 3 face branches collide.
        let corner = |x: f64, y: f64, z: f64, b: Vec<BranchId>| Vertex::new([x, y, z], b);
        let v000 = brep.add_vertex(corner(-1.0, -1.0, -1.0, vec![bx0, by0, bz0]));
        let v100 = brep.add_vertex(corner(1.0, -1.0, -1.0, vec![bx1, by0, bz0]));
        let v010 = brep.add_vertex(corner(-1.0, 1.0, -1.0, vec![bx0, by1, bz0]));
        let v110 = brep.add_vertex(corner(1.0, 1.0, -1.0, vec![bx1, by1, bz0]));
        let v001 = brep.add_vertex(corner(-1.0, -1.0, 1.0, vec![bx0, by0, bz1]));
        let v101 = brep.add_vertex(corner(1.0, -1.0, 1.0, vec![bx1, by0, bz1]));
        let v011 = brep.add_vertex(corner(-1.0, 1.0, 1.0, vec![bx0, by1, bz1]));
        let v111 = brep.add_vertex(corner(1.0, 1.0, 1.0, vec![bx1, by1, bz1]));

        // Helper: an edge as a polyline between two corner vertices, with the
        // two face branches whose collision forms it.
        let edge = |brep: &mut BRep,
                        a: VertexId,
                        b: VertexId,
                        ba: BranchId,
                        bb: BranchId|
         -> EdgeId {
            let pa = brep.vertex(a).position;
            let pb = brep.vertex(b).position;
            brep.add_edge(Edge::new(
                EdgeGeometry::Polyline(vec![pa, pb]),
                ba,
                bb,
                Some(a),
                Some(b),
            ))
        };

        // 12 edges. Each is shared by exactly two face branches.
        // 4 bottom (z = -1, branch bz0):
        let e_b_x0 = edge(&mut brep, v000, v100, by0, bz0); // y=-1,z=-1
        let e_b_x1 = edge(&mut brep, v010, v110, by1, bz0); // y=+1,z=-1
        let e_b_y0 = edge(&mut brep, v000, v010, bx0, bz0); // x=-1,z=-1
        let e_b_y1 = edge(&mut brep, v100, v110, bx1, bz0); // x=+1,z=-1
        // 4 top (z = +1, branch bz1):
        let e_t_x0 = edge(&mut brep, v001, v101, by0, bz1);
        let e_t_x1 = edge(&mut brep, v011, v111, by1, bz1);
        let e_t_y0 = edge(&mut brep, v001, v011, bx0, bz1);
        let e_t_y1 = edge(&mut brep, v101, v111, bx1, bz1);
        // 4 vertical (along z):
        let e_v00 = edge(&mut brep, v000, v001, bx0, by0); // x=-1,y=-1
        let e_v10 = edge(&mut brep, v100, v101, bx1, by0); // x=+1,y=-1
        let e_v01 = edge(&mut brep, v010, v011, bx0, by1); // x=-1,y=+1
        let e_v11 = edge(&mut brep, v110, v111, bx1, by1); // x=+1,y=+1

        // Helper: an analytic plane face from a normal/offset and 4 edges.
        let plane = |normal: [f32; 3], offset: f32| SurfaceGeometry::Analytic {
            surface: HalfSpace::Plane { normal, offset },
            transform: Transform::IDENTITY,
        };
        let face = |brep: &mut BRep,
                        branch: BranchId,
                        surf: SurfaceGeometry,
                        ring: [(EdgeId, bool); 4]|
         -> FaceId {
            brep.add_face(Face {
                branch_id: branch,
                surface: surf,
                outer: Loop::new(ring.to_vec()),
                inner: Vec::new(),
            })
        };

        // 6 faces, each an outer loop of 4 edges. The `reversed` flags give
        // every face a CCW winding when viewed from outside, so that each of
        // the 12 edges is traversed exactly once forward and once reversed —
        // i.e. the shell is an orientable, watertight manifold (see
        // [`BRep::manifold_violations`]). Each directed cycle is noted above
        // its ring as the vertex walk it realises.
        // f_bz0 (z=-1, normal -Z): v000→v010→v110→v100→v000
        let f_bz0 = face(
            &mut brep,
            bz0,
            plane([0.0, 0.0, -1.0], 1.0),
            [(e_b_y0, false), (e_b_x1, false), (e_b_y1, true), (e_b_x0, true)],
        );
        // f_bz1 (z=+1, normal +Z): v001→v101→v111→v011→v001
        let f_bz1 = face(
            &mut brep,
            bz1,
            plane([0.0, 0.0, 1.0], 1.0),
            [(e_t_x0, false), (e_t_y1, false), (e_t_x1, true), (e_t_y0, true)],
        );
        // f_bx0 (x=-1, normal -X): v000→v001→v011→v010→v000
        let f_bx0 = face(
            &mut brep,
            bx0,
            plane([-1.0, 0.0, 0.0], 1.0),
            [(e_v00, false), (e_t_y0, false), (e_v01, true), (e_b_y0, true)],
        );
        // f_bx1 (x=+1, normal +X): v100→v110→v111→v101→v100
        let f_bx1 = face(
            &mut brep,
            bx1,
            plane([1.0, 0.0, 0.0], 1.0),
            [(e_b_y1, false), (e_v11, false), (e_t_y1, true), (e_v10, true)],
        );
        // f_by0 (y=-1, normal -Y): v000→v100→v101→v001→v000
        let f_by0 = face(
            &mut brep,
            by0,
            plane([0.0, -1.0, 0.0], 1.0),
            [(e_b_x0, false), (e_v10, false), (e_t_x0, true), (e_v00, true)],
        );
        // f_by1 (y=+1, normal +Y): v010→v011→v111→v110→v010
        let f_by1 = face(
            &mut brep,
            by1,
            plane([0.0, 1.0, 0.0], 1.0),
            [(e_v01, false), (e_t_x1, false), (e_v11, true), (e_b_x1, true)],
        );

        // One closed shell of all 6 faces, one solid with no voids.
        let shell = brep.add_shell(Shell::new(
            vec![f_bz0, f_bz1, f_bx0, f_bx1, f_by0, f_by1],
            true,
        ));
        brep.add_solid(Solid::new(shell, Vec::new()));

        brep
    }

    #[test]
    fn cube_counts() {
        let brep = build_cube();
        assert_eq!(brep.vertex_count(), 8);
        assert_eq!(brep.edge_count(), 12);
        assert_eq!(brep.face_count(), 6);
        assert_eq!(brep.shell_count(), 1);
        assert_eq!(brep.solid_count(), 1);
    }

    #[test]
    fn accessors_round_trip_fields() {
        let brep = build_cube();

        // Vertex 0 is the (-1,-1,-1) corner with three colliding branches.
        let v = brep.vertex(VertexId(0));
        assert_eq!(v.position, [-1.0, -1.0, -1.0]);
        assert_eq!(v.branches.len(), 3);

        // Edge 0 is a polyline; its endpoints match its vertices' positions.
        let e = brep.edge(EdgeId(0));
        match &e.geometry {
            EdgeGeometry::Polyline(pts) => {
                assert_eq!(pts.len(), 2);
                assert_eq!(pts[0], brep.vertex(e.start.unwrap()).position);
                assert_eq!(pts[1], brep.vertex(e.end.unwrap()).position);
            }
            other => panic!("expected polyline, got {other:?}"),
        }

        // Face 0 is an analytic plane with a 4-edge outer loop, no holes.
        let f = brep.face(FaceId(0));
        assert!(matches!(
            f.surface,
            SurfaceGeometry::Analytic {
                surface: HalfSpace::Plane { .. },
                ..
            }
        ));
        assert_eq!(f.outer.edges.len(), 4);
        assert!(f.inner.is_empty());

        // The single solid's outer shell is closed and holds all 6 faces.
        let s = brep.solid(SolidId(0));
        let sh = brep.shell(s.outer);
        assert!(sh.is_closed);
        assert_eq!(sh.faces.len(), 6);
    }

    #[test]
    fn validate_accepts_well_formed_cube() {
        let brep = build_cube();
        assert_eq!(brep.validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_dangling_vertex_reference() {
        let mut brep = build_cube();
        let bogus = VertexId(999);
        brep.add_edge(Edge::new(
            EdgeGeometry::Polyline(vec![[0.0; 3], [1.0; 3]]),
            BranchId(0),
            BranchId(1 << 4),
            Some(bogus),
            None,
        ));
        let err = brep.validate().expect_err("dangling vertex should fail");
        assert!(err.iter().any(|p| p.contains("out of range")), "{err:?}");
    }

    #[test]
    fn manifold_accepts_well_formed_cube() {
        let brep = build_cube();
        assert_eq!(
            brep.manifold_violations(),
            Vec::<String>::new(),
            "the well-formed cube must be a watertight orientable manifold"
        );
        assert_eq!(
            brep.euler_characteristic(),
            2,
            "a closed genus-0 solid has Euler characteristic V−E+F = 2"
        );
    }

    #[test]
    fn manifold_rejects_dropped_edge_use() {
        let mut brep = build_cube();
        // Drop one edge reference from one face's outer loop. That edge now
        // has only a single loop-use (count 1, open boundary), and the face it
        // was removed from no longer closes — so the shell is not watertight.
        let dropped = brep.faces[0].outer.edges.remove(0).0;
        let violations = brep.manifold_violations();
        assert!(
            !violations.is_empty(),
            "removing an edge use must break the manifold invariant"
        );
        assert!(
            violations.iter().any(|p| p.contains(&format!("edge {}", dropped.0))),
            "violations must name the affected edge {}: {violations:?}",
            dropped.0
        );
    }

    #[test]
    fn manifold_empty_brep_is_vacuously_watertight() {
        let brep = BRep::new();
        assert_eq!(brep.manifold_violations(), Vec::<String>::new());
    }

    #[test]
    fn serde_round_trip() {
        let brep = build_cube();
        let json = serde_json::to_string(&brep).expect("serialise");
        let back: BRep = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(brep, back);
    }

    #[test]
    fn edge_signature_is_canonical() {
        let a = BranchId(2 << 4);
        let b = BranchId(5 << 4);
        let e_ab = Edge::new(EdgeGeometry::Polyline(vec![]), a, b, None, None);
        let e_ba = Edge::new(EdgeGeometry::Polyline(vec![]), b, a, None, None);
        assert_eq!(e_ab.signature, e_ba.signature);
        assert!(e_ab.signature.0.as_u64() <= e_ab.signature.1.as_u64());
        assert_eq!(e_ab.signature, (a, b));
    }
}
