//! Branch-seed weaver — Phase 2b of the Parametric-Weaving rebuild
//! (`docs/PARAMETRIC-WEAVING-PLAN.md`).
//!
//! The analytic counterpart to the octree route ([`crate::brep::weaving::weave`]).
//! Where the octree weaver samples a narrow band to *find* edges, this weaver
//! seeds edges directly from CSG branch adjacency
//! ([`crate::brep::branch_seed::seed_branch_pairs`]), traces each one with the
//! active-interval tracer ([`crate::brep::edge_trace::trace_edge`] +
//! `both_branches_active` as the active predicate), deduplicates the trim
//! boundaries into corner vertices, and hands the resulting wireframe to the
//! shared [`crate::brep::weaving::finish_weave`]. The downstream face grouping,
//! loop ordering, watertightness and stable `id_hash` naming are byte-identical
//! to the octree route — only the wireframe's *provenance* differs.
//!
//! This module replaces the four hand-written `weave_parametric` special cases
//! (box / cylinder / edgeless) with one general pipeline that also covers
//! Boolean combinations and analytic sweeps.
//!
//! Two entry modes (see [`SeedMode`]):
//!
//! - **Strict** ([`weave_branch_seeds`] / [`seed_wireframe`]): all-or-nothing —
//!   a body is either fully seed-woven or fully octree-woven (see
//!   [`scope_supported`] and the dynamic gates in `build_seed_wireframe`), so the
//!   `id_hash` toggle-invariance contract is preserved for the bodies it fully
//!   covers. This is the path `weave_parametric` chains.
//! - **Graceful** ([`weave_branch_seeds_partial`], tickets 7vbe/qmvn): when the
//!   strict path bails on a single unresolvable transition seam, this keeps every
//!   branch pair that *did* resolve — each operand's own analytic edges plus the
//!   analytically resolvable Boolean boundary (SSI cross-pair) edges — and skips
//!   only the genuine transition seams. The result is a *partial*, non-watertight
//!   wireframe; it is used solely as a fallback *after* strict returned `None`
//!   (in [`crate::body::Body::ensure_brep`]), so the strict contract is untouched.

use std::collections::HashMap;

use crate::brep::branch_seed::{seed_branch_pairs, SeedResult};
use crate::brep::edge_trace::{
    edge_tangent, project_to_edge, trace_edge, EdgeEnd, Field, TraceParams,
};
use crate::brep::edge_walker::{
    body_has_warp, branch_field, branch_leaf_map, cluster_positions,
    collect_non_lipschitz_features, extrude_rim_corner, near_traced, refine_triple_vertex,
    root_displace, root_warp_chain, traced_to_hermite, TripleRefine,
};
use crate::sdf_dual::{unwarp_chain, warp_chain, DomainWarp};
use crate::brep::extraction::both_branches_active;
use crate::brep::vec3::{add, dist as dist3, dot, normalize, scale, sub};
use crate::brep::weaving::{branch_halfspace_map, finish_weave};
use crate::brep::{BRep, Edge, EdgeGeometry, HermiteSample, Vertex, VertexId};
use crate::csg::BranchId;
use crate::bvh::BoundedCsg;

/// Canonical `(lo, hi)` signature key of a branch pair — the identity the
/// Stufe-C octree fill matches edges on.
fn pair_key(a: BranchId, b: BranchId) -> (u32, u32) {
    let (x, y) = (a.as_u32(), b.as_u32());
    if x <= y { (x, y) } else { (y, x) }
}

/// A partial seed wireframe plus the canonical signatures of the transition
/// seams `Partial` mode had to skip — the work list for the Stufe-C octree
/// fill ([`weave_branch_seeds_hybrid`]).
struct SeedWireframe {
    brep: BRep,
    skipped: std::collections::HashSet<(u32, u32)>,
}

/// How `build_seed_wireframe` reacts to a branch pair / corner the analytic
/// pipeline cannot resolve cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeedMode {
    /// All-or-nothing: the FIRST unresolvable seam aborts the whole wireframe
    /// (`None`) → the caller octree-weaves the entire body. This preserves the
    /// `id_hash` toggle-invariance contract for the bodies the seed path fully
    /// covers (every existing `seed_*` test asserts handle-identity with the
    /// octree, so the strict output must never change).
    Strict,
    /// Graceful: an unresolvable seam is SKIPPED rather than aborting. The
    /// resolvable operand edges and the analytically-resolvable Boolean boundary
    /// edges are still emitted; only the genuine transition seams are absent, so
    /// the resulting wireframe is *partial* (a non-watertight shell). Used only
    /// as a fallback after [`SeedMode::Strict`] has already returned `None`, so
    /// the strict contract is untouched (tickets 7vbe/qmvn).
    Partial,
}

/// Weave `body` analytically from branch-pair seeds, or return `None` to fall
/// back to the octree route ([`crate::brep::weaving::weave`]).
///
/// `None` is returned for any body outside the analytic scope ([`scope_supported`])
/// or that the seed pipeline cannot fully resolve (unresolved branch pairs, a
/// stalled march, or a spurious corner — see `build_seed_wireframe`). The
/// caller treats `None` as "use the octree".
pub fn weave_branch_seeds(body: &BoundedCsg) -> Option<BRep> {
    Some(finish_weave(seed_wireframe(body)?, body))
}

/// **Graceful** seed weave (Stufe A+B, tickets 7vbe/qmvn): when the strict
/// all-or-nothing seed path ([`weave_branch_seeds`]) bails on a single
/// unresolvable transition seam, this emits the wireframe of every branch pair
/// that *did* resolve — each operand's own analytic edges PLUS the analytically
/// resolvable Boolean boundary (SSI cross-pair) edges — and skips only the
/// genuinely unresolvable seams.
///
/// The result is a *partial*, non-watertight B-Rep: it carries the parametric
/// single-operand and analytic-boundary edges the user wants to see, while the
/// genuine transition zones simply have no edges. Consumers that may build an
/// octree should prefer [`weave_branch_seeds_hybrid`] (Stufe C), which fills
/// exactly those zones; this partial entry remains for octree-forbidden bodies
/// (space-filling TPMS). Returns `None` only for out-of-scope bodies or when
/// not a single seam resolved (→ whole-body octree).
pub fn weave_branch_seeds_partial(body: &BoundedCsg) -> Option<BRep> {
    if !scope_supported_partial(body) {
        return None;
    }
    let wireframe = build_seed_wireframe(body, SeedMode::Partial)?;
    // Nothing resolved → no graceful gain over the octree; let the caller octree.
    if wireframe.edges.is_empty() && wireframe.vertices.is_empty() {
        return None;
    }
    Some(finish_weave(wireframe, body))
}

/// **Hybrid** seed weave — Stufe C (`docs/PARAMETRIC-WEAVING-PLAN.md`): the
/// graceful partial weave keeps every analytically resolvable seam at
/// parametric precision, and the transition seams it had to skip are filled
/// from the octree wireframe — only the skipped pairs' edges are consumed,
/// their endpoint vertices welded into the seed wireframe's corners. The
/// merged wireframe then assembles through the shared [`finish_weave`], so
/// the result is a COMPLETE B-Rep (box edges exact, torus transition seams
/// octree-quality) instead of the partial route's open shell.
///
/// Any analytic leftovers of a skipped pair (fragments from seeds of the same
/// pair that did trace before the skip) are dropped first — a skipped pair is
/// owned wholesale by the octree, never stitched half-and-half.
///
/// `min_cell` is the octree resolution the caller would have used for the
/// full octree fallback ([`crate::body::topology_min_cell_size`]). Returns
/// `None` exactly like [`weave_branch_seeds_partial`] (out-of-scope body or
/// nothing resolved → whole-body octree).
pub fn weave_branch_seeds_hybrid(body: &BoundedCsg, min_cell: f32) -> Option<BRep> {
    if !scope_supported_partial(body) {
        return None;
    }
    let SeedWireframe { mut brep, skipped } =
        build_seed_wireframe_tracked(body, SeedMode::Partial)?;
    if brep.edges.is_empty() && brep.vertices.is_empty() {
        return None;
    }
    if skipped.is_empty() {
        return Some(finish_weave(brep, body));
    }
    if std::env::var("SEED_WEAVER_DEBUG").is_ok() {
        eprintln!("SEED-WEAVER hybrid: octree fill for skipped pairs {skipped:?}");
    }

    drop_pair_edges(&mut brep, &skipped);

    let octree = crate::brep::octree::Octree::build(body, min_cell);
    let oct = crate::brep::edge_walker::build_wireframe(body, &octree);
    let weld = crate::brep::edge_walker::VERTEX_CLUSTER_GAP_FACTOR
        * crate::brep::edge_walker::max_vertex_cell_size(&octree);
    merge_octree_fill(&mut brep, &oct, &skipped, weld);

    Some(finish_weave(brep, body))
}

/// Remove every edge whose canonical signature is in `skipped`, compacting
/// away vertices that lose all references (the arena remap keeps the kept
/// edges' endpoint ids valid).
fn drop_pair_edges(brep: &mut BRep, skipped: &std::collections::HashSet<(u32, u32)>) {
    let kept: Vec<Edge> = brep
        .edges
        .drain(..)
        .filter(|e| !skipped.contains(&pair_key(e.signature.0, e.signature.1)))
        .collect();
    let mut used = vec![false; brep.vertices.len()];
    for e in &kept {
        if let Some(v) = e.start {
            used[v.0 as usize] = true;
        }
        if let Some(v) = e.end {
            used[v.0 as usize] = true;
        }
    }
    let old: Vec<Vertex> = brep.vertices.drain(..).collect();
    let mut remap: Vec<Option<VertexId>> = vec![None; old.len()];
    for (i, v) in old.into_iter().enumerate() {
        if used[i] {
            remap[i] = Some(brep.add_vertex(v));
        }
    }
    for mut e in kept {
        e.start = e.start.and_then(|v| remap[v.0 as usize]);
        e.end = e.end.and_then(|v| remap[v.0 as usize]);
        brep.add_edge(e);
    }
}

/// Append the octree wireframe's edges of the `skipped` signatures to `brep`,
/// welding their endpoint vertices into existing seed corners within `weld`
/// (branch sets union — the seed corner learns the filled pair's branches, so
/// its final branch set matches what a full octree weave would carry there).
fn merge_octree_fill(
    brep: &mut BRep,
    oct: &BRep,
    skipped: &std::collections::HashSet<(u32, u32)>,
    weld: f64,
) {
    let mut vmap: HashMap<u32, VertexId> = HashMap::new();
    // Deterministic fill order: octree arena order is deterministic per body.
    for e in &oct.edges {
        if !skipped.contains(&pair_key(e.signature.0, e.signature.1)) {
            continue;
        }
        let mut map_end = |end: Option<VertexId>, brep: &mut BRep| -> Option<VertexId> {
            let vid = end?;
            if let Some(&mapped) = vmap.get(&vid.0) {
                return Some(mapped);
            }
            let ov = &oct.vertices[vid.0 as usize];
            let mut best: Option<(f64, VertexId)> = None;
            for (i, v) in brep.vertices.iter().enumerate() {
                let d = dist3(v.position, ov.position);
                if d <= weld && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, VertexId(i as u32)));
                }
            }
            let mapped = match best {
                Some((_, id)) => {
                    // Union the branch sets (canonicalised by Vertex::new).
                    let existing = &brep.vertices[id.0 as usize];
                    let mut branches = existing.branches.clone();
                    branches.extend(ov.branches.iter().copied());
                    brep.vertices[id.0 as usize] =
                        Vertex::new(existing.position, branches);
                    id
                }
                None => brep.add_vertex(ov.clone()),
            };
            vmap.insert(vid.0, mapped);
            Some(mapped)
        };
        let start = map_end(e.start, brep);
        let end = map_end(e.end, brep);
        brep.add_edge(Edge::new(
            e.geometry.clone(),
            e.signature.0,
            e.signature.1,
            start,
            end,
        ));
    }
}

/// The scope-gated *raw* seed wireframe (deduplicated corner vertices + signed
/// traced edges), before [`finish_weave`]. Returns `None` for out-of-scope or
/// unresolvable bodies (→ octree). This is the form [`weave_parametric`] chains
/// after its exact grundform fast-paths; [`weave_branch_seeds`] is the
/// standalone entry that also runs `finish_weave`.
///
/// [`weave_parametric`]: crate::brep::weaving::weave_parametric
pub(crate) fn seed_wireframe(body: &BoundedCsg) -> Option<BRep> {
    if !scope_supported(body) {
        return None;
    }
    build_seed_wireframe(body, SeedMode::Strict)
}

/// Static structural scope gate. Returns `false` (→ octree) for bodies the
/// analytic branch-seed path provably cannot handle:
///
/// - **Domain warps** (Twist / Bend / Displace): [`branch_halfspace_map`] and
///   the branch fields describe the *un-warped* primitive, so seeds and traces
///   would lie on the wrong surface (the same reason the octree route disables
///   its analytic fast-path under a warp).
/// - **Non-Lipschitz features** (Thread / Gyroid): their field is not a clean
///   Euclidean distance, so neither the SSI seeding nor the tracer can condition
///   an edge there.
///
/// Everything else — analytic primitives, Booleans over them, and analytic
/// Extrude/Revolve sweeps — is admitted here; the *dynamic* gates in
/// `build_seed_wireframe` reject anything that still fails to resolve at trace
/// time.
///
/// [`branch_halfspace_map`]: crate::brep::weaving::branch_halfspace_map
fn scope_supported(body: &BoundedCsg) -> bool {
    if body_has_warp(body) {
        return false;
    }
    let mut non_lipschitz = std::collections::HashSet::new();
    collect_non_lipschitz_features(body, &mut non_lipschitz);
    if !non_lipschitz.is_empty() {
        return false;
    }
    true
}

/// Scope gate of the PARTIAL path: like [`scope_supported`], but a **gyroid**
/// is admitted — its Boolean rim seams seed via SSI (gyroid as implicit
/// operand) and trace via the dual field, both proven; every seam a gyroid
/// pair cannot resolve is gracefully skipped by `Partial` mode anyway. The
/// other non-Lipschitz features (Thread / Helix / Sweep) stay rejected: they
/// have no analytic half-space carrier at all, so nothing would resolve.
fn scope_supported_partial(body: &BoundedCsg) -> bool {
    if has_non_gyroid_non_lipschitz(body) {
        return false;
    }
    // Domain warps: admitted when the WHOLE body is one un-warped CSG under a
    // root Twist/Bend chain ([`root_warp_chain`]) — the SSI seeds and corner
    // Newton then run in the un-warped space and everything is pushed forward
    // through the warp's point map W. A root Displace chain
    // ([`root_displace`]) is a VALUE perturbation with no point map, but the
    // seam of two faces is the common zero of the two DISPLACED per-face
    // fields ([`Field::DisplacedAnalytic`]): the un-displaced SSI seeds land
    // within Newton range (|disp| ≤ Σ depth) and `project_to_edge` pulls them
    // on. Partial warps (inside a Boolean) and mixed Twist/Bend-under-Displace
    // keep the octree fallback.
    !body_has_warp(body) || root_warp_chain(body).is_some() || root_displace(body).is_some()
}

