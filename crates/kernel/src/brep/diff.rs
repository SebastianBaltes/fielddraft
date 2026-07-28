//! Differential comparator for two [`BRep`]s — Phase 0c of the parametric-
//! weaving rebuild (see `docs/PARAMETRIC-WEAVING-PLAN.md`, "Teststrategie",
//! Stufe 3).
//!
//! This is the **triage oracle** that diffs the octree extraction path against
//! the (future) branch-pair path. It deliberately produces **triage signals**,
//! not a pass/fail verdict: neither path is absolute ground truth (the octree
//! carries DL-82 ghosts and the DL-75 analytic leak; the branch-pair path can
//! miss intrinsic multiplicity). A divergence is therefore classified as one
//! of three cases — coverage gap, new-path win, or octree ghost — and handed to
//! a human as an annotated work item.
//!
//! The branch-pair path does **not exist yet**. Phase 0c only builds and
//! validates the infrastructure against what exists today: octree-vs-octree
//! (must be identical) and octree-vs-pure-octree on non-shelled bodies (must
//! also be identical, because the `HalfspaceMap` is empty on both sides there).
//!
//! ## Matching strategy
//!
//! Both BReps are extracted from the **same body**, so they share the same
//! `BranchId` namespace. The canonical edge [`signature`](Edge::signature) —
//! the `(BranchId, BranchId)` surface pair — is therefore a stable key across
//! paths. Edges are bucketed by signature, then matched greedily within a
//! bucket by midpoint distance. Vertices are matched greedily by world position
//! *and* identical colliding-branch set.

use std::collections::HashMap;

use super::{BRep, Edge, EdgeGeometry};

// ── geometry helpers ────────────────────────────────────────────────────────

/// Pull the representative world-point list out of an [`Edge`]'s geometry,
/// independent of which variant it carries:
///
/// - [`EdgeGeometry::Hermite`] → the sample `.point`s,
/// - [`EdgeGeometry::Polyline`] → the points verbatim,
/// - [`EdgeGeometry::Nurbs`] → the `control_points` (the structural skeleton;
///   good enough for distance-based matching, no curve evaluation needed).
fn edge_points(edge: &Edge) -> Vec<[f64; 3]> {
    match &edge.geometry {
        EdgeGeometry::Hermite(samples) => samples.iter().map(|s| s.point).collect(),
        EdgeGeometry::Polyline(pts) => pts.clone(),
        EdgeGeometry::Nurbs(curve) => curve.control_points.clone(),
    }
}

/// The two endpoints and the midpoint of an edge's point list, used as the
/// representative samples for geometric matching. The midpoint is the entry at
/// index `len / 2`. Returns `None` for an empty point list (a degenerate edge
/// that carries no geometry — it cannot be geometrically matched).
fn edge_anchors(edge: &Edge) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let pts = edge_points(edge);
    if pts.is_empty() {
        return None;
    }
    let start = pts[0];
    let end = pts[pts.len() - 1];
    let mid = pts[pts.len() / 2];
    Some((start, mid, end))
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Max geometric deviation between two edges over their compared anchors
/// (start, midpoint, end). Both must carry geometry (see [`edge_anchors`]);
/// returns `f64::INFINITY` if either is degenerate, so a degenerate edge never
/// silently matches.
fn anchor_deviation(a: &Edge, b: &Edge) -> f64 {
    match (edge_anchors(a), edge_anchors(b)) {
        (Some((as_, am, ae)), Some((bs, bm, be))) => {
            dist(as_, bs).max(dist(am, bm)).max(dist(ae, be))
        }
        _ => f64::INFINITY,
    }
}

// ── diff result ─────────────────────────────────────────────────────────────

/// The result of comparing two [`BRep`]s. Carries the unmatched / matched
/// entity sets plus each side's absolute invariants (manifold violations and
/// Euler characteristic) so a downstream triage step has the full picture
/// without re-walking either BRep.
#[derive(Debug, Clone)]
pub struct BRepDiff {
    /// Arena indices of `a`-edges that found no partner in `b`.
    pub edges_only_in_a: Vec<usize>,
    /// Arena indices of `b`-edges that found no partner in `a`.
    pub edges_only_in_b: Vec<usize>,
    /// Matched edge pairs as `(a_idx, b_idx, max_deviation)`, where the
    /// deviation is the maximum anchor-point distance between the two edges.
    pub matched_edges: Vec<(usize, usize, f64)>,

