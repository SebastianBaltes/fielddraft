//! Coincidence canonicalisation (plan Phases 2+3): point-contact filter,
//! seam vertex weld, per-class edge merge, membrane cull and fin drop —
//! the steps that turn per-branch tracing output into manifold topology on
//! coincident geometry.

use crate::brep::BranchSurfaceMap;
use std::collections::HashMap;

use crate::brep::{
    BRep, Edge, EdgeId,
    SurfaceGeometry, Vertex, VertexId,
};
use crate::bvh::BoundedCsg;
use crate::csg::BranchId;
use fielddraft_protocol::HalfSpace;

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use crate::brep::vec3::{cross, dot, normalize as unit, scale, sub};
use crate::brep::vec3::norm;
use super::*;

/// Merge duplicate seam traces (Phase 3 edge canonicalisation): two edges are
/// the same topological edge iff their signatures map to the same canonical
/// (class-minimal) branch pair AND they are the same segment of the (unique)
/// carrier-pair intersection curve. The carrier-pair fact makes identity
/// cheap: two closed loops on the same carrier pair ARE the same curve
/// (sanity-checked by centroid); two open edges are identical iff their
/// refined endpoint vertices coincide. The survivor takes the union of both
/// sides' memberships, the canonical signature (deterministic naming — the
/// plan's "kleinste Branch-IDs der Klasse"), and the partner's endpoint
/// vertices are aliased onto its own so neighbouring rings keep connecting.
pub(super) fn merge_coincident_edges(
    brep: &mut BRep,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    memberships: &mut Vec<Vec<u32>>,
    diag: f64,
) {
    // Identity tolerances: endpoint vertices are Newton-refined (tight);
    // closed-loop centroids come from chord sampling of curved rims
    // (sag-limited, looser). Rationale in `brep::tol`.
    let tol_pt = crate::brep::tol::on_boundary(diag);
    let tol_loop = crate::brep::tol::loop_centroid(diag);

    let canon = |b: BranchId| -> u32 {
        registry
            .class_of(b)
            .map(|(cls, _)| {
                registry.classes()[cls]
                    .members
                    .iter()
                    .map(|(m, _)| m.as_u32())
                    .min()
                    .unwrap()
            })
            .unwrap_or(b.as_u32())
    };

    // Group edges by EACH classed branch of their signature (canonical class
    // representative). Keying per single class — not per signature pair — is
    // deliberate: the two trace copies of a seam rim share the membrane's
    // class, but their *wall* partners may be sub-µm mesh-derived planes that
    // sit outside the registry tolerance (face-extrude geometry). The carrier
    // class plus geometric identity of the segment is sufficient: both copies
    // lie on the SAME carrier, so coincident endpoints mean the same edge.
    let mut groups: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, e) in brep.edges.iter().enumerate() {
        for b in [e.signature.0, e.signature.1] {
            if registry.class_of(b).is_some() {
                let c = canon(b);
                let g = groups.entry(c).or_default();
                if g.last() != Some(&i) {
                    g.push(i);
                }
            }
        }
    }
    let mut keys: Vec<u32> = groups.keys().copied().collect();
    keys.sort_unstable();

    let mut edge_keep = vec![true; brep.edges.len()];
    let mut vertex_alias: Vec<u32> = (0..brep.vertices.len() as u32).collect();
    let mut merged_any = false;

    for key in keys {
        let idxs = &groups[&key];
        // Greedy geometric subgrouping: first occurrence is the survivor.
        let mut reps: Vec<usize> = Vec::new();
        for &i in idxs {
            if !edge_keep[i] {
                continue; // already merged away under another class key
            }
            let mut merged = false;
            for &r in &reps {
                if r != i && edges_identical(brep, r, i, tol_pt, tol_loop) {
                    // Union memberships onto the survivor.
                    let extra: Vec<u32> = memberships[i].clone();
                    for m in extra {
                        if !memberships[r].contains(&m) {
                            memberships[r].push(m);
                        }
                    }
                    // Alias the duplicate's endpoint vertices to the
                    // survivor's, pairing by distance.
                    let (rs, re) = (brep.edges[r].start, brep.edges[r].end);
                    let (is_, ie) = (brep.edges[i].start, brep.edges[i].end);
                    if let (Some(rs), Some(re), Some(is_), Some(ie)) = (rs, re, is_, ie) {
                        let d = |a: VertexId, b: VertexId| {
                            norm(sub(brep.vertex(a).position, brep.vertex(b).position))
                        };
                        let straight = d(is_, rs) + d(ie, re);
                        let crossed = d(is_, re) + d(ie, rs);
                        let (m0, m1) = if straight <= crossed { (rs, re) } else { (re, rs) };
                        if is_ != m0 {
                            vertex_alias[is_.0 as usize] = m0.0;
                        }
                        if ie != m1 {
                            vertex_alias[ie.0 as usize] = m1.0;
                        }
                    }
                    edge_keep[i] = false;
                    merged_any = true;
                    merged = true;
                    break;
                }
            }
            if !merged {
                reps.push(i);
                // Canonical signature for deterministic seam naming (the
                // plan's "kleinste Branch-IDs der Klasse").
                let e = &mut brep.edges[i];
                let (a, b) = (canon(e.signature.0), canon(e.signature.1));
                e.signature = if a <= b {
                    (BranchId(a), BranchId(b))
                } else {
                    (BranchId(b), BranchId(a))
                };
            }
        }
    }
    if !merged_any {
        return;
    }

    // Resolve alias chains, rewrite edge endpoints, union vertex branch sets.
    let resolve = |mut v: u32, alias: &[u32]| -> u32 {
        while alias[v as usize] != v {
            v = alias[v as usize];
        }
        v
    };
    let alias_pairs: Vec<(usize, u32)> = (0..brep.vertices.len())
        .filter(|&i| vertex_alias[i] != i as u32)
        .map(|i| (i, resolve(i as u32, &vertex_alias)))
        .collect();
    for &(from, to) in &alias_pairs {
        let extra = brep.vertices[from].branches.clone();
        let target = &mut brep.vertices[to as usize].branches;
        for b in extra {
            if !target.contains(&b) {
                target.push(b);
            }
        }
        target.sort_unstable_by_key(|b| b.as_u32());
    }
    for e in &mut brep.edges {
        e.start = e.start.map(|v| VertexId(resolve(v.0, &vertex_alias)));
        e.end = e.end.map(|v| VertexId(resolve(v.0, &vertex_alias)));
    }

    compact_edges_and_vertices(brep, &edge_keep, memberships);
}

