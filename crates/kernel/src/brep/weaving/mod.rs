//! B-Rep extraction, **Phase 4: B-Rep weaving**.
//!
//! Implements the final stage of the B-Rep pipeline described in
//! `docs/GEOMETRY-KERNEL-CONCEPT.md` ("Phase 4: B-Rep Weaving"). It takes the
//! wireframe [`BRep`] produced by the Phase-3.5 edge walker
//! ([`crate::brep::edge_walker::build_wireframe`]) — deduplicated vertices plus
//! Hermite edge chains bounded by `start`/`end` vertices — and finishes it into
//! a watertight solid:
//!
//! 1. **Hermite → NURBS fitting.** Each edge's `(point, tangent)` chain is
//!    converted into a piecewise-cubic-Bézier [`NurbsCurve`] (degree 3). Every
//!    consecutive Hermite segment becomes one cubic Bézier whose interior
//!    control points sit at `± m/3` of the segment's endpoint tangents; the
//!    unit edge-walk tangents are rescaled to chord-length magnitude (DL-21).
//!    The Béziers are stitched into a single clamped NURBS with `C⁰` interior
//!    joins (DL-22).
//! 2. **Face / trimmed-surface weaving.** Every [`BranchId`] that appears in an
//!    edge signature becomes one [`Face`]. Its boundary edges (all edges whose
//!    signature contains the branch) are ordered into an outer [`Loop`] plus
//!    zero or more inner (hole) loops by a **reflex-aware turning-angle walk**
//!    (DL-58): at each vertex the next edge is chosen by signed turning angle in
//!    the face's tangent plane (oriented by the surface normal), so concave
//!    (reflex) vertices and vertices shared with holes are traversed correctly;
//!    disjoint leftover rings become inner loops, classified outer-vs-inner by
//!    projected signed area (planar) or ring extent (curved). The face's
//!    analytic surface is looked up from the body's branch→half-space table
//!    (DL-19, DL-20).
//! 3. **Shell + solid assembly.** All faces form one [`Shell`]; it is
//!    `is_closed` iff every edge is referenced by exactly two face loops — the
//!    manifold / watertight criterion (DL-24). One [`Solid`] wraps that shell.
//!
//! ## Layout
//!
//! This module holds the pipeline spine — [`weave`], [`weave_pure_octree`],
//! [`finish_weave`] and the Hermite→NURBS fit — and delegates to:
//!
//! - [`surface_registry`] — branch → analytic half-space map, complement
//!   parity (the symbolic source)
//! - [`sketch_surfaces`] — DL-50 lift of extrude/revolve sketch profiles
//! - [`surface_queries`] — pointwise normals, uv, tangents, angles
//! - [`loop_ordering`] — turning-angle walk, ring metrics, outer/inner
//! - [`canonicalize`] — coincidence canonicalisation (plan Phases 2+3)
//! - [`wireframes`] — closed-form fast paths + `weave_parametric` dispatch

use std::collections::HashMap;

use crate::brep::edge_walker::{build_wireframe, build_wireframe_with};
use crate::brep::octree::Octree;
use crate::brep::{
    BRep, BranchSurfaceMap, EdgeGeometry, EdgeId, Face, HermiteSample, Loop, NurbsCurve, Shell,
    Solid, SurfaceGeometry, VertexId,
};
use crate::bvh::BoundedCsg;
use crate::csg::BranchId;

// ─── small vector helpers (f64): shared, see brep::vec3 ─────────────────────
use crate::brep::vec3::{add, normalize as unit, scale, sub};
use crate::brep::vec3::norm;

mod canonicalize;
mod loop_ordering;
pub(crate) mod sketch_surfaces;
mod surface_queries;
mod surface_registry;
mod wireframes;
#[cfg(test)]
mod tests;

pub use surface_registry::branch_halfspace_map;
pub(crate) use surface_registry::{branch_is_complemented, complemented_features};
pub use wireframes::weave_parametric;

use canonicalize::*;
use loop_ordering::*;
use sketch_surfaces::*;
use surface_queries::*;
use wireframes::*;


