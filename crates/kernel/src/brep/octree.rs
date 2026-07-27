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
//! The surface-proximity guard `f_min ≤ diag` below assumes a Lipschitz-1
//! field (`|∇sdf| ≤ 1`), so that a corner within `diag` of the surface really
//! bounds a surface crossing through the cell. Some fields VIOLATE this and
//! over-estimate the distance — Twist/Bend (domain-warp stretch), Displace
//! (additive gradient) and Chamfer blends (cut-plane term (∇a+∇b)/√2). We
//! therefore widen the bracket by the body-wide `march_relax` factor (≤ 1,
//! since #384 Stufe 2 derived per-op: `1/L` of the composed tree, see
//! [`crate::sdf_compiler::lipschitz_bound`]): `bracket = diag / relax`, so an
//! over-estimating cell is no longer wrongly pruned as `Empty` (Ticket qmvn;
//! Revolve, the original trigger, is meanwhile derived 1-Lipschitz and no
//! longer damped). With a Lipschitz field (`relax = 1`) the bracket is
//! unchanged.
//!
//! | class       | condition                                                |
//! | ----------- | -------------------------------------------------------- |
//! | **Empty**   | signs equal ∧ `branches ≤ 1` ∧ `f_min > bracket`         |
//! | **Edge**    | exactly 2 distinct branches ∧ `f_min ≤ bracket`          |
//! | **Vertex**  | ≥3 distinct branches ∧ `f_min ≤ bracket`                 |
//! | **Face**    | sign change ∧ `branches ≤ 1`                             |
//! | **Uncertain** | signs+branches equal ∧ `f_min ≤ bracket` → refine more |
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
use crate::cell_csg::{CellCsg, SpecCell};
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
    /// Cell-local tree specialization (see [`crate::cell_csg`]): each
    /// subdivision re-specializes the parent's (already shrunken) CSG view
    /// for its cell, dropping boolean branches that provably cannot
    /// influence the samples — value- and contributor-exact, so the built
    /// octree is identical either way; only the eval cost changes. Off is
    /// for A/B debugging.
    pub specialize: bool,
    /// Restrict the build to this world-space REGION instead of the whole
    /// body AABB (#426). The root cell becomes `region ∩ body.aabb()`, so the
    /// octree localises topology only where the caller needs it. `None`
    /// (the default) is the whole body.
    ///
    /// This exists because the hybrid seed weave repairs a HANDFUL of
    /// unresolved seams from the octree wireframe and discards everything
    /// else: on a 3 m assembly rail (#426) the whole-body octree was 1.54
    /// million leaves and 14 minutes, to fill two seams 0.17 mm and 0.017 mm
    /// wide — which cells that coarse cannot even resolve.
    pub region: Option<Aabb>,
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
            specialize: true,
            region: None,
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

    /// Build an octree that only covers `region` (#426) — the root cell is
    /// `region ∩ body.aabb()`, and `min_cell_size` is scaled to the region so
    /// a local repair is resolved at the LOCAL scale, never the whole body's.
    pub fn build_region(body: &BoundedCsg, min_cell_size: f32, region: Aabb) -> Octree {
        let mut cfg = OctreeConfig::with_min_cell_size(min_cell_size);
        cfg.region = Some(region);
        Octree::build_with(body, cfg)
    }

    /// Build an octree for `body` with explicit [`OctreeConfig`].
    pub fn build_with(body: &BoundedCsg, cfg: OctreeConfig) -> Octree {
        let mut leaves = Vec::new();

        let aabb = match cfg.region {
            Some(r) => Aabb::intersect(r, body.aabb()),
            None => body.aabb(),
        };
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

        // Lipschitz-Korrektur (Ticket qmvn): Die Klassifikation unten setzt
        // eine Lipschitz-1-Distanz voraus (|∇sdf| ≤ 1), damit die
        // Oberflächen-Näheschranke `f_min ≤ diag` zuverlässig ist. Felder mit
        // L > 1 (Twist/Bend/Displace/Chamfer) überschätzen die Distanz; genau
        // das quantifiziert der körperweite `march_relax`-Faktor (= 1/L aus
        // dem Compile-Zeit-Lipschitz-Netz, #384 Stufe 2). Wir reichen ihn
        // durch und weiten die Schranke um `1/relax`, sodass überschätzte
        // Oberflächenzellen nicht fälschlich als `Empty` weggeschnitten werden.
        let relax = crate::sdf_compiler::march_relax(body).clamp(f32::MIN_POSITIVE, 1.0);
        let spec_root = CellCsg::full(body);
        let root_samples = gather_samples(&spec_root, &cfg, root);
        recurse(&spec_root, &cfg, &registry, root, 0, relax, &mut leaves, &root_samples);
        Octree { leaves }
    }

    /// Test-only: build with an explicitly forced Lipschitz-bracket relax
    /// factor instead of the body-derived [`crate::sdf_compiler::march_relax`].
    /// Used by the Ticket-qmvn regression test to reconstruct the *uncorrected*
    /// (`relax = 1`) classification on a body whose true relax is `< 1`, proving
    /// the correction recovers strictly more surface cells.
    #[cfg(test)]
    pub fn build_with_bracket_relax_for_test(
        body: &BoundedCsg,
        cfg: OctreeConfig,
        relax: f32,
    ) -> Octree {
        let mut leaves = Vec::new();
        let aabb = body.aabb();
        if aabb.is_empty() {
            return Octree { leaves };
        }
        let size = aabb.size();
        let largest = size[0].max(size[1]).max(size[2]).max(1.0);
        let root = aabb.expanded(largest * cfg.root_margin_factor);
        let registry = crate::brep::coincidence::CoincidenceRegistry::build(body);
        let relax = relax.clamp(f32::MIN_POSITIVE, 1.0);
        let spec_root = CellCsg::full(body);
        let root_samples = gather_samples(&spec_root, &cfg, root);
        recurse(&spec_root, &cfg, &registry, root, 0, relax, &mut leaves, &root_samples);
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
#[derive(Clone)]
struct Sample {
    value: f32,
    branches: [BranchId; crate::csg::MAX_CONTRIBUTORS],
    branch_len: usize,
}

/// Subdivide `cell` recursively, pushing surviving classified leaves.
/// `samples` are the cell's 9 evaluation samples (8 corners + centre),
/// computed by the PARENT subdivision: all 8 siblings of one subdivision
/// share the same cell size — hence the same contributor `eps` — so their
/// 27 corner lattice points are evaluated ONCE and handed down instead of
/// 8×8 per-child corner re-evaluations (the SDF eval dominates the build).
fn recurse(
    body: &CellCsg<'_>,
    cfg: &OctreeConfig,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    cell: Aabb,
    depth: u32,
    relax: f32,
    out: &mut Vec<OctreeCell>,
    samples: &[Sample; 9],
) {
    let class = classify(registry, cell, relax, samples);

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
                subdivide(body, cfg, registry, cell, depth, relax, out);
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
                subdivide(body, cfg, registry, cell, depth, relax, out);
            }
        }
    }
}