/// Whether two edges on the SAME coincidence-class carrier are the same
/// segment of an intersection curve. Closed loops: identical iff centroid
/// AND extent agree (a centroid match alone would merge concentric loops of
/// different size). Open edges: identical iff the refined endpoint vertex
/// positions coincide as an unordered pair.
pub(super) fn edges_identical(brep: &BRep, a: usize, b: usize, tol_pt: f64, _tol_loop: f64) -> bool {
    let (ea, eb) = (&brep.edges[a], &brep.edges[b]);
    let closed_a = ea.start.is_none() && ea.end.is_none();
    let closed_b = eb.start.is_none() && eb.end.is_none();
    if closed_a != closed_b {
        return false;
    }
    if closed_a {
        // The tolerance absorbs chord-sampling sag on curved rims
        // (~1.2e-3·R at 16 samples/span) but stays proportional to the loop
        // itself, so different loops sharing a centroid never merge.
        let metrics = |e: &Edge| -> Option<([f64; 3], f64)> {
            let pts = crate::brep::edge_sampling::sample_edge(&e.geometry);
            if pts.len() < 3 {
                return None;
            }
            let inv = 1.0 / pts.len() as f64;
            let c = pts.iter().fold([0.0; 3], |acc, p| {
                [acc[0] + p[0] * inv, acc[1] + p[1] * inv, acc[2] + p[2] * inv]
            });
            let extent = pts.iter().map(|p| norm(sub(*p, c))).fold(0.0, f64::max);
            Some((c, extent))
        };
        let (Some((ca, ra)), Some((cb, rb))) = (metrics(ea), metrics(eb)) else {
            return false;
        };
        let tol_id = tol_pt.max(2e-3 * ra.max(rb));
        return norm(sub(ca, cb)) < tol_id && (ra - rb).abs() < tol_id;
    }
    let (Some(as_), Some(ae)) = (ea.start, ea.end) else {
        return false;
    };
    let (Some(bs), Some(be)) = (eb.start, eb.end) else {
        return false;
    };
    let p = |v: VertexId| brep.vertex(v).position;
    let straight = norm(sub(p(as_), p(bs))) < tol_pt && norm(sub(p(ae), p(be))) < tol_pt;
    let crossed = norm(sub(p(as_), p(be))) < tol_pt && norm(sub(p(ae), p(bs))) < tol_pt;
    straight || crossed
}

/// Pass-A membrane cull (Phase 2): which branches lie nowhere on ∂body?
/// Candidates are exactly the branches whose coincidence class contains an
/// opposite-oriented partner — the only way a traced branch face can be a
/// membrane (flush-add contact plane, flush-remove zero-thickness wall, a
/// pin refilling its hole). Each candidate is probed at its edge-sample
/// centroid plus up to eight samples pulled 20% toward it; a branch is
/// culled only if EVERY probe sees the same SDF sign on both sides of the
/// carrier. The probe decides sidedness of a known analytic surface point —
/// the coincidence fact itself came symbolically from the registry.
pub(super) fn cull_membrane_branches(
    body: &BoundedCsg,
    brep: &BRep,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    halfspaces: &BranchSurfaceMap,
    memberships: &[Vec<u32>],
    diag: f64,
) -> std::collections::HashSet<u32> {
    let eps = crate::brep::tol::on_boundary(diag);
    let mut edges_of: HashMap<u32, Vec<EdgeId>> = HashMap::new();
    for (i, members) in memberships.iter().enumerate() {
        for &b in members {
            edges_of.entry(b).or_default().push(EdgeId(i as u32));
        }
    }
    let mut culled = std::collections::HashSet::new();
    let mut branches: Vec<u32> = edges_of.keys().copied().collect();
    branches.sort_unstable();
    for b in branches {
        let candidate = registry.class_of(BranchId(b)).is_some_and(|(cls, rel)| {
            registry.classes()[cls].members.iter().any(|&(_, other)| other != rel)
        });
        if !candidate {
            continue;
        }
        let Some((hs, xf)) = halfspaces.get(&b) else {
            continue;
        };
        let surface = SurfaceGeometry::Analytic { surface: hs.clone(), transform: *xf };
        if branch_is_membrane(body, brep, &edges_of[&b], &surface, eps) {
            culled.insert(b);
        }
    }
    culled
}

/// The ±ε sidedness probe behind [`cull_membrane_branches`]. Returns `false`
/// (= keep) whenever any ingredient is unavailable (no carrier snap, no
/// normal, too few samples) — the cull must never act on guesswork.
pub(super) fn branch_is_membrane(
    body: &BoundedCsg,
    brep: &BRep,
    edge_ids: &[EdgeId],
    surface: &SurfaceGeometry,
    eps: f64,
) -> bool {
    use crate::csg::EvalMode;
    let ring: Vec<(EdgeId, bool)> = edge_ids.iter().map(|&e| (e, false)).collect();
    let samples = ring_sample_points(brep, &ring);
    if samples.len() < 3 {
        return false;
    }
    let inv = 1.0 / samples.len() as f64;
    let centroid = samples.iter().fold([0.0; 3], |acc, p| {
        [acc[0] + p[0] * inv, acc[1] + p[1] * inv, acc[2] + p[2] * inv]
    });

    // Probe the centroid plus up to 8 boundary-biased points: each is a
    // boundary sample pulled 20% toward the centroid — far enough inside the
    // face that the ±ε probe clears the bounding edges' own zero set, close
    // enough to the rim to catch a face that is membrane only near its
    // middle but crosses material toward the boundary (partial overlap).
    // The exact fraction is uncritical (anything well inside (0, 0.5)
    // works); 8 probes match the octree's facewise sampling budget.
    let mut probes: Vec<[f64; 3]> = vec![centroid];
    let step = (samples.len() / 8).max(1);
    for s in samples.iter().step_by(step).take(8) {
        probes.push([
            s[0] + (centroid[0] - s[0]) * 0.2,
            s[1] + (centroid[1] - s[1]) * 0.2,
            s[2] + (centroid[2] - s[2]) * 0.2,
        ]);
    }

    for q in probes {
        let Some(q) = snap_to_carrier(surface, q) else {
            return false;
        };
        let Some(n) = surface_normal_at(surface, q) else {
            return false;
        };
        let probe = |side: f64| -> f32 {
            let p = [
                (q[0] + n[0] * eps * side) as f32,
                (q[1] + n[1] * eps * side) as f32,
                (q[2] + n[2] * eps * side) as f32,
            ];
            body.sdf(p, EvalMode::Logical)
        };
        if (probe(1.0) < 0.0) != (probe(-1.0) < 0.0) {
            return false; // a real boundary crossing — keep the face
        }
    }
    true
}