/// Any Thread leaf, Helix or Sweep under `node` (the non-Lipschitz features
/// WITHOUT an analytic carrier — gyroids are deliberately not counted).
fn has_non_gyroid_non_lipschitz(node: &BoundedCsg) -> bool {
    use fielddraft_protocol::Primitive;
    match node {
        BoundedCsg::Leaf { primitive, .. } => {
            primitive.is_non_lipschitz() && !matches!(primitive, Primitive::Gyroid { .. })
        }
        BoundedCsg::Helix { .. } | BoundedCsg::Sweep { .. } => true,
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::MaskedBlend { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            has_non_gyroid_non_lipschitz(left) || has_non_gyroid_non_lipschitz(right)
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => has_non_gyroid_non_lipschitz(inner),
        BoundedCsg::Extrude { .. } | BoundedCsg::Revolve { .. } | BoundedCsg::Empty => false,
    }
}

// Curvature-adaptive trace fractions — shared with the octree route via
// `brep::tol`: a seed-woven edge and an octree-woven one of the same geometry
// must sample to the same chord fidelity.
use super::tol::{ADAPT_MAX_FRACTION, ADAPT_MIN_FRACTION, ADAPT_TOL_FRACTION};

/// One traced seam edge, captured *after* its Hermite conversion so we no longer
/// hold the borrowing `Field`s (which borrow `leaf_map`). Open edges carry their
/// `(start, end)` trim termini in `ends`; a `closed` ring carries none.
struct EdgeRecord {
    a: BranchId,
    b: BranchId,
    samples: Vec<HermiteSample>,
    closed: bool,
    /// `ends.0` is the BACKWARD terminus (`points.first()`), `ends.1` the
    /// FORWARD one (`points.last()`) — see [`trace_edge`]. Preserved here so the
    /// trim→vertex mapping assigns start/end on the matching sides.
    ends: (EdgeEnd, EdgeEnd),
}

/// Largest axis-aligned extent of the positions named by `members` (indices
/// into `all`) — an O(n) upper bound on the cluster's true pairwise diameter,
/// used to decide whether a still-`Spurious` cluster can be split any further.
fn group_diameter(all: &[[f64; 3]], members: &[usize]) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for &m in members {
        let p = all[m];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Build the wireframe `BRep` (deduplicated corner vertices + signed traced
/// edges) for an in-scope `body`, or `None` if the analytic pipeline cannot
/// fully resolve it (handed off to the octree).
///
/// Pipeline (Phase 2b): seed every branch pair, trace each seed with the
/// active-interval tracer, fuse co-incident trim termini into corner vertices
/// (re-solved analytically by `refine_triple_vertex`), and emit signed edges.
/// Any condition the analytic path cannot lift cleanly returns `None` so the
/// caller falls back to the whole-body octree weave.
fn build_seed_wireframe(body: &BoundedCsg, mode: SeedMode) -> Option<BRep> {
    build_seed_wireframe_tracked(body, mode).map(|sw| sw.brep)
}

fn build_seed_wireframe_tracked(body: &BoundedCsg, mode: SeedMode) -> Option<SeedWireframe> {
    let mut skipped: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    // 1. Seed every candidate branch pair. An unresolved pair is one whose field
    //    we cannot lift analytically (missing half-space / SSI `Unsupported`) —
    //    the analytic route cannot trace it. In `Strict` mode the WHOLE body goes
    //    to octree (the all-or-nothing contract that keeps `id_hash` toggle-
    //    invariant); in `Partial` mode the unresolved pairs are simply absent
    //    from `seeds` (they never produced one), so we just proceed — every pair
    //    that DID seed is still woven, only the unresolvable seams are skipped.
    let SeedResult { mut seeds, unresolved } = seed_branch_pairs(body);
    if mode == SeedMode::Strict && !unresolved.is_empty() {
        return None;
    }
    // Domain-warp chain (Twist/Bend at the body root). Non-empty only on the
    // Partial route (`scope_supported` rejects warps for Strict). The SSI
    // seeds, the corner Newton and the analytic half-space map all describe
    // the UN-warped geometry; traces, witnesses and the emitted wireframe live
    // in the WARPED world. `warp_chain` (W) maps un-warped → warped,
    // `unwarp_chain` (T) the reverse — see [`crate::sdf_dual::DomainWarp`].
    let warp: Vec<DomainWarp> = root_warp_chain(body).unwrap_or_default();
    // Root displace chain (mutually exclusive with `warp`: `root_warp_chain`
    // rejects any Displace, `root_displace` rejects any Twist/Bend). Seeds
    // need no forward map — the traces run on the DISPLACED per-face fields
    // and `project_to_edge` pulls each un-displaced seed onto the real seam.
    let disp: Vec<(f64, f64)> = root_displace(body).unwrap_or_default();
    // SSI seeds lie on the un-warped carriers — push them forward through W
    // onto the real (warped) edges before tracing / co-component dedup.
    if !warp.is_empty() {
        for s in &mut seeds {
            let w = warp_chain(
                &warp,
                [s.point[0] as f64, s.point[1] as f64, s.point[2] as f64],
            );
            s.point = [w[0] as f32, w[1] as f32, w[2] as f32];
        }
    }
    for pair in &unresolved {
        skipped.insert(pair_key(pair.a, pair.b));
    }

    // 2. The per-branch field ingredients, built once and shared across seeds.
    let halfspaces = branch_halfspace_map(body);
    let leaf_map = branch_leaf_map(body);

    // Per-branch scalar field. Under a warp chain every analytic carrier
    // becomes a [`Field::WarpedAnalytic`] (`hs ∘ xf⁻¹ ∘ T` — the warped face's
    // exact field); a branch without an analytic carrier yields `None`, which
    // Partial mode skips exactly like an unliftable branch. With no warp this
    // is byte-identical to [`branch_field`].
    let field_for = |branch: BranchId| -> Option<Field<'_>> {
        if !disp.is_empty() {
            let (hs, xf) = halfspaces.get(&branch.as_u32())?;
            return Some(Field::DisplacedAnalytic {
                hs: hs.clone(),
                xf: *xf,
                disp: disp.clone(),
            });
        }
        if warp.is_empty() {
            return branch_field(&halfspaces, &leaf_map, branch.as_u32());
        }
        let (hs, xf) = halfspaces.get(&branch.as_u32())?;
        Some(Field::WarpedAnalytic {
            hs: hs.clone(),
            xf: *xf,
            warps: warp.clone(),
        })
    };

    // Displace-Route: die SSI-Seeds liegen auf den UN-displacten Trägern —
    // |disp| ≤ Σ depth neben der echten Naht, also weit außerhalb des
    // Co-Komponenten-Dedup-Radius (`near_tol`): ohne Projektion würde JEDER
    // Seed dieselbe Kante erneut tracen (gemessen: ~60 Duplikat-Fragmente je
    // Kasten-Kante). Es gibt kein Punkt-Map W; stattdessen zieht der
    // 2D-Newton (`project_to_edge`) jeden Seed auf die gemeinsame Nullstelle
    // der beiden DISPLACTEN Flächen-Felder. Nicht-konvergierende Seeds
    // bleiben unprojiziert (trace_edge projiziert selbst erneut/verwirft).
    if !disp.is_empty() {
        for s in &mut seeds {
            let (Some(fa), Some(fb)) = (field_for(s.a), field_for(s.b)) else {
                continue;
            };
            let q = [s.point[0] as f64, s.point[1] as f64, s.point[2] as f64];
            if let Some(p) = project_to_edge(&fa, &fb, q) {
                s.point = [p[0] as f32, p[1] as f32, p[2] as f32];
            }
        }
    }

    // f64-Nachpolitur eines analytisch verfeinerten Ecken-Punkts — NUR auf der
    // Warp-Route. `refine_triple_vertex` iteriert in f32 mit LM-Dämpfung und
    // akzeptiert Residuen bis knapp unter VERTEX_REFINE_TOL; das ist GRÖBER als
    // das Aktivitätsband dieses Routers (1e-5·diag ≈ 2e-4 mm hier). Ungewarpt
    // ist das folgenlos (die Ecke wird nie aktivitäts-geprüft bzw. die Toleranz
    // reicht historisch); unter Warp fiel damit der Bend-Ecken-Witness durch
    // die both_branches_active-Prüfung, der Ecken-Cluster degradierte zum
    // 2-Branch-Zentroid (z um 2.8e-4 daneben). Gauss-Newton in f64 über ALLE
    // Branch-Felder zieht die UN-gewarpte Ecke auf die gemeinsame Nullstelle
    // (Dämpfung 1e-12 nur gegen die tangentiale Null-Achse). Auf der
    // ungewarpten Route bewusst NICHT angewandt: Byte-Identität der
    // bestehenden Weaves.
    //
    // DISPLACE-Route: dieselbe f64-Nachpolitur, aber über die DISPLACTEN
    // Felder (hs − disp) — `refine_triple_vertex` löst die UN-displacte
    // Konkurrenz, die echte Ecke liegt auf dem displacten Body (≤ Σ depth
    // daneben); der Gauss-Newton zieht sie dorthin.
    let polish_corner = |branches: &[BranchId], p0: [f64; 3]| -> [f64; 3] {
        use nalgebra::{Matrix3, Vector3};
        let mut p = p0;
        for _ in 0..8 {
            let mut jtj = Matrix3::<f64>::zeros();
            let mut jtf = Vector3::<f64>::zeros();
            let mut worst = 0.0f64;
            for b in branches {
                let Some((hs, xf)) = halfspaces.get(&b.as_u32()) else {
                    return p0;
                };
                let (f, g) = if disp.is_empty() {
                    crate::sdf_dual::halfspace_value_grad(hs, xf, p)
                } else {
                    crate::sdf_dual::displaced_halfspace_value_grad(hs, xf, &disp, p)
                };
                if !f.is_finite() {
                    return p0;
                }
                worst = worst.max(f.abs());
                let gv = Vector3::new(g[0], g[1], g[2]);
                jtj += gv * gv.transpose();
                jtf += gv * f;
            }
            if worst < 1e-12 {
                break;
            }
            let Some(inv) = (jtj + Matrix3::identity() * 1e-12).try_inverse() else {
                return p0;
            };
            let delta = inv * jtf;
            p = [p[0] - delta[0], p[1] - delta[1], p[2] - delta[2]];
            if !(p[0].is_finite() && p[1].is_finite() && p[2].is_finite()) {
                return p0;
            }
        }
        p
    };

    // Absolute length scale for curvature-adaptive tracing and dedup tolerances:
    // the model AABB diagonal (mirrors `build_wireframe`, edge_walker.rs:1585).
    // A degenerate/empty body falls back to 1.0 (it has no seams to trace).
    let diag = {
        let bb = body.aabb();
        if bb.is_empty() {
            1.0
        } else {
            let s = bb.size();
            ((s[0] as f64).powi(2) + (s[1] as f64).powi(2) + (s[2] as f64).powi(2)).sqrt()
        }
    };

    // Trace parameters, mirroring the octree route's adaptive stepping. The one
    // divergence: the octree route bounds `max_arc` by the seam group's octree
    // `span`; we have no octree here, so the model diagonal is the natural
    // spatial bound — an edge longer than a few diagonals is an unbounded seam
    // (e.g. a plane parallel to an infinite cylinder) that must stall → `Open`.
    let tol = ADAPT_TOL_FRACTION * diag;
    let min_step = ADAPT_MIN_FRACTION * diag;
    // Half the shared max fraction (ticket 7sp1): together with the march's
    // midpoint gap guard this bounds the smallest trimmed-away stretch a
    // stride can leap over to ~2.5% of the model diagonal (the g4rr lens gap
    // is 4.2%). Costs a few extra points on long straight edges, nothing on
    // curved ones (there the sagitta tolerance governs).
    let max_step = (0.5 * ADAPT_MAX_FRACTION * diag).max(min_step);
    // Deviation from the spec's `from_step(max_step)`: the *initial* predictor
    // (and hence `close_tol`) must be small. The octree route seeds this from its
    // small octree `spacing`; with no octree we seed it from `min_step`. Starting
    // at `max_step` on a tight ring (e.g. a cylinder rim) forces a long shrink-to-
    // tolerance run whose retries burn `step_i` without emitting points, after
    // which the now-tiny adaptive `close_tol` fires a FALSE loop closure one step
    // from the seed (a 1-point "ring"). A small initial step is accepted within
    // tolerance immediately, and curvature then grows the stride up to `max_step`
    // on straight runs — so straight box edges still take single spans.
    let mut params = TraceParams::from_step(min_step).with_adaptive(tol, min_step, max_step);
    params.max_arc = 4.0 * diag;

    // Active-predicate band. `both_branches_active` reports both branches active
    // within `eps` (surface distance) of each surface, so a march OVERSHOOTS the
    // true corner by ~`eps / sin φ` (φ = crossing angle against the trimming
    // surface) before `np` goes inactive and the bisection stops it. The band is
    // therefore the RESOLUTION FLOOR of the whole analytic route: any feature
    // thinner than `eps` is invisible to the activity test. Ticket g4rr (a sphere
    // grazing a box edge by 0.33 mm on a 276 mm model) showed `1e-3·diag` is far
    // too coarse — the eps-shoulders let both lens arcs march clean around their
    // corners and every terminus of two DISTINCT corners gathered into one
    // cluster. `1e-5·diag` keeps ~100× headroom over f32 evaluation noise (the
    // fields evaluate in f32: ~6e-8 relative on coordinates of order `diag`)
    // while resolving features 100× smaller than before. The activity bisection
    // reprojects onto the curve (`bisect_active`), so the band no longer needs to
    // exceed the chord sagitta of a march step.
    let eps = (1e-5 * diag) as f32;

    // Aktivitäts-Prädikat der DISPLACE-Route. `both_branches_active` prüft
    // die Contributor-Menge des Evaluators — die aber im UN-displacten
    // eps-Band attributiert wird (`BoundedCsg::Displace` zieht `disp` nur vom
    // WERT ab): die reale displacte Naht liegt |disp| ≈ depth ≫ eps neben den
    // Trägern, jeder Naht-Punkt wäre „inactive". Die Route reicht dem Test
    // aber ausschließlich Punkte, die per Konstruktion auf BEIDEN displacten
    // Flächen-Feldern liegen (`project_to_edge` / Ecken-Politur) — dort ist
    // die On-Boundary-Prüfung |f_body| ≤ eps (reales displactes Feld) das
    // ganze verbleibende Kriterium: jenseits eines Trims läuft die
    // A∩B-Kurve ins Material (f < −eps) oder in die Luft (f > eps).
    let displaced_on_boundary = |pf: [f32; 3]| -> bool {
        let (f, _) = body.evaluate(pf, eps, crate::csg::EvalMode::Logical);
        f.abs() <= eps
    };

    // Co-component seed dedup radius and corner-cluster gap, both scaled to the
    // model. A second seed of the SAME component (SSI emits several per curve)
    // lies within the polyline's chord sagitta (≤ the adaptive tol — the dedup
    // measures SEGMENT distance via `near_traced`, so the stride length is
    // irrelevant) plus the seed's own bisection residual; 10× the adaptive tol
    // covers both with headroom. It must NOT be any larger: `max_step`
    // (= 0.1·diag) as the old radius swallowed genuinely DISTINCT parallel
    // edges — a gyroid shell's twin rims run ~2·t·s·0.2887 apart, so on the
    // 7sp1 sliver every second sheet rim was "covered" by the first sheet's
    // polyline and half the Boolean rim edges silently vanished (ticket 7sp1).
    let near_tol = 10.0 * ADAPT_TOL_FRACTION * diag;
    // `gap = 0.1·diag` is the *gather* scale: large enough to collect all of one
    // corner's edge termini — each ~`eps`-overshot along its free axis, and at a
    // grazing crossing overshot by `eps/sinθ` (which can reach a few ·1e-2·diag)
    // — into a single cluster so its full branch-set is known before refining.
    // It MAY also lump two genuinely distinct corners together when a large
    // primitive inflates the model diagonal (e.g. a big sphere grazing a box
    // edge places two triple-points only ≈0.1·diag apart); the split-on-spurious
    // loop below separates those, so the gather gap need not also resolve them.
    let gap = 0.1 * diag;
    // Floor for the split-on-spurious recursion: at/below the active band the
    // termini of one corner are indistinguishable, so a still-spurious cluster
    // this tight is a genuine phantom (DL-81) → octree.
    let min_gap = (eps as f64).max(1e-9);

    // 2b. Vertex witnesses (ticket 7sp1 follow-up). The march samples the
    //    active predicate only at (mid)points of its strides, so a stride can
    //    still leap a trimmed-away stretch narrower than the resolution floor
    //    and bridge two arcs a third surface separates. The gap BOUNDARIES,
    //    however, are triple points — concurrences of the pair's two surfaces
    //    with the trimming third — and those are solvable analytically BEFORE
    //    tracing: for every branch triple whose three pairs all seeded, run
    //    the corner Newton from that triple's seam seeds and keep every
    //    concurrence that lies ON the body boundary. The marches get them as
    //    bounding vertices, so a march SNAPS to the gap boundary instead of
    //    leaping it — and the a-posteriori machinery (interior splitting,
    //    trim clustering) keeps working as the backstop.
    let witness_merge = (1e-3 * diag).max(64.0 * eps as f64);
    let mut witnesses: Vec<([f64; 3], [BranchId; 3])> = Vec::new();
    {
        use std::collections::{BTreeSet, HashSet};
        let ckey = |x: BranchId, y: BranchId| {
            let (a32, b32) = (x.as_u32(), y.as_u32());
            (a32.min(b32), a32.max(b32))
        };
        let seeded: HashSet<(u32, u32)> = seeds.iter().map(|s| ckey(s.a, s.b)).collect();
        let mut partner: HashMap<u32, HashSet<u32>> = HashMap::new();
        for &(x, y) in &seeded {
            partner.entry(x).or_default().insert(y);
            partner.entry(y).or_default().insert(x);
        }
        let mut triples: BTreeSet<[u32; 3]> = BTreeSet::new();
        for &(x, y) in &seeded {
            if let (Some(px), Some(py)) = (partner.get(&x), partner.get(&y)) {
                for &z in px.intersection(py) {
                    let mut t = [x, y, z];
                    t.sort_unstable();
                    triples.insert(t);
                }
            }
        }
        // Newton guesses: the triple's pair with the FEWEST seeds (any seam
        // seed walks to the concurrence), capped — witnesses are corners, a
        // handful of starts per triple finds them all.
        const GUESS_CAP: usize = 48;
        for t in triples {
            let branches = [BranchId(t[0]), BranchId(t[1]), BranchId(t[2])];
            let mut best: Option<Vec<[f32; 3]>> = None;
            for (x, y) in [(t[0], t[1]), (t[0], t[2]), (t[1], t[2])] {
                let list: Vec<[f32; 3]> = seeds
                    .iter()
                    .filter(|s| ckey(s.a, s.b) == (x, y))
                    .map(|s| s.point)
                    .collect();
                if best.as_ref().is_none_or(|b| list.len() < b.len()) && !list.is_empty() {
                    best = Some(list);
                }
            }
            let Some(guesses) = best else { continue };
            let stride = (guesses.len() / GUESS_CAP).max(1);
            for g in guesses.iter().step_by(stride) {
                let start = [g[0] as f64, g[1] as f64, g[2] as f64];
                // The corner Newton solves the UN-warped analytic concurrence;
                // seeds live in the warped world → pull the start back via T.
                let start = if warp.is_empty() {
                    start
                } else {
                    unwarp_chain(&warp, start)
                };
                let TripleRefine::Refined(p) =
                    refine_triple_vertex(&halfspaces, &branches, start)
                else {
                    continue;
                };
                // Warp: Politur im UN-gewarpten Raum. Displace: Politur zieht
                // die un-displacte Ecke auf die DISPLACTE Konkurrenz.
                let p = if warp.is_empty() && disp.is_empty() {
                    p
                } else {
                    polish_corner(&branches, p)
                };
                // Only ACTIVE concurrences become witnesses: extended analytic
                // surfaces concur outside the material too — far off (a cap
                // plane crossing a box edge line) or even ON the body boundary
                // (a shell inner wall\'s carrier plane reaches the outer face,
                // where the wall itself does not exist). Snapping there would
                // split real edges at phantom corners, so a witness must pass
                // the SAME activity test the marches trim by, for ALL THREE of
                // its pairs: every surface genuinely on the boundary here.
                // Rank gate: a corner is a POINT only when the three surface
                // normals genuinely span 3-space. Near-coincident carriers (a
                // µm-drifted coplanar wall pair, o2j9) concur along a whole
                // CURVE — the Newton "converges" anywhere on it and every
                // start seed mints another phantom witness marching the seam.
                // The scalar triple product of the unit normals is exactly
                // that spanning volume; refuse anything close to flat.
                {
                    let mut n = [[0.0f64; 3]; 3];
                    let mut ok = true;
                    for (k, br) in branches.iter().enumerate() {
                        let Some((hs, xf)) = halfspaces.get(&br.as_u32()) else {
                            ok = false;
                            break;
                        };
                        let (_, g) = crate::sdf_dual::halfspace_value_grad(hs, xf, p);
                        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                        if l < 1e-9 {
                            ok = false;
                            break;
                        }
                        n[k] = [g[0] / l, g[1] / l, g[2] / l];
                    }
                    let vol = (n[0][0] * (n[1][1] * n[2][2] - n[1][2] * n[2][1])
                        - n[0][1] * (n[1][0] * n[2][2] - n[1][2] * n[2][0])
                        + n[0][2] * (n[1][0] * n[2][1] - n[1][1] * n[2][0]))
                        .abs();
                    if !ok || vol < 1e-2 {
                        continue;
                    }
                }
                // The witness lives in the WARPED world (the marches snap to
                // it there); the activity test evaluates the real body, so it
                // needs the warped position too.
                let p = if warp.is_empty() { p } else { warp_chain(&warp, p) };
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let all_pairs_active = if disp.is_empty() {
                    [(0usize, 1usize), (0, 2), (1, 2)]
                        .iter()
                        .all(|&(i, j)| {
                            both_branches_active(body, pf, branches[i], branches[j], eps)
                        })
                } else {
                    // Displace: die Ecke liegt nach der Politur exakt auf den
                    // drei displacten Feldern — On-Boundary genügt (s.o.).
                    displaced_on_boundary(pf)
                };
                if !all_pairs_active {
                    if std::env::var("SEED_WEAVER_DEBUG").is_ok() {
                        eprintln!("SEED-WEAVER witness inactive: p={p:?} branches={branches:?}");
                    }
                    continue;
                }
                if witnesses.iter().any(|(w, _)| dist3(*w, p) < witness_merge) {
                    continue;
                }
                witnesses.push((p, branches));
            }
        }
    }

    // 2c. TANGENTIAL seams — the ones no SSI can seed and no tracer can follow
    //     (see `brep::tangential_seams`). They are known exactly, so they are
    //     HANDED to the weave instead of searched for. Their corners go in as
    //     witnesses FIRST: the cap-rim marches leave the body *grazingly* here
    //     (a wall plane departs tangentially into its fillet cylinder), so
    //     activity bisection alone would misplace the corner by √(2·r·eps) —
    //     millimetres. With the witness present the march snaps to it exactly.
    let seams = crate::brep::tangential_seams::collect(body);
    for seam in &seams {
        for corner in seam.corners.iter().flatten() {
            let p = corner.pos;
            // The same activity contract every witness must satisfy: all three
            // surfaces genuinely on the boundary here. A corner on an OPENED
            // shell face (its cap does not exist) fails this and is dropped —
            // which is exactly right.
            let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
            let all_active = [
                (corner.branches[0], corner.branches[1]),
                (corner.branches[0], corner.branches[2]),
                (corner.branches[1], corner.branches[2]),
            ]
            .iter()
            .all(|&(x, y)| both_branches_active(body, pf, x, y, eps));
            if !all_active {
                if std::env::var("SEED_WEAVER_DEBUG").is_ok() {
                    eprintln!("SEED-WEAVER seam corner inactive: p={p:?}");
                }
                continue;
            }
            if witnesses.iter().any(|(w, _)| dist3(*w, p) < witness_merge) {
                continue;
            }
            witnesses.push((p, corner.branches));
        }
    }

    // 3. Group seeds by canonical (min, max) branch signature, then process each
    //    group, collapsing co-component seeds into a single traced edge.
    let mut groups: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, seed) in seeds.iter().enumerate() {
        let (a32, b32) = (seed.a.as_u32(), seed.b.as_u32());
        let key = (a32.min(b32), a32.max(b32));
        groups.entry(key).or_default().push(i);
    }

    // Deterministic group order: HashMap iteration is randomised per process,
    // which would shuffle the edge arena (and with it the face order every
    // consumer sees) from run to run. Handles are arena-order-independent,
    // but UI face lists and test pick order are not.
    let mut group_list: Vec<((u32, u32), Vec<usize>)> = groups.into_iter().collect();
    group_list.sort_unstable_by_key(|(key, _)| *key);

    let mut records: Vec<EdgeRecord> = Vec::new();
    for (_key, members) in group_list {
        // This pair's vertex witnesses: triple points whose branch set contains
        // BOTH branches (they lie exactly on this pair's seam curve). Handed to
        // every trace of the group as bounding vertices; `wit_global` maps the
        // per-group index a march reports back to the global witness id.
        let (wit_global, wit_pos): (Vec<usize>, Vec<[f64; 3]>) = witnesses
            .iter()
            .enumerate()
            .filter(|(_, (_, br))| {
                br.iter().any(|x| x.as_u32() == _key.0) && br.iter().any(|x| x.as_u32() == _key.1)
            })
            .map(|(i, (p, _))| (i, *p))
            .unzip();
        // `covered` marks group seeds already absorbed by an emitted polyline.
        let mut covered = vec![false; members.len()];
        for idx in 0..members.len() {
            if covered[idx] {
                continue;
            }
            let seed = &seeds[members[idx]];
            let (a, b) = (seed.a, seed.b);

            // Build the two fields. In `Strict` mode `unresolved` is empty so
            // both lookups must succeed; a `None` here would be an internal
            // inconsistency, so we bail to octree defensively rather than panic.
            // In `Partial` mode an unliftable branch is expected (it is exactly
            // a transition seam we mean to skip), so we drop just this seed.
            let (Some(fa), Some(fb)) = (field_for(a), field_for(b)) else {
                if mode == SeedMode::Partial {
                    skipped.insert(pair_key(a, b));
                    continue;
                }
                return None;
            };

            let active = |p: [f64; 3]| {
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                if disp.is_empty() {
                    both_branches_active(body, pf, a, b, eps)
                } else {
                    // Displace-Route: der Tracer reicht nur Punkte auf beiden
                    // displacten Feldern herein — On-Boundary genügt (s.o.).
                    displaced_on_boundary(pf)
                }
            };
            let seed_pt = [
                seed.point[0] as f64,
                seed.point[1] as f64,
                seed.point[2] as f64,
            ];

            let Some(mut traced) = trace_edge(
                &fa,
                &fb,
                seed_pt,
                &wit_pos,
                &params,
                Some(&active as &dyn Fn([f64; 3]) -> bool),
            ) else {
                if std::env::var("SEED_WEAVER_TRACE_DEBUG").is_ok() {
                    eprintln!("TRACE none: pair=({a:?},{b:?}) seed={seed_pt:?}");
                }
                // The seed sat in a trimmed-away (inactive) region — this branch
                // pair does meet, but not on the real model boundary here. Not a
                // failure: skip the seed (proven safe by
                // `trace_edge_rejects_inactive_seed`).
                continue;
            };

            if std::env::var("SEED_WEAVER_TRACE_DEBUG").is_ok() {
                eprintln!(
                    "TRACE pair=({a:?},{b:?}) pts={} closed={} ends={:?} seed={seed_pt:?}",
                    traced.points.len(),
                    traced.closed,
                    traced.ends
                );
            }
            // A closed ring carries `(Open, Open)` ends by construction (it has no
            // termini) — only an OPEN arc's `Open` end signals a stalled march. On
            // the seed path there is no sample-fit backstop for a stall, so an open
            // arc that failed to reach a clean end is an unbounded/unresolved seam.
            // In `Strict` mode this aborts to the whole-body octree; in `Partial`
            // mode it is precisely a transition seam, so we drop just this record.
            if !traced.closed
                && (matches!(traced.ends.0, EdgeEnd::Open) || matches!(traced.ends.1, EdgeEnd::Open))
            {
                if mode == SeedMode::Partial {
                    skipped.insert(pair_key(a, b));
                    continue;
                }
                return None;
            }

            // Remap witness-vertex ends from the group-local list the march
            // saw to GLOBAL witness ids (materialised as B-Rep vertices below).
            for end in [&mut traced.ends.0, &mut traced.ends.1] {
                if let EdgeEnd::Vertex(li) = end {
                    *end = EdgeEnd::Vertex(wit_global[*li]);
                }
            }

            // A seed sitting (numerically) ON a witness corner ends BOTH
            // march directions right at that corner — one side snapped to the
            // vertex, the other trim-bisected onto the same spot — a µm-scale
            // self-loop stub, not an edge. Positions decide (the end KINDS
            // vary): an open arc whose termini coincide within the corner
            // merge radius and whose whole arc is a few march floors long is
            // degenerate. A genuine same-vertex teardrop seam is orders of
            // magnitude longer.
            if !traced.closed && traced.points.len() >= 2 {
                let first = traced.points[0];
                let last = *traced.points.last().expect("nonempty");
                if dist3(first, last) < witness_merge {
                    let len: f64 = traced
                        .points
                        .windows(2)
                        .map(|w| dist3(w[0], w[1]))
                        .sum();
                    if len < 8.0 * min_step {
                        continue;
                    }
                }
            }

            // Convert to Hermite NOW, while the borrowing fields are alive, so
            // the stored record is self-contained.
            let samples = traced_to_hermite(&fa, &fb, &traced);
            if samples.len() < 2 {
                continue; // degenerate — too short to be a real edge.
            }

            // Co-component dedup: every other not-yet-traced seed in this group
            // that lands on this polyline is the SAME component (SSI can emit
            // multiple seeds per curve) — mark it covered so it is not re-traced.
            // A seed on a genuinely disjoint component lies off the polyline and
            // survives to trace its own edge.
            covered[idx] = true;
            // The dedup polyline includes the TRIM TERMINI: the march's last
            // emitted point undershoots the true end by up to one adaptive
            // stride, and a same-arc seed inside that gap would otherwise
            // escape the (deliberately tight) radius and re-trace the arc —
            // the historical reason `near_tol` had to be a whole `max_step`.
            let mut dedup_poly = traced.points.clone();
            if let EdgeEnd::Trim(p) = traced.ends.0 {
                dedup_poly.insert(0, p);
            }
            if let EdgeEnd::Trim(p) = traced.ends.1 {
                dedup_poly.push(p);
            }
            for other in 0..members.len() {
                if covered[other] {
                    continue;
                }
                let p = &seeds[members[other]].point;
                let q = [p[0] as f64, p[1] as f64, p[2] as f64];
                if near_traced(q, &dedup_poly, traced.closed, near_tol) {
                    covered[other] = true;
                }
            }

            records.push(EdgeRecord {
                a,
                b,
                samples,
                closed: traced.closed,
                ends: traced.ends,
            });
        }
    }

    // 3b. Emit the TANGENTIAL seams as records — AHEAD of the trim clustering,
    //     so they inherit exactly what a traced record gets: activity trimming,
    //     corner resolution, and the interior split at crossing vertices. The
    //     curve itself is exact, so all that is left is to cut it to the parts
    //     that are really on the body (the Kabelbox cuts its base solid several
    //     times — an untrimmed seam would hang out into removed material).
    for seam in &seams {
        let active_at = |p: [f64; 3]| {
            both_branches_active(
                body,
                [p[0] as f32, p[1] as f32, p[2] as f32],
                seam.a,
                seam.b,
                eps,
            )
        };
        // Bisect an active↔inactive bracket down to the model's own tolerance.
        let boundary = |inside: [f64; 3], outside: [f64; 3]| -> [f64; 3] {
            let (mut lo, mut hi) = (inside, outside);
            for _ in 0..32 {
                let mid = [
                    0.5 * (lo[0] + hi[0]),
                    0.5 * (lo[1] + hi[1]),
                    0.5 * (lo[2] + hi[2]),
                ];
                if active_at(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        // The seam's own corners, as global witness ids (they were pushed in
        // 2c unless they failed the activity gate).
        let corner_witness = |p: [f64; 3]| -> Option<usize> {
            witnesses
                .iter()
                .position(|(w, _)| dist3(*w, p) < witness_merge)
        };

        let n = seam.samples.len();
        let flags: Vec<bool> = seam.samples.iter().map(|s| active_at(s.point)).collect();
        if !flags.iter().any(|&f| f) {
            continue; // wholly trimmed away
        }

        // A closed seam that is active all the way round stays one ring.
        if seam.closed && flags.iter().all(|&f| f) {
            records.push(EdgeRecord {
                a: seam.a,
                b: seam.b,
                samples: seam.samples.clone(),
                closed: true,
                ends: (EdgeEnd::Open, EdgeEnd::Open),
            });
            continue;
        }

        // Otherwise: cut into maximal active runs. Indices are cyclic for a
        // closed seam, linear for an open one.
        let mut runs: Vec<Vec<usize>> = Vec::new();
        let mut cur: Vec<usize> = Vec::new();
        for i in 0..n {
            if flags[i] {
                cur.push(i);
            } else if !cur.is_empty() {
                runs.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            runs.push(cur);
        }
        // Cyclic wrap: a closed seam whose run straddles index 0 is ONE run.
        if seam.closed && runs.len() > 1 && flags[0] && flags[n - 1] {
            let first = runs.remove(0);
            runs.last_mut().expect("nonempty").extend(first);
        }

        for run in runs {
            let (i0, i1) = (run[0], *run.last().expect("nonempty"));
            let mut samples: Vec<HermiteSample> =
                run.iter().map(|&i| seam.samples[i].clone()).collect();

            // Backward end: an inactive neighbour means the seam leaves the
            // body there (bisect it); no neighbour means the seam's own end.
            let back = if i0 > 0 || seam.closed {
                let prev = (i0 + n - 1) % n;
                if flags[prev] {
                    None // (cyclic wrap already merged — cannot happen)
                } else {
                    Some(boundary(seam.samples[i0].point, seam.samples[prev].point))
                }
            } else {
                None
            };
            let fwd = if i1 + 1 < n || seam.closed {
                let next = (i1 + 1) % n;
                if flags[next] {
                    None
                } else {
                    Some(boundary(seam.samples[i1].point, seam.samples[next].point))
                }
            } else {
                None
            };

            let end_for = |trim: Option<[f64; 3]>,
                           own: Option<&crate::brep::tangential_seams::SeamCorner>|
             -> EdgeEnd {
                match trim {
                    // The seam ran off the body here — a trim terminus, which
                    // the clustering below turns into a corner vertex.
                    Some(p) => EdgeEnd::Trim(p),
                    // It ran to its own end: the cap corner, already a witness.
                    None => match own.and_then(|c| corner_witness(c.pos)) {
                        Some(wi) => EdgeEnd::Vertex(wi),
                        None => EdgeEnd::Open,
                    },
                }
            };
            let ends = (
                end_for(back, seam.corners[0].as_ref()),
                end_for(fwd, seam.corners[1].as_ref()),
            );
            // An `Open` end means the seam neither reached a corner nor left
            // the body — its cap corner failed the activity gate. Nothing can
            // bound it, so drop it rather than emit a dangling edge.
            if matches!(ends.0, EdgeEnd::Open) || matches!(ends.1, EdgeEnd::Open) {
                if std::env::var("SEED_WEAVER_DEBUG").is_ok() {
                    eprintln!(
                        "SEED-WEAVER seam unbounded: pair=({:?},{:?}) ends={ends:?}",
                        seam.a, seam.b
                    );
                }
                continue;
            }
            // Pin the exact endpoints: the run's first/last SAMPLE undershoots
            // the true end by up to one sampling stride.
            if let EdgeEnd::Trim(p) = ends.0 {
                samples.insert(0, HermiteSample { point: p, tangent: samples[0].tangent });
            } else if let Some(c) = seam.corners[0].as_ref() {
                samples.insert(0, HermiteSample { point: c.pos, tangent: samples[0].tangent });
            }
            if let EdgeEnd::Trim(p) = ends.1 {
                let t = samples.last().expect("nonempty").tangent;
                samples.push(HermiteSample { point: p, tangent: t });
            } else if let Some(c) = seam.corners[1].as_ref() {
                let t = samples.last().expect("nonempty").tangent;
                samples.push(HermiteSample { point: c.pos, tangent: t });
            }
            if samples.len() < 2 {
                continue;
            }
            records.push(EdgeRecord {
                a: seam.a,
                b: seam.b,
                samples,
                closed: false,
                ends,
            });
        }
    }

    // 4. Trim-vertex clustering. Gather every open record's `Trim` termini as
    //    `(record_idx, side, position)` (side 0 = start/backward, 1 = end/
    //    forward), cluster co-incident ones into corners, and re-solve each
    //    corner analytically.
    let mut trim_refs: Vec<(usize, usize)> = Vec::new(); // (record_idx, side)
    let mut trim_pos: Vec<[f64; 3]> = Vec::new();
    for (ri, rec) in records.iter().enumerate() {
        if rec.closed {
            continue;
        }
        if let EdgeEnd::Trim(p) = rec.ends.0 {
            trim_refs.push((ri, 0));
            trim_pos.push(p);
        }
        if let EdgeEnd::Trim(p) = rec.ends.1 {
            trim_refs.push((ri, 1));
            trim_pos.push(p);
        }
    }

    let mut brep = BRep::new();
    // Per (record, side) → the corner vertex it terminates at.
    let mut side_vertex: HashMap<(usize, usize), VertexId> = HashMap::new();

    // Collect which (record, side) termini reference each witness. The
    // vertices are materialised AFTER trim clustering: a witness vertex's
    // branch set must be the union of every incident record's two branches —
    // exactly what the cluster path builds — or the vertex id_hash diverges
    // from the octree route and the handle-identity contract breaks (the
    // flush-union classification gates). Trim clusters resolving onto a
    // referenced witness merge INTO it below, so a corner reached by a
    // vertex-snapped march on one edge and an activity-trimmed march on
    // another still fuses into ONE vertex.
    let mut witness_refs: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (ri, rec) in records.iter().enumerate() {
        for (side, end) in [(0usize, rec.ends.0), (1, rec.ends.1)] {
            if let EdgeEnd::Vertex(wi) = end {
                witness_refs.entry(wi).or_default().push((ri, side));
            }
        }
    }

    // Resolve corners with split-on-spurious. Each work item is a set of trim
    // termini (indices into `trim_*`) to cluster at gap `g`. A cluster that
    // refines cleanly becomes a vertex; a `Spurious` cluster has lumped ≥2
    // distinct corners, so we re-queue it at a halved gap. Splitting is
    // top-down, so each real corner's full branch-set stays together (it refines
    // before it can be split) while distinct corners peel apart — down to the
    // active-band floor, past which a still-spurious cluster is a genuine phantom
    // and the whole body goes to octree.
    let mut work: Vec<(Vec<usize>, f64)> = vec![((0..trim_pos.len()).collect(), gap)];
    while let Some((members, g)) = work.pop() {
        let sub_pos: Vec<[f64; 3]> = members.iter().map(|&m| trim_pos[m]).collect();
        for cl in cluster_positions(&sub_pos, g) {
            let group: Vec<usize> = cl.iter().map(|&i| members[i]).collect();

            // Branch-set: the union of the incident records' two branches. The
            // corner is where these surfaces concur; `Vertex::new` canonicalises.
            let mut branches: Vec<BranchId> = Vec::new();
            let mut centroid = [0.0f64; 3];
            for &m in &group {
                let (ri, _side) = trim_refs[m];
                branches.push(records[ri].a);
                branches.push(records[ri].b);
                let p = trim_pos[m];
                centroid[0] += p[0];
                centroid[1] += p[1];
                centroid[2] += p[2];
            }
            let n = group.len() as f64;
            centroid = [centroid[0] / n, centroid[1] / n, centroid[2] / n];

            // Re-solve the EXACT corner from the analytic surfaces. The raw
            // centroid only seeds it, so the trim band's imprecision washes out.
            // Under a warp the Newton runs in the UN-warped space (T on the
            // centroid) and the corner is mapped back through W — the trim
            // termini it must cohere with are warped-world points.
            let refine_seed = if warp.is_empty() {
                centroid
            } else {
                unwarp_chain(&warp, centroid)
            };
            let pos = match refine_triple_vertex(&halfspaces, &branches, refine_seed) {
                TripleRefine::Refined(p) => {
                    let p = if !warp.is_empty() {
                        warp_chain(&warp, polish_corner(&branches, p))
                    } else if !disp.is_empty() {
                        // Kein Punkt-Map: die Politur über die displacten
                        // Felder IST der Weg auf den realen Body.
                        polish_corner(&branches, p)
                    } else {
                        p
                    };
                    // Coherence gate (ticket g4rr): a cluster that gathered the
                    // termini of TWO distinct corners of the same branch triple
                    // still refines cleanly — Newton converges to ONE of the
                    // genuine corners, so `Spurious` never fires and both arcs
                    // would bind both ends to a single vertex (the reported
                    // self-crossing lens). A member's legitimate distance to its
                    // refined corner is bounded by the trim overshoot
                    // `eps / sin φ`; anything an order of magnitude beyond that
                    // means the gather lumped distinct corners → re-cluster at a
                    // halved gap, exactly like the spurious path below.
                    let slack = (1e-3 * diag).max(64.0 * eps as f64);
                    let coherent = group
                        .iter()
                        .all(|&m| dist3(trim_pos[m], p) <= slack);
                    if !coherent {
                        let tighter = (g * 0.5).min(0.5 * group_diameter(&trim_pos, &group));
                        if group.len() > 1 && tighter >= min_gap {
                            work.push((group, tighter));
                            continue;
                        }
                        // Cannot place this corner. Strict → whole-body octree;
                        // partial → skip just this cluster (its incident edges
                        // emit unbounded on the affected side; the hybrid fill
                        // re-owns their pairs from the octree).
                        if mode == SeedMode::Partial {
                            for &m in &group {
                                let (ri, _) = trim_refs[m];
                                skipped.insert(pair_key(records[ri].a, records[ri].b));
                            }
                            continue;
                        }
                        return None;
                    }
                    p
                }
                // Cannot judge analytically (non-analytic branch / rank-
                // deficient): try the exact swept-extrude rim corner; failing
                // that, keep the centroid (octree also leaves such a corner at
                // centroid).
                TripleRefine::Indeterminate => {
                    if std::env::var("SEED_WEAVER_DEBUG").is_ok() {
                        eprintln!(
                            "SEED-WEAVER Indeterminate corner: centroid={centroid:?} branches={branches:?}"
                        );
                    }
                    extrude_rim_corner(&leaf_map, &branches).unwrap_or(centroid)
                }
                // The surfaces provably do not concur at one point. Either this
                // cluster merged distinct corners — re-cluster tighter — or, once
                // the gap reaches the active-band floor without splitting, it is a
                // genuine phantom the analytic route cannot place → octree.
                //
                // The next gap is capped at HALF the cluster's diameter, not just
                // `g/2`: single-linkage clustering can only separate members once
                // the gap drops below their spacing, so a cluster much tighter
                // than `g` (six termini of two corners 11 mm apart gathered at a
                // 27 mm gap — ticket g4rr) would otherwise re-cluster into the
                // SAME single cluster, and the old `diameter > tighter` guard
                // turned that dead halving step into a bail-to-octree.
                TripleRefine::Spurious => {
                    let tighter = (g * 0.5).min(0.5 * group_diameter(&trim_pos, &group));
                    if group.len() > 1 && tighter >= min_gap {
                        work.push((group, tighter));
                        continue;
                    }
                    // Genuine phantom corner. Strict → whole-body octree;
                    // partial → skip just this cluster.
                    if mode == SeedMode::Partial {
                        for &m in &group {
                            let (ri, _) = trim_refs[m];
                            skipped.insert(pair_key(records[ri].a, records[ri].b));
                        }
                        continue;
                    }
                    return None;
                }
            };

            // A corner that coincides with a REFERENCED witness must not be
            // duplicated — loops chain by VertexId, and two vertices at one
            // position break the ring connectivity. Merge this cluster's
            // termini into the witness; its branch union picks them up when
            // the witness vertices are materialised below.
            let coincident = witness_refs
                .keys()
                .find(|wi| dist3(witnesses[**wi].0, pos) <= witness_merge)
                .copied();
            if let Some(wi) = coincident {
                let refs = witness_refs.get_mut(&wi).expect("key just found");
                for &m in &group {
                    refs.push(trim_refs[m]);
                }
                continue;
            }
            let vid = brep.add_vertex(Vertex::new(pos, branches));
            for &m in &group {
                side_vertex.insert(trim_refs[m], vid);
            }
        }
    }

    // Materialise the referenced witnesses (deterministic order), each with
    // the branch UNION of every terminus that ends there — cluster-path
    // semantics, so the vertex id_hash matches the octree route.
    let mut wit_list: Vec<(usize, Vec<(usize, usize)>)> = witness_refs.into_iter().collect();
    wit_list.sort_unstable_by_key(|(wi, _)| *wi);
    for (wi, refs) in wit_list {
        let mut branches: Vec<BranchId> = Vec::new();
        for &(ri, _) in &refs {
            branches.push(records[ri].a);
            branches.push(records[ri].b);
        }
        let vid = brep.add_vertex(Vertex::new(witnesses[wi].0, branches));
        for r in refs {
            side_vertex.insert(r, vid);
        }
    }

    // 5. Split + emit edges. Vertices are added first (above) so every
    //    `VertexId` an edge references already exists — mirrors
    //    `build_box_wireframe` ordering.
    //
    //    Interior-vertex splitting (ticket g4rr): an adaptive march can stride
    //    clean over a trimmed-away pocket — a third branch covering a stretch
    //    of the curve narrower than the local step (a sphere grazing a box
    //    edge carves an 11 mm pocket out of a 100 mm edge; the straight-run
    //    step is 27 mm). The pocket's BOUNDARIES are still known exactly: the
    //    other two pairs of the same branch triple traced arcs that trim at
    //    them, so the resolved corner vertices exist. Any vertex whose branch
    //    set contains both of this edge's branches and that lies ON the
    //    polyline interior is such a missed crossing → split there, then drop
    //    every fragment whose arc midpoint is not on the model boundary.
    let split_tol = 8.0 * tol; // chord-sagitta budget of the march is `tol`
    for (ri, rec) in records.iter().enumerate() {
        let start = (!rec.closed)
            .then(|| side_vertex.get(&(ri, 0)).copied())
            .flatten();
        let end = (!rec.closed)
            .then(|| side_vertex.get(&(ri, 1)).copied())
            .flatten();

        // Locate interior crossings: nearest polyline point per candidate
        // vertex. (seg, t) addresses the chord `samples[seg] → samples[seg+1]`
        // (the wrap chord for a ring); hits at the record's own bound vertices
        // are not interior and are skipped by id.
        let pts: Vec<[f64; 3]> = rec.samples.iter().map(|s| s.point).collect();
        let nseg = if rec.closed { pts.len() } else { pts.len() - 1 };
        let mut hits: Vec<(usize, f64, VertexId, [f64; 3])> = Vec::new();
        for (vi, v) in brep.vertices.iter().enumerate() {
            let vid = VertexId(vi as u32);
            if Some(vid) == start || Some(vid) == end {
                continue;
            }
            if !(v.branches.contains(&rec.a) && v.branches.contains(&rec.b)) {
                continue;
            }
            let mut best: Option<(usize, f64, f64)> = None; // (seg, t, dist)
            for seg in 0..nseg {
                let p0 = pts[seg];
                let p1 = pts[(seg + 1) % pts.len()];
                let d = sub(p1, p0);
                let len2 = dot(d, d);
                let t = if len2 > 0.0 {
                    (dot(sub(v.position, p0), d) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = add(p0, scale(d, t));
                let dd = dist3(v.position, q);
                if best.is_none_or(|(_, _, bd)| dd < bd) {
                    best = Some((seg, t, dd));
                }
            }
            if let Some((seg, t, dd)) = best {
                if dd <= split_tol {
                    hits.push((seg, t, vid, v.position));
                }
            }
        }
        hits.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap());

        // No interior crossing (the overwhelmingly common case): emit as-is.
        if hits.is_empty() || (rec.closed && hits.len() < 2) {
            let geom = EdgeGeometry::Hermite(rec.samples.clone());
            if rec.closed {
                // A closed ring (e.g. a cap∩wall rim) carries no bounding vertices.
                brep.add_edge(Edge::new(geom, rec.a, rec.b, None, None));
            } else {
                brep.add_edge(Edge::new(geom, rec.a, rec.b, start, end));
            }
            continue;
        }

        // Fields for cut-node tangents and the fragment-midpoint activity test.
        // In partial mode an unliftable field here (the record traced but its
        // branch is non-analytic for the split test) means we cannot split — emit
        // the record whole rather than abort the body.
        let (Some(fa), Some(fb)) = (field_for(rec.a), field_for(rec.b)) else {
            if mode == SeedMode::Partial {
                let geom = EdgeGeometry::Hermite(rec.samples.clone());
                if rec.closed {
                    brep.add_edge(Edge::new(geom, rec.a, rec.b, None, None));
                } else {
                    brep.add_edge(Edge::new(geom, rec.a, rec.b, start, end));
                }
                continue;
            }
            return None;
        };
        let cut_node = |pos: [f64; 3], seg: usize| -> HermiteSample {
            let chord = sub(pts[(seg + 1) % pts.len()], pts[seg]);
            let mut t = edge_tangent(&fa, &fb, pos).unwrap_or_else(|| normalize(chord));
            if dot(t, chord) < 0.0 {
                t = scale(t, -1.0);
            }
            HermiteSample { point: pos, tangent: t }
        };
        // On-boundary test at a fragment's arc midpoint. The midpoint of the
        // chord chain is reprojected onto the exact curve so the activity test
        // is evaluated on-surface.
        let fragment_live = |samples: &[HermiteSample]| -> bool {
            let mut total = 0.0;
            for w in samples.windows(2) {
                total += dist3(w[0].point, w[1].point);
            }
            let mut rem = 0.5 * total;
            let mut mid = samples[0].point;
            for w in samples.windows(2) {
                let seg = dist3(w[0].point, w[1].point);
                if seg >= rem && seg > 0.0 {
                    mid = add(w[0].point, scale(sub(w[1].point, w[0].point), rem / seg));
                    break;
                }
                rem -= seg;
            }
            let mid = project_to_edge(&fa, &fb, mid).unwrap_or(mid);
            both_branches_active(
                body,
                [mid[0] as f32, mid[1] as f32, mid[2] as f32],
                rec.a,
                rec.b,
                eps,
            )
        };
        let mut emit = |samples: Vec<HermiteSample>, s: Option<VertexId>, e: Option<VertexId>| {
            if samples.len() >= 2 && fragment_live(&samples) {
                brep.add_edge(Edge::new(
                    EdgeGeometry::Hermite(samples),
                    rec.a,
                    rec.b,
                    s,
                    e,
                ));
            }
        };

        if rec.closed {
            // Ring with ≥2 crossings: cut into cyclic open fragments, each
            // bounded by consecutive crossing vertices.
            for i in 0..hits.len() {
                let (seg_a, _, vid_a, pos_a) = hits[i];
                let (seg_b, _, vid_b, pos_b) = hits[(i + 1) % hits.len()];
                let mut samples = vec![cut_node(pos_a, seg_a)];
                let mut k = (seg_a + 1) % pts.len();
                let stop = (seg_b + 1) % pts.len();
                while k != stop {
                    samples.push(rec.samples[k].clone());
                    k = (k + 1) % pts.len();
                }
                samples.push(cut_node(pos_b, seg_b));
                emit(samples, Some(vid_a), Some(vid_b));
            }
        } else {
            // Open chain: fragments between consecutive cuts, outer fragments
            // keep the record's original bound vertices.
            let mut from_sample = 0usize;
            let mut from_vertex = start;
            let mut from_node: Option<HermiteSample> = None;
            for &(seg, _, vid, pos) in &hits {
                let mut samples: Vec<HermiteSample> = Vec::new();
                if let Some(n) = from_node.take() {
                    samples.push(n);
                }
                samples.extend_from_slice(&rec.samples[from_sample..=seg]);
                samples.push(cut_node(pos, seg));
                emit(samples, from_vertex, Some(vid));
                from_sample = seg + 1;
                from_vertex = Some(vid);
                from_node = Some(cut_node(pos, seg));
            }
            let mut samples: Vec<HermiteSample> = Vec::new();
            if let Some(n) = from_node.take() {
                samples.push(n);
            }
            samples.extend_from_slice(&rec.samples[from_sample..]);
            emit(samples, from_vertex, end);
        }
    }

    Some(SeedWireframe { brep, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg::CsgNode;
    use crate::sketch2d::{Sketch2d, Sketch2dPrimitive};
    use fielddraft_protocol::{FeatureId, Primitive};

    fn box_leaf(id: u32, center: [f32; 3], half_extents: [f32; 3]) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Box { center, half_extents },
        }
    }

    fn cylinder_leaf(id: u32, center: [f32; 3], radius: f32, half_height: f32) -> CsgNode {
        CsgNode::Leaf {
            id: FeatureId::from_u32(id),
            primitive: Primitive::Cylinder { center, radius, half_height },
        }
    }

    #[test]
    fn scope_admits_plain_box() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        assert!(scope_supported(&body));
    }

    #[test]
    fn scope_admits_boolean_of_analytic_primitives() {
        let csg = CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        );
        let body = BoundedCsg::from_csg(&csg);
        assert!(scope_supported(&body));
    }

    #[test]
    fn scope_admits_extrude() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let csg = CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(7),
            sketch: Box::new(sketch),
            half_height: 2.0,
        };
        let body = BoundedCsg::from_csg(&csg);
        assert!(scope_supported(&body));
    }

    #[test]
    fn scope_rejects_warp() {
        // A Twist wrapping a box: out of scope (the half-space map describes the
        // un-warped box, so analytic seeding would trace the wrong surface).
        let csg = CsgNode::Twist(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            0.01,
        );
        let body = BoundedCsg::from_csg(&csg);
        assert!(!scope_supported(&body));
    }

    /// Warp-B-Rep (2026-07-13): ein Twist/Bend an der WURZEL ist jetzt im
    /// PARTIAL-Scope (die SSI-Seeds + Ecken-Newton laufen im UN-gewarpten
    /// Raum und werden durch W vorwärts gemappt); Strict bleibt zu (id_hash-
    /// Toggle-Invarianz gegen den Oktree gilt dort weiter). Innen liegende
    /// Warps (nur ein Boolean-Ast gewarpt) bleiben Oktree-Fallback.
    /// Displace-B-Rep (2026-07-13, Fortsetzung): ein ROOT-Displace ist
    /// ebenfalls im Partial-Scope — kein Punkt-Map, aber die Naht ist die
    /// gemeinsame Nullstelle der displacten Flächen-Felder
    /// ([`Field::DisplacedAnalytic`]); Twist/Bend UNTER dem Displace bleibt
    /// Oktree.
    #[test]
    fn partial_scope_admits_root_warp_only() {
        let twist = BoundedCsg::from_csg(&CsgNode::Twist(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [4.0, 6.0, 4.0])),
            0.05,
        ));
        assert!(scope_supported_partial(&twist), "Root-Twist im Partial-Scope");
        assert!(!scope_supported(&twist), "Strict bleibt zu");

        let bend = BoundedCsg::from_csg(&CsgNode::Bend(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [6.0, 3.0, 3.0])),
            0.05,
        ));
        assert!(scope_supported_partial(&bend), "Root-Bend im Partial-Scope");

        // Warp nur über EINEM Boolean-Ast: kein gemeinsamer Root-Chain.
        let inner_warp = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(CsgNode::Twist(
                Box::new(box_leaf(1, [0.0, 0.0, 0.0], [4.0, 6.0, 4.0])),
                0.05,
            )),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere { center: [10.0, 0.0, 0.0], radius: 3.0 },
            }),
        ));
        assert!(!scope_supported_partial(&inner_warp), "innerer Warp bleibt Oktree");

        // Displace an der Wurzel: Wert-Störung ohne Punktabbildung, aber die
        // displacten Flächen-Felder tragen den Seed-Weave → Partial-Scope.
        let displace = BoundedCsg::from_csg(&CsgNode::Displace(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [4.0, 4.0, 4.0])),
            0.3,
            1.5,
        ));
        assert!(
            scope_supported_partial(&displace),
            "Root-Displace im Partial-Scope"
        );
        assert!(!scope_supported(&displace), "Strict bleibt zu");

        // Twist UNTER dem Displace: keine der beiden Root-Ketten greift.
        let mixed = BoundedCsg::from_csg(&CsgNode::Displace(
            Box::new(CsgNode::Twist(
                Box::new(box_leaf(1, [0.0, 0.0, 0.0], [4.0, 6.0, 4.0])),
                0.05,
            )),
            0.3,
            1.5,
        ));
        assert!(!scope_supported_partial(&mixed), "Twist unter Displace bleibt Oktree");
    }

    /// Warp-B-Rep: der Hybrid-Weave liefert für einen getwisteten Kasten die
    /// EXAKTE Kasten-Topologie — 8 gewarpte Ecken (W-Bilder der Kasten-Ecken,
    /// auf der Oberfläche), 12 Kanten (4 Helizes + 8 Deckelkanten, alle an
    /// echten Ecken gebunden), 6 Flächen — statt der zersplitterten Oktree-
    /// Sample-Fits (73 Fragmente vor der Änderung).
    #[test]
    fn hybrid_weaves_twisted_box_exact_warped_topology() {
        assert_warped_box_topology(
            &BoundedCsg::from_csg(&CsgNode::Twist(
                Box::new(box_leaf(1, [0.0, 0.0, 0.0], [4.0, 6.0, 4.0])),
                0.05,
            )),
            &[DomainWarp::Twist { rate: 0.05 }],
            [4.0, 6.0, 4.0],
        );
    }

    /// Wie [`hybrid_weaves_twisted_box_exact_warped_topology`], für Bend
    /// (dessen Vorwärts-Warp W den 1D-Newton braucht — s. DomainWarp::warp).
    #[test]
    fn hybrid_weaves_bent_box_exact_warped_topology() {
        assert_warped_box_topology(
            &BoundedCsg::from_csg(&CsgNode::Bend(
                Box::new(box_leaf(1, [0.0, 0.0, 0.0], [6.0, 3.0, 3.0])),
                0.05,
            )),
            &[DomainWarp::Bend { rate: 0.05 }],
            [6.0, 3.0, 3.0],
        );
    }

    fn assert_warped_box_topology(
        body: &BoundedCsg,
        chain: &[DomainWarp],
        half: [f64; 3],
    ) {
        let brep = weave_branch_seeds_hybrid(body, 0.5).expect("Warp-Body im Partial-Scope");
        assert_eq!(brep.vertices.len(), 8, "8 gewarpte Ecken");
        assert_eq!(brep.edges.len(), 12, "12 Kanten");
        assert_eq!(brep.faces.len(), 6, "6 Flächen");
        for v in &brep.vertices {
            let pf = [
                v.position[0] as f32,
                v.position[1] as f32,
                v.position[2] as f32,
            ];
            let (d, _) = crate::sdf_dual::sdf_gradient(body, pf);
            assert!(
                d.abs() < 1e-3,
                "Ecke {:?} liegt nicht auf dem gewarpten Körper: f = {d}",
                v.position
            );
            // Rück-Warp T führt die Ecke exakt auf eine UN-gewarpte Kasten-Ecke.
            let u = unwarp_chain(chain, v.position);
            for k in 0..3 {
                assert!(
                    (u[k].abs() - half[k]).abs() < 1e-5,
                    "T(Ecke) {u:?} ist keine Kasten-Ecke (±{half:?})"
                );
            }
        }
        for e in &brep.edges {
            assert!(
                e.start.is_some() && e.end.is_some(),
                "Kante {:?} ohne gebundene Ecken",
                e.signature
            );
        }
    }

    /// Displace-B-Rep, GESCHNITZTER Fall (disp < 0 auf allen Nähten): der
    /// Hybrid-Weave liefert die EXAKTE Kasten-Topologie — 8 Ecken (Gauss-
    /// Newton-Nullstellen der drei DISPLACTEN Flächen-Felder, auf der realen
    /// Oberfläche), 12 Kanten, 6 Flächen — statt des Oktree-Fallbacks
    /// (Displace war vorher pauschal aus dem Scope, §8r Punkt 1).
    ///
    /// GRENZE DES MODELLS (bewusst, ehrlich): wo disp > 0 (Beule nach AUSSEN),
    /// ist die Box-SDF außen die EUKLIDISCHE Ecken-/Kanten-Distanz — die
    /// displacte Oberfläche ist dort ein RUNDER Blend-Streifen (Radius ~disp),
    /// KEINE scharfe Naht, und die drei displacten Flächen-Felder konkurrieren
    /// nirgends auf dem Body (die gemeinsame Nullstelle liegt (√3−1)·disp
    /// AUSSERHALB). Der Aktivitäts-Test verwirft solche Zeugen korrekt; die
    /// Fixture hier liegt deshalb ganz im disp-<-0-Regime (Achsen-Fenster ohne
    /// sin-Vorzeichenwechsel, ein Faktor negativ).
    #[test]
    fn hybrid_weaves_displaced_box_exact_topology_carved_regime() {
        let (depth, freq) = (0.3f32, 0.1f32);
        // x∈[8,16]: sin(0.1x)>0; y∈[3,15]: sin(0.13y)>0; z∈[−19,−11]:
        // sin(0.07z)<0 → disp<0 überall auf dem Kasten.
        let center = [12.0f64, 9.0, -15.0];
        let half = [4.0f64, 6.0, 4.0];
        let body = BoundedCsg::from_csg(&CsgNode::Displace(
            Box::new(box_leaf(1, [12.0, 9.0, -15.0], [4.0, 6.0, 4.0])),
            depth,
            freq,
        ));
        let brep = weave_branch_seeds_hybrid(&body, 0.5).expect("Displace-Body im Partial-Scope");
        if std::env::var("SEED_WEAVER_DEBUG").is_ok() {
            let mut hist: std::collections::BTreeMap<(u32, u32), (usize, f64)> = Default::default();
            for e in &brep.edges {
                let k = pair_key(e.signature.0, e.signature.1);
                let len = match &e.geometry {
                    EdgeGeometry::Hermite(s) => s
                        .windows(2)
                        .map(|w| dist3(w[0].point, w[1].point))
                        .sum::<f64>(),
                    _ => -1.0,
                };
                let ent = hist.entry(k).or_insert((0, 0.0));
                ent.0 += 1;
                ent.1 += len;
            }
            for (k, (n, l)) in &hist {
                eprintln!("EDGE-HIST {k:?}: {n} Fragmente, Σlen={l:.3}");
            }
        }
        assert_eq!(brep.vertices.len(), 8, "8 displacte Ecken");
        assert_eq!(brep.edges.len(), 12, "12 Kanten");
        assert_eq!(brep.faces.len(), 6, "6 Flächen");
        for v in &brep.vertices {
            let pf = [
                v.position[0] as f32,
                v.position[1] as f32,
                v.position[2] as f32,
            ];
            let (d, _) = crate::sdf_dual::sdf_gradient(&body, pf);
            assert!(
                d.abs() < 1e-3,
                "Ecke {:?} liegt nicht auf dem displacten Körper: f = {d}",
                v.position
            );
            // Die Ecke ist die Nullstelle der displacten Felder — nahe der
            // un-displacten Kasten-Ecke (|disp| ≤ depth), aber NICHT darauf.
            for k in 0..3 {
                assert!(
                    ((v.position[k] - center[k]).abs() - half[k]).abs() < 3.0 * depth as f64,
                    "Ecke {:?} zu weit von der Kasten-Ecke (Achse {k})",
                    v.position
                );
            }
        }
        // Kanten-Samples liegen auf BEIDEN displacten Flächen-Feldern (also
        // auf der echten Naht), nicht auf den un-displacten Trägern.
        let halfspaces = branch_halfspace_map(&body);
        let disp: Vec<(f64, f64)> = vec![(depth as f64, freq as f64)];
        let mut checked = 0usize;
        for e in &brep.edges {
            assert!(
                e.start.is_some() && e.end.is_some(),
                "Kante {:?} ohne gebundene Ecken",
                e.signature
            );
            let pts: Vec<[f64; 3]> = match &e.geometry {
                EdgeGeometry::Hermite(samples) => samples.iter().map(|s| s.point).collect(),
                EdgeGeometry::Polyline(p) => p.clone(),
                // Piecewise-kubische Bézier-Kette (hermite_to_nurbs): jeder
                // 3. Kontrollpunkt ist ein On-Curve-Hermite-Sample.
                EdgeGeometry::Nurbs(c) => c.control_points.iter().copied().step_by(3).collect(),
            };
            for p in pts.iter().step_by(4) {
                for br in [e.signature.0, e.signature.1] {
                    let (hs, xf) = &halfspaces[&br.as_u32()];
                    let (f, _) =
                        crate::sdf_dual::displaced_halfspace_value_grad(hs, xf, &disp, *p);
                    assert!(
                        f.abs() < 1e-6,
                        "Kanten-Sample {p:?} nicht auf displacter Fläche {br:?}: g = {f}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 0, "keine Kanten-Samples geprüft");
    }

    /// Displace-B-Rep, GEMISCHTER Fall (disp wechselt das Vorzeichen längs
    /// der Kanten): keine exakte Kasten-Topologie möglich (s.o. — die
    /// disp>0-Abschnitte sind runde Blend-Streifen ohne scharfe Naht). Der
    /// ehrliche Kontrakt: der Weave läuft durch, und JEDER emittierte
    /// Ketten-Sample liegt auf dem realen displacten Body (nichts schwebt im
    /// Blend-Streifen oder in der Luft).
    #[test]
    fn hybrid_displaced_box_mixed_sign_chain_samples_on_body() {
        let body = BoundedCsg::from_csg(&CsgNode::Displace(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [4.0, 6.0, 4.0])),
            0.3,
            1.5,
        ));
        let brep = weave_branch_seeds_hybrid(&body, 0.5).expect("Displace-Body im Partial-Scope");
        assert!(!brep.edges.is_empty(), "geschnitzte Naht-Abschnitte müssen weben");
        let mut checked = 0usize;
        for e in &brep.edges {
            let pts: Vec<[f64; 3]> = match &e.geometry {
                EdgeGeometry::Hermite(samples) => samples.iter().map(|s| s.point).collect(),
                EdgeGeometry::Polyline(p) => p.clone(),
                // Piecewise-kubische Bézier-Kette (hermite_to_nurbs): jeder
                // 3. Kontrollpunkt ist ein On-Curve-Hermite-Sample.
                EdgeGeometry::Nurbs(c) => c.control_points.iter().copied().step_by(3).collect(),
            };
            for p in pts.iter().step_by(4) {
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let (d, _) = crate::sdf_dual::sdf_gradient(&body, pf);
                assert!(
                    d.abs() < 2e-3,
                    "Ketten-Sample {p:?} nicht auf dem displacten Body: f = {d}"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "keine Kanten-Samples geprüft");
    }

    #[test]
    fn scope_rejects_non_lipschitz_gyroid() {
        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Gyroid { cell_size: 4.0, thickness: 1.0 },
        };
        let body = BoundedCsg::from_csg(&csg);
        assert!(!scope_supported(&body));
    }

    /// A gyroid body stays out of the STRICT scope (id_hash toggle-invariance)
    /// but is admitted by the PARTIAL gate.
    #[test]
    fn partial_scope_admits_gyroid_but_strict_does_not() {
        let csg = CsgNode::Intersection(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Gyroid { cell_size: 4.0, thickness: 0.3 },
            }),
        );
        let body = BoundedCsg::from_csg(&csg);
        assert!(!scope_supported(&body), "strict scope must keep rejecting gyroid");
        assert!(scope_supported_partial(&body), "partial scope must admit gyroid");
    }

    /// End-to-end: Box∩Gyroid through the graceful partial weave — the DL-71
    /// lift. The result must carry real edges: the box's own 12 edges (where
    /// they survive the intersection) plus the gyroid rim seams on the box
    /// faces, every sampled point ON the body boundary.
    #[test]
    fn partial_weave_box_intersect_gyroid_has_rim_edges() {
        let csg = CsgNode::Intersection(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Gyroid { cell_size: 4.0, thickness: 0.3 },
            }),
        );
        let body = BoundedCsg::from_csg(&csg);
        let brep = weave_branch_seeds_partial(&body).expect("gyroid body must partial-weave");
        assert!(!brep.edges.is_empty(), "no edges woven");

        // At least one seam must involve the gyroid branch (feature 2).
        let gyroid_edges = brep
            .edges
            .iter()
            .filter(|e| e.signature.0.feature_id() == 2 || e.signature.1.feature_id() == 2)
            .count();
        assert!(gyroid_edges > 0, "no gyroid rim seams woven");
        println!(
            "box∩gyroid partial weave: {} edges total, {} gyroid rim seams, {} vertices",
            brep.edges.len(),
            gyroid_edges,
            brep.vertices.len()
        );

        // Every sampled edge point lies on the body boundary (Visual mode, the
        // renderer's field): |f| within a loose band — the samples are exact
        // f64 on their two carrier surfaces, the body eval is f32.
        let mut checked = 0usize;
        for e in &brep.edges {
            // finish_weave fits Hermite chains to NURBS — sample via the
            // shared tessellator (same path body_brep_edges exports).
            let pts = crate::brep::edge_sampling::sample_edge(&e.geometry);
            for q in pts {
                let f = body.sdf(
                    [q[0] as f32, q[1] as f32, q[2] as f32],
                    crate::csg::EvalMode::Logical,
                );
                assert!(f.abs() < 1e-2, "edge point {q:?} off the body boundary: {f}");
                checked += 1;
            }
        }
        assert!(checked > 100, "suspiciously few edge samples: {checked}");
    }

    /// Ticket 7sp1: the l45r gyroid sliver was missing its two vertical
    /// cylinder×box-side-face seams (the SSI scan masked away crossings lying
    /// on the overlap wall). Both must now be woven.
    #[test]
    fn ticket_7sp1_cylinder_box_side_seams_are_woven() {
        let gy = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Gyroid { cell_size: 10.0, thickness: 1.0 },
        };
        let sph = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere { center: [0.0; 3], radius: 50.0 },
        };
        let cyl = cylinder_leaf(3, [0.0, 0.0, 0.0], 30.0, 60.0);
        let bx = box_leaf(4, [7.970110677489131, 22.314295, 29.869688], [3.0, 4.0, 3.0]);
        let csg = CsgNode::Intersection(
            Box::new(CsgNode::Difference(
                Box::new(CsgNode::Intersection(Box::new(sph), Box::new(gy))),
                Box::new(cyl),
            )),
            Box::new(bx),
        );
        let body = BoundedCsg::from_csg(&csg);
        let brep = weave_branch_seeds_partial(&body).expect("sliver must weave");
        let has = |fa: u32, sa: u32, fb: u32, sb: u32| {
            brep.edges.iter().any(|e| {
                let (a, b) = e.signature;
                (a.feature_id() == fa && a.sub() == sa && b.feature_id() == fb && b.sub() == sb)
                    || (a.feature_id() == fb && a.sub() == sb && b.feature_id() == fa && b.sub() == sa)
            })
        };
        assert!(has(3, 0, 4, 1), "middle vertical seam (cylinder × box −x) missing");
        assert!(has(3, 0, 4, 0), "right vertical seam (cylinder × box +x) missing");
        assert!(brep.edges.len() >= 12, "expected the full sliver edge set, got {}", brep.edges.len());
    }

    /// Ticket a26a: a small *scaled* sphere unioned onto the l45r gyroid
    /// sliver must weave its sphere×gyroid-wall seam — the reporter saw the
    /// selected gyroid face missing exactly that bounding edge.
    #[test]
    fn ticket_a26a_scaled_sphere_gyroid_seam_is_woven() {
        let gy = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Gyroid { cell_size: 10.0, thickness: 1.0 },
        };
        let sph = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere { center: [0.0; 3], radius: 50.0 },
        };
        let cyl = cylinder_leaf(3, [0.0, 0.0, 0.0], 30.0, 60.0);
        let bx = box_leaf(4, [7.970110677489131, 22.314295, 29.869688], [3.0, 4.0, 3.0]);
        let sliver = CsgNode::Intersection(
            Box::new(CsgNode::Difference(
                Box::new(CsgNode::Intersection(Box::new(sph), Box::new(gy))),
                Box::new(cyl),
            )),
            Box::new(bx),
        );
        // Sphere 6: r=50 scaled ×0.02 → effective r=1, translated onto the wall.
        let small = CsgNode::Transform(
            fielddraft_protocol::Transform {
                translate: [5.2107882, 22.11431, 29.24847473977133],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [0.02, 0.02, 0.02],
            },
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(6),
                primitive: Primitive::Sphere { center: [0.0; 3], radius: 50.0 },
            }),
        );
        let csg = CsgNode::Union(Box::new(sliver), Box::new(small));
        let body = BoundedCsg::from_csg(&csg);
        let brep = weave_branch_seeds_partial(&body).expect("body must partial-weave");

        let has_pair = |fa: u32, fb: u32| {
            brep.edges.iter().any(|e| {
                let (a, b) = e.signature;
                (a.feature_id() == fa && b.feature_id() == fb)
                    || (a.feature_id() == fb && b.feature_id() == fa)
            })
        };
        // The sphere pokes out of the gyroid wall (one cap → one closed ring)…
        assert!(has_pair(2, 6), "scaled sphere × gyroid wall seam missing");
        // …and out of the box −x face; the SSI closed form used to lift the
        // sphere's LOCAL radius (50) into the world circle, so this pair
        // silently seeded nothing and the face loops of 4.1/6.0 collapsed.
        assert!(has_pair(4, 6), "scaled sphere × box face seam missing");
        assert!(has_pair(3, 6), "scaled sphere × cylinder bore seam missing");

        // Every sampled edge point lies ON the body boundary.
        for e in &brep.edges {
            for q in crate::brep::edge_sampling::sample_edge(&e.geometry) {
                let f = body.sdf(
                    [q[0] as f32, q[1] as f32, q[2] as f32],
                    crate::csg::EvalMode::Logical,
                );
                assert!(
                    f.abs() < 1e-2,
                    "edge point {q:?} off the body boundary: {f} (edge {:?})",
                    e.signature
                );
            }
        }
    }

    /// Ticket 7cv2 diagnostic: face loops of the sliver.
    #[test]
    #[ignore]
    fn ticket_7cv2_face_loops_dump() {
        let gy = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Gyroid { cell_size: 10.0, thickness: 1.0 },
        };
        let sph = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere { center: [0.0; 3], radius: 50.0 },
        };
        let cyl = cylinder_leaf(3, [0.0, 0.0, 0.0], 30.0, 60.0);
        let bx = box_leaf(4, [7.970110677489131, 22.314295, 29.869688], [3.0, 4.0, 3.0]);
        let csg = CsgNode::Intersection(
            Box::new(CsgNode::Difference(
                Box::new(CsgNode::Intersection(Box::new(sph), Box::new(gy))),
                Box::new(cyl),
            )),
            Box::new(bx),
        );
        let body = BoundedCsg::from_csg(&csg);
        let brep = weave_branch_seeds_partial(&body).expect("must weave");
        for (k, e) in brep.edges.iter().enumerate() {
            println!("edge {k}: {}.{} x {}.{}", e.signature.0.feature_id(), e.signature.0.sub(),
                e.signature.1.feature_id(), e.signature.1.sub());
        }
        for (k, e) in brep.edges.iter().enumerate() {
            let pts = crate::brep::edge_sampling::sample_edge(&e.geometry);
            let coords: Vec<String> = pts.iter().map(|p| format!("[{:.4},{:.4},{:.4}]", p[0], p[1], p[2])).collect();
            println!("POLY {k} {}.{}x{}.{} [{}]",
                e.signature.0.feature_id(), e.signature.0.sub(),
                e.signature.1.feature_id(), e.signature.1.sub(), coords.join(","));
        }
        for (fk, f) in brep.faces.iter().enumerate() {
            let fmt_loop = |lp: &crate::brep::Loop| lp.edges.iter().map(|(e, r)| format!("{}{}", e.0, if *r {"r"} else {""})).collect::<Vec<_>>().join(",");
            println!("face {fk}: branch {}.{} outer=[{}] inner={}",
                f.branch_id.feature_id(), f.branch_id.sub(),
                fmt_loop(&f.outer),
                f.inner.iter().map(|l| format!("[{}]", fmt_loop(l))).collect::<Vec<_>>().join(" "));
        }
    }

    /// Ticket p6pv diagnostic: full Sphere ∩ Gyroid (the holey ball).
    #[test]
    #[ignore]
    fn ticket_p6pv_sphere_gyroid_dump() {
        let gy = CsgNode::Leaf {
            id: FeatureId::from_u32(2),
            primitive: Primitive::Gyroid { cell_size: 10.0, thickness: 1.0 },
        };
        let sph = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Sphere { center: [0.0; 3], radius: 50.0 },
        };
        let csg = CsgNode::Intersection(Box::new(sph), Box::new(gy));
        let body = BoundedCsg::from_csg(&csg);
        let res = crate::brep::branch_seed::seed_branch_pairs(&body);
        println!("seeds: {} unresolved: {}", res.seeds.len(), res.unresolved.len());
        let t0 = std::time::Instant::now();
        let brep = weave_branch_seeds_partial(&body);
        println!("weave took {:?}", t0.elapsed());
        match &brep {
            Some(b) => {
                println!("edges: {} vertices: {}", b.edges.len(), b.vertices.len());
                let mut closed = 0; let mut open = 0;
                for e in &b.edges { if e.start.is_none() && e.end.is_none() { closed += 1; } else { open += 1; } }
                println!("closed rings: {closed}, open arcs: {open}");
            }
            None => println!("weave returned NONE"),
        }
    }

    #[test]
    fn cylinder_yields_two_closed_rim_rings() {
        // A plain cylinder weaves into a wall plus two caps, joined at exactly
        // two rim circles — closed-loop edges with NO bounding vertices (the two
        // cap∩wall seams never reach a third surface, so no trim occurs).
        let body = BoundedCsg::from_csg(&cylinder_leaf(1, [0.0, 0.0, 0.0], 5.0, 8.0));
        let wf = build_seed_wireframe(&body, SeedMode::Strict).expect("cylinder is seed-weavable");
        assert_eq!(wf.edges.len(), 2, "two rims");
        assert_eq!(wf.vertices.len(), 0, "no corner vertices on a plain cylinder");
        for e in &wf.edges {
            assert!(e.start.is_none() && e.end.is_none(), "rims are closed loops");
        }
    }

    #[test]
    fn box_yields_eight_corners_and_twelve_edges() {
        // Acceptance test for the eps/refine path: the raw trim points are only
        // approximate, but `refine_triple_vertex` re-solves each corner exactly,
        // so all 8 vertices must land precisely on the box corners (±10).
        let h = 10.0f32;
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [h, h, h]));
        let wf = build_seed_wireframe(&body, SeedMode::Strict).expect("box is seed-weavable");
        assert_eq!(wf.vertices.len(), 8, "eight box corners");
        assert_eq!(wf.edges.len(), 12, "twelve box edges");

        // Every coordinate is ±10 within 1e-3, and all 8 sign combinations occur.
        let mut signs = std::collections::HashSet::new();
        for v in &wf.vertices {
            let mut bits = [false; 3];
            for (axis, &c) in v.position.iter().enumerate() {
                assert!(
                    (c.abs() - h as f64).abs() < 1e-3,
                    "corner coord {c} not ±{h} (axis {axis})"
                );
                bits[axis] = c > 0.0;
            }
            signs.insert(bits);
        }
        assert_eq!(signs.len(), 8, "all eight sign combinations present");

        // Every box edge is open with both endpoints resolved to corners.
        for e in &wf.edges {
            assert!(
                e.start.is_some() && e.end.is_some(),
                "box edges are vertex-bounded"
            );
        }
    }

    #[test]
    fn box_corner_branch_sets_are_triples() {
        let body = BoundedCsg::from_csg(&box_leaf(3, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let wf = build_seed_wireframe(&body, SeedMode::Strict).expect("box is seed-weavable");
        for v in &wf.vertices {
            assert_eq!(v.branches.len(), 3, "a box corner joins exactly three faces");
            for b in &v.branches {
                assert_eq!(b.feature_id(), 3, "all three faces are this box's");
            }
        }
    }

    #[test]
    fn weave_branch_seeds_box_is_watertight() {
        let body = BoundedCsg::from_csg(&box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        let brep = weave_branch_seeds(&body).expect("box weaves via seeds");
        assert_eq!(brep.faces.len(), 6, "a box has six faces");
    }

    /// Sorted multiset of stable element handles — what `qFace`/`qEdge`/`qVertex`
    /// address. Mirrors `topo_handles` in `weaving.rs` tests.
    fn handles(brep: &BRep) -> (Vec<u64>, Vec<u64>, Vec<u64>) {
        use crate::brep::naming::TopoRef;
        use crate::brep::{EdgeId, FaceId, VertexId};
        let mut f: Vec<u64> = (0..brep.faces.len())
            .map(|k| TopoRef::from_face(brep, FaceId(k as u32)).id_hash)
            .collect();
        let mut e: Vec<u64> = (0..brep.edges.len())
            .map(|k| TopoRef::from_edge(brep, EdgeId(k as u32)).id_hash)
            .collect();
        let mut v: Vec<u64> = (0..brep.vertices.len())
            .map(|k| TopoRef::from_vertex(brep, VertexId(k as u32)).id_hash)
            .collect();
        f.sort_unstable();
        e.sort_unstable();
        v.sort_unstable();
        (f, e, v)
    }

    /// Assert the seed weave of `body` is watertight and byte-identical in element
    /// handles to the octree weave — the Phase-2b contract for the bodies the
    /// branch-seed path newly covers (booleans, sweeps, the cone).
    fn assert_seed_matches_octree(body: &BoundedCsg, cell: f32) {
        use crate::brep::octree::Octree;
        use crate::brep::weaving::weave;
        let octree = Octree::build(body, cell);
        let sdf = weave(body, &octree);
        let seed = weave_branch_seeds(body).expect("body must be seed-weavable");
        assert!(seed.shells[0].is_closed, "seed weave must be watertight");
        assert_eq!(
            (seed.faces.len(), seed.edges.len(), seed.vertices.len()),
            (sdf.faces.len(), sdf.edges.len(), sdf.vertices.len()),
            "seed and octree topology counts must agree"
        );
        assert_eq!(
            handles(&seed),
            handles(&sdf),
            "seed and octree weave must yield identical element handles"
        );
    }

    #[test]
    fn seed_box_minus_cylinder_matches_octree() {
        // The headline boolean: a through-hole. 12 box edges + 2 hole rims = 14
        // edges, 7 faces (6 box + cylinder wall), 8 corners. The cylinder's own
        // cap rims are trimmed away by the difference.
        let body = BoundedCsg::from_csg(&CsgNode::Difference(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(cylinder_leaf(2, [0.0, 0.0, 0.0], 5.0, 12.0)),
        ));
        assert_seed_matches_octree(&body, 0.5);
    }

    #[test]
    fn seed_box_union_sphere_matches_octree() {
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere { center: [10.0, 0.0, 0.0], radius: 8.0 },
            }),
        ));
        assert_seed_matches_octree(&body, 0.5);
    }

    #[test]
    fn seed_box_union_big_sphere_trimmed_arcs_matches_octree() {
        // Regression for the "missing edges" bug: a sphere large enough that its
        // intersection circle with a box face pokes BEYOND that face's edges, so
        // a third surface (the adjacent box faces) trims the single circle
        // component into several disjoint active arcs. With one seed per
        // component only one arc was traced and the rest vanished (the reported
        // screenshot). SSI now seeds densely along each component, so every arc
        // is traced — the seam must close and match the octree exactly.
        //
        // Box half [10,25,10]; sphere r=12 centred on the +x face plane (10,0,0):
        // the great circle r=12 in plane x=10 leaves the face through z=±10
        // (|12·sinθ|≤10 ⇒ two arcs) while staying within |y|≤25, and the sphere
        // also bulges out the z=±10 faces — a closed 4-arc neck seam.
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 25.0, 10.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere { center: [10.0, 0.0, 0.0], radius: 12.0 },
            }),
        ));
        assert_seed_matches_octree(&body, 0.5);
    }

    #[test]
    fn seed_box_union_corner_sphere_three_faces_matches_octree() {
        // The reported screenshot: box half 50, sphere r=50 offset toward a
        // corner so it pokes out of THREE faces at once. Each face's
        // intersection circle is trimmed by the two adjacent faces into arcs,
        // and the arcs on the three faces stitch into one closed corner seam.
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [50.0, 50.0, 50.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere { center: [30.0, 30.0, 30.0], radius: 50.0 },
            }),
        ));
        assert_seed_matches_octree(&body, 2.0);
    }

    #[test]
    fn seed_box_union_sphere_grazing_face_corner_watertight() {
        // EXACT geometry from the reported .fdraft (project (3).fdraft): a big
        // sphere grazes the box's -x face near the +y/-z corner so its
        // intersection circle pokes past that face's boundary edges, crossing
        // each box edge it meets at TWO points. Those two triple-points sit only
        // ≈0.1·diag apart (the sphere inflates the model diagonal), so the corner
        // gather cluster lumped them into one impossible vertex → `Spurious` →
        // whole body to octree, which itself wove degenerate/missing arcs (the
        // reported screenshot). The split-on-spurious resolver must now separate
        // the two corners and weave a watertight B-Rep analytically — WITHOUT
        // the octree fallback (octree is wrong here, so we assert the seed path
        // succeeds on its own rather than matching it).
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [-23.490438, 1.4090451, 5.554038], [50.0, 50.0, 50.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere {
                    center: [-86.59913, 4.6575217, 0.0],
                    radius: 50.0,
                },
            }),
        ));
        let seed = weave_branch_seeds(&body)
            .expect("grazing sphere must weave analytically, not fall back to octree");
        assert!(
            seed.shells[0].is_closed,
            "seed weave of the grazing-sphere union must be watertight"
        );
        // 8 faces: 6 box + sphere, PLUS the corner sliver (F0-1 per-region
        // materialisation): the circle crosses both adjacent box edges twice,
        // so the grazed -x wall keeps TWO disjoint regions — the main face
        // (rect edges + arc) and a tiny corner triangle between the two
        // triple-point pairs. Pre-F0-1 the sliver was packed into the main
        // face as a fake "hole"; now it is its own face (spatial_index
        // disambiguates). Watertightness is the correctness gate.
        assert_eq!(seed.faces.len(), 8, "6 box faces + sphere + corner sliver");
    }

    #[test]
    fn seed_box_union_sphere_grazing_top_edge_watertight() {
        // Ticket 20260609-193134-el1x: the SAME .fdraft as the grazing-face-corner
        // test, but after the user dragged the sphere up (`translate` id 12, z) so
        // it now grazes the box's TOP edge near the -x/+y corner. The sphere centre
        // sits at z=103.04 — only its underside (z down to 53.04) dips below the box
        // top (z=55.55), poking through the -x face AND the +z face in a sliver.
        // The reported render showed the -x-face intersection arc overshooting the
        // box top edge into a self-crossing closed lens (z up to ~56.1, above the
        // box) instead of being trimmed at the top edge. The seed weave must close
        // watertight with the 7-face count, no overshoot past the box bounds.
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [-23.490438, 1.4090451, 5.554038], [50.0, 50.0, 50.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere {
                    center: [-86.59913, 4.6575217, 103.036758],
                    radius: 50.0,
                },
            }),
        ));
        // The octree route (the legacy `parametric_weave = false` path) CANNOT
        // resolve this: the grazing lens is ~1.3 mm tall but the body-relative
        // octree cell here is ~6 mm, so the feature falls between samples and the
        // octree weaves a degenerate shell (4 faces, missing the sphere cap — the
        // reported "Union falsch"). The seed/parametric path resolves it from the
        // analytic branch adjacency, independent of any cell size — that is exactly
        // the failure class the parametric-weaving rebuild replaces the octree for.
        let seed = weave_branch_seeds(&body)
            .expect("grazing-top-edge sphere must weave analytically, not fall back to octree");
        assert!(
            seed.shells[0].is_closed,
            "seed weave of the grazing-top-edge union must be watertight"
        );
        assert_eq!(seed.faces.len(), 7, "6 box faces + 1 sphere face");
    }

    #[test]
    fn seed_box_union_sphere_grazing_top_edge_sliver_watertight() {
        // Ticket 20260610-084953-g4rr: same .fdraft family as the el1x guards,
        // sphere dragged even higher (z=103.458) so it penetrates the box's top
        // -x edge by only ~0.33 mm — a quarter of the el1x sliver. The user
        // reports "Im Union-Bereich Kugel / Würfel degeneriert die Kante".
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [-23.490438, 1.4090451, 5.554038], [50.0, 50.0, 50.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Sphere {
                    center: [-86.59913, 4.231104, 103.45809],
                    radius: 50.0,
                },
            }),
        ));
        let seed = weave_branch_seeds(&body)
            .expect("sliver-grazing sphere must weave analytically, not fall back to octree");
        assert!(
            seed.shells[0].is_closed,
            "seed weave of the sliver-grazing union must be watertight"
        );
        assert_eq!(seed.faces.len(), 7, "6 box faces + 1 sphere face");
        // The full corner topology, which the original report degenerated:
        // the lens crossing the top edge yields TWO distinct triple vertices
        // (not one merged cluster), and the grazed top edge is SPLIT there
        // with the sphere-covered stretch (y ∈ [-1.55, 10.01]) dropped.
        assert_eq!(seed.vertices.len(), 10, "8 box corners + 2 lens triple points");
        assert_eq!(
            seed.edges.len(),
            15,
            "11 plain box edges + 2 top-edge fragments + 2 lens arcs"
        );
        // No lens-arc sample may overshoot the box bounds (the reported
        // self-crossing scribble): the -x face plane is x = -73.4904, the
        // top face plane z = 55.554; allow a few microns of trim slack.
        for e in &seed.edges {
            let (a, b) = (e.signature.0.as_u32(), e.signature.1.as_u32());
            if a != 8192 && b != 8192 {
                continue;
            }
            if let EdgeGeometry::Hermite(samples) = &e.geometry {
                for s in samples {
                    assert!(
                        s.point[2] < 55.5541 + 0.01,
                        "lens sample above the box top: {:?}",
                        s.point
                    );
                    assert!(
                        s.point[0] > -73.4905 - 0.01,
                        "lens sample beyond the -x face: {:?}",
                        s.point
                    );
                }
            }
        }
    }

    #[test]
    fn seed_extrude_rectangle_matches_octree() {
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [0.0, 0.0],
            half_extents: [5.0, 3.0],
        });
        let body = BoundedCsg::from_csg(&CsgNode::Extrude { wall_subs: None,
            id: FeatureId::from_u32(7),
            sketch: Box::new(sketch),
            half_height: 2.0,
        });
        assert_seed_matches_octree(&body, 0.5);
    }

    #[test]
    fn seed_frustum_cone_matches_octree() {
        // A lone cone is NOT a grundform special case (DL-76 apex convention left
        // it to the octree in M1); the seed path now weaves it analytically into
        // the same watertight 3-face frustum, handle-identical.
        let body = BoundedCsg::from_csg(&CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Cone { r_bottom: 1.5, r_top: 0.8, half_height: 0.6 },
        });
        assert_seed_matches_octree(&body, 0.1);
    }

    #[test]
    fn seed_revolve_rectangle_matches_octree() {
        // A rectangle profile at r ∈ [3, 7], y ∈ [−10, 10] revolved about Y: a
        // genus-1 tube. All four wall groups lift analytically (two coaxial
        // cylinders + two annulus planes); the four rim circles come from the
        // new coaxial SSI route. Handles must match the octree weave exactly.
        let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 0.0],
            half_extents: [2.0, 10.0],
        });
        let body = BoundedCsg::from_csg(&CsgNode::Revolve { wall_subs: None,
            id: FeatureId::from_u32(3),
            sketch: Box::new(sketch),
        });
        assert_seed_matches_octree(&body, 0.5);
    }

    #[test]
    fn seed_revolve_bezier_profile_matches_octree() {
        // A closed path profile with a quadratic-Bézier outer wall: the curved
        // group carries a RevolvedProfile carrier (no Plane/Cyl/Cone lift), so
        // this exercises the coaxial SSI route against a sampled carrier. The
        // four corner circles bound four faces; the Bézier wall's circles meet
        // the two annulus planes at (≈6.x, ±5).
        use fielddraft_protocol::SketchSegment;
        let seg_line = |a: [f32; 2], b: [f32; 2]| SketchSegment::Line { start: a, end: b };
        let profile = Sketch2d::Prim(Sketch2dPrimitive::Path {
            segments: vec![
                seg_line([1.0, -5.0], [6.0, -5.0]),
                SketchSegment::QuadBezier {
                    p0: [6.0, -5.0],
                    p1: [8.0, 0.0],
                    p2: [6.0, 5.0],
                },
                seg_line([6.0, 5.0], [1.0, 5.0]),
                seg_line([1.0, 5.0], [1.0, -5.0]),
            ],
            closed: true,
        });
        let body = BoundedCsg::from_csg(&CsgNode::Revolve { wall_subs: None,
            id: FeatureId::from_u32(4),
            sketch: Box::new(profile),
        });
        assert_seed_matches_octree(&body, 0.25);
    }

    #[test]
    fn octree_revolve_union_extrude_does_not_degenerate() {
        // Regression Ticket qmvn: der Octree-BRep-Weave degenerierte für Körper
        // mit Revolve; der Fix führte den relax-korrigierten Lipschitz-Bracket
        // (`diag / march_relax`) ein, der überschätzte Oberflächenzellen wieder
        // einfängt. Seit #384 Stufe 2 ist Revolve als 1-Lipschitz hergeleitet
        // (relax = 1.0) — der Test prüft den Bracket-Mechanismus daher mit
        // einem ERZWUNGENEN relax < 1 (er gilt weiterhin für echte
        // Über-Schätzer wie Twist/Displace/Chamfer) plus den unveränderten
        // Weave auf dem Revolve-Body.
        //
        // Aufbau: ein Rechteck-Profil bei r ∈ [3, 7], y ∈ [−4, 4] um Y revolved
        // (ein Ring/Rohr) ∪ ein überlappendes Extrude-Prisma, das in die
        // Revolve-Wand hineinragt.
        use crate::brep::octree::{Octree, OctreeConfig};
        use crate::brep::weaving::weave;

        let make_body = || {
            let ring_profile = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                center: [5.0, 0.0],
                half_extents: [2.0, 4.0],
            });
            let prism_profile = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
                center: [5.0, 0.0],
                half_extents: [2.5, 2.0],
            });
            BoundedCsg::from_csg(&CsgNode::Union(
                Box::new(CsgNode::Revolve { wall_subs: None,
                    id: FeatureId::from_u32(3),
                    sketch: Box::new(ring_profile),
                }),
                Box::new(CsgNode::Extrude { wall_subs: None,
                    id: FeatureId::from_u32(7),
                    sketch: Box::new(prism_profile),
                    half_height: 6.0,
                }),
            ))
        };
        let body = make_body();

        // #384 Stufe 2: Das Revolve-Tape-Feld ist als 1-Lipschitz hergeleitet
        // (Meridian-Map + exakte 2D-Distanz), `march_relax` ist daher 1.0 —
        // die frühere 0.7-Pauschale ist weg. Der Bracket-MECHANISMUS (Ticket
        // qmvn) bleibt für echte Über-Schätzer (Twist/Displace/Chamfer)
        // nötig; dieser Test erzwingt ihn deshalb explizit mit relax = 0.7
        // statt über den Body-Wert.
        let relax = crate::sdf_compiler::march_relax(&body);
        assert_eq!(relax, 1.0, "Revolve-Union ist 1-Lipschitz, war {relax}");

        let cfg = OctreeConfig::with_min_cell_size(0.5);
        let octree = Octree::build_with_bracket_relax_for_test(&body, cfg, 0.7);

        // Kern-Assertion (discriminating): der relax-korrigierte Bracket muss
        // STRIKT MEHR oberflächennahe Edge/Vertex-Zellen behalten als der
        // unkorrigierte (Lipschitz-)Bracket. Letzteren bilden wir nach, indem
        // wir denselben Body um 1/relax aufblasen (alle Distanzen × 1/relax,
        // diag wächst genauso) — das hebt die relax-Korrektur exakt auf. Vor dem
        // Fix waren beide Zahlen gleich (der Octree ignorierte relax); jetzt
        // gewinnt die korrigierte Variante die überschätzten Zellen zurück.
        let feature_cells = |o: &Octree| o.edge_cells().count() + o.vertex_cells().count();
        let corrected = feature_cells(&octree);

        // Referenzlauf ohne Korrektur: gleiche Klassifikation, aber Bracket = diag.
        let uncorrected = {
            let body = make_body();
            let oct = Octree::build_with_bracket_relax_for_test(&body, cfg, 1.0);
            oct.edge_cells().count() + oct.vertex_cells().count()
        };
        assert!(
            corrected > uncorrected,
            "relax-Korrektur muss mehr Oberflächenzellen behalten: \
             korrigiert={corrected} vs. unkorrigiert={uncorrected}"
        );

        // Produktionspfad-Absicherung (#384 Stufe 2): auch mit dem
        // body-eigenen relax = 1.0 (nackter Lipschitz-Bracket) darf die
        // Klassifikation keine Revolve-Oberflächenzellen verlieren — sonst
        // wäre die 1-Lipschitz-Herleitung falsch.
        let production = Octree::build_with(&body, cfg);
        let brep_prod = weave(&body, &production);
        assert!(
            brep_prod.validate().is_ok() && !brep_prod.faces.is_empty(),
            "Produktions-Weave (relax 1.0) degeneriert: {:?}, {} Faces",
            brep_prod.validate(),
            brep_prod.faces.len()
        );

        // Und der gewobene BRep ist wohlgeformt und nicht-degeneriert.
        let brep = weave(&body, &octree);
        assert!(
            brep.validate().is_ok(),
            "B-Rep muss wohlgeformt sein: {:?}",
            brep.validate()
        );
        assert!(
            !brep.faces.is_empty() && !brep.edges.is_empty(),
            "BRep degeneriert: {} Faces, {} Edges",
            brep.faces.len(),
            brep.edges.len()
        );
    }

    #[test]
    fn seed_shelled_box_matches_octree() {
        // Shell duplicates every branch as its inward-offset inner wall. Before
        // the Shell-aware pair enumeration the seed path silently wove a
        // 6-face SOLID box for this body (no cavity at all — no inner-wall
        // branches were ever paired); the octree weaves the true 12 faces.
        let csg = CsgNode::Shell(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            2.0,
            vec![],
        );
        let body = BoundedCsg::from_csg(&csg);
        assert_seed_matches_octree(&body, 0.5);
    }


    #[test]
    fn hybrid_seed_fills_skipped_seams_from_the_octree() {
        // Stufe C on the Stufe-A/B scenario: box ∪ torus — the torus tube
        // pokes out of the box's ±x/±z faces, every box×torus cross pair is
        // analytically unresolvable (`unresolved`). The PARTIAL weave keeps
        // the box's exact wireframe but has NO transition seams (open shell);
        // the HYBRID weave must fill exactly those from the octree.
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: 8.0,
                    minor_radius: 3.0,
                },
            }),
        ));
        // Premise: partial has zero cross-signature seams.
        let partial = weave_branch_seeds_partial(&body).expect("partial weaves");
        let cross = |brep: &BRep| {
            brep.edges
                .iter()
                .filter(|e| {
                    e.signature.0.feature_id() != e.signature.1.feature_id()
                })
                .count()
        };
        assert_eq!(cross(&partial), 0, "premise: partial skips all transition seams");

        let min_cell = 0.5;
        let hybrid = weave_branch_seeds_hybrid(&body, min_cell)
            .expect("hybrid weaves whenever partial does");

        // The box's exact analytic edges survive at parametric precision …
        let box_edges = hybrid
            .edges
            .iter()
            .filter(|e| e.signature.0.feature_id() == 1 && e.signature.1.feature_id() == 1)
            .count();
        assert!(
            box_edges >= 12,
            "the box's analytic edges must survive the fill, got {box_edges}"
        );
        for v in hybrid.vertices.iter().filter(|v| {
            v.branches.iter().all(|b| b.feature_id() == 1) && v.branches.len() == 3
        }) {
            for &c in &v.position {
                assert!(
                    (c.abs() - 10.0).abs() < 1e-3,
                    "box corner coord {c} not ±10 (analytic precision lost in the merge)"
                );
            }
        }

        // … and the transition seams are now present, filled from the octree.
        assert!(
            cross(&hybrid) > 0,
            "hybrid must carry box×torus transition seams (octree fill)"
        );
        // Structural sanity of the merged arena.
        assert_eq!(hybrid.validate(), Ok(()), "merged B-Rep must be referentially sound");
        // Closedness metric: edges used by exactly two face loops. The
        // partial (open-shell) result leaves every pierced box face without
        // its seam ring — the fill must add closed seam edges, not just any
        // edges.
        let closed_edges = |brep: &BRep| {
            let mut uses = vec![0usize; brep.edges.len()];
            for f in &brep.faces {
                for &(eid, _) in f
                    .outer
                    .edges
                    .iter()
                    .chain(f.inner.iter().flat_map(|l| l.edges.iter()))
                {
                    uses[eid.0 as usize] += 1;
                }
            }
            uses.iter().filter(|&&u| u == 2).count()
        };
        assert!(
            closed_edges(&hybrid) > closed_edges(&partial),
            "hybrid ({}) must close more edges than partial ({})",
            closed_edges(&hybrid),
            closed_edges(&partial)
        );

        // With the tessellated ring-winding alignment the merged body is
        // fully manifold — every seam ring complements its analytic
        // partner's orientation.
        assert_eq!(
            hybrid.manifold_violations(),
            Vec::<String>::new(),
            "hybrid box∪torus must be orientation-clean"
        );
        // And the pure octree route shares the same convention.
        let octree = crate::brep::octree::Octree::build(&body, min_cell);
        let full = crate::brep::weaving::weave(&body, &octree);
        assert_eq!(
            full.manifold_violations(),
            Vec::<String>::new(),
            "octree box∪torus must be orientation-clean"
        );
    }

    #[test]
    fn partial_seed_recovers_operand_edges_when_strict_bails() {
        // Stufe A+B (tickets 7vbe/qmvn): a Union of a fully seed-weavable
        // operand (a box) with an operand whose walls are NON-analytic (a torus
        // — its tube wall has no closed-form half-space carrier, so every
        // box×torus cross pair is `unresolved`). The torus pokes its tube
        // (major 8 + minor 3 = outer radius 11 > box half-extent 10) out of the
        // box's ±x/±z faces, so the bodies genuinely intersect.
        //
        // The STRICT all-or-nothing seed path bails on the very first
        // unresolvable cross pair → the WHOLE body would be octree-woven
        // (coarse, tessellated, no parametric edge structure). The GRACEFUL
        // path skips only the unresolvable torus transition seams and keeps the
        // box operand's full analytic wireframe — exactly the user's ask "der
        // BRep soll weiter über die parametrischen Einzelkörper laufen".
        let body = BoundedCsg::from_csg(&CsgNode::Union(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(2),
                primitive: Primitive::Torus {
                    center: [0.0, 0.0, 0.0],
                    major_radius: 8.0,
                    minor_radius: 3.0,
                },
            }),
        ));

        // Premise: the body really does bail the strict path (otherwise this
        // test would not exercise the graceful fallback at all).
        let res = seed_branch_pairs(&body);
        assert!(
            !res.unresolved.is_empty(),
            "premise: the torus cross pairs must be unresolved (got {} unresolved)",
            res.unresolved.len()
        );
        assert!(
            weave_branch_seeds(&body).is_none(),
            "premise: strict seed weave must bail (all-or-nothing) for this body"
        );

        // The graceful path recovers a real B-Rep instead of nothing.
        let partial =
            weave_branch_seeds_partial(&body).expect("graceful seed weave must yield a B-Rep");

        // It carries the box operand's FULL parametric wireframe: 8 corner
        // vertices + 12 straight edges, all tagged with the box feature (id 1).
        // The strict path delivered ZERO of these (it returned None); these are
        // the "parametrischen Einzelkörper" edges the ticket wants kept.
        let box_edges = partial
            .edges
            .iter()
            .filter(|e| e.signature.0.feature_id() == 1 && e.signature.1.feature_id() == 1)
            .count();
        assert_eq!(box_edges, 12, "all 12 analytic box edges must be present");

        let box_corners = partial
            .vertices
            .iter()
            .filter(|v| v.branches.iter().all(|b| b.feature_id() == 1) && v.branches.len() == 3)
            .count();
        assert_eq!(box_corners, 8, "all 8 analytic box corners must be present");

        // Each box corner sits exactly on a ±10 box corner (analytic precision,
        // not the octree's sampled approximation).
        for v in partial.vertices.iter().filter(|v| {
            v.branches.iter().all(|b| b.feature_id() == 1) && v.branches.len() == 3
        }) {
            for &c in &v.position {
                assert!(
                    (c.abs() - 10.0).abs() < 1e-3,
                    "box corner coord {c} not ±10 (analytic precision lost)"
                );
            }
        }

        // The recovered wireframe is structurally richer than the bare octree
        // fallback the strict bail would otherwise force: at least the 12 exact
        // box edges, every one a 2-point Polyline / NURBS the octree route never
        // produces (it tessellates). We assert the parametric provenance rather
        // than a raw edge count (the octree may over- or under-count).
        assert!(
            partial.edges.len() >= 12,
            "partial weave must keep at least the 12 box edges, had {}",
            partial.edges.len()
        );
    }

    #[test]
    fn weave_branch_seeds_returns_none_for_unresolvable() {
        // A domain warp is out of analytic scope: the half-space map describes the
        // un-warped box, so seeding is invalid. `scope_supported` gates it before
        // the dynamic pipeline ever runs, yielding None (→ octree).
        let csg = CsgNode::Twist(
            Box::new(box_leaf(1, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
            0.01,
        );
        let body = BoundedCsg::from_csg(&csg);
        assert!(weave_branch_seeds(&body).is_none());
    }


}