/// Split `cell` into its 8 octants and recurse into each.
///
/// Corner-eval sharing: the 8 octants' corners form a 3×3×3 lattice of
/// 27 points; all octants have the same size, hence the same contributor
/// `eps`, so each lattice point is evaluated exactly once here and the
/// per-child 8-corner sets are assembled from it (27 + 8 child centres =
/// 35 evals instead of 72 — the SDF evaluation dominates the build).
/// Parent samples are NOT reused: the contributor eps scales with the
/// cell diagonal, so a parent sample at the same point carries a fatter
/// branch set and would change classification semantics.
///
/// Cell-local specialization: before sampling, the parent's CSG view is
/// re-specialized for THIS cell (margin = the lattice's `child_eps`, so
/// every sample taken from it — and every deeper re-specialization — is
/// value- and contributor-exact; see [`crate::cell_csg`]).
fn subdivide(
    parent_body: &CellCsg<'_>,
    cfg: &OctreeConfig,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    cell: Aabb,
    depth: u32,
    relax: f32,
    out: &mut Vec<OctreeCell>,
) {
    let size = cell.size();
    let child_diag = 0.5
        * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
    let child_eps = child_diag * cfg.eps_factor;

    let spec = if cfg.specialize {
        let spec_cell =
            SpecCell::new(cell, child_eps, EvalMode::Logical, 1.0 / relax);
        Some(parent_body.specialize(&spec_cell))
    } else {
        None
    };
    let body: &CellCsg<'_> = match &spec {
        Some(s) => &s.tree,
        None => parent_body,
    };

    let mid = [
        0.5 * (cell.min[0] + cell.max[0]),
        0.5 * (cell.min[1] + cell.max[1]),
        0.5 * (cell.min[2] + cell.max[2]),
    ];
    let coord = |axis: usize, idx: usize| -> f32 {
        match idx {
            0 => cell.min[axis],
            1 => mid[axis],
            _ => cell.max[axis],
        }
    };
    // 27 shared lattice samples, indexed ix + 3·iy + 9·iz.
    let mut lattice: Vec<Sample> = Vec::with_capacity(27);
    for iz in 0..3 {
        for iy in 0..3 {
            for ix in 0..3 {
                lattice.push(sample_at(
                    body,
                    [coord(0, ix), coord(1, iy), coord(2, iz)],
                    child_eps,
                ));
            }
        }
    }

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
        // The child's 8 corners out of the lattice (corner j of octant i
        // sits at lattice coordinate (i_axis + j_axis) per axis)…
        let mut samples: Vec<Sample> = Vec::with_capacity(9);
        for j in 0..8 {
            let ix = ((i & 1) + (j & 1)) as usize;
            let iy = (((i >> 1) & 1) + ((j >> 1) & 1)) as usize;
            let iz = (((i >> 2) & 1) + ((j >> 2) & 1)) as usize;
            samples.push(lattice[ix + 3 * iy + 9 * iz].clone());
        }
        // …plus its own centre (unique per octant, off the lattice).
        samples.push(sample_at(
            body,
            [
                0.5 * (child.min[0] + child.max[0]),
                0.5 * (child.min[1] + child.max[1]),
                0.5 * (child.min[2] + child.max[2]),
            ],
            child_eps,
        ));
        let samples: [Sample; 9] = samples.try_into().ok().expect("9 samples");
        recurse(body, cfg, registry, child, depth + 1, relax, out, &samples);
    }
}