/// Snap a near-surface point exactly onto a face's analytic carrier (the
/// probe points are centroid-lerped chord samples, which sag off curved
/// surfaces). Plane: drop the normal component; cylinder/cone: re-scale the
/// radial component in the local frame; sphere: re-scale from the centre.
/// `None` for carriers the Phase-2 cull does not probe (torus/gyroid/
/// tessellated) — those faces are simply kept.
pub(super) fn snap_to_carrier(surface: &SurfaceGeometry, p: [f64; 3]) -> Option<[f64; 3]> {
    let SurfaceGeometry::Analytic { surface: hs, transform } = surface else {
        return None;
    };
    let l32 = crate::world_to_local(transform, [p[0] as f32, p[1] as f32, p[2] as f32]);
    let l = [l32[0] as f64, l32[1] as f64, l32[2] as f64];
    let snapped = match hs {
        HalfSpace::Plane { normal, offset } => {
            let n = unit([normal[0] as f64, normal[1] as f64, normal[2] as f64]);
            let d = l[0] * n[0] + l[1] * n[1] + l[2] * n[2] - *offset as f64;
            Some(sub(l, scale(n, d)))
        }
        HalfSpace::InfiniteCylinder { radius } => {
            let r_xz = norm([l[0], 0.0, l[2]]);
            if r_xz < 1e-12 {
                return None;
            }
            let k = *radius as f64 / r_xz;
            Some([l[0] * k, l[1], l[2] * k])
        }
        HalfSpace::Sphere { radius } => {
            let r = norm(l);
            if r < 1e-12 {
                return None;
            }
            let k = *radius as f64 / r;
            Some(scale(l, k))
        }
        HalfSpace::Cone { .. }
        | HalfSpace::Torus { .. }
        | HalfSpace::Gyroid { .. }
        | HalfSpace::RevolvedProfile { .. }
        | HalfSpace::Blend { .. } => None,
    }?;
    Some(xf_point(
        transform,
        [snapped[0] as f32, snapped[1] as f32, snapped[2] as f32],
    ))
}

/// Drop zero-length point-contact traces. At a flush seam the tracer also
/// intersects CROSS pairs — box1's wall against box2's *adjacent* wall —
/// whose activity overlap is a single point (the seam corner), yielding
/// degenerate edges with coincident endpoints and no arc. They carry no
/// boundary information and poison ring ordering. Only runs on coincident
/// bodies (the caller gates on a non-empty registry).
pub(super) fn drop_point_contact_edges(
    brep: &mut BRep,
    memberships: &mut Vec<Vec<u32>>,
    diag: f64,
) {
    let tol = crate::brep::tol::on_boundary(diag);
    let keep: Vec<bool> = brep
        .edges
        .iter()
        .map(|e| {
            let (Some(s), Some(t)) = (e.start, e.end) else {
                return true; // closed loops are never point contacts
            };
            let (ps, pt) = (brep.vertex(s).position, brep.vertex(t).position);
            if norm(sub(ps, pt)) > tol {
                return true;
            }
            // Coincident endpoints: a genuine self-loop has arc extent; a
            // point contact does not.
            let extent = crate::brep::edge_sampling::sample_edge(&e.geometry)
                .iter()
                .map(|p| norm(sub(*p, ps)))
                .fold(0.0, f64::max);
            extent > tol * 4.0
        })
        .collect();
    if keep.iter().all(|&k| k) {
        return;
    }
    compact_edges_and_vertices(brep, &keep, memberships);
}

/// Weld duplicate seam vertices. Each side of a flush joint traces its own
/// corner vertices; the copies refine to the same exact intersection point
/// but stay distinct arena entries, so rings across the seam cannot connect.
/// Vertices closer than the (diagonal-relative) identity tolerance collapse
/// onto the lowest-index entry, which takes the union of the branch sets.
/// Only runs on coincident bodies (caller gates on a non-empty registry).
pub(super) fn weld_seam_vertices(brep: &mut BRep, memberships: &mut Vec<Vec<u32>>, diag: f64) {
    let tol = crate::brep::tol::on_boundary(diag);
    let n = brep.vertices.len();
    let mut alias: Vec<u32> = (0..n as u32).collect();
    let mut welded = false;
    for i in 0..n {
        if alias[i] != i as u32 {
            continue;
        }
        for j in (i + 1)..n {
            if alias[j] != j as u32 {
                continue;
            }
            if norm(sub(brep.vertices[i].position, brep.vertices[j].position)) < tol {
                alias[j] = i as u32;
                welded = true;
                let extra = brep.vertices[j].branches.clone();
                let target = &mut brep.vertices[i].branches;
                for b in extra {
                    if !target.contains(&b) {
                        target.push(b);
                    }
                }
                target.sort_unstable_by_key(|b| b.as_u32());
            }
        }
    }
    if !welded {
        return;
    }
    for e in &mut brep.edges {
        e.start = e.start.map(|v| VertexId(alias[v.0 as usize]));
        e.end = e.end.map(|v| VertexId(alias[v.0 as usize]));
    }
    let keep = vec![true; brep.edges.len()];
    compact_edges_and_vertices(brep, &keep, memberships);
}

/// Drop fin edges: traces that lost their membrane partner to the cull AND
/// demonstrably bound nothing. Candidates are edges whose membership keeps
/// fewer than two faces; a candidate is dropped only with **proof**:
///
/// - zero survivors (both branches were membranes) — the edge can bound no
///   face at all; or
/// - one survivor, and a ±ε probe inside the survivor's carrier shows the
///   edge sits clearly OFF the body boundary on BOTH sides (a flush-remove
///   ghost trace on a measure-zero `f = 0` sheet, e.g. body-wall × erased
///   tool-wall traces); or
/// - one survivor, and the probe shows the edge is INTERIOR to the
///   survivor's own face: ON the body boundary on both sides with the
///   survivor among the contributors there. That is the rim of a culled
///   internal membrane crossing the middle of a face — a two-face extrude's
///   union profile abuts its parts along a shared sketch edge, and the
///   resulting zero-thickness wall pair leaves its cap rims striping the
///   merged cap (ticket o2j9). An edge bounding two regions of the SAME
///   face separates nothing in a 2-manifold. A genuine seam rim is NOT
///   caught by this: across a real rim the carrier continues into material
///   (f < 0) or off the body, so the far probe fails the on-boundary test.
///
/// Anything ambiguous stays: a genuine seam rim whose duplicate failed to
/// merge (sub-µm mesh-derived drift) remains as an honestly OPEN edge, so
/// `is_watertight` stays false and the production octree fallback still
/// fires. The fin drop must never manufacture watertightness.
pub(super) fn drop_fin_edges(
    body: &BoundedCsg,
    brep: &mut BRep,
    memberships: &mut Vec<Vec<u32>>,
    culled: &std::collections::HashSet<u32>,
    halfspaces: &BranchSurfaceMap,
    diag: f64,
) {
    use crate::csg::EvalMode;
    if culled.is_empty() {
        return;
    }
    let eps = crate::brep::tol::on_boundary(diag);
    let keep: Vec<bool> = brep
        .edges
        .iter()
        .zip(memberships.iter())
        .map(|(edge, m)| {
            let lost_to_cull = m.iter().any(|b| culled.contains(b));
            let survivors: Vec<u32> =
                m.iter().copied().filter(|b| !culled.contains(b)).collect();
            if !lost_to_cull || survivors.len() >= 2 {
                return true;
            }
            if survivors.is_empty() {
                return false; // membrane-internal: no face can reference it
            }
            // One survivor: probe the edge midpoint within the survivor's
            // carrier, perpendicular to the edge. A real boundary edge has
            // the survivor's face (f ≈ 0) on at least one side; a ghost
            // trace is clearly off-boundary on both.
            let Some((hs, xf)) = halfspaces.get(&survivors[0]) else {
                return true;
            };
            let surface = SurfaceGeometry::Analytic { surface: hs.clone(), transform: *xf };
            let pts = crate::brep::edge_sampling::sample_edge(&edge.geometry);
            if pts.len() < 2 {
                return true;
            }
            let mid = pts.len() / 2;
            let t = unit(sub(pts[(mid + 1).min(pts.len() - 1)], pts[mid.saturating_sub(1)]));
            let Some(q) = snap_to_carrier(&surface, pts[mid]) else {
                return true;
            };
            let Some(n) = surface_normal_at(&surface, q) else {
                return true;
            };
            let d = unit(cross(n, t));
            if norm(d) < 0.5 {
                return true;
            }
            let probe = |side: f64| -> [f32; 3] {
                [
                    (q[0] + d[0] * eps * side) as f32,
                    (q[1] + d[1] * eps * side) as f32,
                    (q[2] + d[2] * eps * side) as f32,
                ]
            };
            let off_boundary = |side: f64| -> bool {
                (body.sdf(probe(side), EvalMode::Logical) as f64).abs() > eps * 0.3
            };
            // Interior-to-survivor probe: on the body boundary AND the
            // survivor branch owns the surface there.
            let in_survivor_face = |side: f64| -> bool {
                let (f, contributors) =
                    body.evaluate(probe(side), (eps * 0.3) as f32, EvalMode::Logical);
                (f as f64).abs() <= eps * 0.3
                    && contributors.as_slice().contains(&BranchId(survivors[0]))
            };
            let ghost = off_boundary(1.0) && off_boundary(-1.0);
            let interior = !ghost && in_survivor_face(1.0) && in_survivor_face(-1.0);
            !(ghost || interior)
        })
        .collect();
    if keep.iter().all(|&k| k) {
        return;
    }
    compact_edges_and_vertices(brep, &keep, memberships);
}