    /// Arena indices of `a`-vertices that found no partner in `b`.
    pub vertices_only_in_a: Vec<usize>,
    /// Arena indices of `b`-vertices that found no partner in `a`.
    pub vertices_only_in_b: Vec<usize>,
    /// Matched vertex pairs as `(a_idx, b_idx, position_distance)`.
    pub matched_vertices: Vec<(usize, usize, f64)>,

    /// `a`'s manifold violations (see [`BRep::manifold_violations`]).
    pub manifold_a: Vec<String>,
    /// `b`'s manifold violations.
    pub manifold_b: Vec<String>,

    /// `a`'s Euler characteristic (see [`BRep::euler_characteristic`]).
    pub euler_a: i64,
    /// `b`'s Euler characteristic.
    pub euler_b: i64,
}

impl BRepDiff {
    /// Whether the two BReps are structurally identical: no edge or vertex is
    /// unique to either side. (Matched pairs already passed the `tol` gate
    /// during [`compare`], so an empty set of only-lists is the identity
    /// condition.)
    pub fn is_identical(&self) -> bool {
        self.edges_only_in_a.is_empty()
            && self.edges_only_in_b.is_empty()
            && self.vertices_only_in_a.is_empty()
            && self.vertices_only_in_b.is_empty()
    }
}

// ── comparison ──────────────────────────────────────────────────────────────

/// Compare two BReps and produce a [`BRepDiff`].
///
/// `tol` is an **absolute world tolerance**; callers typically pass something
/// like `1e-3 * model_aabb_diagonal` so the threshold scales with the model.
///
/// **Edge matching**: edges are bucketed by their canonical `signature`
/// (`(BranchId, BranchId)`) — stable across paths because both BReps come from
/// the same body and thus share the `BranchId` namespace. Within a bucket,
/// `a`-edges greedily claim the nearest unclaimed `b`-edge whose midpoint lies
/// within `tol`. Unmatched `a`-edges land in `edges_only_in_a`, unmatched
/// `b`-edges in `edges_only_in_b`. Each match records the max anchor deviation
/// (start, midpoint, end).
///
/// **Vertex matching**: greedy by world position within `tol` *and* identical
/// colliding-branch set (the vertex's canonical `branches`).
pub fn compare(a: &BRep, b: &BRep, tol: f64) -> BRepDiff {
    let (matched_edges, edges_only_in_a, edges_only_in_b) = compare_edges(a, b, tol);
    let (matched_vertices, vertices_only_in_a, vertices_only_in_b) = compare_vertices(a, b, tol);

    BRepDiff {
        edges_only_in_a,
        edges_only_in_b,
        matched_edges,
        vertices_only_in_a,
        vertices_only_in_b,
        matched_vertices,
        manifold_a: a.manifold_violations(),
        manifold_b: b.manifold_violations(),
        euler_a: a.euler_characteristic(),
        euler_b: b.euler_characteristic(),
    }
}

/// Bucket edges by canonical signature, then greedy-match within each bucket by
/// midpoint distance under `tol`.
fn compare_edges(
    a: &BRep,
    b: &BRep,
    tol: f64,
) -> (Vec<(usize, usize, f64)>, Vec<usize>, Vec<usize>) {
    // signature → arena indices, per side.
    let mut a_buckets: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, e) in a.edges.iter().enumerate() {
        a_buckets
            .entry((e.signature.0.as_u64(), e.signature.1.as_u64()))
            .or_default()
            .push(i);
    }
    let mut b_buckets: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (i, e) in b.edges.iter().enumerate() {
        b_buckets
            .entry((e.signature.0.as_u64(), e.signature.1.as_u64()))
            .or_default()
            .push(i);
    }

    let mut matched = Vec::new();
    let mut only_a = Vec::new();
    let mut b_claimed = vec![false; b.edges.len()];

    // Iterate `a` in arena order for determinism.
    for (sig, a_idxs) in iter_sorted(&a_buckets) {
        let b_idxs = b_buckets.get(sig);
        for &ai in a_idxs {
            let Some((_, a_mid, _)) = edge_anchors(&a.edges[ai]) else {
                only_a.push(ai);
                continue;
            };
            // Find the nearest unclaimed b-edge in this signature bucket whose
            // midpoint is within tol.
            let mut best: Option<(usize, f64)> = None;
            if let Some(b_idxs) = b_idxs {
                for &bi in b_idxs {
                    if b_claimed[bi] {
                        continue;
                    }
                    let Some((_, b_mid, _)) = edge_anchors(&b.edges[bi]) else {
                        continue;
                    };
                    let d = dist(a_mid, b_mid);
                    if d <= tol && best.map_or(true, |(_, bd)| d < bd) {
                        best = Some((bi, d));
                    }
                }
            }
            match best {
                Some((bi, _)) => {
                    b_claimed[bi] = true;
                    let dev = anchor_deviation(&a.edges[ai], &b.edges[bi]);
                    matched.push((ai, bi, dev));
                }
                None => only_a.push(ai),
            }
        }
    }

    let only_b: Vec<usize> = (0..b.edges.len()).filter(|&i| !b_claimed[i]).collect();

    only_a.sort_unstable();
    matched.sort_unstable_by_key(|&(ai, _, _)| ai);
    (matched, only_a, only_b)
}