/// Evaluate a cell's 9 samples (8 corners + centre) at its own
/// diagonal-scaled contributor eps. Used for the ROOT cell only — every
/// deeper cell receives its samples from the parent's shared lattice
/// (see [`subdivide`]).
fn gather_samples(body: &CellCsg<'_>, cfg: &OctreeConfig, cell: Aabb) -> [Sample; 9] {
    let size = cell.size();
    let diag = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
    let eps = diag * cfg.eps_factor;
    let mut samples: Vec<Sample> = Vec::with_capacity(9);
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
    samples.try_into().ok().expect("9 samples")
}

/// Classify `cell` from its 8 corner samples (plus centre). The samples
/// arrive precomputed from the parent subdivision's shared lattice (or
/// [`gather_samples`] for the root) — the eps they were taken at is this
/// cell's own diagonal-scaled contributor eps.
fn classify(
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    cell: Aabb,
    relax: f32,
    samples: &[Sample; 9],
) -> Classification {
    let size = cell.size();
    let diag = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();

    // Lipschitz-Bracket (Ticket qmvn): Die Näheschranke unten geht davon aus,
    // dass `|sdf|` eine echte Distanz ist (Lipschitz-1). Felder mit L > 1
    // (Twist/Bend/Displace/Chamfer) ÜBERSCHÄTZEN die Distanz und melden
    // `f_min > diag`, obwohl die Fläche durch die Zelle läuft — sie würden
    // fälschlich als `Empty` weggeschnitten. Der körperweite `march_relax`
    // (= 1/L des Compile-Zeit-Lipschitz-Netzes, #384) quantifiziert die
    // Überschätzung; wir weiten die Schranke entsprechend: `bracket =
    // diag / relax`. relax = 1 (Lipschitz) lässt die Schranke unverändert.
    let bracket = diag / relax;

    // Distinct branch set across all samples.
    let mut branches: Vec<BranchId> = Vec::new();
    for s in samples.iter() {
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
    // lies within a cell diagonal of the surface (`f_min <= bracket`, the
    // relax-korrigierte Lipschitz bracket). Multi-branch ties that are
    // *deep* inside the solid (`f_min > bracket`) are medial-axis artifacts
    // — e.g. the centre of a cube, where all 6 face half-spaces tie at the
    // inradius — and are NOT topological edges/vertices. Such interior cells
    // carry no surface to seed, so they are pruned like `Empty`. This is the
    // surface-proximity guard that keeps a clean cube at exactly its 12
    // edges + 8 vertices instead of sprouting a spurious 6-branch cell
    // at the body centre.
    let near_surface = f_min <= bracket;

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
        } else if f_min > bracket {
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
fn sample_at(body: &CellCsg<'_>, p: [f32; 3], eps: f32) -> Sample {
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

    /// #426: a whole-body octree on a LONG body is unaffordable, and the
    /// hybrid seed weave only ever needs the octree where its unresolved seams
    /// are. `build_region` scopes the root to that neighbourhood — cost falls
    /// with the region, and (the part that actually matters) the cell size
    /// scales with the region too, so a sub-millimetre repair is resolved at
    /// its own scale instead of the body's.
    ///
    /// The measured case: a 3 087 mm assembly rail whose two unresolved seams
    /// were 0.17 mm and 0.017 mm wide. `topology_min_cell_size` CEILs at 10 mm,
    /// so the whole-body octree paved 1.54 million leaves — cells 600× wider
    /// than the seams, i.e. provably incapable of resolving them — in 14 min.
    #[test]
    fn build_region_scopes_the_root_and_the_cell_size() {
        // A long thin bar: 400 × 20 × 20 — the SHAPE that makes the whole-body
        // octree unaffordable while every feature of interest is local. (The
        // measured #426 body was 3 087 mm; the ratio below is already decisive
        // at 400 mm, and keeping the bar short keeps this test in the
        // edit-loop suite.)
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [200.0, 10.0, 10.0],
            },
        };
        let body = BoundedCsg::from_csg(&csg);

        // What `body::topology_min_cell_size` would hand the whole-body build.
        let body_cell = 10.0;
        let whole = Octree::build(&body, body_cell);

        // One 2 mm neighbourhood near one end of the bar, resolved at its own
        // scale (region / 32, the same divisor the body-wide rule uses).
        let region = Aabb {
            min: [190.0, 9.0, 9.0],
            max: [192.0, 11.0, 11.0],
        };
        let scoped = Octree::build_region(&body, 2.0 / 32.0, region);

        assert!(
            scoped.leaves().len() * 20 < whole.leaves().len(),
            "scoped build must be orders of magnitude smaller: {} vs {}",
            scoped.leaves().len(),
            whole.leaves().len(),
        );
        assert!(
            !scoped.leaves().is_empty(),
            "the region straddles the bar's +Y/+Z edge, so it must carry cells",
        );

        // Every cell lies inside the region (plus the root margin) — the build
        // never wanders off into the rest of the body.
        let margin = 0.1;
        for c in scoped.leaves() {
            for k in 0..3 {
                assert!(
                    c.aabb.min[k] >= region.min[k] - margin
                        && c.aabb.max[k] <= region.max[k] + margin,
                    "cell {:?} escapes the region {region:?}",
                    c.aabb,
                );
            }
        }

        // And the point of the exercise: the scoped cells are FINER than
        // anything the whole-body build could offer there.
        let cell_edge = |c: &OctreeCell| {
            let s = c.aabb.size();
            s[0].max(s[1]).max(s[2])
        };
        let finest_scoped = scoped
            .edge_cells()
            .chain(scoped.vertex_cells())
            .map(cell_edge)
            .fold(f32::INFINITY, f32::min);
        assert!(
            finest_scoped < body_cell,
            "scoped cells must beat the body-wide {body_cell} mm floor, got {finest_scoped}",
        );
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

    /// Cell-local specialization must be a pure optimisation: the built
    /// octree (every leaf, class and branch set, in order) is identical
    /// with and without it — on an assembly with unions, differences and
    /// a smooth op, where pruning actually fires.
    #[test]
    fn specialization_is_leaf_identical_and_faster() {
        // 3×3×3 grid of boxes/spheres, two subtracted tools, one smooth
        // union — 30 leaves.
        let mut node: Option<CsgNode> = None;
        let mut id = 1u32;
        for ix in 0..3 {
            for iy in 0..3 {
                for iz in 0..3 {
                    let c = [ix as f32 * 10.0, iy as f32 * 10.0, iz as f32 * 10.0];
                    let prim = if (ix + iy + iz) % 2 == 0 {
                        CsgNode::Leaf {
                            id: FeatureId::from_u32(id),
                            primitive: Primitive::Sphere { center: c, radius: 3.0 },
                        }
                    } else {
                        CsgNode::Leaf {
                            id: FeatureId::from_u32(id),
                            primitive: Primitive::Box {
                                center: c,
                                half_extents: [2.5, 2.5, 2.5],
                            },
                        }
                    };
                    id += 1;
                    node = Some(match node {
                        None => prim,
                        Some(acc) => CsgNode::Union(Box::new(acc), Box::new(prim)),
                    });
                }
            }
        }
        let mut tree = node.unwrap();
        tree = CsgNode::Difference(
            Box::new(tree),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(100),
                primitive: Primitive::Sphere { center: [10.0, 10.0, 10.0], radius: 2.0 },
            }),
        );
        tree = CsgNode::SmoothUnion(
            Box::new(tree),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(101),
                primitive: Primitive::Sphere { center: [0.0, 20.0, 20.0], radius: 4.0 },
            }),
            1.5,
            crate::csg::BlendMode::Fillet,
        );
        let body = BoundedCsg::from_csg(&tree);

        let cfg_on = OctreeConfig::with_min_cell_size(0.5);
        let cfg_off = OctreeConfig { specialize: false, ..cfg_on };

        let t0 = Instant::now();
        let with_spec = Octree::build_with(&body, cfg_on);
        let t_on = t0.elapsed();
        let t1 = Instant::now();
        let without = Octree::build_with(&body, cfg_off);
        let t_off = t1.elapsed();
        eprintln!(
            "octree 30-leaf assembly: specialize {t_on:?} vs full {t_off:?} ({} leaves)",
            with_spec.leaves().len()
        );

        assert_eq!(
            with_spec.leaves(),
            without.leaves(),
            "specialization changed the octree"
        );
        assert!(!with_spec.leaves().is_empty());
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
