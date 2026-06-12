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
use crate::brep::vec3::{cross, normalize as unit, scale, sub};
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
        | HalfSpace::RevolvedProfile { .. } => None,
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