/// Compact the edge arena to the kept entries (with `memberships` in
/// lockstep) and drop vertices no surviving edge references. Runs BEFORE
/// face materialisation, so no loop remapping is needed. Arena order is
/// preserved (deterministic element handles).
pub(super) fn compact_edges_and_vertices(
    brep: &mut BRep,
    keep: &[bool],
    memberships: &mut Vec<Vec<u32>>,
) {
    let edges: Vec<Edge> = brep
        .edges
        .drain(..)
        .zip(keep)
        .filter_map(|(e, &k)| k.then_some(e))
        .collect();
    *memberships = std::mem::take(memberships)
        .into_iter()
        .zip(keep)
        .filter_map(|(m, &k)| k.then_some(m))
        .collect();

    let mut vertex_used = vec![false; brep.vertices.len()];
    for e in &edges {
        for v in [e.start, e.end].into_iter().flatten() {
            vertex_used[v.0 as usize] = true;
        }
    }
    let mut vertex_map = vec![u32::MAX; brep.vertices.len()];
    let mut next_v = 0u32;
    for (i, &used) in vertex_used.iter().enumerate() {
        if used {
            vertex_map[i] = next_v;
            next_v += 1;
        }
    }
    let vertices: Vec<Vertex> = brep
        .vertices
        .drain(..)
        .zip(&vertex_used)
        .filter_map(|(v, &u)| u.then_some(v))
        .collect();

    brep.vertices = vertices;
    brep.edges = edges
        .into_iter()
        .map(|mut e| {
            e.start = e.start.map(|v| VertexId(vertex_map[v.0 as usize]));
            e.end = e.end.map(|v| VertexId(vertex_map[v.0 as usize]));
            e
        })
        .collect();
}

/// Manifold / watertight test: every edge referenced by a face's outer loop
/// (faces are single-loop here) must be used by **exactly two** loops. See
/// DL-24.
pub(super) fn is_watertight(brep: &BRep, face_ids: &[crate::brep::FaceId]) -> bool {
    if brep.edges.is_empty() {
        return false;
    }
    let mut uses = vec![0u32; brep.edges.len()];
    for &fid in face_ids {
        let face = brep.face(fid);
        for &(EdgeId(idx), _) in &face.outer.edges {
            uses[idx as usize] += 1;
        }
        for lp in &face.inner {
            for &(EdgeId(idx), _) in &lp.edges {
                uses[idx as usize] += 1;
            }
        }
    }
    uses.iter().all(|&u| u == 2)
}

