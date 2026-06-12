//! B-Rep extraction, **Phase 1: spatial isolation (octree)**.
//!
//! Implements the octree builder described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Phase 1: Räumliche Isolierung
//! (Octree)"). The job of this phase is purely *spatial isolation +
//! cell classification*: it subdivides the body's bounding box and tags
//! each surviving leaf cell as a smooth-surface (`Face`), a 2-branch
//! collision (`Edge`) or a ≥3-branch collision (`Vertex`) cell — the
//! seeds the later phases project, walk and weave. No projection,
//! edge-walking or weaving happens here (that is C.3+).
//!
//! ## Classification (per the concept's Phase-1 table)
//!
//! At each cell the SDF is sampled at its 8 corners (and centre) via
//! [`BoundedCsg::evaluate`] in [`EvalMode::Logical`] — the unmasked hard
//! CSG, the only mode with well-defined contributor sets. From the
//! corner samples we collect
//!
//! - `branches`: the distinct [`BranchId`]s appearing across *all* corner
//!   contributor sets (a `Primitive::Box` expands into 6 plane
//!   half-spaces, so near a cube edge 2 distinct branches show up, near a
//!   corner 3 — exactly the fault signal),
//! - `sign_change`: whether the SDF value flips sign across the corners
//!   (a smooth surface passes through), and
//! - `f_min` / `diag`: the smallest `|value|` over the corners and the
//!   cell's space diagonal length (the Lipschitz / Nyquist guard term).
//!
//! | class       | condition                                              |
//! | ----------- | ------------------------------------------------------ |
//! | **Empty**   | signs equal ∧ `branches ≤ 1` ∧ `f_min > diag`          |
//! | **Edge**    | exactly 2 distinct branches                            |
//! | **Vertex**  | ≥3 distinct branches                                   |
//! | **Face**    | sign change ∧ `branches ≤ 1`                           |
//! | **Uncertain** | signs+branches equal ∧ `f_min ≤ diag` → refine more  |
//!
//! `Empty` cells are discarded (never stored, never recursed). `Uncertain`
//! is purely an internal "keep refining" state; it is never a *stored* leaf
//! class — an `Uncertain` cell that bottoms out at the resolution limit
//! without resolving a feature is simply dropped (no corner ever saw a
//! surface, so there is nothing to seed).
//!
//! ## Feature-adaptive refinement (two thresholds)
//!
//! Refinement is **adaptive on topology, not on geometry**: the octree's
//! only job is to *localise edges and vertices*, so that is the only thing
//! worth refining for. How a face curves on the inside is irrelevant here —
//! the face geometry is resolved analytically downstream, never from this
//! grid (only `edge_cells()`/`vertex_cells()` feed [`crate::brep::extraction::extract_features`];
//! `Face` cells feed nothing but the debug overlay). So:
//!
//! - `Edge`, `Vertex` and `Uncertain` cells refine to the fine
//!   `min_cell_size` — they must pin the feature precisely for the weave.
//! - `Face` cells stop at the coarser `face_cell_size`: a single-branch
//!   patch carries no topology to seed, so subdividing it further only
//!   spends a uniform fine grid on a flat (or smoothly curved) face — the
//!   non-adaptivity visible in the octree-cell overlay.
//!
//! This is safe because the wide contributor `eps` (`= cell_diag · eps_factor`,
//! 0.5) makes each sample register *every* branch within ~half a cell of the
//! fault: a cell anywhere near an edge already collects 2 branches and
//! classifies as `Edge`, the more so the coarser (hence larger-`eps`) it is.
//! A `Face` cell surviving at `face_cell_size` therefore has no second branch
//! within half its (large) diagonal — it is genuinely deep inside one face,
//! far from any edge/vertex. The only thing lost is the uniform fine grid on
//! face interiors, which no consumer reads.

use std::collections::HashSet;

use crate::bvh::BoundedCsg;
use crate::csg::{Aabb, BranchId, EvalMode};
use crate::sdf_dual::sdf_gradient_mode;

/// The stored classification of an octree leaf cell. `Uncertain` from
/// the concept's table is deliberately absent: it is an internal
/// refinement state, never a stored leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellClass {
    /// A smooth, continuous surface passes through (sign change, ≤1
    /// branch).
    Face,
    /// Exactly two branch surfaces collide — a candidate B-Rep edge.
    Edge,
    /// Three or more branch surfaces collide — a candidate B-Rep vertex.
    Vertex,
}

