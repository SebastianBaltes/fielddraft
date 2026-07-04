//! Adaptive Manifold Dual Contouring (Meshing Phase 1c).
//!
//! Full implementation of Schaefer, Ju, Warren — "Manifold Dual
//! Contouring" (TVCG 2007, `docs/Literatur/dualsimp_tvcg.pdf`) on a
//! narrow-band octree, for the export mesher:
//!
//! 1. **Finest level** — Nielson-style multi-vertex DC exactly like
//!    `mdc.rs` (one QEF vertex per contour cycle), but evaluated only
//!    in a narrow band around the surface: a cell whose centre SDF
//!    magnitude exceeds `(√3/2)·size·LIPSCHITZ_GUARD` cannot contain
//!    the contour and is skipped whole. The band is uniform at the
//!    target level, so polygon emission never sees a T-junction.
//! 2. **Vertex clustering** (§IV.A) — a vertex TREE separate from the
//!    octree. Bottom-up, one level at a time: vertices whose contour
//!    components are connected through sign-change edges on the 12
//!    internal faces of the parent cell cluster together; their QEFs
//!    sum. Vertices with no partner are PROMOTED into a single-child
//!    node ("always-promote"), so every tree root always lives at the
//!    current level and the child-edge classification stays a purely
//!    local octant computation.
//! 3. **Manifold criterion** (§V, Proposition 1) — a cluster vertex is
//!    `collapsible` iff its QEF residual is under the tolerance AND
//!    χ(S_v) = 1 AND every face of its cell carries 0 or 2 contour
//!    intersections on its 4 edges. Both quantities are maintained
//!    incrementally per vertex as the 13 numbers of §V.B: χ plus the
//!    intersection counts e_i on the 12 cell edges. Recursion:
//!    χ(S_v) = Σ_k χ(S_vk) − Σ_k e_int(S_vk)/4, where e_int sums the
//!    child-edge intersections on edges INTERIOR to the parent cell.
//! 4. **Polygonalization** (§IV.B) — one polygon per finest sign-change
//!    edge, each corner replaced by its representative: the TOPMOST
//!    collapsible ancestor (the criterion is deliberately non-monotone
//!    along the path — safe collapses may resume above an unsafe one).
//!    Polygons that collapse to an edge/point are discarded (the
//!    guarantee of Prop. 1 covers them). Winding is geometric, exactly
//!    like the uniform mesher (Phase 1a).
//!
//! Invariants this module maintains (violations are bugs):
//! - **Always-promote**: after processing level ℓ, every root node has
//!   `level == ℓ` and its `cell` is the level-ℓ cell containing it.
//! - **Single-child promotion never changes χ**: a lone cluster has no
//!   intersections on parent-internal edges (an internal sign-change
//!   edge always connects ≥ 2 children), so e_int = 0.
//! - **e_i bookkeeping**: `e[i]` counts contour intersections of S_v on
//!   edge `i` of the cell the node was created in. Child edges on the
//!   parent boundary aggregate into parent `e_i`; interior child edges
//!   feed the χ recursion and then vanish.
//! - **Determinism**: all iteration orders derive from sorted cell
//!   coordinates / stable vertex indices, never from hash-map order.
//!
//! Edge pinning (Phase 1b) integrates twice: finest vertices whose
//! cell is crossed by a woven B-Rep edge curve are snapped before
//! clustering and marked `pinned`; the flag propagates up the tree and
//! used representatives get a final exact re-snap.

use std::collections::HashMap;

use crate::bvh::BoundedCsg;
use crate::csg::{EvalMode, MAX_CONTRIBUTORS};
use crate::dc::{sdf_normal, Mesh, CONTRIBUTOR_SENTINEL};
use crate::mdc::{
    compute_edge_cycles, QefData, MC_CORNER_POS, MC_EDGE_TO_GRID, MC_EDGE_VERTS, QUAD_CELLS,
};
use fielddraft_protocol::mc_tables::EDGE_TABLE;

/// Default safety factor on the Lipschitz bound of the SDF for the
/// band test. The exact primitives and hard booleans are 1-Lipschitz;
/// warp operators and blend fields stretch gradients, so the band is
/// widened. Bodies containing non-Lipschitz primitives (Thread/Gyroid/
/// Helix, |∇f| far from 1) pass a larger guard via
/// [`adaptive_mdc`]'s `lipschitz_guard`. A missed band cell would
/// surface as a hole — the validator catches it and the caller falls
/// back to the uniform pass.
pub const LIPSCHITZ_GUARD_DEFAULT: f32 = 1.6;
/// Guard for bodies with non-Lipschitz primitives.
pub const LIPSCHITZ_GUARD_NON_LIPSCHITZ: f32 = 3.0;

/// Hard cap on band cells (memory/time guard). At the default export
/// cell this is far beyond any realistic part; the guard exists so a
/// pathological field degrades to the uniform fallback instead of
/// exhausting the WASM heap.
const MAX_BAND_CELLS: usize = 6_000_000;

/// Per-cell vertex assignment, mirroring `mdc.rs`.
struct CellInfo {
    edge_vertex: [u32; 12],
}

/// One node of the Schaefer vertex tree.
struct VertexNode {
    pos: [f32; 3],
    qef: QefData,
    /// Euler characteristic χ(S_v) of the un-simplified surface patch.
    chi: i64,
    /// False once a degenerate crossing broke the χ bookkeeping — the
    /// node (and every ancestor) can then never be collapsible.
    chi_valid: bool,
    /// Contour intersection counts on the 12 edges of this node's cell.
    e: [u32; 12],
    /// Octree cell (at `level`) this node was created in.
    cell: [i32; 3],
    level: u32,
    parent: Option<u32>,
    collapsible: bool,
    pinned: bool,
}