/// Re-attribute edge memberships where coincident carriers OVERLAP (plan
/// Phase 3, the case the flush stacks never hit —
/// `gate_union_crossing_boxes_coplanar_tops`). Two failure shapes of the raw
/// signature semantics on an overlapping ON-same class:
///
/// - **buried signature branch**: the central stretch of box1's top∩wall
///   edge lies on the shared top plane, but box1's WALL is buried inside the
///   crossing box there — the edge's real second face is the *coincident
///   sibling's* region (box2.top), not the wall. Feeding the wall's face
///   loop that edge double-covers the overlap and breaks orientation.
/// - **interior seam**: a traced pair line that crosses the overlap (box1.top
///   ∩ box2.wall inside the plus's centre) separates nothing — both sides
///   belong to the same kept region. It must be dropped entirely.
///
/// For every edge whose signature touches a coincidence class, the true pair
/// of adjacent faces is decided by ±ε side probes in each carrier's tangent
/// plane (the plan-blessed probe form: sidedness of a KNOWN analytic surface
/// point; the coincidence facts came symbolically from the registry). A side
/// is adjacent when it lies on the body boundary; among coincident class
/// members active there, the smallest branch owns the region (the same
/// deterministic representative rule the edge merge uses). Bails to the raw
/// semantics whenever an ingredient is missing — never acts on guesswork.
pub(super) fn reattribute_coincident_memberships(
    body: &BoundedCsg,
    brep: &mut BRep,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    halfspaces: &BranchSurfaceMap,
    memberships: &mut Vec<Vec<u32>>,
    culled: &std::collections::HashSet<u32>,
    diag: f64,
) {
    use crate::csg::EvalMode;

    let leaf_map = crate::brep::edge_walker::branch_leaf_map(body);
    let bnd = crate::brep::tol::on_boundary(diag);
    // Probe offset: comfortably clear of the boundary band and the eval
    // attribution noise, far below any face extent that matters.
    let probe_eps = 16.0 * bnd;

    let mut keep = vec![true; brep.edges.len()];
    let mut changed_any = false;

    for i in 0..brep.edges.len() {
        let (sa, sb) = {
            let e = &brep.edges[i];
            (e.signature.0, e.signature.1)
        };
        let cls_a = registry.class_of(sa);
        let cls_b = registry.class_of(sb);
        if cls_a.is_none() && cls_b.is_none() {
            continue; // no coincidence involved — raw semantics are exact
        }

        // Midpoint + unit tangent from the (already refined) edge geometry.
        let samples = crate::brep::edge_sampling::sample_edge(&brep.edges[i].geometry);
        if samples.len() < 2 {
            continue;
        }
        let mid_idx = samples.len() / 2;
        let m = samples[mid_idx];
        let t = {
            let lo = mid_idx.saturating_sub(1);
            let hi = (mid_idx + 1).min(samples.len() - 1);
            let d = sub(samples[hi], samples[lo]);
            if norm(d) < 1e-12 {
                continue;
            }
            unit(d)
        };

        // Candidate class members share the signature branch's carrier.
        let group_of = |branch: BranchId| -> Vec<u32> {
            match registry.class_of(branch) {
                Some((cls, _)) => registry.classes()[cls]
                    .members
                    .iter()
                    .map(|&(b, _)| b.as_u32())
                    .collect(),
                None => vec![branch.as_u32()],
            }
        };
        // Carrier-side probe: on `anchor`'s carrier, step probe_eps to both
        // sides of the edge in the tangent plane and report the body field
        // there. `None` when any ingredient is missing (no analytic carrier,
        // degenerate tangent) — the caller then leaves this edge alone.
        let side_fields = |anchor: u32| -> Option<[(f32, [f32; 3]); 2]> {
            let (hs, xf) = halfspaces.get(&anchor)?;
            let surface = SurfaceGeometry::Analytic {
                surface: hs.clone(),
                transform: *xf,
            };
            let q = snap_to_carrier(&surface, m)?;
            let n = surface_normal_at(&surface, q)?;
            let d = cross(n, t);
            if norm(d) < 1e-9 {
                return None;
            }
            let d = unit(d);
            let probe = |side: f64| -> (f32, [f32; 3]) {
                let p = [
                    (q[0] + d[0] * probe_eps * side) as f32,
                    (q[1] + d[1] * probe_eps * side) as f32,
                    (q[2] + d[2] * probe_eps * side) as f32,
                ];
                (body.sdf(p, EvalMode::Logical), p)
            };
            Some([probe(1.0), probe(-1.0)])
        };
        // Region containment via the member's OWN leaf field: the probe sits
        // on the shared carrier, so |f_leaf| vanishes exactly inside that
        // member's footprint and grows past its rim. (Contributor attribution
        // is NOT a containment oracle here — extrude leaves report both wall
        // groups at any axial distance.)
        let contains = |c: u32, p: [f32; 3]| -> Option<bool> {
            let leaf = leaf_map.get(&BranchId(c).feature_id())?;
            // TIGHT containment: the probe sits on the shared carrier, so the
            // true owner's leaf field vanishes there to drift noise, while a
            // member whose region ends short of the probe reads ~probe_eps.
            // A loose bound here would blur abutting regions into fake
            // overlaps (flush stacks) — bnd separates the two regimes.
            Some((leaf.sdf(p, EvalMode::Logical) as f64).abs() <= bnd)
        };

        // Master gate — genuine region OVERLAP only: the pass may act on
        // this edge only when some on-boundary side probe of a class carrier
        // is tightly contained by TWO OR MORE class members (the plus's
        // centre square). Abutting regions (flush stacks, drift-split tiers)
        // never double-contain a probe; for them the merge pass's membership
        // is already the truth and stays untouched.
        let mut overlap_here = false;
        for anchor in [sa, sb] {
            let group = group_of(anchor);
            if group.len() < 2 {
                continue;
            }
            let Some(sides) = side_fields(anchor.as_u32()) else {
                continue;
            };
            for &(f, p) in &sides {
                if (f as f64).abs() > bnd {
                    continue;
                }
                // STRICT containment for the gate: a genuine overlap reads
                // exactly zero in both members' leaf fields (the probe lies
                // well inside both footprints on the shared carrier), while
                // near-coincidence drift artefacts read ~bnd. A quarter of
                // the boundary tolerance separates the regimes with margin.
                let strictly_contains = |c: u32| -> bool {
                    leaf_map
                        .get(&BranchId(c).feature_id())
                        .is_some_and(|leaf| {
                            (leaf.sdf(p, EvalMode::Logical) as f64).abs() <= 0.25 * bnd
                        })
                };
                let n_containing = group
                    .iter()
                    .filter(|c| !culled.contains(*c))
                    .filter(|&&c| strictly_contains(c))
                    .count();
                if n_containing >= 2 {
                    overlap_here = true;
                }
            }
        }
        if !overlap_here {
            continue;
        }

        // Step 1 — removal, conservative: a member goes only when BOTH side
        // probes on its own carrier are CLEARLY off the boundary (beyond
        // 4×bnd, buried or airborne). Marginal readings (drift-scale flush
        // stacks) keep the member — merge's answer stands.
        let rm_tol = 4.0 * bnd;
        let mut members = memberships[i].clone();
        let before = members.clone();
        members.retain(|&b| {
            let Some(sides) = side_fields(b) else {
                return true; // no carrier — never guess
            };
            let clearly_off = sides
                .iter()
                .all(|&(f, _)| (f as f64).abs() > rm_tol);
            !clearly_off
        });

        // Step 1b — ownership partition on OVERLAPPING class regions: when
        // two same-class members both tightly contain the SAME side probe,
        // their regions genuinely overlap there (the plus's centre square) —
        // regions that merely ABUT (flush stacks, drift-split tiers) never
        // double-contain a probe and are skipped, so merge's answer stands
        // for them. Inside an overlap the smallest containing member owns
        // each on-boundary side; class members owning no side leave the
        // membership.
        for anchor in [sa, sb] {
            let group = group_of(anchor);
            if group.len() < 2 {
                continue;
            }
            let Some(sides) = side_fields(anchor.as_u32()) else {
                continue;
            };
            let mut owners: Vec<u32> = Vec::new();
            let mut overlap = false;
            let mut oracle_ok = true;
            for &(f, p) in &sides {
                if (f as f64).abs() > bnd {
                    continue; // off-boundary side owns nothing
                }
                let mut containing: Vec<u32> = Vec::new();
                for &c in &group {
                    if culled.contains(&c) {
                        continue;
                    }
                    match contains(c, p) {
                        Some(true) => containing.push(c),
                        Some(false) => {}
                        None => oracle_ok = false,
                    }
                }
                if containing.len() >= 2 {
                    overlap = true;
                }
                if let Some(&o) = containing.iter().min() {
                    if !owners.contains(&o) {
                        owners.push(o);
                    }
                }
            }
            if !oracle_ok || !overlap {
                continue;
            }
            members.retain(|b| !group.contains(b) || owners.contains(b));
        }

        // Step 2 — sibling addition, only when removal (or a thin merge) left
        // fewer than two real faces: on each signature carrier, the smallest
        // class member containing an on-boundary side probe owns that side;
        // an owner that is not yet a member joins.
        let live = |ms: &[u32]| ms.iter().filter(|b| !culled.contains(*b)).count();
        if live(&members) < 2 {
            for anchor in [sa, sb] {
                let group = group_of(anchor);
                if group.len() < 2 {
                    continue; // no class — nothing to add
                }
                let Some(sides) = side_fields(anchor.as_u32()) else {
                    continue;
                };
                for &(f, p) in &sides {
                    if (f as f64).abs() > bnd {
                        continue; // this side is off the boundary
                    }
                    let mut owner: Option<u32> = None;
                    let mut oracle_ok = true;
                    for &c in &group {
                        if culled.contains(&c) {
                            continue;
                        }
                        match contains(c, p) {
                            Some(true) => {
                                owner = Some(owner.map_or(c, |o: u32| o.min(c)));
                            }
                            Some(false) => {}
                            None => oracle_ok = false,
                        }
                    }
                    if !oracle_ok {
                        continue;
                    }
                    if let Some(o) = owner {
                        if !members.contains(&o) {
                            members.push(o);
                        }
                    }
                }
            }
        }

        members.sort_unstable();
        if std::env::var("REATTR_DBG").is_ok() && members != before {
            eprintln!(
                "REATTR edge {i}: sig=({},{}) m={:?} {:?} -> {:?}",
                sa.as_u32(),
                sb.as_u32(),
                m,
                before,
                members
            );
        }
        // Step 3 — an edge with fewer than two live faces separates nothing:
        // it was interior to a single region (the overlap centre) — drop it.
        if live(&members) < 2 && live(&before) >= 2 {
            keep[i] = false;
            changed_any = true;
        } else if members != memberships[i] {
            memberships[i] = members;
            changed_any = true;
        }
    }

    if changed_any {
        compact_edges_and_vertices(brep, &keep, memberships);
    }
}