// ─── (1) Hermite → NURBS fitting ────────────────────────────────────────────

/// Convert an ordered Hermite chain into a single clamped, piecewise-cubic
/// [`NurbsCurve`] (degree 3, all weights 1).
///
/// Each consecutive pair `(p_i, t_i) → (p_{i+1}, t_{i+1})` is one cubic Bézier
/// with control points `b0 = p_i`, `b1 = p_i + m_i/3`, `b2 = p_{i+1} − m_{i+1}/3`,
/// `b3 = p_{i+1}`. The edge-walk tangents are *unit* directions, so each segment
/// scales the unit tangents by the segment's chord length (DL-21) to recover a
/// magnitude. The per-segment Béziers are concatenated with `C⁰` interior joins:
/// shared endpoints are stored once, giving `3·s + 1` control points for `s`
/// segments and a clamped knot vector with end multiplicity 4 and interior
/// multiplicity 3 (DL-22).
///
/// Panics if `samples.len() < 2` (an edge must have at least two samples).
pub fn hermite_to_nurbs(samples: &[HermiteSample]) -> NurbsCurve {
    assert!(
        samples.len() >= 2,
        "hermite_to_nurbs needs at least 2 samples"
    );

    let mut control_points: Vec<[f64; 3]> = Vec::new();
    let n_seg = samples.len() - 1;

    for i in 0..n_seg {
        let p0 = samples[i].point;
        let p1 = samples[i + 1].point;
        // Chord-length magnitude: unit tangents × this segment's chord (DL-21).
        let chord = norm(sub(p1, p0));
        let t0 = unit(samples[i].tangent);
        let t1 = unit(samples[i + 1].tangent);
        let m0 = scale(t0, chord);
        let m1 = scale(t1, chord);
        let b1 = add(p0, scale(m0, 1.0 / 3.0));
        let b2 = sub(p1, scale(m1, 1.0 / 3.0));

        if i == 0 {
            control_points.push(p0);
        }
        control_points.push(b1);
        control_points.push(b2);
        control_points.push(p1);
    }

    let degree = 3;
    let n = control_points.len();
    let weights = vec![1.0; n];
    // Clamped knot vector: end multiplicity 4 (= degree + 1), each interior
    // segment join at multiplicity 3 (= degree) for a C⁰ Bézier joint (DL-22).
    // Total length = n + degree + 1 = 3·n_seg + 1 + 4 = 3·n_seg + 5.
    let mut knots: Vec<f64> = Vec::with_capacity(n + degree + 1);
    knots.extend(std::iter::repeat_n(0.0, degree + 1));
    for s in 1..n_seg {
        knots.extend(std::iter::repeat_n(s as f64, degree));
    }
    knots.extend(std::iter::repeat_n(n_seg as f64, degree + 1));
    debug_assert_eq!(knots.len(), n + degree + 1);

    NurbsCurve {
        degree,
        control_points,
        weights,
        knots,
    }
}

// ─── branch → (HalfSpace, Transform) lookup ─────────────────────────────────

/// Run the full B-Rep extraction and weaving pipeline: build the wireframe
/// (C.4), fit every Hermite edge to a NURBS curve, weave one trimmed face per
/// branch, and assemble a single watertight shell + solid.
pub fn weave(body: &BoundedCsg, octree: &Octree) -> BRep {
    finish_weave(build_wireframe(body, octree), body)
}

/// Like [`weave`], but a **purely numeric** octree path: the analytic DL-75
/// branch-projection fast-path is disabled, so the `HalfspaceMap` stays empty
/// and edge/vertex features come solely from the octree numerics.
///
/// This is the reference path for the differential oracle (Phase 0b of the
/// parametric-weaving rebuild). A valid oracle must compare numeric-vs-analytic
/// inside shelled regions; the normal [`weave`] takes the analytic fast-path
/// there, so it would be comparing analytic-vs-analytic and miss exactly the
/// analytic bugs we're hunting.
///
/// NOTE: with the empty map, shelled bodies also lose the cross-sheet guard
/// that the analytic projection provides. That is intentional — this path is
/// deliberately "naively numeric" and may produce a non-manifold result on
/// shelled bodies. Do not use it as the production weave.
pub fn weave_pure_octree(body: &BoundedCsg, octree: &Octree) -> BRep {
    finish_weave(build_wireframe_with(body, octree, true), body)
}