/// A classified octree leaf cell. Carries its bounds, its class and the
/// distinct branch surfaces that collide within it (sorted ascending and
/// deduplicated). For `Face` cells `branches` holds the single owning
/// branch (or is empty for a far-but-uncertain remnant — see note in the
/// module docs; such cells are never stored).
#[derive(Debug, Clone, PartialEq)]
pub struct OctreeCell {
    pub aabb: Aabb,
    pub class: CellClass,
    /// Distinct colliding branches, sorted ascending & deduplicated.
    pub branches: Vec<BranchId>,
}

impl OctreeCell {
    /// Geometric centre of the cell.
    pub fn center(&self) -> [f32; 3] {
        [
            0.5 * (self.aabb.min[0] + self.aabb.max[0]),
            0.5 * (self.aabb.min[1] + self.aabb.max[1]),
            0.5 * (self.aabb.min[2] + self.aabb.max[2]),
        ]
    }

    /// The canonical `(BranchId, BranchId)` signature of an `Edge` cell
    /// (smaller branch first), or `None` for non-edge cells.
    pub fn edge_signature(&self) -> Option<(BranchId, BranchId)> {
        if self.class == CellClass::Edge && self.branches.len() == 2 {
            Some((self.branches[0], self.branches[1]))
        } else {
            None
        }
    }
}

/// Tunables for the octree build. The defaults match the concept's
/// 0.1 mm resolution target with a depth cap that keeps a runaway from
/// hanging (a 256³ effective grid).
#[derive(Debug, Clone, Copy)]
pub struct OctreeConfig {
    /// Resolution limit for **feature** cells (`Edge`/`Vertex`/`Uncertain`):
    /// they are not subdivided below this edge size.
    pub min_cell_size: f32,
    /// Resolution limit for **`Face`** cells, which localise no topology and
    /// feed no weave consumer. Coarser than `min_cell_size` (see
    /// [`FACE_CELL_FACTOR`]) so face interiors are not paved with a uniform
    /// fine grid. Must be `>= min_cell_size`.
    pub face_cell_size: f32,
    /// Hard depth cap so a degenerate field can never hang the build.
    pub max_depth: u32,
    /// `eps` passed to [`BoundedCsg::evaluate`] is `cell_diag *
    /// eps_factor`. 0.5 means "see surfaces within ~half a cell", wide
    /// enough that an edge's two faces both register near the fault.
    pub eps_factor: f32,
    /// Margin (relative to the body AABB's largest extent) by which the
    /// root cell is expanded so boundary surfaces are never clipped.
    pub root_margin_factor: f32,
}

/// How much coarser a `Face` cell may be than a feature cell. Faces carry no
/// topology to seed, so this trades a uniform fine face grid for `FACTOR`× the
/// linear cell size (≈ `FACTOR³`× fewer face cells) while leaving every
/// `Edge`/`Vertex` cell — and thus the woven B-Rep — untouched.
pub const FACE_CELL_FACTOR: f32 = 4.0;

impl OctreeConfig {
    /// Build a config from just `min_cell_size`, with sane defaults for
    /// the rest. `face_cell_size` is derived as `min_cell_size · FACE_CELL_FACTOR`.
    pub fn with_min_cell_size(min_cell_size: f32) -> Self {
        Self {
            min_cell_size,
            face_cell_size: min_cell_size * FACE_CELL_FACTOR,
            ..Self::default()
        }
    }
}

impl Default for OctreeConfig {
    fn default() -> Self {
        Self {
            min_cell_size: 0.1,
            face_cell_size: 0.1 * FACE_CELL_FACTOR,
            max_depth: 8,
            eps_factor: 0.5,
            root_margin_factor: 0.01,
        }
    }
}

/// Internal pre-leaf classification, including the `Uncertain` refine
/// state and the branch set gathered while sampling.
#[derive(Debug)]
enum Classification {
    /// Discard — no feature can lie here.
    Empty,
    /// A stored class with its colliding branches (sorted, deduped).
    Leaf(CellClass, Vec<BranchId>),
    /// Inconclusive at this resolution; keep subdividing.
    Uncertain,
}

/// The sparse octree of a single body: the surviving classified leaf
/// cells. Empty cells are pruned during the build and never stored.
#[derive(Debug, Clone, Default)]
pub struct Octree {
    leaves: Vec<OctreeCell>,
}