/// Materialise the registry's **order-1 (tangent) seams** symbolically
/// (ARCHITECTURE-ROADMAP Punkt 2): for each detected tangent circle,
///
/// 1. drop every traced edge of the tangent PAIR's signature — those are
///    the tracer's attempts at the degenerate curve (`∇f_a × ∇f_b → 0`
///    along the whole seam): on the octree route a garbage star in the
///    √-shaped tolerance tube, on the seed route usually nothing at all;
/// 2. drop every other edge lying ON the circle (the coincident rim copies
///    of a face pinched between the tangent partners — the ghost cap of a
///    sphere-capped cylinder loses BOTH its circles here and thus never
///    materialises, which is exactly right: it is interior);
/// 3. add the EXACT parametric circle as the seam edge of the pair.
///
/// The circle is a rational quadratic NURBS (a true circle), so the seam is
/// resolution-independent — the tracer is never pointed at the degenerate
/// curve, per the plan's constructive-provenance directive.
pub(super) fn materialize_tangent_seams(
    body: &BoundedCsg,
    brep: &mut BRep,
    registry: &crate::brep::coincidence::CoincidenceRegistry,
    memberships: &mut Vec<Vec<u32>>,
    diag: f64,
) {
    for seam in registry.tangent_seams() {
        // BOUNDARY gate: a tangent circle is a seam only when the two
        // carriers SWAP the outward role across it (sphere cap on a
        // cylinder: wall outside below, sphere outside above). An INTERIOR
        // tangency — an inscribed sphere grazing the wall from inside —
        // keeps one carrier outside on both sides and contributes no
        // boundary edge at all. Decided by ±ε probes along each carrier's
        // meridian at a known circle point (the sanctioned probe form); ε
        // must clear the √-shaped separation tube (the surfaces separate
        // QUADRATICALLY off the circle), so it scales with √(diag·tol).
        if !tangent_seam_on_boundary(body, seam, diag) {
            continue;
        }
        let (ka, kb) = (seam.a.as_u32(), seam.b.as_u32());
        let inv = crate::inverse(&seam.frame);
        // Local-frame tolerance: the world boundary tolerance divided by the
        // frame scale (rim copies are traced to refinement noise, far below).
        let scale = (seam.frame.scale[0].abs() as f64).max(1e-9);
        let tol = 4.0 * super::super::tol::on_boundary(diag) / scale;

        let on_circle = |e: &Edge| -> bool {
            let pts = crate::brep::edge_sampling::sample_edge(&e.geometry);
            !pts.is_empty()
                && pts.iter().all(|p| {
                    let l = crate::world_to_local(
                        &seam.frame,
                        [p[0] as f32, p[1] as f32, p[2] as f32],
                    );
                    let _ = &inv; // frame inversion is folded into world_to_local
                    let r = ((l[0] as f64).powi(2) + (l[2] as f64).powi(2)).sqrt();
                    (r - seam.radius as f64).abs() <= tol
                        && (l[1] as f64 - seam.y_local as f64).abs() <= tol
                })
        };

        let keep: Vec<bool> = brep
            .edges
            .iter()
            .map(|e| {
                let sig = (e.signature.0.as_u32(), e.signature.1.as_u32());
                let is_pair = (sig.0 == ka && sig.1 == kb) || (sig.0 == kb && sig.1 == ka);
                !(is_pair || on_circle(e))
            })
            .collect();
        if keep.iter().any(|&k| !k) {
            compact_edges_and_vertices(brep, &keep, memberships);
        }

        brep.add_edge(Edge::new(
            EdgeGeometry::Nurbs(super::wireframes::nurbs_circle(
                &seam.frame,
                seam.radius,
                seam.y_local,
            )),
            seam.a,
            seam.b,
            None,
            None,
        ));
        memberships.push(vec![ka.min(kb), ka.max(kb)]);
    }
}