/// Finish a wireframe [`BRep`] — deduplicated vertices plus signed edges — into
/// a watertight solid: upgrade Hermite edges to NURBS, weave one trimmed face
/// per branch, assemble the shell + solid.
///
/// Shared by the octree route ([`weave`]) and the analytic route
/// ([`weave_parametric`]). The ONLY difference between the two is how the input
/// wireframe was produced; everything downstream — face grouping, loop
/// ordering, watertightness, the stable `id_hash` naming — is byte-identical,
/// so toggling parametric weaving never changes a body's element handles.
pub(crate) fn finish_weave(mut brep: BRep, body: &BoundedCsg) -> BRep {
    upgrade_hermite_edges(&mut brep);

    let halfspaces = branch_halfspace_map(body);
    let complemented = complemented_features(body);
    let (memberships, culled) = canonicalize_coincident(&mut brep, body, &halfspaces);

    // First attempt with the canonicalised memberships as-is.
    let mut attempt = brep.clone();
    let face_ids =
        materialize_faces(&mut attempt, &halfspaces, &complemented, &memberships, &culled);

    // Repair stage (plan Phase 3, the OVERLAP case —
    // gate_union_crossing_boxes_coplanar_tops): when coincident class
    // regions overlap on a shared carrier, the merge memberships
    // double-cover the overlap and the result is non-manifold. Only then —
    // never on an already-manifold body — re-attribute the memberships by
    // ±ε ownership probes and materialise again. Gating the repair on the
    // actual failure keeps every abutting-only body (flush stacks, the
    // drift-laden o2j9 family) byte-identical to the direct path.
    align_ambiguous_ring_windings(&mut attempt);
    if !attempt.manifold_violations().is_empty() {
        let registry = crate::brep::coincidence::CoincidenceRegistry::build(body);
        if !registry.is_empty() {
            let aabb = body.aabb();
            let diag = norm([
                (aabb.max[0] - aabb.min[0]) as f64,
                (aabb.max[1] - aabb.min[1]) as f64,
                (aabb.max[2] - aabb.min[2]) as f64,
            ])
            .max(1.0);
            let mut repaired = brep.clone();
            let mut repaired_members = memberships.clone();
            reattribute_coincident_memberships(
                body,
                &mut repaired,
                &registry,
                &halfspaces,
                &mut repaired_members,
                &culled,
                diag,
            );
            let mut repaired_attempt = repaired.clone();
            let repaired_face_ids = materialize_faces(
                &mut repaired_attempt,
                &halfspaces,
                &complemented,
                &repaired_members,
                &culled,
            );
            align_ambiguous_ring_windings(&mut repaired_attempt);
            if repaired_attempt.manifold_violations().len()
                < attempt.manifold_violations().len()
            {
                return assemble_solid(repaired_attempt, repaired_face_ids);
            }
        }
    }
    assemble_solid(attempt, face_ids)
}