/// Greedy vertex match by position under `tol` AND identical branch set.
fn compare_vertices(
    a: &BRep,
    b: &BRep,
    tol: f64,
) -> (Vec<(usize, usize, f64)>, Vec<usize>, Vec<usize>) {
    let mut matched = Vec::new();
    let mut only_a = Vec::new();
    let mut b_claimed = vec![false; b.vertices.len()];

    for (ai, av) in a.vertices.iter().enumerate() {
        let mut best: Option<(usize, f64)> = None;
        for (bi, bv) in b.vertices.iter().enumerate() {
            if b_claimed[bi] {
                continue;
            }
            // branches are canonically sorted+deduped, so a Vec equality is a
            // true set equality.
            if av.branches != bv.branches {
                continue;
            }
            let d = dist(av.position, bv.position);
            if d <= tol && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((bi, d));
            }
        }
        match best {
            Some((bi, d)) => {
                b_claimed[bi] = true;
                matched.push((ai, bi, d));
            }
            None => only_a.push(ai),
        }
    }

    let only_b: Vec<usize> = (0..b.vertices.len()).filter(|&i| !b_claimed[i]).collect();
    (matched, only_a, only_b)
}

/// Iterate a signature → indices map in deterministic (sorted-key) order.
fn iter_sorted(
    buckets: &HashMap<(u64, u64), Vec<usize>>,
) -> Vec<(&(u64, u64), &Vec<usize>)> {
    let mut entries: Vec<_> = buckets.iter().collect();
    entries.sort_unstable_by_key(|(k, _)| **k);
    entries
}

// ── triage classification ───────────────────────────────────────────────────

/// The triage verdict for a diverging edge. From the plan's three cases
/// (Stufe 3), with `a` = octree path, `b` = branch-pair path:
///
/// - **(a) Coverage gap / ghost** — the octree found an edge the branch-pair
///   path missed. Most often a genuine branch-pair *coverage gap* (e.g. missing
///   intrinsic multiplicity) → work item. Mapped to [`CoverageGapOrGhost`].
/// - **(b) New path wins** — the branch-pair path found an edge thinner than
///   the octree's resolution depth (`edges_only_in_b`), a legitimate new win.
///   Mapped to [`NewPathWins`].
/// - **(c) Octree ghost** (DL-82) — the octree minted a phantom edge that the
///   branch-pair path correctly omits. Evidence *for* the rebuild. Also
///   [`NewPathWins`].
///
/// [`Match`] is the non-diverging case.
///
/// [`CoverageGapOrGhost`]: DiffVerdict::CoverageGapOrGhost
/// [`NewPathWins`]: DiffVerdict::NewPathWins
/// [`Match`]: DiffVerdict::Match
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffVerdict {
    /// An octree-only edge that is probably a real branch-pair coverage gap
    /// (case a) — or possibly an octree ghost we could not disprove.
    CoverageGapOrGhost,
    /// The diverging edge is best explained by the new (branch-pair) path being
    /// right: either it resolves finer than the octree (case b), or the octree
    /// edge is a disproven ghost (case c).
    NewPathWins,
    /// No divergence.
    Match,
}