/// The ±ε boundary probe behind [`materialize_tangent_seams`]: walk a small
/// step along each carrier's meridian from a point on the tangent circle and
/// read the body field. The circle is a boundary seam iff each carrier is on
/// the boundary on EXACTLY one side and the two carriers cover OPPOSITE
/// sides (the outward-role swap of a G1 join).
fn tangent_seam_on_boundary(
    body: &BoundedCsg,
    seam: &crate::brep::coincidence::TangentSeam,
    diag: f64,
) -> bool {
    use crate::csg::EvalMode;

    let bnd = crate::brep::tol::on_boundary(diag);
    // Quadratic separation: at arc-distance ε off the circle the two
    // surfaces sit ~ε²/(2R) apart. ε = 4·√(diag·bnd) puts that gap at
    // ~8·bnd·diag/R ≫ the on/off threshold for any feature-scale R.
    let eps = 4.0 * (diag * bnd).sqrt();
    let on_tol = 4.0 * bnd;

    // A point on the circle, in the weave frame.
    let p0 = place_point3(
        &seam.frame,
        [seam.radius as f64, seam.y_local as f64, 0.0],
    );
    // Meridian direction of a revolution carrier at p0: project the axis
    // direction into the carrier's tangent plane. For BOTH carriers the
    // meridian at the tangent circle is the same line (the surfaces are
    // tangent there!) — so a single ± probe pair, evaluated per carrier
    // FIELD, cannot distinguish them. Instead probe the BODY field only:
    // the role swap manifests as "boundary on both sides of the circle
    // along the COMMON tangent direction" vs. the interior case where one
    // side dives into material. On-boundary on both sides → seam; interior
    // tangency shows f < −on_tol on at least one side (the side where the
    // grazing carrier runs under the surviving one).
    let axis = {
        let r = crate::quat_rotate(seam.frame.rotation, [0.0, 1.0, 0.0]);
        let n = ((r[0] * r[0] + r[1] * r[1] + r[2] * r[2]) as f64).sqrt().max(1e-12);
        [r[0] as f64 / n, r[1] as f64 / n, r[2] as f64 / n]
    };
    // The probe walks the common MERIDIAN of the two carriers at p0 — the
    // axis direction projected into the shared tangent plane. Walking the
    // raw axis is only right for cylinder-like walls; on a sloped cone wall
    // it leaves the surface LINEARLY (ε·slope) and would misread every
    // probe. The shared surface normal at p0 is well-conditioned (both
    // carriers agree there — that is what tangency means) and comes from a
    // central-difference gradient of the body field.
    let grad = {
        let h = (1e-4 * diag).max(1e-6) as f32;
        let f = |p: [f32; 3]| body.sdf(p, EvalMode::Logical);
        let b = [p0[0] as f32, p0[1] as f32, p0[2] as f32];
        let g = [
            (f([b[0] + h, b[1], b[2]]) - f([b[0] - h, b[1], b[2]])) as f64,
            (f([b[0], b[1] + h, b[2]]) - f([b[0], b[1] - h, b[2]])) as f64,
            (f([b[0], b[1], b[2] + h]) - f([b[0], b[1], b[2] - h])) as f64,
        ];
        let n = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-12);
        [g[0] / n, g[1] / n, g[2] / n]
    };
    let meridian = {
        let an = axis[0] * grad[0] + axis[1] * grad[1] + axis[2] * grad[2];
        let m = [
            axis[0] - an * grad[0],
            axis[1] - an * grad[1],
            axis[2] - an * grad[2],
        ];
        let n = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt().max(1e-12);
        [m[0] / n, m[1] / n, m[2] / n]
    };
    // The decisive fact is the ROLE SWAP, read from the contributor sets:
    // on a true seam (sphere cap on a cylinder) one side of the circle is
    // bound by carrier `a` alone and the other by carrier `b` alone — the
    // boundary hands over at the circle. On a grazing contact (an inscribed
    // sphere, a sphere leaving through the CAP instead of the wall) the
    // same carrier binds BOTH sides and the other never reaches the band.
    // The quadratic separation puts the non-binding carrier ~ε²/(2R) ≈
    // 8·ε²/diag off the probe — an order of magnitude beyond the `on_tol`
    // band — so membership is well separated. (A field magnitude alone
    // cannot decide this: along the common meridian the surviving wall
    // reads ~0 on both sides in every case.)
    let probe = |side: f64| -> (f64, bool, bool) {
        let p = [
            (p0[0] + meridian[0] * eps * side) as f32,
            (p0[1] + meridian[1] * eps * side) as f32,
            (p0[2] + meridian[2] * eps * side) as f32,
        ];
        let (f, contributors) = body.evaluate(p, on_tol as f32, crate::csg::EvalMode::Logical);
        let slice = contributors.as_slice();
        (
            f as f64,
            slice.contains(&seam.a),
            slice.contains(&seam.b),
        )
    };
    let (f_up, a_up, b_up) = probe(1.0);
    let (f_dn, a_dn, b_dn) = probe(-1.0);
    let on = |f: f64| f.abs() <= on_tol + (eps * eps / diag) * 8.0;
    if !(on(f_up) && on(f_dn)) {
        return false; // one side leaves the boundary entirely — no seam
    }
    (a_up && !b_up && b_dn && !a_dn) || (b_up && !a_up && a_dn && !b_dn)
}

/// `place_point` for f64 local coordinates (scale → rotate → translate).
fn place_point3(t: &fielddraft_protocol::Transform, p: [f64; 3]) -> [f64; 3] {
    let scaled = [
        (p[0] * t.scale[0] as f64) as f32,
        (p[1] * t.scale[1] as f64) as f32,
        (p[2] * t.scale[2] as f64) as f32,
    ];
    let r = crate::quat_rotate(t.rotation, scaled);
    [
        r[0] as f64 + t.translate[0] as f64,
        r[1] as f64 + t.translate[1] as f64,
        r[2] as f64 + t.translate[2] as f64,
    ]
}

// ─── c-M2: procedural blend-seam materialisation ────────────────────────────
//
// A filleted junction's blend surface leaves each wall along a G1-TANGENT
// seam — a curve the octree tracer cannot sample (parallel gradients along the
// whole curve, the same transversality wall as the tangent-contact seams).
// But the seam is symbolic: `wall_A = 0 ∧ wall_B = r` is the transversal
// intersection of wall_A with the r-offset of wall_B, computable in closed
// form. This materialises the CLOSED (circular) seams — a fillet between a
// cylinder and a perpendicular plane (round-min → exact torus band; both seams
// are coaxial circles). Non-closed seams (planar straight edges → clipped
// lines with runout caps) and other combos stay on the octree fallback.

/// One filleted junction to seam: the blend's own branch, its two wall
/// branches, and the blend radius.
struct BlendSeamSpec {
    blend: u32,
    wall_a: u32,
    wall_b: u32,
    radius: f32,
}