/// Phase (1): upgrade every Hermite edge to NURBS. The edge-walk samples stop
/// roughly one octree cell short of each bounding corner (the corner is a
/// Vertex cell, not an Edge cell), so we anchor the chain's first/last
/// sample to the resolved start/end vertex position before fitting — the
/// NURBS then interpolates its topological vertices exactly (DL-26).
fn upgrade_hermite_edges(brep: &mut BRep) {
    let vertex_pos: Vec<[f64; 3]> = brep.vertices.iter().map(|v| v.position).collect();
    for edge in &mut brep.edges {
        if let EdgeGeometry::Hermite(samples) = &edge.geometry {
            // Panic-safety (DL-29): `hermite_to_nurbs` asserts >= 2
            // samples. A degenerate edge-point group can in principle
            // survive `build_wireframe` with a single sample; leave
            // such an edge as Hermite rather than feeding the
            // assertion an undersized chain and crashing on arbitrary
            // user geometry.
            if samples.len() < 2 {
                continue;
            }
            let mut samples = samples.clone();
            if let Some(VertexId(i)) = edge.start {
                samples.first_mut().unwrap().point = vertex_pos[i as usize];
            }
            if let Some(VertexId(i)) = edge.end {
                samples.last_mut().unwrap().point = vertex_pos[i as usize];
            }
            // Closed-loop edge (no bounding vertices — e.g. a sphere ∩ box-face
            // circle): the tracer stores `p0..pN-1` and leaves the wrap implicit
            // in the `closed` flag, so `hermite_to_nurbs` would build an OPEN
            // curve missing the `pN-1 → p0` segment — the visible gap in the ring
            // (DL-80). Repeat the first sample to add that closing Bézier. Guard
            // on the ends actually differing so an already-wrapped chain is left
            // alone.
            let is_loop = edge.start.is_none() && edge.end.is_none();
            if is_loop {
                let first = samples.first().unwrap().clone();
                let last = samples.last().unwrap();
                let gap = sub(first.point, last.point);
                if norm(gap) > 1e-9 {
                    samples.push(first);
                }
            }
            edge.geometry = EdgeGeometry::Nurbs(hermite_to_nurbs(&samples));
        }
    }
}

/// Phase (1b): coincidence canonicalisation (Phases 2+3 of
/// BREP-CLASSIFICATION-PLAN, ticket o2j9). The registry names — purely
/// symbolically — which branches share a carrier surface. With that fact
/// established, three bookkeeping steps repair the branch→face attribution
/// that coincidence breaks:
///
///  - duplicate seam traces (each side of a flush joint traces its own
///    copy of the rim) merge into ONE canonical edge carrying the union of
///    both sides' face memberships and the class-minimal signature;
///  - membrane branches (an opposite-oriented coincidence partner AND no
///    material crossing anywhere on the face: flush-add contact planes,
///    flush-remove zero-thickness walls, pins refilling their holes) are
///    culled before face materialisation;
///  - fin edges — traces that lost their second face to the cull (they lay
///    on a measure-zero f = 0 sheet, not on ∂body) — are dropped.
///
/// Transversal bodies (empty registry) skip all of this.
///
/// Returns the per-edge face memberships (initialised from the signatures,
/// grown by seam merging) and the set of culled membrane branches.
fn canonicalize_coincident(
    brep: &mut BRep,
    body: &BoundedCsg,
    halfspaces: &BranchSurfaceMap,
) -> (Vec<Vec<u32>>, std::collections::HashSet<u32>) {
    let registry = crate::brep::coincidence::CoincidenceRegistry::build(body);
    let mut memberships: Vec<Vec<u32>> = brep
        .edges
        .iter()
        .map(|e| {
            let (a, b) = (e.signature.0.as_u32(), e.signature.1.as_u32());
            if a == b {
                vec![a]
            } else {
                vec![a, b]
            }
        })
        .collect();
    let mut culled: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let has_tangency = !registry.tangent_seams().is_empty();
    // c-M2: filleted junctions carry a procedural blend carrier whose two
    // seams are G1-tangent (untraceable by the octree). Materialise their
    // closed circular seams symbolically first, so the loop ordering sees the
    // exact curves. Runs whenever the body has a blend carrier, independent of
    // coincidence.
    let has_blend = halfspaces
        .values()
        .any(|(hs, _)| matches!(hs, fielddraft_protocol::HalfSpace::Blend { .. }));
    if has_blend {
        let aabb = body.aabb();
        let diag = norm([
            (aabb.max[0] - aabb.min[0]) as f64,
            (aabb.max[1] - aabb.min[1]) as f64,
            (aabb.max[2] - aabb.min[2]) as f64,
        ])
        .max(1.0);
        materialize_blend_seams(body, brep, halfspaces, &mut memberships, diag);
    }
    if !registry.is_empty() || has_tangency {
        let aabb = body.aabb();
        let diag = norm([
            (aabb.max[0] - aabb.min[0]) as f64,
            (aabb.max[1] - aabb.min[1]) as f64,
            (aabb.max[2] - aabb.min[2]) as f64,
        ])
        .max(1.0);
        // Order-1 seams first: they REPLACE the tracer's output for their
        // pairs, so every later step sees the symbolic circle instead of the
        // degenerate fragments.
        if has_tangency {
            materialize_tangent_seams(body, brep, &registry, &mut memberships, diag);
        }
        if !registry.is_empty() {
            drop_point_contact_edges(brep, &mut memberships, diag);
            weld_seam_vertices(brep, &mut memberships, diag);
            merge_coincident_edges(brep, &registry, &mut memberships, diag);
            culled =
                cull_membrane_branches(body, brep, &registry, halfspaces, &memberships, diag);
            drop_fin_edges(body, brep, &mut memberships, &culled, halfspaces, diag);
        }
    }
    (memberships, culled)
}