impl Octree {
    /// Build an octree for `body`, refining down to `min_cell_size`
    /// with default depth / eps tunables. See [`Octree::build_with`] for
    /// full control.
    pub fn build(body: &BoundedCsg, min_cell_size: f32) -> Octree {
        Octree::build_with(body, OctreeConfig::with_min_cell_size(min_cell_size))
    }

    /// Build an octree for `body` with explicit [`OctreeConfig`].
    pub fn build_with(body: &BoundedCsg, cfg: OctreeConfig) -> Octree {
        let mut leaves = Vec::new();

        let aabb = body.aabb();
        if aabb.is_empty() {
            return Octree { leaves };
        }
        // Expand the root so surfaces exactly on the AABB boundary are
        // bracketed by corner samples on both sides.
        let size = aabb.size();
        let largest = size[0].max(size[1]).max(size[2]).max(1.0);
        let root = aabb.expanded(largest * cfg.root_margin_factor);

        // Phase 4 (BREP-CLASSIFICATION-PLAN): cell classification counts
        // *coincidence classes*, not raw branches — two stacked features
        // whose walls share one carrier surface are ONE effective surface,
        // so their shared wall classifies as Face instead of smearing into
        // edge cells along the whole contact (the oa9h/vyee mechanic).
        let registry = crate::brep::coincidence::CoincidenceRegistry::build(body);
        recurse(body, &cfg, &registry, root, 0, &mut leaves);
        Octree { leaves }
    }

    /// All surviving classified leaf cells.
    pub fn leaves(&self) -> &[OctreeCell] {
        &self.leaves
    }

    /// Iterator over the `Edge` (2-branch collision) leaf cells.
    pub fn edge_cells(&self) -> impl Iterator<Item = &OctreeCell> {
        self.leaves.iter().filter(|c| c.class == CellClass::Edge)
    }

    /// Iterator over the `Vertex` (≥3-branch collision) leaf cells.
    pub fn vertex_cells(&self) -> impl Iterator<Item = &OctreeCell> {
        self.leaves.iter().filter(|c| c.class == CellClass::Vertex)
    }

    /// Iterator over the `Face` (smooth-surface) leaf cells.
    pub fn face_cells(&self) -> impl Iterator<Item = &OctreeCell> {
        self.leaves.iter().filter(|c| c.class == CellClass::Face)
    }

    /// The set of distinct canonical edge signatures `(a, b)` (raw `u32`
    /// branch payloads, smaller first) over all `Edge` cells. The
    /// resolution-robust topological summary of the edges: a clean cube
    /// yields exactly 12 entries regardless of how finely it was
    /// refined.
    pub fn distinct_edge_signatures(&self) -> HashSet<(u32, u32)> {
        self.edge_cells()
            .filter_map(|c| c.edge_signature())
            .map(|(a, b)| (a.as_u32(), b.as_u32()))
            .collect()
    }

    /// The set of distinct sorted branch sets over all `Vertex` cells
    /// (raw `u32` payloads). The resolution-robust topological summary of
    /// the vertices: a clean cube yields exactly 8 entries, one per
    /// corner, each its unique triple of face branches.
    pub fn distinct_vertex_branch_sets(&self) -> HashSet<Vec<u32>> {
        self.vertex_cells()
            .map(|c| c.branches.iter().map(|b| b.as_u32()).collect::<Vec<_>>())
            .collect()
    }
}

/// One corner / centre sample.
struct Sample {
    value: f32,
    branches: [BranchId; crate::csg::MAX_CONTRIBUTORS],
    branch_len: usize,
}

/// Subdivide `cell` recursively, pushing surviving classified leaves.
fn recurse(
    body: &BoundedCsg,
    cfg: &OctreeConfig,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    cell: Aabb,
    depth: u32,
    out: &mut Vec<OctreeCell>,
) {
    let class = classify(body, cfg, registry, cell);

    let size = cell.size();
    let largest_edge = size[0].max(size[1]).max(size[2]);
    let depth_capped = depth >= cfg.max_depth;

    match class {
        Classification::Empty => {
            // Guaranteed feature-free → prune.
        }
        Classification::Leaf(class, branches) => {
            // `Face` cells localise no topology, so they bottom out at the
            // coarse `face_cell_size`; `Edge`/`Vertex` cells must reach the
            // fine `min_cell_size` to pin the feature for the weave.
            let target = match class {
                CellClass::Face => cfg.face_cell_size,
                CellClass::Edge | CellClass::Vertex => cfg.min_cell_size,
            };
            if largest_edge <= target || depth_capped {
                out.push(OctreeCell {
                    aabb: cell,
                    class,
                    branches,
                });
            } else {
                subdivide(body, cfg, registry, cell, depth, out);
            }
        }
        Classification::Uncertain => {
            // An undecided cell is still hunting a feature → refine to the
            // fine limit before giving up.
            if largest_edge <= cfg.min_cell_size || depth_capped {
                // Bottomed out without resolving a feature: no corner
                // ever bracketed a surface, so there is nothing to seed.
                // Drop it (do not store an `Uncertain` leaf).
            } else {
                subdivide(body, cfg, registry, cell, depth, out);
            }
        }
    }
}