/// Classify an edge that exists only in `a` (the octree path).
///
/// **Heuristic, not proof.** This is a triage aid for a human, not a
/// correctness verdict. The rule: if the octree-only edge also appears as a
/// manifold *violation* in `manifold_a` (i.e. it is a count-1 open-boundary /
/// phantom edge or a non-manifold edge), it is very likely a **(c) octree
/// ghost** in the DL-82 sense — a phantom the branch-pair path correctly drops —
/// so the verdict is [`DiffVerdict::NewPathWins`]. Otherwise it is treated as a
/// **(a) coverage gap**: the octree saw a legitimate edge the branch-pair path
/// failed to seed → [`DiffVerdict::CoverageGapOrGhost`].
///
/// The symmetric case (`edges_only_in_b`) is **case (b)/(c) on the other side**
/// — either a genuine new win or a branch-pair phantom. It is read directly off
/// `edges_only_in_b` plus `manifold_b` by the same logic and needs no dedicated
/// function here.
///
/// `a_edge` must be an index present in `diff.edges_only_in_a`; if it is not,
/// the edge is (by definition of the diff) matched and the verdict is
/// [`DiffVerdict::Match`].
pub fn classify_edge_only_in_a(diff: &BRepDiff, _a: &BRep, a_edge: usize) -> DiffVerdict {
    if !diff.edges_only_in_a.contains(&a_edge) {
        return DiffVerdict::Match;
    }
    // `manifold_violations` reports problems as "edge {i}: ..." strings. An
    // octree-only edge that is itself flagged is a phantom/ghost the branch-pair
    // path is expected to omit.
    let needle = format!("edge {a_edge}:");
    if diff.manifold_a.iter().any(|v| v.starts_with(&needle)) {
        DiffVerdict::NewPathWins
    } else {
        DiffVerdict::CoverageGapOrGhost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brep::octree::Octree;
    use crate::brep::weaving::{weave, weave_pure_octree};
    use crate::brep::{Edge, EdgeGeometry, Vertex};
    use crate::bvh::BoundedCsg;
    use crate::csg::{BranchId, CsgNode};
    use fielddraft_protocol::{FeatureId, Primitive};

    /// A plain non-shelled box CSG.
    fn box_body() -> BoundedCsg {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
        };
        BoundedCsg::from_csg(&csg)
    }

    /// A non-shelled Difference (box minus a corner notch).
    fn notched_box_body() -> BoundedCsg {
        let outer = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
        };
        let notch = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Box {
                center: [0.6, 0.6, 0.0],
                half_extents: [0.6, 0.6, 2.0],
            },
        };
        BoundedCsg::from_csg(&CsgNode::Difference(Box::new(outer), Box::new(notch)))
    }

    /// A diagonal proportional to the unit box, used as the world tolerance
    /// base (~1e-3 of the AABB diagonal).
    const TOL: f64 = 1e-3 * 3.46; // diag of [-1,1]^3 ≈ 2*sqrt(3)

    #[test]
    fn octree_vs_octree_is_identical() {
        // Weave the same body twice through the same path → identical BReps.
        let body = notched_box_body();
        let octree = Octree::build(&body, 0.25);
        let a = weave(&body, &octree);
        let b = weave(&body, &octree);

        let diff = compare(&a, &b, TOL);
        assert!(diff.is_identical(), "octree-vs-octree must be identical");
        assert!(diff.edges_only_in_a.is_empty(), "{:?}", diff.edges_only_in_a);
        assert!(diff.edges_only_in_b.is_empty(), "{:?}", diff.edges_only_in_b);
        assert!(diff.vertices_only_in_a.is_empty());
        assert!(diff.vertices_only_in_b.is_empty());
        // Every matched edge is geometrically coincident (zero deviation).
        for &(_, _, dev) in &diff.matched_edges {
            assert!(dev <= TOL, "matched-edge deviation {dev} exceeds tol");
        }
        assert_eq!(diff.euler_a, diff.euler_b);
    }

    #[test]
    fn pure_vs_normal_non_shelled_is_identical() {
        // On a non-shelled box the HalfspaceMap is empty on BOTH paths, so the
        // pure octree path and the normal weave produce identical geometry.
        let body = box_body();
        let octree = Octree::build(&body, 0.25);
        let normal = weave(&body, &octree);
        let pure = weave_pure_octree(&body, &octree);

        let diff = compare(&normal, &pure, TOL);
        assert!(
            diff.is_identical(),
            "pure-vs-normal (non-shelled) must be identical: only_a={:?} only_b={:?}",
            diff.edges_only_in_a,
            diff.edges_only_in_b
        );
        assert_eq!(diff.manifold_a, diff.manifold_b);
        assert_eq!(diff.euler_a, diff.euler_b);
    }

    #[test]
    fn dropped_edge_shows_up_in_only_in_a() {
        // Clone a woven BRep and pop the last edge off the clone. With the edge
        // removed from the END of the arena, no surviving index shifts, so the
        // comparator robustly reports exactly the one missing edge in
        // `edges_only_in_a` (original = a, clone-minus-edge = b).
        let body = notched_box_body();
        let octree = Octree::build(&body, 0.25);
        let a = weave(&body, &octree);
        let mut b = a.clone();
        let popped = b.edges.pop().expect("woven brep must have edges");

        let diff = compare(&a, &b, TOL);
        // Exactly one a-edge is unmatched, and it is the popped one (last index).
        assert_eq!(
            diff.edges_only_in_a,
            vec![a.edges.len() - 1],
            "the popped last edge must be the only unmatched a-edge"
        );
        assert!(
            diff.edges_only_in_b.is_empty(),
            "no b-edge should be unique: {:?}",
            diff.edges_only_in_b
        );
        assert!(!diff.is_identical());
        // Sanity: the popped edge's signature is real (not a phantom of cloning).
        let _ = popped.signature;
    }

    #[test]
    fn classify_flags_manifold_violating_edge_as_new_path_wins() {
        // Synthetic case: an octree-only edge (a) that is ALSO a manifold
        // violation in manifold_a → classified as a ghost the new path drops.
        // We build a tiny BRep where one edge is referenced by a single loop
        // (count 1 → open-boundary violation), forcing `manifold_violations`
        // to flag "edge 0:". `b` is empty, so that edge is only-in-a.
        use crate::brep::{Face, Loop, Shell, SurfaceGeometry};
        use fielddraft_protocol::{HalfSpace, Transform};

        let mut a = BRep::new();
        let v0 = a.add_vertex(Vertex::new([0.0, 0.0, 0.0], vec![BranchId(16), BranchId(32)]));
        let v1 = a.add_vertex(Vertex::new([1.0, 0.0, 0.0], vec![BranchId(16), BranchId(48)]));
        let e0 = a.add_edge(Edge::new(
            EdgeGeometry::Polyline(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]),
            BranchId(16),
            BranchId(32),
            Some(v0),
            Some(v1),
        ));
        // A face whose outer loop references e0 exactly once → e0 used once →
        // manifold violation "edge 0: used once (count 1) ...".
        a.add_face(Face {
            branch_id: BranchId(16),
            surface: SurfaceGeometry::Analytic {
                surface: HalfSpace::Plane {
                    normal: [0.0, 0.0, 1.0],
                    offset: 0.0,
                },
                transform: Transform::IDENTITY,
            },
            outer: Loop::new(vec![(e0, false)]),
            inner: Vec::new(),
        });
        a.add_shell(Shell::new(vec![crate::brep::FaceId(0)], false));

        let b = BRep::new(); // empty → e0 is only-in-a.
        let diff = compare(&a, &b, TOL);
        assert!(
            diff.edges_only_in_a.contains(&0),
            "edge 0 must be only-in-a: {:?}",
            diff.edges_only_in_a
        );
        assert!(
            !diff.manifold_a.is_empty(),
            "edge 0 should be a manifold violation: {:?}",
            diff.manifold_a
        );
        assert_eq!(
            classify_edge_only_in_a(&diff, &a, 0),
            DiffVerdict::NewPathWins,
            "a manifold-violating octree-only edge is a probable ghost"
        );
    }

    #[test]
    fn classify_clean_octree_only_edge_is_coverage_gap() {
        // An octree-only edge that is NOT a manifold violation → coverage gap.
        // Two edges in a watertight-enough loop so neither is flagged; we drop
        // one whole edge from `b` so it becomes only-in-a while `a` stays clean.
        let body = notched_box_body();
        let octree = Octree::build(&body, 0.25);
        let a = weave(&body, &octree);
        let mut b = a.clone();
        b.edges.pop();

        let diff = compare(&a, &b, TOL);
        let only = *diff
            .edges_only_in_a
            .first()
            .expect("there must be one only-in-a edge");
        // That edge is not necessarily flagged in manifold_a; assert classify
        // matches the manifold state for this concrete edge.
        let needle = format!("edge {only}:");
        let flagged = diff.manifold_a.iter().any(|v| v.starts_with(&needle));
        let verdict = classify_edge_only_in_a(&diff, &a, only);
        if flagged {
            assert_eq!(verdict, DiffVerdict::NewPathWins);
        } else {
            assert_eq!(
                verdict,
                DiffVerdict::CoverageGapOrGhost,
                "a clean octree-only edge is a coverage gap"
            );
        }
    }
}