/// Walk the CSG body and collect every `MaskedBlend` that carries a B-Rep
/// identity (a real fillet, not a hand-built node).
fn collect_blend_specs(node: &BoundedCsg, out: &mut Vec<BlendSeamSpec>) {
    match node {
        BoundedCsg::MaskedBlend { left, right, blend_radius, provenance, .. } => {
            if provenance.branch != 0 && *blend_radius > 0.0 {
                out.push(BlendSeamSpec {
                    blend: provenance.branch,
                    wall_a: provenance.wall_a,
                    wall_b: provenance.wall_b,
                    radius: *blend_radius,
                });
            }
            collect_blend_specs(left, out);
            collect_blend_specs(right, out);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Difference { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::SmoothUnion { left, right, .. }
        | BoundedCsg::SmoothDifference { left, right, .. }
        | BoundedCsg::Morph { left, right, .. } => {
            collect_blend_specs(left, out);
            collect_blend_specs(right, out);
        }
        BoundedCsg::Twist { inner, .. }
        | BoundedCsg::Bend { inner, .. }
        | BoundedCsg::Displace { inner, .. }
        | BoundedCsg::Shell { inner, .. }
        | BoundedCsg::InnerWall { inner, .. }
        | BoundedCsg::Offset { inner, .. }
        | BoundedCsg::ArcChannel { inner, .. } => collect_blend_specs(inner, out),
        BoundedCsg::Leaf { .. }
        | BoundedCsg::Extrude { .. }
        | BoundedCsg::Revolve { .. }
        | BoundedCsg::Helix { .. }
        | BoundedCsg::Sweep { .. }
        | BoundedCsg::Loft { .. }
        | BoundedCsg::Empty => {}
    }
}

/// Materialise the closed circular seams of every filleted cylinder↔plane
/// junction: drop the octree's degenerate blend fragments + the sharp wall
/// stub, insert the two exact seam circles. `finish_weave`'s loop ordering
/// then assembles the torus band + punches the plane's hole.
pub(super) fn materialize_blend_seams(
    body: &BoundedCsg,
    brep: &mut BRep,
    halfspaces: &BranchSurfaceMap,
    memberships: &mut Vec<Vec<u32>>,
    diag: f64,
) {
    let mut specs = Vec::new();
    collect_blend_specs(body, &mut specs);
    for spec in specs {
        let Some((hs_a, xf_a)) = halfspaces.get(&spec.wall_a).cloned() else { continue };
        let Some((hs_b, xf_b)) = halfspaces.get(&spec.wall_b).cloned() else { continue };
        let Some(circles) = blend_seam_circles(
            &hs_a, &xf_a, spec.wall_a, &hs_b, &xf_b, spec.wall_b, spec.radius, body, diag,
        ) else {
            continue;
        };
        // Drop every edge touching the blend branch (the octree's degenerate
        // tangent-seam attempts) and the sharp wall_a↔wall_b stub.
        let keep: Vec<bool> = brep
            .edges
            .iter()
            .map(|e| {
                let (s0, s1) = (e.signature.0.as_u32(), e.signature.1.as_u32());
                let touches_blend = s0 == spec.blend || s1 == spec.blend;
                let sharp = (s0 == spec.wall_a && s1 == spec.wall_b)
                    || (s0 == spec.wall_b && s1 == spec.wall_a);
                !(touches_blend || sharp)
            })
            .collect();
        if keep.iter().any(|&k| !k) {
            compact_edges_and_vertices(brep, &keep, memberships);
        }
        for (frame, radius, y_local, wall) in circles {
            brep.add_edge(Edge::new(
                EdgeGeometry::Nurbs(super::wireframes::nurbs_circle(&frame, radius, y_local)),
                BranchId(spec.blend),
                BranchId(wall),
                None,
                None,
            ));
            memberships.push(vec![spec.blend.min(wall), spec.blend.max(wall)]);
        }
    }
}

/// The two closed seam circles of a cylinder↔plane fillet, each as
/// `(frame, radius_local, y_local, wall_branch)` for [`nurbs_circle`] in the
/// cylinder's frame (both seams are coaxial with the cylinder). `None` for any
/// other wall combination or a non-perpendicular plane (→ octree fallback).
#[allow(clippy::too_many_arguments)]
fn blend_seam_circles(
    hs_a: &HalfSpace,
    xf_a: &fielddraft_protocol::Transform,
    branch_a: u32,
    hs_b: &HalfSpace,
    xf_b: &fielddraft_protocol::Transform,
    branch_b: u32,
    radius: f32,
    body: &BoundedCsg,
    diag: f64,
) -> Option<Vec<(fielddraft_protocol::Transform, f32, f32, u32)>> {
    // Identify which wall is the cylinder and which the plane.
    let (cyl_r, cyl_xf, cyl_branch, pl_normal, pl_offset, pl_xf, pl_branch) = match (hs_a, hs_b) {
        (HalfSpace::InfiniteCylinder { radius: rc }, HalfSpace::Plane { normal, offset }) => {
            (*rc, xf_a, branch_a, *normal, *offset, xf_b, branch_b)
        }
        (HalfSpace::Plane { normal, offset }, HalfSpace::InfiniteCylinder { radius: rc }) => {
            (*rc, xf_b, branch_b, *normal, *offset, xf_a, branch_a)
        }
        _ => return None,
    };

    let axis = unit([
        crate::quat_rotate(cyl_xf.rotation, [0.0, 1.0, 0.0])[0] as f64,
        crate::quat_rotate(cyl_xf.rotation, [0.0, 1.0, 0.0])[1] as f64,
        crate::quat_rotate(cyl_xf.rotation, [0.0, 1.0, 0.0])[2] as f64,
    ]);
    let n_local = unit([pl_normal[0] as f64, pl_normal[1] as f64, pl_normal[2] as f64]);
    let n_world = crate::quat_rotate(pl_xf.rotation, [
        n_local[0] as f32,
        n_local[1] as f32,
        n_local[2] as f32,
    ]);
    let n_world = unit([n_world[0] as f64, n_world[1] as f64, n_world[2] as f64]);
    // The seam is a circle only when the plane is perpendicular to the axis.
    if dot(axis, n_world).abs() < 0.999 {
        return None;
    }

    // Plane point in the cylinder's local frame → its axial coordinate.
    let pl_pt_world = place_point3(pl_xf, [
        (pl_normal[0] * pl_offset) as f64,
        (pl_normal[1] * pl_offset) as f64,
        (pl_normal[2] * pl_offset) as f64,
    ]);
    let pl_local = crate::world_to_local(cyl_xf, [
        pl_pt_world[0] as f32,
        pl_pt_world[1] as f32,
        pl_pt_world[2] as f32,
    ]);
    let y_plane = pl_local[1] as f64;
    let scale = (cyl_xf.min_scale() as f64).max(1e-9);
    let r_local = radius as f64 / scale;
    let r_cyl = cyl_r as f64;
    let tol = crate::brep::tol::on_boundary(diag) * 4.0;

    // A circle at (radius, y) in the cylinder frame is on the body boundary iff
    // its 0° sample reads ~0 under the (blend-aware) field.
    let on_body = |radius_l: f64, y_l: f64| -> f64 {
        let w = place_point3(cyl_xf, [radius_l, y_l, 0.0]);
        body.sdf([w[0] as f32, w[1] as f32, w[2] as f32], crate::csg::EvalMode::Visual)
            .abs() as f64
    };
    // Pick the offset sign whose sample sits on the boundary.
    let pick = |cands: [(f64, f64); 2]| -> Option<(f64, f64)> {
        cands
            .into_iter()
            .filter(|&(rl, yl)| on_body(rl, yl) <= tol)
            .min_by(|a, b| {
                on_body(a.0, a.1).partial_cmp(&on_body(b.0, b.1)).unwrap()
            })
    };

    // Seam on the cylinder wall: radius = R, axial = y_plane ± r.
    let (rc_cyl, y_cyl) =
        pick([(r_cyl, y_plane + r_local), (r_cyl, y_plane - r_local)])?;
    // Seam on the plane: radius = R ± r, axial = y_plane.
    let (r_pl, y_pl) =
        pick([(r_cyl + r_local, y_plane), (r_cyl - r_local, y_plane)])?;

    Some(vec![
        (cyl_xf.clone(), rc_cyl as f32, y_cyl as f32, cyl_branch),
        (cyl_xf.clone(), r_pl as f32, y_pl as f32, pl_branch),
    ])
}