/// Split `cell` into its 8 octants and recurse into each.
fn subdivide(
    body: &BoundedCsg,
    cfg: &OctreeConfig,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    cell: Aabb,
    depth: u32,
    out: &mut Vec<OctreeCell>,
) {
    let mid = [
        0.5 * (cell.min[0] + cell.max[0]),
        0.5 * (cell.min[1] + cell.max[1]),
        0.5 * (cell.min[2] + cell.max[2]),
    ];
    for i in 0..8 {
        let (lo_x, hi_x) = if i & 1 == 0 {
            (cell.min[0], mid[0])
        } else {
            (mid[0], cell.max[0])
        };
        let (lo_y, hi_y) = if i & 2 == 0 {
            (cell.min[1], mid[1])
        } else {
            (mid[1], cell.max[1])
        };
        let (lo_z, hi_z) = if i & 4 == 0 {
            (cell.min[2], mid[2])
        } else {
            (mid[2], cell.max[2])
        };
        let child = Aabb {
            min: [lo_x, lo_y, lo_z],
            max: [hi_x, hi_y, hi_z],
        };
        recurse(body, cfg, registry, child, depth + 1, out);
    }
}

/// Classify `cell` from its 8 corner samples (plus centre).
fn classify(
    body: &BoundedCsg,
    cfg: &OctreeConfig,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    cell: Aabb,
) -> Classification {
    let size = cell.size();
    let diag = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
    let eps = diag * cfg.eps_factor;

    // 8 corners + centre.
    let mut samples = Vec::with_capacity(9);
    for i in 0..8 {
        let x = if i & 1 == 0 { cell.min[0] } else { cell.max[0] };
        let y = if i & 2 == 0 { cell.min[1] } else { cell.max[1] };
        let z = if i & 4 == 0 { cell.min[2] } else { cell.max[2] };
        samples.push(sample_at(body, [x, y, z], eps));
    }
    samples.push(sample_at(
        body,
        [
            0.5 * (cell.min[0] + cell.max[0]),
            0.5 * (cell.min[1] + cell.max[1]),
            0.5 * (cell.min[2] + cell.max[2]),
        ],
        eps,
    ));

    // Distinct branch set across all samples.
    let mut branches: Vec<BranchId> = Vec::new();
    for s in &samples {
        for &b in &s.branches[..s.branch_len] {
            if !branches.contains(&b) {
                branches.push(b);
            }
        }
    }
    branches.sort_unstable_by_key(|b| b.as_u32());

    // Phase 4: members of one coincidence class share a single carrier
    // surface — for cell classification they are ONE effective surface,
    // collapsed onto the class's smallest branch id present (the
    // representative). This covers both lattice entries: ON-same walls
    // (stacked features' shared wall → Face cells instead of an edge smear
    // along the whole contact) AND ON-opposite membranes (the flush contact
    // plane → Face cells whose cull the canonicalisation owns). The seam
    // rim then counts exactly two carriers (wall class ∩ cap class) and
    // classifies as Edge. Downstream pairing sees the representatives;
    // `both_branches_active_classed` matches a representative against any
    // member of its class.
    if !registry.is_empty() {
        let mut reps: Vec<BranchId> = Vec::with_capacity(branches.len());
        for &b in &branches {
            let rep = match registry.class_of(b) {
                Some((cls, _)) => branches
                    .iter()
                    .copied()
                    .find(|&o| matches!(registry.class_of(o), Some((c, _)) if c == cls))
                    .unwrap_or(b),
                None => b,
            };
            if !reps.contains(&rep) {
                reps.push(rep);
            }
        }
        branches = reps;
    }
    let n_branches = branches.len();

    // Sign change & f_min over the corner samples (not the centre — the
    // table is defined on the 8 corners; the centre only enriches the
    // branch set).
    let mut any_pos = false;
    let mut any_neg = false;
    let mut f_min = f32::INFINITY;
    for s in &samples[..8] {
        if s.value > 0.0 {
            any_pos = true;
        } else if s.value < 0.0 {
            any_neg = true;
        } else {
            // exactly on the surface → both sides count as bracketed
            any_pos = true;
            any_neg = true;
        }
        f_min = f_min.min(s.value.abs());
    }
    let sign_change = any_pos && any_neg;

    // A *feature* must straddle the zero level-set: at least one corner
    // lies within a cell diagonal of the surface (`f_min <= diag`, the
    // Lipschitz bracket). Multi-branch ties that are *deep* inside the
    // solid (`f_min > diag`) are medial-axis artifacts — e.g. the centre
    // of a cube, where all 6 face half-spaces tie at the inradius — and
    // are NOT topological edges/vertices. Such interior cells carry no
    // surface to seed, so they are pruned like `Empty`. This is the
    // surface-proximity guard that keeps a clean cube at exactly its 12
    // edges + 8 vertices instead of sprouting a spurious 6-branch cell
    // at the body centre.
    let near_surface = f_min <= diag;

    // Classification table.
    if n_branches >= 3 {
        if near_surface {
            Classification::Leaf(CellClass::Vertex, branches)
        } else {
            Classification::Empty
        }
    } else if n_branches == 2 {
        if near_surface {
            Classification::Leaf(CellClass::Edge, branches)
        } else {
            Classification::Empty
        }
    } else {
        // ≤1 distinct branch.
        if sign_change {
            Classification::Leaf(CellClass::Face, branches)
        } else if f_min > diag {
            Classification::Empty
        } else {
            // Lipschitz guard: a surface might still thread through
            // without touching a corner → refine.
            Classification::Uncertain
        }
    }
}