/// (axis, transverse-bit-1, transverse-bit-2) of each of the 12 cell
/// edges, derived from `MC_EDGE_VERTS`/`MC_CORNER_POS`. The transverse
/// axes are the two non-edge axes in ascending order.
fn edge_axis_pos(i: usize) -> (usize, u32, u32) {
    let [a, b] = MC_EDGE_VERTS[i];
    let pa = MC_CORNER_POS[a];
    let pb = MC_CORNER_POS[b];
    let axis = (0..3).find(|&k| pa[k] != pb[k]).unwrap();
    let trans: Vec<usize> = (0..3).filter(|&k| k != axis).collect();
    (axis, pa[trans[0]] as u32, pa[trans[1]] as u32)
}

/// Inverse of [`edge_axis_pos`]: the cell-edge index for (axis, q1, q2).
fn edge_index(axis: usize, q1: u32, q2: u32) -> usize {
    (0..12)
        .find(|&i| edge_axis_pos(i) == (axis, q1, q2))
        .expect("every (axis, q1, q2) combination names a cell edge")
}

/// A finest-level sign-change grid edge: the emission unit and the
/// clustering connectivity unit.
struct FineEdge {
    /// The ≤4 finest vertex ids around the edge, in `QUAD_CELLS` cyclic
    /// order (u32::MAX when a cell contributed no vertex — such edges
    /// are skipped at emission and produce a validator hole).
    verts: [u32; 4],
    axis: usize,
    /// SDF at the two edge endpoints (drives the geometric winding).
    da: f32,
    db: f32,
}

pub struct AdaptiveResult {
    pub mesh: Mesh,
    /// Cells along the root cube (2^level) — reported as resolution.
    pub resolution: u32,
    pub actual_cell_mm: f32,
    /// Finest vertices pinned onto B-Rep edge curves.
    pub edges_pinned: usize,
    /// Vertex-tree statistics: (finest vertices, output vertices).
    pub finest_vertices: usize,
    /// Finest cells carrying MORE than one contour cycle — the wall
    /// between two sheets is thinner than one cell there. A cheap,
    /// free-of-charge thin-feature indicator: the export dialog warns
    /// that sub-cell walls may be under-resolved.
    pub thin_cells: usize,
}

/// Pinning geometry shared with the uniform pass (Phase 1b): world-space
/// edge polylines + corner points of the woven B-Rep.
pub struct PinGeometry {
    pub polylines: Vec<Vec<[f32; 3]>>,
    pub corners: Vec<[f32; 3]>,
}

impl PinGeometry {
    pub fn empty() -> Self {
        Self { polylines: Vec::new(), corners: Vec::new() }
    }

    fn is_empty(&self) -> bool {
        self.polylines.is_empty() && self.corners.is_empty()
    }
}