/// Phases (2) + (2b): materialise one [`Face`] per branch.
///
/// (2) groups edges by branch — every branch in an edge's membership set is
/// one face (memberships start as the two signature branches and grow by
/// seam merging), first-seen order preserved for determinism — and orders
/// each face's boundary into outer + inner loops. (2b) adds the edgeless
/// closed analytic faces (a lone full sphere, SA-6). Membrane branches in
/// `culled` are skipped.
fn materialize_faces(
    brep: &mut BRep,
    halfspaces: &BranchSurfaceMap,
    complemented: &std::collections::HashSet<u32>,
    memberships: &[Vec<u32>],
    culled: &std::collections::HashSet<u32>,
) -> Vec<crate::brep::FaceId> {
    let mut branch_order: Vec<u32> = Vec::new();
    let mut edges_by_branch: HashMap<u32, Vec<EdgeId>> = HashMap::new();
    for (i, members) in memberships.iter().enumerate() {
        let eid = EdgeId(i as u32);
        for &key in members {
            edges_by_branch
                .entry(key)
                .or_insert_with(|| {
                    branch_order.push(key);
                    Vec::new()
                })
                .push(eid);
        }
    }

    // Build one face per CONNECTED REGION of each branch (F0-1) — skipping
    // membrane branches culled in (1b).
    let mut face_ids = Vec::with_capacity(branch_order.len());
    for &branch_u32 in &branch_order {
        if culled.contains(&branch_u32) {
            continue;
        }
        let edge_ids = &edges_by_branch[&branch_u32];
        let branch_id = BranchId(branch_u32);

        // Analytic surface from the lookup, else a tessellated fallback (DL-25).
        let surface = match halfspaces.get(&branch_u32) {
            Some((hs, xf)) => SurfaceGeometry::Analytic {
                surface: hs.clone(),
                transform: *xf,
            },
            None => SurfaceGeometry::Tessellated {
                positions: Vec::new(),
                indices: Vec::new(),
            },
        };

        // Order the boundary into FACE GROUPS (F0-1): one outer ring + inner
        // (hole) rings PER CONNECTED REGION of the branch's kept surface,
        // reflex-correct, with outer/inner classification (DL-58). A branch
        // split into disjoint regions (transversal plus) materialises one
        // face per region — the naming layer's spatial_index disambiguates
        // their otherwise identical signatures. If ordering fails for a
        // degenerate face, fall back to ONE face with the unordered forward
        // list as a last resort (kept so the face is still recorded) — this
        // never happens on a watertight body, which is asserted via the
        // connectivity check below.
        let groups = match order_loop_groups(
            brep,
            edge_ids,
            &surface,
            branch_is_complemented(branch_id, complemented),
        ) {
            Some(groups) => groups
                .into_iter()
                .map(|loops| {
                    let outer = Loop::new(loops.outer);
                    let inner: Vec<Loop> =
                        loops.inner.into_iter().map(Loop::new).collect();
                    // Connectivity guarantee: a successfully-ordered face must
                    // be made of connected rings, else we'd emit invalid STEP.
                    debug_assert!(
                        outer.is_connected_ring(&brep.edges)
                            && inner.iter().all(|lp| lp.is_connected_ring(&brep.edges)),
                        "ordered face loops for branch {branch_u32} must be connected rings"
                    );
                    (outer, inner)
                })
                .collect::<Vec<_>>(),
            None => {
                // Last-resort fallback for degenerate topology (e.g. a face
                // whose boundary is incomplete because Phase-3 extraction
                // dropped a vertex/edge at a reflex region). Recorded loudly so
                // it is never a silent invalid-STEP path. Watertight bodies
                // built from complete topology never reach here.
                eprintln!(
                    "weave: WARNING could not order face loops for branch \
                     {branch_u32}; emitting unordered fallback loop (degenerate \
                     topology)"
                );
                vec![(
                    Loop::new(edge_ids.iter().map(|&e| (e, false)).collect()),
                    Vec::new(),
                )]
            }
        };

        for (outer, inner) in groups {
            let fid = brep.add_face(Face {
                branch_id,
                surface: surface.clone(),
                outer,
                inner,
            });
            face_ids.push(fid);
        }
    }

    // (2b) Edgeless closed analytic faces (SA-6). A *lone* closed analytic
    // surface — a full sphere — has ONE branch, no branch collisions, hence no
    // edge cells: it never appears in any edge signature, so the loop above
    // emits zero faces. Such a surface is a single face bounded by NOTHING (an
    // empty outer loop) on a closed shell. We materialise it only when the body
    // produced no edges at all: with edges present, an analytic branch missing
    // from `edges_by_branch` would be a *degenerate* (unbounded / clipped-away)
    // face, not a genuinely closed one — those stay out (DL-54).
    if brep.edges.is_empty() {
        for (&branch_u32, (hs, xf)) in halfspaces {
            if edges_by_branch.contains_key(&branch_u32) {
                continue;
            }
            let fid = brep.add_face(Face {
                branch_id: BranchId(branch_u32),
                surface: SurfaceGeometry::Analytic {
                    surface: hs.clone(),
                    transform: *xf,
                },
                outer: Loop::new(Vec::new()),
                inner: Vec::new(),
            });
            face_ids.push(fid);
        }
    }

    face_ids
}