/// Evaluate the body at `p`. We use [`EvalMode::Logical`] for the
/// provenance-bearing value+contributor pair. The gradient is available
/// via [`sdf_gradient`] for later phases; Phase 1 does not need it for
/// classification, but we touch it here so the dependency stays wired and
/// honest (it is cheap relative to the 9 evaluate calls and keeps the
/// import meaningful for C.3 to lean on).
fn sample_at(body: &BoundedCsg, p: [f32; 3], eps: f32) -> Sample {
    let (value, contributors) = body.evaluate(p, eps, EvalMode::Logical);
    let mut branches = [BranchId::default(); crate::csg::MAX_CONTRIBUTORS];
    let slice = contributors.as_slice();
    let branch_len = slice.len();
    branches[..branch_len].copy_from_slice(slice);
    Sample {
        value,
        branches,
        branch_len,
    }
}

/// Expose the analytic gradient at a point for the projection phase (C.3),
/// evaluated in [`EvalMode::Logical`] to stay consistent with the rest of the
/// B-Rep pipeline (the octree classifies, and Phase 2/3 sample, in Logical
/// mode — hard CSG, well-defined branches). Using the rendering-default
/// Visual gradient here would walk the projection onto smoothed blend
/// surfaces instead of the hard branch half-spaces. See DL-9. For blend-free
/// geometry both modes coincide.
pub fn surface_gradient(body: &BoundedCsg, p: [f32; 3]) -> (f32, [f32; 3]) {
    sdf_gradient_mode(body, p, EvalMode::Logical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use fielddraft_protocol::{FeatureId, Primitive};
    use std::time::Instant;

    /// Build a `BoundedCsg` for an axis-aligned box centred at the
    /// origin with the given half-extents.
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

    fn sphere_body(radius: f32) -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere {
                center: [0.0, 0.0, 0.0],
                radius,
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    #[test]
    fn cube_yields_12_edge_signatures_and_8_vertex_sets() {
        // 2×2×2 cube, refined to 0.25 → ~4 levels. Fast and reliable.
        let body = box_body([1.0, 1.0, 1.0]);

        let t0 = Instant::now();
        let octree = Octree::build(&body, 0.25);
        let elapsed = t0.elapsed();
        eprintln!(
            "cube octree: {} leaves, {} edge cells, {} vertex cells, {:?}",
            octree.leaves().len(),
            octree.edge_cells().count(),
            octree.vertex_cells().count(),
            elapsed
        );

        let edges = octree.distinct_edge_signatures();
        let verts = octree.distinct_vertex_branch_sets();
        assert_eq!(edges.len(), 12, "a cube has 12 edges; got {edges:?}");
        assert_eq!(verts.len(), 8, "a cube has 8 corners; got {verts:?}");

        // Every vertex set is a triple of distinct branches.
        for v in &verts {
            assert_eq!(v.len(), 3, "cube corner should collide 3 faces: {v:?}");
        }
        // Sanity: the build must complete fast.
        assert!(
            elapsed.as_secs_f32() < 5.0,
            "octree build too slow: {elapsed:?}"
        );
    }

    #[test]
    fn lone_sphere_has_no_edges_or_vertices() {
        // A single smooth surface (one branch) → only Face cells, never
        // a false edge/vertex.
        let body = sphere_body(1.0);
        let octree = Octree::build(&body, 0.25);
        assert_eq!(
            octree.distinct_edge_signatures().len(),
            0,
            "a lone sphere has no faults"
        );
        assert_eq!(
            octree.distinct_vertex_branch_sets().len(),
            0,
            "a lone sphere has no corners"
        );
        // It should still find smooth-surface cells.
        assert!(
            octree.face_cells().count() > 0,
            "the sphere surface should produce Face cells"
        );
    }

    #[test]
    fn face_cells_are_coarser_than_feature_cells() {
        // Adaptivity: `Face` cells bottom out at the coarse `face_cell_size`,
        // `Edge`/`Vertex` cells at the fine `min_cell_size`. A cube's flat
        // faces must therefore carry distinctly larger cells than its edges,
        // while its topology (12 edges, 8 corners) is unchanged — Face cells
        // feed no weave consumer, so coarsening them is invisible downstream.
        let body = box_body([1.0, 1.0, 1.0]);
        let min_cell = 0.125;
        let octree = Octree::build(&body, min_cell);

        let cell_edge = |c: &OctreeCell| {
            let s = c.aabb.size();
            s[0].max(s[1]).max(s[2])
        };
        let max_face = octree.face_cells().map(cell_edge).fold(0.0_f32, f32::max);
        let max_feature = octree
            .edge_cells()
            .chain(octree.vertex_cells())
            .map(cell_edge)
            .fold(0.0_f32, f32::max);

        // Feature cells reach the fine limit; faces stop ~FACE_CELL_FACTOR coarser.
        assert!(
            max_feature <= min_cell * 1.5,
            "edge/vertex cells must reach min_cell_size, got {max_feature}"
        );
        assert!(
            max_face > max_feature,
            "face cells must be coarser than feature cells: face {max_face} vs feature {max_feature}"
        );
        assert!(
            max_face <= min_cell * FACE_CELL_FACTOR * 1.5,
            "face cells must respect face_cell_size, got {max_face}"
        );

        // Topology is byte-identical to the uniform build.
        assert_eq!(octree.distinct_edge_signatures().len(), 12);
        assert_eq!(octree.distinct_vertex_branch_sets().len(), 8);
    }

    #[test]
    fn empty_far_region_is_pruned() {
        // A tiny body — the root AABB hugs it, and empty octants around
        // the surface are pruned, so the leaf count stays bounded and the
        // build does not explode.
        let body = box_body([0.5, 0.5, 0.5]);
        let octree = Octree::build(&body, 0.25);
        // A 1×1×1 cube at 0.25 resolution: a full dense grid would be
        // (1/0.25)³ = 64 cells at the leaf level; with empty-pruning only
        // boundary cells survive, far fewer than a multi-level dense
        // tree. Generous cap guards against a pruning regression.
        assert!(
            octree.leaves().len() < 4096,
            "empty pruning failed: {} leaves",
            octree.leaves().len()
        );
        assert!(!octree.leaves().is_empty(), "the body surface must survive");
    }

    #[test]
    fn cell_center_and_edge_signature_helpers() {
        let cell = OctreeCell {
            aabb: Aabb {
                min: [0.0, 0.0, 0.0],
                max: [2.0, 2.0, 2.0],
            },
            class: CellClass::Edge,
            branches: vec![BranchId(3), BranchId(7)],
        };
        assert_eq!(cell.center(), [1.0, 1.0, 1.0]);
        assert_eq!(cell.edge_signature(), Some((BranchId(3), BranchId(7))));

        let face = OctreeCell {
            class: CellClass::Face,
            ..cell.clone()
        };
        assert_eq!(face.edge_signature(), None);
    }
}