/// Run adaptive Manifold DC. `cell_target` is the finest cell size in
/// mm (the band level is chosen so the actual cell is ≤ target);
/// `tol_mm` bounds the clustering error (QEF residual threshold) — the
/// geometric deviation budget of the simplification.
pub fn adaptive_mdc(
    node: &BoundedCsg,
    cell_target: f32,
    tol_mm: f32,
    lipschitz_guard: f32,
    pin: &PinGeometry,
) -> Result<AdaptiveResult, String> {
    let aabb = node.aabb();
    if aabb.is_empty() {
        return Err("empty body AABB".into());
    }
    let size3 = aabb.size();
    let max_dim = size3[0].max(size3[1]).max(size3[2]);
    if max_dim <= 0.0 || cell_target <= 0.0 {
        return Err("degenerate extents".into());
    }

    // Cubic root: AABB plus the same 2-cell margin the uniform mesher
    // uses, rounded up to a power-of-two multiple of the target cell.
    let padded = max_dim + 4.0 * cell_target;
    let level = (padded / cell_target).log2().ceil().max(1.0) as u32;
    if level > 24 {
        return Err(format!("band level {level} out of range"));
    }
    let cells_per_axis = 1u32 << level;
    let root_size = cell_target * cells_per_axis as f32;
    let centre = [
        (aabb.min[0] + aabb.max[0]) * 0.5,
        (aabb.min[1] + aabb.max[1]) * 0.5,
        (aabb.min[2] + aabb.max[2]) * 0.5,
    ];
    let origin = [
        centre[0] - root_size * 0.5,
        centre[1] - root_size * 0.5,
        centre[2] - root_size * 0.5,
    ];
    let cell = cell_target;

    let sdf = |p: [f32; 3]| node.sdf(p, EvalMode::Visual);

    // ── 1. Narrow band: descend the implicit octree ────────────────
    let mut leaves: Vec<([i32; 3], f32)> = Vec::new();
    let mut stack: Vec<(u32, [i32; 3])> = vec![(0, [0, 0, 0])];
    while let Some((l, c)) = stack.pop() {
        let s = root_size / (1u32 << l) as f32;
        let cc = [
            origin[0] + (c[0] as f32 + 0.5) * s,
            origin[1] + (c[1] as f32 + 0.5) * s,
            origin[2] + (c[2] as f32 + 0.5) * s,
        ];
        let d = sdf(cc);
        if d.abs() > 0.8661 * s * lipschitz_guard + 1e-4 {
            continue; // cell provably clear of the surface
        }
        if l == level {
            leaves.push((c, d));
            if leaves.len() > MAX_BAND_CELLS {
                return Err(format!("band exceeds {MAX_BAND_CELLS} cells"));
            }
            continue;
        }
        for oct in 0..8i32 {
            stack.push((
                l + 1,
                [
                    c[0] * 2 + (oct & 1),
                    c[1] * 2 + ((oct >> 1) & 1),
                    c[2] * 2 + ((oct >> 2) & 1),
                ],
            ));
        }
    }
    if leaves.is_empty() {
        return Err("no band cells (surface not found)".into());
    }
    // Deterministic processing order.
    leaves.sort_unstable_by_key(|(c, _)| *c);

    // ── 2. Corner sampling (shared, memoised) ──────────────────────
    let mut corners: HashMap<[i32; 3], f32> = HashMap::with_capacity(leaves.len() * 2);
    let corner_point = |g: [i32; 3]| -> [f32; 3] {
        [
            origin[0] + g[0] as f32 * cell,
            origin[1] + g[1] as f32 * cell,
            origin[2] + g[2] as f32 * cell,
        ]
    };
    for (c, _) in &leaves {
        for oct in 0..8i32 {
            let g = [
                c[0] + (oct & 1),
                c[1] + ((oct >> 1) & 1),
                c[2] + ((oct >> 2) & 1),
            ];
            corners.entry(g).or_insert_with(|| sdf(corner_point(g)));
        }
    }

    // ── 3. Finest-level vertices (one per contour cycle) ───────────
    let normal_eps = cell * 0.25;
    let mut thin_cells = 0usize;
    let mut nodes: Vec<VertexNode> = Vec::new();
    let mut cell_info: HashMap<[i32; 3], CellInfo> = HashMap::with_capacity(leaves.len());

    for (c, centre_d) in &leaves {
        let corner_val = |v: usize| -> f32 {
            let g = [
                c[0] + MC_CORNER_POS[v][0] as i32,
                c[1] + MC_CORNER_POS[v][1] as i32,
                c[2] + MC_CORNER_POS[v][2] as i32,
            ];
            corners[&g]
        };
        let mc: [f32; 8] = std::array::from_fn(corner_val);
        let mut case = 0u8;
        for (v, d) in mc.iter().enumerate() {
            if *d < 0.0 {
                case |= 1 << v;
            }
        }
        if EDGE_TABLE[case as usize] == 0 {
            // The surface provably passes through this cell whenever
            // |f(centre)| is under the inscribed-sphere radius — an
            // empty MC case there means a feature THINNER than the
            // sampling lattice slipped between the corner planes (thin
            // wall or thin slit). Count it: the report warns the user
            // to shrink the cell size.
            if centre_d.abs() < 0.5 * cell - 1e-5 {
                thin_cells += 1;
            }
            continue;
        }
        let (edge_cycle, num_cycles) = compute_edge_cycles(case);
        if num_cycles > 1 {
            // Two contour sheets in one cell: the wall between them is
            // resolved, but only just — flag it as thin as well.
            thin_cells += 1;
        }
        let cell_min = corner_point(*c);
        let mut edge_vertex = [u32::MAX; 12];

        for cycle in 0..num_cycles {
            let cycle_id = cycle as i8;
            let mut qef = QefData::new();
            let mut e = [0u32; 12];
            for ei in 0..12 {
                if edge_cycle[ei] != cycle_id {
                    continue;
                }
                let [v0, v1] = MC_EDGE_VERTS[ei];
                let (d0, d1) = (mc[v0], mc[v1]);
                if (d0 < 0.0) == (d1 < 0.0) {
                    continue;
                }
                e[ei] = 1;
                let p0 = [
                    cell_min[0] + MC_CORNER_POS[v0][0] * cell,
                    cell_min[1] + MC_CORNER_POS[v0][1] * cell,
                    cell_min[2] + MC_CORNER_POS[v0][2] * cell,
                ];
                let p1 = [
                    cell_min[0] + MC_CORNER_POS[v1][0] * cell,
                    cell_min[1] + MC_CORNER_POS[v1][1] * cell,
                    cell_min[2] + MC_CORNER_POS[v1][2] * cell,
                ];
                // Bisection refine of the crossing + Hermite normal —
                // identical numerics to the uniform mesher.
                let sign_lo = d0 < 0.0;
                let (mut lo, mut hi) = (0.0f32, 1.0f32);
                let mut t = d0 / (d0 - d1);
                for _ in 0..6 {
                    let mp = [
                        p0[0] + t * (p1[0] - p0[0]),
                        p0[1] + t * (p1[1] - p0[1]),
                        p0[2] + t * (p1[2] - p0[2]),
                    ];
                    let d = sdf(mp);
                    if (d < 0.0) == sign_lo {
                        lo = t;
                    } else {
                        hi = t;
                    }
                    t = (lo + hi) * 0.5;
                }
                let p = [
                    p0[0] + t * (p1[0] - p0[0]),
                    p0[1] + t * (p1[1] - p0[1]),
                    p0[2] + t * (p1[2] - p0[2]),
                ];
                let n = sdf_normal(node, p, normal_eps);
                qef.add(p, n);
            }
            if qef.num_points == 0 {
                continue;
            }
            let mut pos = qef.solve();
            let cell_max = [cell_min[0] + cell, cell_min[1] + cell, cell_min[2] + cell];
            pos[0] = pos[0].clamp(cell_min[0], cell_max[0]);
            pos[1] = pos[1].clamp(cell_min[1], cell_max[1]);
            pos[2] = pos[2].clamp(cell_min[2], cell_max[2]);

            let idx = nodes.len() as u32;
            nodes.push(VertexNode {
                pos,
                qef,
                chi: 1,
                chi_valid: true,
                e,
                cell: *c,
                level,
                parent: None,
                collapsible: true,
                pinned: false,
            });
            for ei in 0..12 {
                if edge_cycle[ei] == cycle_id {
                    edge_vertex[ei] = idx;
                }
            }
        }
        cell_info.insert(*c, CellInfo { edge_vertex });
    }
    let finest_vertices = nodes.len();
    if finest_vertices == 0 {
        return Err("band produced no contour vertices".into());
    }

    // ── 4. Finest sign-change edge list ────────────────────────────
    // Canonical grid coordinates via each leaf's 12 cell edges.
    let mut seen: HashMap<(usize, [i32; 3]), ()> = HashMap::new();
    let mut fine_edges: Vec<FineEdge> = Vec::new();
    let mut pair_buckets: Vec<Vec<(u32, u32)>> = vec![Vec::new(); level as usize + 1];
    for (c, _) in &leaves {
        for ei in 0..12 {
            let (axis, di, dj, dk) = MC_EDGE_TO_GRID[ei];
            let g = [c[0] + di as i32, c[1] + dj as i32, c[2] + dk as i32];
            if seen.insert((axis, g), ()).is_some() {
                continue;
            }
            let mut gb = g;
            gb[axis] += 1;
            let (Some(&da), Some(&db)) = (corners.get(&g), corners.get(&gb)) else {
                continue;
            };
            if (da < 0.0) == (db < 0.0) {
                continue;
            }
            // The 4 adjacent cells + their vertex on this edge.
            let mut verts = [u32::MAX; 4];
            let mut cells = [[0i32; 3]; 4];
            let mut ok = true;
            for (q, &(qdi, qdj, qdk, mc_edge)) in QUAD_CELLS[axis].iter().enumerate() {
                let qc = [g[0] + qdi, g[1] + qdj, g[2] + qdk];
                // QUAD_CELLS offsets are edge-grid → cell offsets; the
                // canonical edge coordinate g IS the edge grid position.
                cells[q] = qc;
                match cell_info.get(&qc) {
                    Some(info) if info.edge_vertex[mc_edge] != u32::MAX => {
                        verts[q] = info.edge_vertex[mc_edge];
                    }
                    _ => ok = false,
                }
            }
            if !ok {
                // A sign-change edge with a missing neighbour vertex —
                // outside the band (guard too tight) or an empty cycle.
                // Skipping produces a hole the validator will flag.
                continue;
            }
            // Clustering connectivity (§IV.A): the dual polygon's mesh
            // EDGES connect ring-adjacent cell pairs — each pair shares
            // a grid face and joins clusters on its own pair-LCA level
            // (a corner edge connects two in-node children even when
            // its other two cells lie outside the node). Diagonal pairs
            // share no face and connect nothing.
            for (qa, qb) in [(0usize, 1usize), (1, 2), (2, 3), (3, 0)] {
                if verts[qa] == verts[qb] {
                    continue;
                }
                let (ca, cb) = (cells[qa], cells[qb]);
                let mut s = 0u32;
                loop {
                    s += 1;
                    if [ca[0] >> s, ca[1] >> s, ca[2] >> s]
                        == [cb[0] >> s, cb[1] >> s, cb[2] >> s]
                    {
                        break;
                    }
                    if s > level {
                        break;
                    }
                }
                let pair_level = level.saturating_sub(s);
                pair_buckets[pair_level as usize].push((verts[qa], verts[qb]));
            }
            fine_edges.push(FineEdge { verts, axis, da, db });
        }
    }
    // `seen` insertion order depended on leaf order (sorted) ✓ but sort
    // edges again for full determinism of the emission order.
    fine_edges.sort_unstable_by_key(|e| (e.verts, e.axis));

    // ── 5. Pin finest vertices onto B-Rep edge curves (Phase 1b) ───
    let mut edges_pinned = 0usize;
    if !pin.is_empty() {
        let mut positions: Vec<[f32; 3]> = nodes.iter().map(|n| n.pos).collect();
        let mut pinned_flags = vec![false; nodes.len()];
        edges_pinned = pin_points_to_curves(
            &mut positions,
            &mut pinned_flags,
            |i| {
                let c = nodes[i].cell;
                (corner_point(c), cell)
            },
            pin,
        );
        for (i, n) in nodes.iter_mut().enumerate() {
            n.pos = positions[i];
            n.pinned = pinned_flags[i];
        }
    }

    // ── 6. Bottom-up manifold vertex clustering (§IV/§V) ───────────
    // The QEF residual threshold: tol_mm is the deviation budget; the
    // residual sums squared plane distances, so a strict per-cluster
    // budget of tol² keeps the representative within ~tol of every
    // constraint plane set.
    let threshold = (tol_mm as f64) * (tol_mm as f64);

    // climb to the current tree root
    fn root_of(nodes: &[VertexNode], mut v: u32) -> u32 {
        while let Some(p) = nodes[v as usize].parent {
            v = p;
        }
        v
    }

    for lvl in (0..level).rev() {
        // Roots all live at level lvl+1 (always-promote invariant).
        let roots: Vec<u32> = (0..nodes.len() as u32)
            .filter(|&i| nodes[i as usize].parent.is_none())
            .collect();
        debug_assert!(roots.iter().all(|&r| nodes[r as usize].level == lvl + 1));

        // Local union-find over the roots.
        let slot_of: HashMap<u32, usize> =
            roots.iter().enumerate().map(|(s, &r)| (r, s)).collect();
        let mut uf: Vec<usize> = (0..roots.len()).collect();
        fn find(uf: &mut [usize], mut x: usize) -> usize {
            while uf[x] != x {
                uf[x] = uf[uf[x]];
                x = uf[x];
            }
            x
        }
        for &(va, vb) in &pair_buckets[lvl as usize] {
            let (ra, rb) = (root_of(&nodes, va), root_of(&nodes, vb));
            let (sa, sb) = (slot_of[&ra], slot_of[&rb]);
            let (fa, fb) = (find(&mut uf, sa), find(&mut uf, sb));
            if fa != fb {
                uf[fa] = fb;
            }
        }

        // Group roots by union-find class (roots sharing a parent cell
        // only — an edge in bucket lvl connects cells that share their
        // level-lvl ancestor by construction; disconnected roots stay
        // alone and are promoted).
        let mut groups: HashMap<usize, Vec<u32>> = HashMap::new();
        for (s, &r) in roots.iter().enumerate() {
            groups.entry(find(&mut uf, s)).or_default().push(r);
        }
        // Deterministic order: by smallest member id.
        let mut group_list: Vec<Vec<u32>> = groups.into_values().collect();
        for g in group_list.iter_mut() {
            g.sort_unstable();
        }
        group_list.sort_unstable_by_key(|g| g[0]);

        for members in group_list {
            let parent_cell = {
                let c = nodes[members[0] as usize].cell;
                [c[0] >> 1, c[1] >> 1, c[2] >> 1]
            };
            debug_assert!(members.iter().all(|&m| {
                let c = nodes[m as usize].cell;
                [c[0] >> 1, c[1] >> 1, c[2] >> 1] == parent_cell
            }));

            // Aggregate QEF, χ, and the 12 edge intersection counts.
            let mut qef = QefData::new();
            let mut chi_sum: i64 = 0;
            let mut chi_valid = true;
            let mut e_int_sum: i64 = 0;
            let mut e_parent = [0u32; 12];
            let mut pinned = false;
            for &m in &members {
                let n = &nodes[m as usize];
                qef.merge(&n.qef);
                chi_sum += n.chi;
                chi_valid &= n.chi_valid;
                pinned |= n.pinned;
                // Octant bits of this child within the parent.
                let o = [
                    (n.cell[0] & 1) as u32,
                    (n.cell[1] & 1) as u32,
                    (n.cell[2] & 1) as u32,
                ];
                for i in 0..12 {
                    if n.e[i] == 0 {
                        continue;
                    }
                    let (axis, q1, q2) = edge_axis_pos(i);
                    let trans: [usize; 2] = match axis {
                        0 => [1, 2],
                        1 => [0, 2],
                        _ => [0, 1],
                    };
                    let on_boundary_1 = o[trans[0]] == q1;
                    let on_boundary_2 = o[trans[1]] == q2;
                    if on_boundary_1 && on_boundary_2 {
                        // Child edge ON a parent edge: same axis,
                        // parent transverse bits = octant bits.
                        let pi = edge_index(axis, o[trans[0]], o[trans[1]]);
                        e_parent[pi] += n.e[i];
                    } else {
                        // Interior scaffold edge (cell centre-line or
                        // face centre-cross): feeds the χ recursion.
                        // Pair-level clustering guarantees every such
                        // crossing's co-owners are in this cluster, so
                        // the ×2/×4 double count of formula 2 is exact.
                        e_int_sum += n.e[i] as i64;
                    }
                }
            }
            debug_assert!(
                members.len() > 1 || e_int_sum == 0,
                "single-child promotion must have no interior intersections \
                 (pair-level connectivity would have merged it)"
            );
            // Formula 2 (χ = Σχ − Σe_int/4). The sum is divisible by 4
            // for any clean patch (V(M^f) is even — Appendix II); a
            // remainder signals a degenerate crossing, which poisons
            // the χ validity instead of mis-counting.
            let chi = chi_sum - e_int_sum / 4;
            let chi_valid = chi_valid && e_int_sum % 4 == 0;

            let mut pos = qef.solve();
            let s = root_size / (1u32 << lvl) as f32;
            let pmin = [
                origin[0] + parent_cell[0] as f32 * s,
                origin[1] + parent_cell[1] as f32 * s,
                origin[2] + parent_cell[2] as f32 * s,
            ];
            pos[0] = pos[0].clamp(pmin[0], pmin[0] + s);
            pos[1] = pos[1].clamp(pmin[1], pmin[1] + s);
            pos[2] = pos[2].clamp(pmin[2], pmin[2] + s);
            let error = qef.residual(pos);

            // Manifold criterion (Prop. 1): χ = 1 and each cell face
            // carries 0 or 2 intersections on its 4 edges.
            let mut faces_ok = true;
            for axis in 0..3 {
                for side in 0..2u32 {
                    // The 4 edges of the face `axis = side`: all edges
                    // whose transverse position on `axis` equals side…
                    // i.e. edges NOT along `axis` whose (q-bit for
                    // axis) == side.
                    let mut total = 0u32;
                    for i in 0..12 {
                        let (ea, q1, q2) = edge_axis_pos(i);
                        if ea == axis {
                            continue;
                        }
                        let trans: [usize; 2] = match ea {
                            0 => [1, 2],
                            1 => [0, 2],
                            _ => [0, 1],
                        };
                        let q_for_axis = if trans[0] == axis { q1 } else { q2 };
                        if q_for_axis == side {
                            total += e_parent[i];
                        }
                    }
                    if total != 0 && total != 2 {
                        faces_ok = false;
                    }
                }
            }
            let collapsible = error <= threshold && chi_valid && chi == 1 && faces_ok;

            let idx = nodes.len() as u32;
            for &m in &members {
                nodes[m as usize].parent = Some(idx);
            }
            nodes.push(VertexNode {
                pos,
                qef,
                chi,
                chi_valid,
                e: e_parent,
                cell: parent_cell,
                level: lvl,
                parent: None,
                collapsible,
                pinned,
            });
        }
    }

    // ── 7. Representatives: topmost collapsible ancestor ───────────
    let mut repr_cache: Vec<u32> = vec![u32::MAX; nodes.len()];
    // Topmost collapsible ancestor. Cached per QUERY vertex only — an
    // interior chain node's own representative differs from the leaf's
    // whenever the topmost collapsible lies below it.
    let mut repr = |start: u32, nodes: &[VertexNode], cache: &mut Vec<u32>| -> u32 {
        if cache[start as usize] != u32::MAX {
            return cache[start as usize];
        }
        let mut best = start;
        let mut cur = start;
        while let Some(p) = nodes[cur as usize].parent {
            if nodes[p as usize].collapsible {
                best = p;
            }
            cur = p;
        }
        cache[start as usize] = best;
        best
    };

    // ── 8. Polygon emission over the finest edges ───────────────────
    let mut out_index: HashMap<u32, u32> = HashMap::new();
    let mut positions: Vec<f32> = Vec::new();
    let mut used_nodes: Vec<u32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let mut emit_vertex = |node_id: u32,
                           nodes: &[VertexNode],
                           out_index: &mut HashMap<u32, u32>,
                           positions: &mut Vec<f32>,
                           used_nodes: &mut Vec<u32>|
     -> u32 {
        *out_index.entry(node_id).or_insert_with(|| {
            let i = (positions.len() / 3) as u32;
            positions.extend_from_slice(&nodes[node_id as usize].pos);
            used_nodes.push(node_id);
            i
        })
    };

    for e in &fine_edges {
        let r: Vec<u32> = e
            .verts
            .iter()
            .map(|&v| repr(v, &nodes, &mut repr_cache))
            .collect();
        // Distinct representatives in cyclic order.
        let (r00, r10, r11, r01) = (r[0], r[1], r[2], r[3]);
        let distinct: Vec<u32> = {
            let mut d = Vec::with_capacity(4);
            for &x in &[r00, r10, r11, r01] {
                if !d.contains(&x) {
                    d.push(x);
                }
            }
            d
        };
        let tris: Vec<[u32; 3]> = match distinct.len() {
            0..=2 => continue,
            3 => {
                // One cyclically-adjacent pair collapsed → triangle.
                // Diagonal collapse (r00==r11 or r10==r01 with 3
                // distinct impossible cyclically unless adjacent — the
                // diagonal case yields a bowtie, discard).
                if r00 == r11 || r10 == r01 {
                    continue;
                }
                vec![[distinct[0], distinct[1], distinct[2]]]
            }
            _ => vec![[r00, r10, r11], [r00, r11, r01]],
        };

        // Geometric winding (Phase 1a): quad normal vs. the piercing
        // direction along the edge axis.
        let p_of = |id: u32| nodes[id as usize].pos;
        let (a0, a1, a2, a3) = (p_of(r00), p_of(r10), p_of(r11), p_of(r01));
        let d1 = [a2[0] - a0[0], a2[1] - a0[1], a2[2] - a0[2]];
        let d2 = [a3[0] - a1[0], a3[1] - a1[1], a3[2] - a1[2]];
        let n_quad = [
            d1[1] * d2[2] - d1[2] * d2[1],
            d1[2] * d2[0] - d1[0] * d2[2],
            d1[0] * d2[1] - d1[1] * d2[0],
        ];
        let want = n_quad[e.axis] * (e.db - e.da);
        let forward = want >= 0.0;

        for t in tris {
            let (x, y, z) = (
                emit_vertex(t[0], &nodes, &mut out_index, &mut positions, &mut used_nodes),
                emit_vertex(t[1], &nodes, &mut out_index, &mut positions, &mut used_nodes),
                emit_vertex(t[2], &nodes, &mut out_index, &mut positions, &mut used_nodes),
            );
            if x == y || y == z || z == x {
                continue;
            }
            if forward {
                indices.extend_from_slice(&[x, y, z]);
            } else {
                indices.extend_from_slice(&[x, z, y]);
            }
        }
    }

    // ── 9. Re-pin used representatives that carry pinned leaves ────
    if !pin.is_empty() {
        let tau = 4.0 * tol_mm + cell;
        let mut occupied: std::collections::HashSet<[i64; 3]> = std::collections::HashSet::new();
        for (out_i, &nid) in used_nodes.iter().enumerate() {
            let n = &nodes[nid as usize];
            if !n.pinned || n.level == level {
                continue; // finest were pinned exactly already
            }
            let p = [
                positions[out_i * 3],
                positions[out_i * 3 + 1],
                positions[out_i * 3 + 2],
            ];
            if let Some(q) = nearest_on_pin(p, pin, tau) {
                let key = [
                    (q[0] * 8192.0).round() as i64,
                    (q[1] * 8192.0).round() as i64,
                    (q[2] * 8192.0).round() as i64,
                ];
                if occupied.insert(key) {
                    positions[out_i * 3] = q[0];
                    positions[out_i * 3 + 1] = q[1];
                    positions[out_i * 3 + 2] = q[2];
                }
            }
        }
    }

    // ── 9b. Conformal sag refinement ────────────────────────────────
    // The QEF threshold bounds the REPRESENTATIVES' distance to the
    // constraint planes, but not the chord sag of a triangle spanning
    // from a huge flat cluster into a curved neighbourhood (measured:
    // a blend bulge growing out of a box face dipped 0.5 mm under a
    // deck-to-bulge chord). Fix the geometry on the finished manifold:
    // any edge whose midpoint leaves the tolerance band is split
    // conformally (memoised per undirected edge, so both incident
    // triangles stay stitched) and the new vertex is Newton-projected
    // onto the surface — or onto the B-Rep edge curve when both ends
    // are pinned. Edge splits preserve 2-manifoldness trivially.
    {
        let out_pinned: Vec<bool> = used_nodes
            .iter()
            .map(|&nid| nodes[nid as usize].pinned)
            .collect();
        sag_refine(
            &mut positions,
            &mut indices,
            out_pinned,
            &sdf,
            node,
            tol_mm,
            cell * 0.5,
            pin,
        );
    }

    // ── 10. Assemble ────────────────────────────────────────────────
    let vert_count = positions.len() / 3;
    let mut normals = vec![0.0f32; vert_count * 3];
    for t in indices.chunks_exact(3) {
        let v = |i: u32| {
            let b = i as usize * 3;
            [positions[b], positions[b + 1], positions[b + 2]]
        };
        let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
        let u1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let u2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let fnorm = [
            u1[1] * u2[2] - u1[2] * u2[1],
            u1[2] * u2[0] - u1[0] * u2[2],
            u1[0] * u2[1] - u1[1] * u2[0],
        ];
        for &i in t {
            let base = i as usize * 3;
            normals[base] += fnorm[0];
            normals[base + 1] += fnorm[1];
            normals[base + 2] += fnorm[2];
        }
    }
    for i in 0..vert_count {
        let b = i * 3;
        let len = (normals[b] * normals[b]
            + normals[b + 1] * normals[b + 1]
            + normals[b + 2] * normals[b + 2])
            .sqrt();
        if len > 1e-20 {
            normals[b] /= len;
            normals[b + 1] /= len;
            normals[b + 2] /= len;
        }
    }

    Ok(AdaptiveResult {
        mesh: Mesh {
            positions,
            normals,
            indices,
            classifications: vec![1u8; vert_count],
            contributors: vec![CONTRIBUTOR_SENTINEL; vert_count * MAX_CONTRIBUTORS],
            face_index: Vec::new(),
        },
        resolution: cells_per_axis,
        actual_cell_mm: cell,
        edges_pinned,
        finest_vertices,
        thin_cells,
    })
}