/// Post-materialisation winding alignment for rings with **no intrinsic
/// winding authority**:
///
/// - every ring of a **tessellated** face (no uv parametrisation at all —
///   a swept blob, a gyroid patch);
/// - a **v-winding puncture ring** on a closed-cross analytic carrier (a
///   ring around the torus tube / through a sphere): the kept surface lies
///   on BOTH sides of such a ring, so neither the uv shoelace nor the
///   Flächenseite rule can orient it (`classify_rings_curved` abstains for
///   exactly these).
///
/// The manifold convention is PAIRWISE (each edge once forward, once
/// reversed across its two faces) and the authoritative faces' windings are
/// already law (planar Newell rules / uv metrics). So each ambiguous ring
/// takes the COMPLEMENT of how its authoritative partners consumed its
/// edges, decided by majority over the ring (the ring flips as a whole to
/// keep head-to-tail connectivity). Rings with no authoritative partner
/// keep walk order — there is no anchor, and both sides flipping together
/// would be a no-op anyway.
fn align_ambiguous_ring_windings(brep: &mut BRep) {
    use loop_ordering::is_full_revolution;

    // Per face: the loop indices (0 = outer, 1.. = inner) with no intrinsic
    // winding authority.
    let mut ambiguous: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, face) in brep.faces.iter().enumerate() {
        let loops: Vec<&crate::brep::Loop> =
            std::iter::once(&face.outer).chain(face.inner.iter()).collect();
        match &face.surface {
            SurfaceGeometry::Tessellated { .. } => {
                ambiguous.push((i, (0..loops.len()).collect()));
            }
            SurfaceGeometry::Analytic { surface, .. }
                if !matches!(surface, fielddraft_protocol::HalfSpace::Plane { .. }) =>
            {
                // A procedural fillet Blend band (c-M2) is a torus-like band:
                // both its circular seams need partner-parity alignment against
                // the authoritative wall faces, exactly like a closed-cross
                // carrier's full-rev rings.
                let closed_cross = matches!(
                    surface,
                    fielddraft_protocol::HalfSpace::Sphere { .. }
                        | fielddraft_protocol::HalfSpace::Torus { .. }
                        | fielddraft_protocol::HalfSpace::Blend { .. }
                );
                let idxs: Vec<usize> = (0..loops.len())
                    .filter(|&li| {
                        let ring = &loops[li].edges;
                        if !is_full_revolution(brep, ring) {
                            return false;
                        }
                        // Ambiguity families on curved carriers:
                        // - ANY full-revolution ring on a closed-cross
                        //   carrier (sphere/torus): u-winding rims decided by
                        //   the mean-v heuristic and v-winding punctures
                        //   alike — pairwise partner parity outranks both
                        //   (the alignment only flips on majority
                        //   disagreement, so a correct intrinsic result is
                        //   never disturbed);
                        // - a LONE full-revolution rim on any curved carrier
                        //   (single-loop face, e.g. a sphere cap's equator) —
                        //   the F0-2 declared gap: no reference ring exists,
                        //   so the intrinsic rules keep walk order.
                        let lone_rim = loops.len() == 1;
                        lone_rim || closed_cross
                    })
                    .collect();
                if !idxs.is_empty() {
                    ambiguous.push((i, idxs));
                }
            }
            _ => {}
        }
    }
    if ambiguous.is_empty() {
        return;
    }

    // Edge-orientation usage by the AUTHORITATIVE loops only (everything not
    // listed as ambiguous).
    let mut fwd = vec![0u32; brep.edges.len()];
    let mut rev = vec![0u32; brep.edges.len()];
    for (i, face) in brep.faces.iter().enumerate() {
        let amb = ambiguous
            .iter()
            .find(|(fi, _)| *fi == i)
            .map(|(_, idxs)| idxs.as_slice())
            .unwrap_or(&[]);
        for (li, lp) in std::iter::once(&face.outer).chain(face.inner.iter()).enumerate() {
            if amb.contains(&li) {
                continue;
            }
            for &(eid, reversed) in &lp.edges {
                if reversed {
                    rev[eid.0 as usize] += 1;
                } else {
                    fwd[eid.0 as usize] += 1;
                }
            }
        }
    }
    for (fi, idxs) in ambiguous {
        let face = &mut brep.faces[fi];
        for li in idxs {
            let lp = if li == 0 { &mut face.outer } else { &mut face.inner[li - 1] };
            let (mut agree, mut disagree) = (0u32, 0u32);
            for &(eid, reversed) in &lp.edges {
                let (f, r) = (fwd[eid.0 as usize], rev[eid.0 as usize]);
                // Only edges with an unambiguous authoritative orientation
                // vote; unshared or doubly-used edges abstain.
                let partner_reversed = match (f, r) {
                    (1, 0) => false,
                    (0, 1) => true,
                    _ => continue,
                };
                if reversed != partner_reversed {
                    agree += 1; // we already complement the partner
                } else {
                    disagree += 1;
                }
            }
            if disagree > agree {
                lp.edges.reverse();
                for entry in lp.edges.iter_mut() {
                    entry.1 = !entry.1;
                }
            }
        }
    }
}

/// Phase (3): shell + solid assembly. Watertight iff every edge is used by
/// exactly two face loops (DL-24) — or, for an edgeless body, iff it is a
/// single closed analytic face (a lone sphere: one face, no edges, closed
/// shell — SA-6).
fn assemble_solid(mut brep: BRep, face_ids: Vec<crate::brep::FaceId>) -> BRep {
    let is_closed = if brep.edges.is_empty() {
        face_ids.len() == 1
    } else {
        is_watertight(&brep, &face_ids)
    };
    let shell = brep.add_shell(Shell::new(face_ids, is_closed));
    brep.add_solid(Solid::new(shell, Vec::new()));

    brep
}

// ─── parametric weave (analytic wireframe: grundforms + branch-seed general) ──