/// Conformal chord-sag refinement on a closed triangle mesh: split
/// every edge whose midpoint deviates more than `tol` from the surface
/// (min edge length bounds termination), projecting new vertices onto
/// the exact SDF (Newton along the gradient) or, between two pinned
/// vertices, onto the B-Rep edge curves.
#[allow(clippy::too_many_arguments)]
fn sag_refine(
    positions: &mut Vec<f32>,
    indices: &mut Vec<u32>,
    mut pinned: Vec<bool>,
    sdf: &dyn Fn([f32; 3]) -> f32,
    node: &BoundedCsg,
    tol: f32,
    min_edge: f32,
    pin: &PinGeometry,
) {
    const MAX_PASSES: usize = 10;
    let min_edge2 = min_edge * min_edge;

    for _ in 0..MAX_PASSES {
        let vpos = |i: u32, positions: &Vec<f32>| -> [f32; 3] {
            let b = i as usize * 3;
            [positions[b], positions[b + 1], positions[b + 2]]
        };
        // Collect the split set for this pass.
        let mut split_mid: HashMap<(u32, u32), u32> = HashMap::new();
        let tri_count = indices.len() / 3;
        for t in 0..tri_count {
            for e in 0..3 {
                let a = indices[t * 3 + e];
                let b = indices[t * 3 + (e + 1) % 3];
                let key = if a < b { (a, b) } else { (b, a) };
                if split_mid.contains_key(&key) {
                    continue;
                }
                let pa = vpos(a, positions);
                let pb = vpos(b, positions);
                let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < min_edge2 {
                    continue;
                }
                let mid = [
                    (pa[0] + pb[0]) * 0.5,
                    (pa[1] + pb[1]) * 0.5,
                    (pa[2] + pb[2]) * 0.5,
                ];
                if sdf(mid).abs() <= tol {
                    continue;
                }
                // Project the midpoint onto the surface.
                let both_pinned = pinned[a as usize] && pinned[b as usize];
                let target = if both_pinned {
                    nearest_on_pin(mid, pin, tol * 8.0 + min_edge * 4.0)
                } else {
                    None
                };
                let q = target.unwrap_or_else(|| {
                    let mut p = mid;
                    for _ in 0..4 {
                        let dv = sdf(p);
                        if dv.abs() < tol * 0.05 {
                            break;
                        }
                        let g = sdf_normal(node, p, min_edge * 0.5);
                        let g2 = g[0] * g[0] + g[1] * g[1] + g[2] * g[2];
                        if g2 < 1e-12 {
                            break;
                        }
                        p = [p[0] - dv * g[0], p[1] - dv * g[1], p[2] - dv * g[2]];
                    }
                    p
                });
                let vid = (positions.len() / 3) as u32;
                positions.extend_from_slice(&q);
                pinned.push(both_pinned && target.is_some());
                split_mid.insert(key, vid);
            }
        }
        if split_mid.is_empty() {
            return;
        }

        // Re-triangulate conformally (same split patterns as the
        // parametric tessellator's refinement).
        let mut next: Vec<u32> = Vec::with_capacity(indices.len() + split_mid.len() * 6);
        for t in 0..tri_count {
            let [a, b, c] = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]];
            let mid = |x: u32, y: u32| -> Option<u32> {
                split_mid.get(&if x < y { (x, y) } else { (y, x) }).copied()
            };
            match (mid(a, b), mid(b, c), mid(c, a)) {
                (None, None, None) => next.extend_from_slice(&[a, b, c]),
                (Some(m), None, None) => {
                    next.extend_from_slice(&[a, m, c, m, b, c]);
                }
                (None, Some(m), None) => {
                    next.extend_from_slice(&[b, m, a, m, c, a]);
                }
                (None, None, Some(m)) => {
                    next.extend_from_slice(&[c, m, b, m, a, b]);
                }
                (Some(mab), Some(mbc), None) => {
                    next.extend_from_slice(&[b, mbc, mab, a, mab, mbc, a, mbc, c]);
                }
                (None, Some(mbc), Some(mca)) => {
                    next.extend_from_slice(&[c, mca, mbc, b, mbc, mca, b, mca, a]);
                }
                (Some(mab), None, Some(mca)) => {
                    next.extend_from_slice(&[a, mab, mca, c, mca, mab, c, mab, b]);
                }
                (Some(mab), Some(mbc), Some(mca)) => {
                    next.extend_from_slice(&[a, mab, mca, b, mbc, mab, c, mca, mbc, mab, mbc, mca]);
                }
            }
        }
        *indices = next;
    }
}

/// Cell-crossing pin (Phase 1b core, shared shape with the uniform
/// pass): point `i` is snapped when the nearest curve point lies inside
/// its own grid cell. `cell_of(i)` returns (cell-min corner, size).
fn pin_points_to_curves(
    positions: &mut [[f32; 3]],
    flags: &mut [bool],
    cell_of: impl Fn(usize) -> ([f32; 3], f32),
    pin: &PinGeometry,
) -> usize {
    let mut occupied: std::collections::HashSet<[i64; 3]> = std::collections::HashSet::new();
    let mut count = 0usize;
    for i in 0..positions.len() {
        let p = positions[i];
        let (cmin, size) = cell_of(i);
        let slack = size * 0.05;
        let lo = [cmin[0] - slack, cmin[1] - slack, cmin[2] - slack];
        let hi = [
            cmin[0] + size + slack,
            cmin[1] + size + slack,
            cmin[2] + size + slack,
        ];
        let inside = |q: [f32; 3]| (0..3).all(|k| q[k] >= lo[k] && q[k] <= hi[k]);

        // Corners take priority.
        let mut target = pin
            .corners
            .iter()
            .filter(|&&q| inside(q))
            .map(|&q| {
                let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                (q, d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        if target.is_none() {
            let mut best: Option<([f32; 3], f32)> = None;
            for poly in &pin.polylines {
                for w in poly.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
                    let t = if len2 > 1e-20 {
                        (((p[0] - a[0]) * ab[0]
                            + (p[1] - a[1]) * ab[1]
                            + (p[2] - a[2]) * ab[2])
                            / len2)
                            .clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                    if !inside(q) {
                        continue;
                    }
                    let dq = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                    let d2 = dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2];
                    if best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
                        best = Some((q, d2));
                    }
                }
            }
            target = best;
        }
        let Some((q, _)) = target else { continue };
        let key = [
            (q[0] * 8192.0).round() as i64,
            (q[1] * 8192.0).round() as i64,
            (q[2] * 8192.0).round() as i64,
        ];
        if !occupied.insert(key) {
            continue;
        }
        positions[i] = q;
        flags[i] = true;
        count += 1;
    }
    count
}

/// Nearest point on the pin geometry within radius `tau` (no cell
/// constraint — used for the representative re-snap, whose distance is
/// already bounded by the clustering tolerance).
fn nearest_on_pin(p: [f32; 3], pin: &PinGeometry, tau: f32) -> Option<[f32; 3]> {
    let tau2 = tau * tau;
    let mut best: Option<([f32; 3], f32)> = None;
    for c in &pin.corners {
        let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if d2 <= tau2 && best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
            best = Some((*c, d2));
        }
    }
    if best.is_some() {
        return best.map(|(q, _)| q);
    }
    for poly in &pin.polylines {
        for w in poly.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
            let t = if len2 > 1e-20 {
                (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2]) / len2)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
            let dq = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
            let d2 = dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2];
            if d2 <= tau2 && best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
                best = Some((q, d2));
            }
        }
    }
    best.map(|(q, _)| q)
}
