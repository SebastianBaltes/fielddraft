//! Recorded Onshape-import prefixes replayed against the real `Kernel` API.
//!
//! These are the documents that a translator-level unit test cannot express:
//! the input is not a hand-written CSG tree but the exact `addSketch` /
//! `addFeatureCall` sequence the Onshape importer produced, frozen as testdata.
//! That is what makes them durable — they do not depend on the importer, on a
//! fixture, or on a historical commit, only on the kernel.
//!
//! **Not in `just test`.** `.config/nextest.toml` filters this module out of
//! the default profile: replaying a real document costs seconds, not
//! milliseconds. It runs in the commit gate and in the deep tier:
//!
//!     just test-full
//!     cargo nextest run --profile full -E 'test(/^onshape_regression_tests::/)'

use crate::csg::BranchId;
use crate::Kernel;

/// Packed branch word of sub-face `sub` on feature `feature`, instance 0.
/// The diagnostic probes below used to spell these out as decimal literals
/// (`94338` = `qWall(23, 130)`); since #429/E1 the packing is BranchId's
/// business and a literal would silently name something else.
fn br(feature: u32, sub: u32) -> u64 {
    BranchId::from_parts(feature, 0, sub).as_u64()
}
use std::time::Instant;

#[derive(serde::Deserialize)]
struct RecordedCall {
    /// `"sketch"` → [`Kernel::add_sketch`], anything else →
    /// [`Kernel::add_feature_call`].
    kind: String,
    /// The sketch's PLANE QUERY for a sketch call, the feature type otherwise.
    #[serde(rename = "type")]
    ty: String,
    /// The sub-call array (sketch) or the parameter blob (feature), as JSON.
    params: String,
}

/// Replay a recorded call list, returning `(per-call milliseconds, kernel)`.
fn replay(json: &str) -> (Vec<f64>, Kernel) {
    let calls: Vec<RecordedCall> = serde_json::from_str(json).expect("recorded call list");
    let mut kernel = Kernel::new();
    let mut timings = Vec::with_capacity(calls.len());
    for call in &calls {
        let t = Instant::now();
        if call.kind == "sketch" {
            kernel.add_sketch(&call.ty, &call.params).expect("sketch");
        } else {
            kernel
                .add_feature_call(&call.ty, &call.params)
                .expect("feature call");
        }
        timings.push(t.elapsed().as_secs_f64() * 1e3);
    }
    (timings, kernel)
}

/// `(vertices, edges, faces)` of a replayed body's woven B-Rep.
fn brep_counts(kernel: &mut Kernel, body: usize) -> (u64, u64, u64) {
    let j: serde_json::Value =
        serde_json::from_str(&kernel.body_brep_counts(body)).expect("counts json");
    let n = |k: &str| j[k].as_u64().expect("count field");
    (n("vertices"), n("edges"), n("faces"))
}

/// Per body: how many of its faces have NO traversable boundary
/// ([`crate::brep::BRep::open_face_count`]) — the faces the weaver had to emit
/// through the unordered last-resort fallback, announced on stderr as
/// "could not order face loops".
///
/// This is the #436 instrument. It counts FACES, not log lines: the warning is
/// printed once per `materialize_faces` pass, a weave runs that pass twice when
/// the coincidence repair fires, and a document re-weaves a body on several of
/// its calls — so the message count (81 on this replay before the fix) mixes
/// the defect with the number of times the weaver looked at it. The per-body
/// face count is the defect alone, and it is what an exporter would trip over.
fn open_faces(kernel: &mut Kernel) -> Vec<u64> {
    (0..kernel.body_count())
        .map(|i| {
            let j: serde_json::Value =
                serde_json::from_str(&kernel.body_brep_counts(i)).expect("counts json");
            j["openFaces"].as_u64().expect("openFaces field")
        })
        .collect()
}

/// #426 — a `revolve` with `operation: remove` against a target body took
/// **26 minutes** in the browser (14.4 min native release), 99.8 % of the whole
/// #419 import. The same rotation as a NEW body costs 0.35 s.
///
/// The cause is not the boolean and not the operands' distance (the ticket's
/// unverified suspicion; the start offset in question is 8 mm, and both
/// operands come off the SAME sketch, so they are co-located, not far apart).
/// Measured breakdown of the 846.8 s:
///
///     build_seed_wireframe (×2, strict + hybrid)     4.2 s   0.5 %
///     Octree::build                                 44.5 s   5.3 %
///     edge_walker::build_wireframe                 798.2 s  94.3 %
///
/// SSI resolved all 1 891 branch pairs (65 ms). TWO pairs failed later, in the
/// TRACE stage — and those two sent the whole body onto the octree fallback.
/// The body is a 3 087 × 83 × 25 mm assembly rail, `topology_min_cell_size`
/// CEILs the cell at 10 mm, and the result was 1.54 million leaves — to repair
/// two seams 0.17 mm and 0.017 mm wide, which cells 600× wider cannot resolve
/// at all. Every one of those 843 seconds was provably wasted.
///
/// Scoping the fill to the seams (`Octree::build_region`) takes the call to
/// ~5 s with the same body count. The bound below is deliberately loose — it
/// exists to catch the return of a MINUTES-long stall, not to police seconds,
/// so it cannot flake under CPU load.
///
/// #435 finished the job by removing the fallback instead of making it cheap.
/// The two failing pairs were never seams: `Extrude 8` and `Revolve 5` sweep
/// the SAME sketch profile, which makes each extruded wall the exact TANGENT
/// PLANE of the revolved wall along their shared segment (measured: apex on
/// plane to 8e-7 mm, `|n̂·â| = sin α` to 4e-8). A tangent plane meets a cone in
/// a DOUBLE line, so there is no transversal curve to trace — the tracer was
/// right to stall. `brep::tangential_seams::sweep_contact_pairs` now names the
/// configuration from the carriers and keeps the pair out of the seed set, and
/// the body stays on the strict analytic route. The counts pinned below are
/// that route: a timing bound cannot tell the routes apart without flaking,
/// but the topology can.
#[test]
fn onshape_419_revolve_remove_stays_bounded() {
    let (timings, mut kernel) = replay(include_str!("testdata/onshape419_revolve_boolean.json"));
    let bodies = kernel.body_count();

    // Call 10 is `Revolve 5` — `{"profiles":"qCreatedBy(6)","axis":
    // "qSketchLine(6, 82)","operation":"remove","targets":"qCreatedBy(10)"}`.
    let revolve_ms = *timings.last().expect("11 recorded calls");
    assert_eq!(timings.len(), 11, "the recorded prefix is 11 calls");
    assert!(
        revolve_ms < 120_000.0,
        "the #426 stall is back: the removing revolve took {revolve_ms:.0} ms \
         (was 846 787 ms before the scoped octree fill, ~5 200 ms after). \
         Per-call ms: {timings:?}",
    );

    // Every OTHER call in the prefix is sub-second; a regression that moved the
    // cost elsewhere rather than removing it would show up here.
    let others_ms: f64 = timings[..timings.len() - 1].iter().sum();
    assert!(
        others_ms < 30_000.0,
        "the prefix before the revolve regressed: {others_ms:.0} ms, {timings:?}",
    );

    // The repair must still BUILD the document, not just build it fast.
    assert_eq!(bodies, 5, "the recorded prefix partitions into 5 bodies");

    // Body 4 is `Extrude 8 ∖ Revolve 5` — the one that carries the tangential
    // contact. Its topology names the route it took (#435):
    //
    //     octree fallback   54 vertices, 110 edges, 37 faces
    //     strict analytic   52 vertices,  74 edges, 37 faces
    //     …plus #436        51 vertices,  80 edges, 37 faces
    //     …plus #437        51 vertices,  86 edges, 37 faces
    //
    // Same 37 faces on every route: the contact never bounded a face, it only
    // polluted two of them. The 36 extra edges of the octree were its attempt
    // to resolve the double line — 35 sub-micron NURBS loops (0.02–0.07 mm
    // across) heaped onto the extrude wall and the cone, plus one stub. A rise
    // back towards 110 means the pair is being traced again.
    //
    // The +6 edges over the 74 are #436: seams whose whole active interval used
    // to fall between two SSI samples and was never traced at all — among them
    // the 39 mm rib flank at y = 3 and the 10.4 mm slot flank at y = −19.48,
    // both on the z = −24.8 plate. The vertex that goes with them is a LOSS,
    // not a gain: two of the newly traced seams end on corners the old,
    // edge-less topology had left as separate cluster termini, and the corner
    // clustering now merges that pair into one vertex.
    //
    // The next +6 are #437, the same family: the interval seeding used to draw
    // its trim boundaries from the GLOBALLY position-deduped witness list, so a
    // seam whose second boundary happened to sit within `witness_merge`
    // (3,09 mm here) of another triple's witness lost it and stayed unseeded.
    // Drawing them from the per-pair concurrence list instead recovers, among
    // others, the 19 mm flanks at y = −62,53 on both plates (z = −24,8 and
    // z = 0) and the 11,3 mm flanks at y = −60,93. Every one is a straight
    // segment between two exact triple points; none is a duplicate (the count
    // is net of the coincidence canonicalisation, which merges twins).
    //
    // #439 measured that three of these 37 faces sit on carriers that are body
    // boundary NOWHERE (41108, 45195, 45204 — a 240×240 grid over each carrier
    // plane inside the body AABB reads ZERO on-boundary samples, against 6/2/3
    // for the neighbours 40973/41111/43006; see
    // `onshape_419_body4_face_inventory_probe`). Culling them takes this to
    // (51, 78, 34) with 20 open faces and 65 violations, but the same cull
    // costs the web import its `Thicken 6` feature, so it is NOT in the tree —
    // the full stand is in `docs/research/439-interior-carrier-cull.md`.
    assert_eq!(
        brep_counts(&mut kernel, 4),
        (51, 86, 37),
        "body 4 must weave on the STRICT analytic route (octree gave 54/110/37)",
    );

    // #436 instrument — faces whose boundary is not a traversable ring, per
    // body. 28 before the interval seeding, 25 after it (#436), 24 once the
    // seeding stopped losing its trim boundaries to the witness dedup (#437).
    //
    // The 24 that remain are NOT the same defect. #436 counted 17 of its 64
    // ordering failures as „merged twins" — a face on a coincidence-class
    // carrier whose edge list contains the carrier's own INTERIOR seams, so no
    // walk can close it however many edges the tracer finds. That family is
    // reproduced minimally and pinned in
    // `brep::weaving::tests::partial_membrane_on_coincident_carrier_leaves_face_boundary_open`;
    // it needs per-region membership attribution, not more seeds.
    //
    // Both directions are meaningful: a rise means seams stopped being traced,
    // a fall means the remaining families are being solved. Neither may pass
    // silently.
    assert_eq!(
        open_faces(&mut kernel),
        vec![0, 0, 0, 0, 24],
        "faces without a traversable boundary (#436/#437); was [0, 0, 0, 0, 28]",
    );
}

/// #433 — the WHOLE #419 import, not just its worst call: 49 recorded kernel
/// calls (12 sketches, 37 features), the same sequence the browser makes.
///
/// It exists because the number had no home. #426 left „läuft jetzt in 2,8 s"
/// in the record while the import had grown to **107,5 s** in the browser — no
/// test measured it, so it could drift by a factor of 40 unnoticed. This bound
/// is that measurement, kept honestly: an instrument, not a repair.
///
/// The distribution behind the number (measured, native, before the #433 fix):
/// 73,4 s over 49 calls, of which the top six take 65,4 s — a HOTSPOT, not
/// mass. Every one of the six is a weave of the same two mid-size bodies
/// (10×44×44 mm, 12×50×55 mm), and 87 % of each weave sat in ONE stage: the
/// triple-point witness Newton (`seed_weaver`, stage 2b). It ran 48 starts per
/// branch triple — 3,1 M Newton runs over 48 545 triples, 903 k of them
/// re-solving a corner already in hand — to keep 23 witnesses. Stopping when
/// the solution set stops growing (`NO_NEW_STOP`) takes the replay to 21 s.
///
/// The bound is 45 s: 2,1× the measured 21 s. `.config/nextest.toml` gives
/// this test the machine to itself (`threads-required`), because next to
/// 1 400 parallel tests the same replay costs 52 s — a bound that had to
/// absorb THAT could no longer tell a doubling of the work from load.
#[test]
fn onshape_419_full_import_stays_bounded() {
    let (timings, mut kernel) = replay(include_str!("testdata/onshape419_full_import.json"));
    let bodies = kernel.body_count();
    let total_ms: f64 = timings.iter().sum();
    assert_eq!(timings.len(), 49, "the recorded import is 49 kernel calls");

    assert!(
        total_ms < 45_000.0,
        "the #419 import doubled: {total_ms:.0} ms for 49 calls \
         (73 400 ms before the #433 witness fix, 17 400 ms after). \
         Per-call ms: {timings:?}",
    );

    // A single call must not eat the budget either: the six calls that made up
    // 89 % of the old total were 8,3–13,4 s each, now 1,7–2,8 s. 10 s keeps the
    // pre-fix hotspot visible as a failure without policing normal variance.
    let worst_ms = timings.iter().copied().fold(0.0f64, f64::max);
    assert!(
        worst_ms < 10_000.0,
        "a single call of the #419 import is back to hotspot size: {worst_ms:.0} ms. \
         Per-call ms: {timings:?}",
    );

    // Same document, not just a faster one. 12 bodies is the pin the web E2E
    // (`onshapeImport419E2E.slow.test.ts`) asserts on the same input.
    assert_eq!(bodies, 12, "the #419 import partitions into 12 bodies");

    // #436 instrument on the WHOLE document — faces without a traversable
    // boundary, per body. Nailing the number down is the point: it is the only
    // place the document's open faces are counted, and it was drifting
    // unwatched.
    //
    // Body 10 went from 6 to 8 under #437 and that rise is a GAIN, not a
    // regression — measured, not argued. Its counts go (22, 28, 15) to
    // (23, 30, 17): the two new edges are the cap-rim segments of two wall
    // facets (branches 131094 and 131098, 0,43 mm each) that the model-scaled
    // SSI grid had never sampled. Those two branches previously had NO edge at
    // all and therefore NO FACE — the shell simply lacked them, which this
    // instrument does not count. Now they exist with one edge each and are
    // counted as open. Face-by-face diff: the six faces that were open before
    // (110605, 112638, 131093, 133118, 131097, 131099) are the same six after,
    // NOT ONE previously-closed face broke, and 133118 even gained two edges
    // (3 → 5).
    //
    // #439 then measured what the rest of those facets' boundaries are, and the
    // answer is: nothing is missing — there is no seam to find. `Extrude 32`
    // (op Add) is swallowed WHOLE by `Extrude 27` (0 of 2680 material samples
    // of 32 fall outside 27), so all 45 of its wall facets are INTERIOR. Along
    // a facet midpoint the body field reads exactly `f = −z` over two decades
    // (nearest surface always the shared z = 0 cap plane 112638): material
    // above, air below, and the facet touches ∂body only on the LINE z = 0.
    // A line bounds no area, so the vertical seams the earlier note expected
    // cannot exist. Consistently, `both_branches_active` on pair (131111,
    // 131112) is true at exactly ONE sample of 39 974 — the point where that
    // line meets the cap — and the count grows LINEARLY with eps (1/3/33/333
    // at ×1/×10/×100/×1000), the signature of a transversal crossing rather
    // than of a missed interval. Both of the ticket's candidate causes fall:
    // the contributor set names both branches wherever the on-boundary test
    // passes. Instrument: `onshape_419_body10_seam_activity_probe`.
    //
    // The repair — culling carriers that are boundary nowhere and dropping the
    // scratches they leave on the faces they cross — takes body 10 to
    // (16, 25, 11) with ONE open face and 4 manifold violations (from 15). It
    // is NOT in the tree: it costs the web import its `Thicken 6`, whose
    // `qWall(23, 130)` resolves against exactly such a zero-area face.
    // Full stand and numbers: `docs/research/439-interior-carrier-cull.md`.
    //
    // #445 then took body 10 from 8 open faces to 4 WITHOUT culling anything:
    // the four that went are the ones a POLYGONISED ARC had invented. Exact
    // cut arcs (`sketch_arrangement::sub_segment`) make `Extrude 32`'s mantle
    // ONE cylindrical wall instead of a fan of chord facets, and the four faces
    // that disappear are precisely the fan's leftovers — branch subs 120
    // (occurrences 0 and 1) and 89 (occurrences 13 and 14), i.e. the 14th and
    // 15th wall ONE sketch curve was fencing. Body 10's census goes
    // (23, 30, 17) → (20, 27, 13); every other body is unchanged face for face,
    // body 9 included. The same exactness is what takes this replay from
    // 16 579 ms to 5 441 ms.
    assert_eq!(
        open_faces(&mut kernel),
        vec![0, 0, 0, 0, 0, 0, 2, 0, 0, 1, 4, 0],
        "faces without a traversable boundary (#436/#437/#445); was [.., 8, ..] on body 10",
    );
}

// ─── #439 measurement harness ────────────────────────────────────────────────
//
// Three `#[ignore]` probes (deep tier, `just test-deep`). They print findings
// instead of asserting, which is what makes them re-usable: every number in
// `docs/research/439-interior-carrier-cull.md` came out of one of them, and
// the next round can re-measure without rebuilding the scaffolding. Run:
//
//     cargo nextest run --profile deep --run-ignored all \
//         -E 'test(/_probe/) and test(/onshape/)' --no-capture
//
// - `onshape_419_body10_seam_activity_probe` takes `both_branches_active`
//   apart along a facet-pair seam line and walks the facet midpoints up in z,
//   plus the Extrude-32-inside-Extrude-27 containment count.
// - `onshape_419_body4_face_inventory_probe` lists body 4's faces with their
//   edge counts and open flag, and measures per-carrier boundary coverage.
// - `onshape_419_qwall_23_130_face_probe` follows the ONE branch (94338) that
//   the web import's `Thicken 6` resolves against. Its `carrier_coverage`
//   numbers for 94338 are PLANE-ONLY and therefore meaningless — see #440.
//
// Two more from #440 (`-E 'test(/onshape_440_/)'`), written up in
// `docs/research/440-thicken6-face-provenance.md`:
//
// - `onshape_440_thicken6_carrier_probe` names the carrier's analytic shape,
//   measures its boundary coverage shape-agnostically, reports which feature
//   buries it, and scans the revolve's own radial profile.
// - `onshape_440_thicken6_effect_probe` diffs the whole document with and
//   without `Thicken 6` — by SUPPRESSION, never deletion, because
//   `Document::allocate_id` is sequential and a dropped call renumbers every
//   later feature.

/// World implicit of a branch carrier — 0 exactly on the carrier surface
/// (mirror of the private `brep::ssi::implicit_world`).
fn carrier_implicit(map: &crate::brep::BranchSurfaceMap, id: u64, p: [f32; 3]) -> f32 {
    let (hs, t) = map.get(&id).expect("branch carrier in map");
    crate::csg::sdf_halfspace(hs, crate::world_to_local(t, p))
}

/// Unit normal `n` and constant `c` (`n·p = c`) of a PLANAR branch carrier,
/// taken by central differences of its world implicit — exact for a plane
/// regardless of the placement scale.
fn carrier_plane(
    map: &crate::brep::BranchSurfaceMap,
    id: u64,
    anchor: [f32; 3],
    h: f32,
) -> ([f32; 3], f32) {
    let mut g = [0.0f32; 3];
    for k in 0..3 {
        let (mut pp, mut pm) = (anchor, anchor);
        pp[k] += h;
        pm[k] -= h;
        g[k] = (carrier_implicit(map, id, pp) - carrier_implicit(map, id, pm)) / (2.0 * h);
    }
    let len = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
    let n = [g[0] / len, g[1] / len, g[2] / len];
    let f = carrier_implicit(map, id, anchor) / len;
    let c = n[0] * anchor[0] + n[1] * anchor[1] + n[2] * anchor[2] - f;
    (n, c)
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// #439 — instrument, not a regression test. Takes
/// [`crate::brep::extraction::both_branches_active_classed`] apart along the
/// analytic intersection line of two adjacent wall facets of body 10 and
/// reports which of its two sub-tests fails, with numbers.
///
/// Run: `cargo nextest run --profile deep -E 'test(seam_activity_probe)' --no-capture`
#[test]
#[ignore = "measurement harness (#439)"]
fn onshape_419_body10_seam_activity_probe() {
    let (_t, mut kernel) = replay(include_str!("testdata/onshape419_full_import.json"));
    let _ = kernel.body_brep_counts(10); // ensures brep + placement
    let body = &kernel.bodies[10];
    let bounded = crate::body::build_bounded_for_body_local(
        &kernel.decoded,
        &body.feature_indices,
        &body.placement,
    );
    let bb = bounded.aabb();
    let s = bb.size();
    let diag = ((s[0] as f64).powi(2) + (s[1] as f64).powi(2) + (s[2] as f64).powi(2)).sqrt() as f32;
    let eps = 1e-5 * diag;
    println!("body 10: aabb min {:?} max {:?}", bb.min, bb.max);
    println!("diag {diag:.4} mm, seed-weaver eps {eps:.6} mm");

    // Which faces are open, and on which branch do they sit?
    if let Some(brep) = kernel.bodies[10].brep_cached() {
        let open: Vec<(usize, u64, usize)> = brep
            .faces
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                !f.outer.is_connected_ring(&brep.edges)
                    || !f.inner.iter().all(|lp| lp.is_connected_ring(&brep.edges))
            })
            .map(|(i, f)| (i, f.branch_id.as_u64(), f.outer.edges.len()))
            .collect();
        println!("open faces (face, branch, outer edges): {open:?}");
        println!(
            "BODY10 counts {:?} open {} viol {} watertight {}",
            (brep.vertex_count(), brep.edge_count(), brep.face_count()),
            brep.open_face_count(),
            brep.manifold_violations().len(),
            brep.shells.iter().all(|s| s.is_closed),
        );
        let all: Vec<String> = brep
            .faces
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let closed = f.outer.is_connected_ring(&brep.edges);
                format!("{i}:b{} e{}{}", f.branch_id.as_u64(), f.outer.edges.len(), if closed { "" } else { "!" })
            })
            .collect();
        println!("all faces (idx:branch edges, ! = open): {all:?}");
        // The cap face 112638 — which edges does its loop carry?
        for (i, f) in brep.faces.iter().enumerate() {
            // Feature 27, cap-bottom sub (was the packed literal 112638).
            if f.branch_id == BranchId::from_parts(27, 0, BranchId::EXTRUDE_CAP_BOTTOM_SUB) {
                let sigs: Vec<(u64, u64)> = f
                    .outer
                    .edges
                    .iter()
                    .map(|(e, _)| {
                        let ed = &brep.edges[e.0 as usize];
                        (ed.signature.0.as_u64(), ed.signature.1.as_u64())
                    })
                    .collect();
                println!("  cap face {i} (27/CAP_BOTTOM) loop signatures: {sigs:?}");
            }
        }

        // Where are the wall facets actually boundary? Every face on a facet
        // branch plus every edge whose signature names one.
        for want in [21u32, 22, 25, 26, 27, 39, 40]
            .map(|sub| BranchId::from_parts(32, 0, sub).as_u64())
        {
            let faces: Vec<usize> = brep
                .faces
                .iter()
                .enumerate()
                .filter(|(_, f)| f.branch_id.as_u64() == want)
                .map(|(i, _)| i)
                .collect();
            let edges: Vec<String> = brep
                .edges
                .iter()
                .filter(|e| e.signature.0.as_u64() == want || e.signature.1.as_u64() == want)
                .map(|e| {
                    let sv = e.start.map(|v| brep.vertices[v.0 as usize].position);
                    let ev = e.end.map(|v| brep.vertices[v.0 as usize].position);
                    format!(
                        "({},{}) {:?}→{:?}",
                        e.signature.0.as_u64(),
                        e.signature.1.as_u64(),
                        sv.map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]),
                        ev.map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]),
                    )
                })
                .collect();
            println!("  branch {want}: faces {faces:?}, edges {edges:?}");
        }
    }

    let map = crate::brep::weaving::branch_halfspace_map(&bounded);
    let mut ids: Vec<u64> = map.keys().copied().collect();
    ids.sort_unstable();
    println!("branches on body 10: {} → {ids:?}", ids.len());

    // Which features make up this body?
    for &i in &kernel.bodies[10].feature_indices {
        let f = &kernel.decoded[i];
        let s = format!("{:?}", f);
        println!("  feature idx {i}: {}", &s[..s.len().min(150)]);
    }

    // How do the two extrudes relate? Build each feature in isolation.
    {
        let idx27 = 14usize;
        let idx32 = 17usize;
        let place = kernel.bodies[10].placement;
        let b27 = crate::body::build_bounded_for_body_local(&kernel.decoded, &[idx27], &place);
        let b32 = crate::body::build_bounded_for_body_local(&kernel.decoded, &[idx32], &place);
        println!("  feature 27 aabb {:?} {:?}", b27.aabb().min, b27.aabb().max);
        println!("  feature 32 aabb {:?} {:?}", b32.aabb().min, b32.aabb().max);
        // How much of 32's material lies OUTSIDE 27?
        let a32 = b32.aabb();
        let (mut inside32, mut outside27) = (0usize, 0usize);
        let nn = 40;
        for i in 0..nn {
            for j in 0..nn {
                for k in 0..nn {
                    let p = [
                        a32.min[0] + (a32.max[0] - a32.min[0]) * (i as f32 + 0.5) / nn as f32,
                        a32.min[1] + (a32.max[1] - a32.min[1]) * (j as f32 + 0.5) / nn as f32,
                        a32.min[2] + (a32.max[2] - a32.min[2]) * (k as f32 + 0.5) / nn as f32,
                    ];
                    if b32.sdf(p, crate::csg::EvalMode::Logical) < 0.0 {
                        inside32 += 1;
                        if b27.sdf(p, crate::csg::EvalMode::Logical) > 0.0 {
                            outside27 += 1;
                        }
                    }
                }
            }
        }
        println!("  samples inside feature 32: {inside32}, of which OUTSIDE 27: {outside27}");
    }

    // Is the facet wall boundary ANYWHERE above the cap plane? Walk up from
    // the midpoint of each open facet's single edge.
    for (label, mid) in [
        ("32/21", [-2.0410f32, 18.6999, 0.0]),
        ("32/22", [-2.4727f32, 18.6600, 0.0]),
        ("32/25", [-26.5892f32, -7.6000, 0.0]),
        ("32/27", [-26.6393f32, -5.8993, 0.0]),
    ] {
        let mut out = String::new();
        for k in 0..12 {
            let z = 0.05 * (2.0f32).powi(k as i32);
            let p = [mid[0], mid[1], z];
            let (f, c) = bounded.evaluate(p, eps, crate::csg::EvalMode::Logical);
            let set: Vec<u64> = c.as_slice().iter().map(|b| b.as_u64()).collect();
            out.push_str(&format!(" z={z:.2}:f={f:.4}{set:?}"));
            if z > 20.0 {
                break;
            }
        }
        println!("  facet {label} midpoint climb:{out}");
    }

    for &(a, b) in &[(br(32, 39), br(32, 40)), (br(32, 21), br(32, 22))] {
        probe_pair(&bounded, &map, a, b, bb, diag, eps);
    }
}

fn probe_pair(
    bounded: &crate::bvh::BoundedCsg,
    map: &crate::brep::BranchSurfaceMap,
    ida: u64,
    idb: u64,
    bb: crate::csg::Aabb,
    diag: f32,
    eps: f32,
) {
    use crate::csg::BranchId;
    println!("\n══ pair ({ida}, {idb}) ══");
    let (Some((ha, _)), Some((hb, _))) = (map.get(&ida), map.get(&idb)) else {
        println!("  NOT IN CARRIER MAP: a={:?} b={:?}", map.get(&ida).is_some(), map.get(&idb).is_some());
        return;
    };
    println!("  carrier a {ha:?}\n  carrier b {hb:?}");

    let center = [
        0.5 * (bb.min[0] + bb.max[0]),
        0.5 * (bb.min[1] + bb.max[1]),
        0.5 * (bb.min[2] + bb.max[2]),
    ];
    let h = 0.05 * diag;
    let (na, ca) = carrier_plane(map, ida, center, h);
    let (nb, cb) = carrier_plane(map, idb, center, h);
    let cos = dot3(na, nb);
    println!(
        "  n_a {na:?} c_a {ca:.6}\n  n_b {nb:?} c_b {cb:.6}\n  angle between normals: {:.4}°",
        cos.clamp(-1.0, 1.0).acos().to_degrees()
    );

    let t = [
        na[1] * nb[2] - na[2] * nb[1],
        na[2] * nb[0] - na[0] * nb[2],
        na[0] * nb[1] - na[1] * nb[0],
    ];
    let tlen = dot3(t, t).sqrt();
    if tlen < 1e-9 {
        println!("  normals parallel (|n_a × n_b| = {tlen:.3e}) — no intersection line");
        return;
    }
    let t = [t[0] / tlen, t[1] / tlen, t[2] / tlen];

    // Point on the line: rows (n_a, n_b, t), rhs (c_a, c_b, t·center).
    let m = nalgebra::Matrix3::new(
        na[0], na[1], na[2], nb[0], nb[1], nb[2], t[0], t[1], t[2],
    );
    let rhs = nalgebra::Vector3::new(ca, cb, dot3(t, center));
    let Some(inv) = m.try_inverse() else {
        println!("  singular 3×3 — no line");
        return;
    };
    let x = inv * rhs;
    let p0 = [x[0], x[1], x[2]];
    println!(
        "  line point {p0:?}  dir {t:?}  |f_a(p0)| {:.3e}  |f_b(p0)| {:.3e}",
        carrier_implicit(map, ida, p0).abs(),
        carrier_implicit(map, idb, p0).abs(),
    );

    let ba = BranchId(ida);
    let bbr = BranchId(idb);
    let span = 1.2 * diag;
    let step = 0.0039f32;
    let n = (2.0 * span / step) as usize;
    // Per eps decade: how many samples pass the on-boundary test, how many
    // name both branches, how many pass BOTH (= `both_branches_active`).
    for k in [1.0f32, 10.0, 100.0, 1000.0] {
        let e = eps * k;
        let (mut on_b, mut both_c, mut active) = (0usize, 0usize, 0usize);
        let mut min_abs_f = f32::INFINITY;
        let mut min_at = 0.0f32;
        let mut min_contrib: Vec<u64> = Vec::new();
        let mut sets: std::collections::BTreeMap<Vec<u64>, usize> = Default::default();
        for i in 0..=n {
            let s = -span + step * i as f32;
            let p = [p0[0] + t[0] * s, p0[1] + t[1] * s, p0[2] + t[2] * s];
            let (f, c) = bounded.evaluate(p, e, crate::csg::EvalMode::Logical);
            let mut set: Vec<u64> = c.as_slice().iter().map(|b| b.as_u64()).collect();
            set.sort_unstable();
            let has_both = c.as_slice().contains(&ba) && c.as_slice().contains(&bbr);
            let onb = f.abs() <= e;
            if onb {
                on_b += 1;
                *sets.entry(set.clone()).or_default() += 1;
            }
            if has_both {
                both_c += 1;
            }
            if onb && has_both {
                active += 1;
            }
            if f.abs() < min_abs_f {
                min_abs_f = f.abs();
                min_at = s;
                min_contrib = set;
            }
        }
        println!(
            "  eps ×{k:>6}= {e:.6} mm | samples {} | |f|≤eps: {on_b} | both contributors: {both_c} | ACTIVE: {active}",
            n + 1
        );
        println!(
            "      min |f| = {min_abs_f:.6} mm at s = {min_at:.4} mm, contributors there {min_contrib:?}"
        );
        if !sets.is_empty() {
            let mut v: Vec<_> = sets.into_iter().collect();
            v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            v.truncate(6);
            println!("      contributor sets among on-boundary samples: {v:?}");
        }
    }

    // Field profile along the line, in the extrude's own height range.
    println!("  profile  s | z | f_body | contributors (eps = 10·seed eps)");
    let mut s = -12.0f32;
    while s <= 12.0 {
        let p = [p0[0] + t[0] * s, p0[1] + t[1] * s, p0[2] + t[2] * s];
        let (f, c) = bounded.evaluate(p, eps * 10.0, crate::csg::EvalMode::Logical);
        let set: Vec<u64> = c.as_slice().iter().map(|b| b.as_u64()).collect();
        println!("    {s:>7.2} | {:>7.3} | {f:>10.5} | {set:?}", p[2]);
        s += 1.0;
    }
}

/// #439 — does branch 94338 (`qWall(23, 130)`, the face the web E2E's
/// `Thicken 6` resolves against) still own a face? Instrument only.
#[test]
#[ignore = "measurement harness (#439)"]
fn onshape_419_qwall_23_130_face_probe() {
    let (_t, mut kernel) = replay(include_str!("testdata/onshape419_full_import.json"));
    for i in 0..kernel.body_count() {
        let _ = kernel.body_brep_counts(i);
    }
    for i in 0..kernel.body_count() {
        let Some(brep) = kernel.bodies[i].brep_cached() else { continue };
        let hits: Vec<(usize, usize, bool)> = brep
            .faces
            .iter()
            .enumerate()
            .filter(|(_, f)| f.branch_id.as_u64() == br(23, 130))
            .map(|(k, f)| (k, f.outer.edges.len(), f.outer.is_connected_ring(&brep.edges)))
            .collect();
        let edges = brep
            .edges
            .iter()
            .filter(|e| e.signature.0.as_u64() == 94338 || e.signature.1.as_u64() == 94338)
            .count();
        println!("QWALL body {i}: faces {hits:?}, edges naming 94338: {edges}");
        let b = &kernel.bodies[i];
        let bounded = crate::body::build_bounded_for_body_local(
            &kernel.decoded,
            &b.feature_indices,
            &b.placement,
        );
        if crate::brep::weaving::branch_halfspace_map(&bounded).contains_key(&94338) {
            println!("  body {i} HAS carrier 94338:");
            carrier_coverage(&bounded, 94338);
            carrier_coverage(&bounded, 94347);
        }
    }
}

/// #440 — carrier-SHAPE-AGNOSTIC boundary coverage of a branch carrier.
///
/// [`carrier_coverage`] linearises the carrier at the body-AABB centre and
/// rasters that TANGENT PLANE. That is exact for a planar carrier and
/// meaningless for a curved one: a plane meets a cylinder in at most two
/// lines, so a cylindrical carrier reads "0 on-boundary samples" no matter how
/// much of it is boundary. This variant never assumes a shape — it Newton-
/// projects a dense volume grid onto the carrier's own zero set along
/// `∇(carrier implicit)` and evaluates the body field there.
fn carrier_coverage_curved(bounded: &crate::bvh::BoundedCsg, id: u64, res: usize) {
    let map = crate::brep::weaving::branch_halfspace_map(bounded);
    let bb = bounded.aabb();
    let s = bb.size();
    let diag = ((s[0] as f64).powi(2) + (s[1] as f64).powi(2) + (s[2] as f64).powi(2)).sqrt() as f32;
    let eps = 1e-4 * diag;
    if map.get(&id).is_none() {
        println!("  curved-coverage {id}: NOT IN MAP");
        return;
    }
    let h = 1e-3 * diag;
    let grad = |p: [f32; 3]| {
        let mut g = [0.0f32; 3];
        for k in 0..3 {
            let (mut pp, mut pm) = (p, p);
            pp[k] += h;
            pm[k] -= h;
            g[k] = (carrier_implicit(&map, id, pp) - carrier_implicit(&map, id, pm)) / (2.0 * h);
        }
        g
    };
    let (mut projected, mut on, mut owned) = (0usize, 0usize, 0usize);
    let mut hit_min = [f32::INFINITY; 3];
    let mut hit_max = [f32::NEG_INFINITY; 3];
    for i in 0..res {
        for j in 0..res {
            for k in 0..res {
                let mut p = [
                    bb.min[0] + (bb.max[0] - bb.min[0]) * (i as f32 + 0.5) / res as f32,
                    bb.min[1] + (bb.max[1] - bb.min[1]) * (j as f32 + 0.5) / res as f32,
                    bb.min[2] + (bb.max[2] - bb.min[2]) * (k as f32 + 0.5) / res as f32,
                ];
                // Newton onto the carrier's zero set.
                let mut ok = false;
                for _ in 0..24 {
                    let f = carrier_implicit(&map, id, p);
                    if f.abs() <= 0.1 * eps {
                        ok = true;
                        break;
                    }
                    let g = grad(p);
                    let gl2 = g[0] * g[0] + g[1] * g[1] + g[2] * g[2];
                    if gl2 < 1e-18 {
                        break;
                    }
                    for c in 0..3 {
                        p[c] -= f * g[c] / gl2;
                    }
                }
                if !ok
                    || p[0] < bb.min[0]
                    || p[0] > bb.max[0]
                    || p[1] < bb.min[1]
                    || p[1] > bb.max[1]
                    || p[2] < bb.min[2]
                    || p[2] > bb.max[2]
                {
                    continue;
                }
                projected += 1;
                let (f, cs) = bounded.evaluate(p, eps, crate::csg::EvalMode::Logical);
                if f.abs() <= eps {
                    on += 1;
                    for c in 0..3 {
                        hit_min[c] = hit_min[c].min(p[c]);
                        hit_max[c] = hit_max[c].max(p[c]);
                    }
                    if cs.as_slice().contains(&crate::csg::BranchId(id)) {
                        owned += 1;
                    }
                }
            }
        }
    }
    println!(
        "  curved-coverage branch {id}: projected {projected}, on-boundary {on}, contributor {owned} (eps {eps:.5}, res {res}³)"
    );
    if on > 0 {
        println!("    on-boundary hits span {hit_min:?} .. {hit_max:?}");
    }
}

/// #440 — what does the web import's `Thicken 6` actually stand on?
///
/// `qWall(23, 130)` = branch `23·4096 + 130 = 94338`: op 23 of the replay is
/// `Revolve 1`, entity 130 is the Onshape sketch line `2L94FTnhDuaQ.right` of
/// `Sketch 3`. This probe prints (a) the carrier's ANALYTIC shape, (b) a
/// shape-agnostic boundary coverage for it and its sibling 94347, (c) body 9's
/// feature list and face inventory, (d) the edges naming 94338 with their
/// sampled geometry, and (e) for points on the carrier, which single feature
/// of the body covers them. Instrument only.
#[test]
#[ignore = "measurement harness (#440)"]
fn onshape_440_thicken6_carrier_probe() {
    let (_t, mut kernel) = replay(include_str!("testdata/onshape419_full_import.json"));
    for i in 0..kernel.body_count() {
        let _ = kernel.body_brep_counts(i);
    }
    let bi = 9usize;
    let b = &kernel.bodies[bi];
    let idxs = b.feature_indices.clone();
    let place = b.placement;
    println!("BODY {bi} features (replay call index → feature):");
    for &i in &idxs {
        let s = format!("{:?}", kernel.decoded[i]);
        println!("  idx {i}: {}", &s[..s.len().min(220)]);
    }
    let bounded = crate::body::build_bounded_for_body_local(&kernel.decoded, &idxs, &place);
    let map = crate::brep::weaving::branch_halfspace_map(&bounded);
    for want in [br(23, 130), br(23, 139)] {
        match map.get(&want) {
            Some((hs, t)) => {
                let d = format!("{hs:?}");
                println!("carrier {want}: {} ", &d[..d.len().min(400)]);
                println!("  transform {t:?}");
            }
            None => println!("carrier {want}: NOT IN MAP"),
        }
    }
    let bb = bounded.aabb();
    println!("body aabb {:?} .. {:?}", bb.min, bb.max);

    let brep = kernel.bodies[bi].brep_cached().expect("brep").clone();
    let inv: Vec<String> = brep
        .faces
        .iter()
        .enumerate()
        .map(|(i, f)| {
            format!(
                "{i}:b{}({}/{}) e{}{}",
                f.branch_id.as_u64(),
                f.branch_id.as_u64() >> 12,
                f.branch_id.as_u64() & 4095,
                f.outer.edges.len(),
                if f.outer.is_connected_ring(&brep.edges) { "" } else { "!" }
            )
        })
        .collect();
    println!("BODY {bi} faces (idx:branch(op/entity) edges, ! = open): {inv:?}");

    for (ei, e) in brep.edges.iter().enumerate() {
        let (a, c) = (e.signature.0.as_u64(), e.signature.1.as_u64());
        if a != 94338 && c != 94338 {
            continue;
        }
        let pts = crate::brep::edge_sampling::sample_edge(&e.geometry);
        let n = pts.len();
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        let mut len = 0.0f64;
        for (k, p) in pts.iter().enumerate() {
            for c in 0..3 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
            if k > 0 {
                let q = pts[k - 1];
                len += ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2))
                    .sqrt();
            }
        }
        println!(
            "  edge {ei} sig ({a},{c}) closed={} pts {n} len {len:.4} bbox {lo:?}..{hi:?}",
            e.start.is_none() && e.end.is_none()
        );
        println!("    first {:?} mid {:?} last {:?}", pts[0], pts[n / 2], pts[n - 1]);
    }

    // (e) Who covers the carrier? Sample the CYLINDER ITSELF on a (θ, y)
    // grid — the only honest parameterisation for this shape — and classify
    // the body field plus every single feature of the body at each point.
    let s = bb.size();
    let diag = ((s[0] as f64).powi(2) + (s[1] as f64).powi(2) + (s[2] as f64).powi(2)).sqrt() as f32;
    let eps = 1e-4 * diag;
    let per_feature: Vec<(usize, String, crate::bvh::BoundedCsg)> = idxs
        .iter()
        .map(|&i| {
            (
                i,
                kernel.decoded[i].name.clone(),
                crate::body::build_bounded_for_body_local(&kernel.decoded, &[i], &place),
            )
        })
        .collect();
    let r = 7.7349997f32;
    // The Onshape sketch line `2L94FTnhDuaQ.right` runs y_local 46.677 … 60.927
    // — the WHOLE swept face, not just the part under `Extrude 4`'s block.
    let (y0, y1) = (44.0f32, 62.0f32);
    let (nt, ny) = (360usize, 180usize);
    let mut on_body = 0usize;
    let mut buried = 0usize;
    let mut outside = 0usize;
    // Per (θ, y) cell, who strictly buries the point (feature sdf < −eps).
    let mut burier: std::collections::BTreeMap<Vec<usize>, usize> = Default::default();
    let mut theta_on: Vec<bool> = vec![false; nt];
    let mut y_on: Vec<bool> = vec![false; ny];
    for it in 0..nt {
        let th = std::f32::consts::TAU * (it as f32 + 0.5) / nt as f32;
        for iy in 0..ny {
            let y = y0 + (y1 - y0) * (iy as f32 + 0.5) / ny as f32;
            let p = [r * th.cos(), y, r * th.sin()];
            let (f, _) = bounded.evaluate(p, eps, crate::csg::EvalMode::Logical);
            if f.abs() <= eps {
                on_body += 1;
                theta_on[it] = true;
                y_on[iy] = true;
            } else if f < 0.0 {
                buried += 1;
                let set: Vec<usize> = per_feature
                    .iter()
                    .filter(|(_, _, bc)| bc.sdf(p, crate::csg::EvalMode::Logical) < -eps)
                    .map(|(i, _, _)| *i)
                    .collect();
                *burier.entry(set).or_default() += 1;
            } else {
                outside += 1;
            }
        }
    }
    println!(
        "cylinder r={r} y∈[{y0},{y1}] on a {nt}×{ny} (θ,y) grid: on-boundary {on_body}, BURIED {buried}, outside {outside} (eps {eps:.5})"
    );
    println!(
        "  θ-slices with any boundary: {} of {nt}; y-slices with any boundary: {} of {ny}",
        theta_on.iter().filter(|b| **b).count(),
        y_on.iter().filter(|b| **b).count()
    );
    {
        let ys: Vec<f32> = (0..ny)
            .filter(|iy| y_on[*iy])
            .map(|iy| y0 + (y1 - y0) * (iy as f32 + 0.5) / ny as f32)
            .collect();
        if let (Some(a), Some(b)) = (ys.first(), ys.last()) {
            println!("  boundary y-range {a:.4} … {b:.4} ({} slices)", ys.len());
        }
        // Per y-slice: how many of the 360 θ samples are boundary?
        let mut line = String::new();
        for iy in 0..ny {
            let y = y0 + (y1 - y0) * (iy as f32 + 0.5) / ny as f32;
            let mut c = 0usize;
            for it in 0..nt {
                let th = std::f32::consts::TAU * (it as f32 + 0.5) / nt as f32;
                let p = [r * th.cos(), y, r * th.sin()];
                let (f, _) = bounded.evaluate(p, eps, crate::csg::EvalMode::Logical);
                if f.abs() <= eps {
                    c += 1;
                }
            }
            if iy % 6 == 0 {
                line.push_str(&format!(" y={y:.2}:{c}"));
            }
        }
        println!("  boundary θ-count per y (every 6th slice):{line}");
    }
    println!("  who BURIES the carrier (feature idx with sdf < −eps → count):");
    let mut v: Vec<_> = burier.into_iter().collect();
    v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (set, c) in v.into_iter().take(12) {
        let names: Vec<&str> = set
            .iter()
            .map(|i| {
                per_feature
                    .iter()
                    .find(|(k, _, _)| k == i)
                    .map(|(_, n, _)| n.as_str())
                    .unwrap_or("?")
            })
            .collect();
        println!("    {set:?} {names:?} → {c}");
    }
    for (i, n, bc) in &per_feature {
        println!("  feature idx {i} ({n}) aabb {:?} .. {:?}", bc.aabb().min, bc.aabb().max);
    }

    // (f) The revolve's own (r, y) profile at θ = 90° (a plane that no
    // extrude's block reaches): where does its material actually end?
    let rev = &per_feature.iter().find(|(i, _, _)| *i == 11).expect("revolve").2;
    println!("  Revolve 23 material radius per y (θ=90°, step 0.05 mm):");
    let mut ycur = 19.0f32;
    while ycur <= 61.5 {
        let mut rmax = -1.0f32;
        let mut rmin = -1.0f32;
        let mut rr = 0.0f32;
        while rr <= 12.0 {
            let p = [0.0, ycur, rr];
            if rev.sdf(p, crate::csg::EvalMode::Logical) < 0.0 {
                if rmin < 0.0 {
                    rmin = rr;
                }
                rmax = rr;
            }
            rr += 0.05;
        }
        println!("    y={ycur:>7.3}  material r ∈ [{rmin:.3}, {rmax:.3}]");
        ycur += 1.5;
    }

    carrier_coverage_curved(&bounded, 94338, 90);
    carrier_coverage_curved(&bounded, 94347, 90);
}

/// #440 — what does `Thicken 6` actually CHANGE in the replayed document?
///
/// The web importer binds it to the one face `qWall(23, 130)` resolves to; the
/// recording froze that as the `qFace(...)` fingerprint. This probe replays the
/// document with and without that single call and diffs every body, so the cost
/// of skipping the feature is a number instead of an opinion. Instrument only.
#[test]
#[ignore = "measurement harness (#440)"]
fn onshape_440_thicken6_effect_probe() {
    let full = include_str!("testdata/onshape419_full_import.json");
    let calls: Vec<serde_json::Value> = serde_json::from_str(full).expect("call list");
    let idx = calls
        .iter()
        .position(|c| c["type"] == "thicken" && c["params"].as_str().unwrap().contains("Thicken 6"))
        .expect("Thicken 6 in the recording");
    println!("Thicken 6 is recorded call #{idx}: {}", calls[idx]["params"]);

    let (_a, mut ka) = replay(full);
    // SUPPRESS, never delete: `Document::allocate_id` is sequential, so dropping
    // a call renumbers every later feature and silently re-points every
    // `qCreatedBy(n)` after it. Suppression keeps the ids.
    let (_b, mut kb) = replay(full);
    kb.set_feature_suppressed(idx as u32 + 1, true)
        .expect("suppress Thicken 6");
    println!("bodies: with {} / without {}", ka.body_count(), kb.body_count());
    let n = ka.body_count().max(kb.body_count());
    for i in 0..n {
        let ca = (i < ka.body_count()).then(|| brep_counts(&mut ka, i));
        let cb = (i < kb.body_count()).then(|| brep_counts(&mut kb, i));
        let aa = (i < ka.body_count()).then(|| {
            let b = &ka.bodies[i];
            crate::body::build_bounded_for_body_local(&ka.decoded, &b.feature_indices, &b.placement)
                .aabb()
        });
        let ab = (i < kb.body_count()).then(|| {
            let b = &kb.bodies[i];
            crate::body::build_bounded_for_body_local(&kb.decoded, &b.feature_indices, &b.placement)
                .aabb()
        });
        let same = ca == cb;
        println!(
            "  body {i}: counts {ca:?} vs {cb:?} {}",
            if same { "= equal" } else { "≠ DIFFERENT" }
        );
        if let (Some(x), Some(y)) = (aa, ab) {
            if x.min != y.min || x.max != y.max {
                println!("    aabb {:?}..{:?}  vs  {:?}..{:?}", x.min, x.max, y.min, y.max);
            }
        }
    }
    println!("open faces with    {:?}", open_faces(&mut ka));
    println!("open faces without {:?}", open_faces(&mut kb));

    // CONTROL — the instrument must be able to see a suppression at all.
    // Recorded call #24 is `Extrude 4` (feature id 25), the block that buries
    // the carrier; suppressing it has to move body 9.
    let (_c, mut kc) = replay(full);
    kc.set_feature_suppressed(25, true).expect("suppress Extrude 4");
    println!(
        "CONTROL suppress feature 25 (Extrude 4): body 9 counts {:?} (unsuppressed {:?}), open {:?}",
        brep_counts(&mut kc, 9),
        brep_counts(&mut ka, 9),
        open_faces(&mut kc)
    );

    // Volume of every body, by Monte-Carlo-free grid count over its own AABB.
    for (label, k) in [("with", &mut ka), ("without", &mut kb)] {
        let mut vols = Vec::new();
        for i in 0..k.body_count() {
            let b = &k.bodies[i];
            let bc = crate::body::build_bounded_for_body_local(
                &k.decoded,
                &b.feature_indices,
                &b.placement,
            );
            let bb = bc.aabb();
            let res = 64usize;
            let mut inside = 0usize;
            for a in 0..res {
                for c in 0..res {
                    for d in 0..res {
                        let p = [
                            bb.min[0] + (bb.max[0] - bb.min[0]) * (a as f32 + 0.5) / res as f32,
                            bb.min[1] + (bb.max[1] - bb.min[1]) * (c as f32 + 0.5) / res as f32,
                            bb.min[2] + (bb.max[2] - bb.min[2]) * (d as f32 + 0.5) / res as f32,
                        ];
                        if bc.sdf(p, crate::csg::EvalMode::Logical) < 0.0 {
                            inside += 1;
                        }
                    }
                }
            }
            let cell = (bb.max[0] - bb.min[0]) * (bb.max[1] - bb.min[1]) * (bb.max[2] - bb.min[2])
                / (res * res * res) as f32;
            vols.push((inside as f32 * cell * 1000.0).round() / 1000.0);
        }
        println!("volumes {label:>8} (mm³, 64³ grid): {vols:?}");
    }
}

/// #439 — face inventory of body 4 of the revolve-remove replay, for the
/// face-by-face justification of the pinned counts. Instrument only.
#[test]
#[ignore = "measurement harness (#439)"]
fn onshape_419_body4_face_inventory_probe() {
    let (_t, mut kernel) = replay(include_str!("testdata/onshape419_revolve_boolean.json"));
    let _ = kernel.body_brep_counts(4);
    let brep = kernel.bodies[4].brep_cached().expect("brep");
    let inv: Vec<String> = brep
        .faces
        .iter()
        .map(|f| {
            format!(
                "b{}:{}{}",
                f.branch_id.as_u64(),
                f.outer.edges.len(),
                if f.outer.is_connected_ring(&brep.edges) { "" } else { "!" }
            )
        })
        .collect();
    println!("BODY4 faces: {inv:?}");
    println!(
        "BODY4 counts {:?} open {} viol {} watertight {}",
        (brep.vertex_count(), brep.edge_count(), brep.face_count()),
        brep.open_face_count(),
        brep.manifold_violations().len(),
        brep.shells.iter().all(|s| s.is_closed),
    );
    let sigs: Vec<(u64, u64)> = brep
        .edges
        .iter()
        .map(|e| (e.signature.0.as_u64(), e.signature.1.as_u64()))
        .collect();
    println!("BODY4 edge signatures: {sigs:?}");

    // Counter-check: how much of each carrier is ACTUALLY ∂body?
    let b = &kernel.bodies[4];
    let bounded = crate::body::build_bounded_for_body_local(
        &kernel.decoded,
        &b.feature_indices,
        &b.placement,
    );
    for want in [br(10, 148), br(11, 139), br(11, 148), br(10, 13), br(10, 151), br(10, 0x7FE)] {
        carrier_coverage(&bounded, want);
    }
}

/// Fraction of a PLANAR carrier that lies on ∂body: a dense grid over the
/// body AABB, projected onto the carrier plane, counting samples with
/// `|f| ≤ ε` (and, separately, with the branch among the contributors).
///
/// **PLANAR ONLY** (#440). [`carrier_plane`] linearises the carrier at the AABB
/// centre, so this rasters a TANGENT PLANE. A plane meets a cylinder in at most
/// two lines, so every curved carrier reads "0 on-boundary samples" here no
/// matter how much of it is boundary — that is precisely how #439 concluded
/// that branch 94338 (an `InfiniteCylinder`) was nowhere boundary. For anything
/// that is not a [`fielddraft_protocol::HalfSpace::Plane`], use
/// [`carrier_coverage_curved`].
fn carrier_coverage(bounded: &crate::bvh::BoundedCsg, id: u64) {
    let map = crate::brep::weaving::branch_halfspace_map(bounded);
    let bb = bounded.aabb();
    let s = bb.size();
    let diag = ((s[0] as f64).powi(2) + (s[1] as f64).powi(2) + (s[2] as f64).powi(2)).sqrt() as f32;
    let eps = 1e-4 * diag;
    let center = [
        0.5 * (bb.min[0] + bb.max[0]),
        0.5 * (bb.min[1] + bb.max[1]),
        0.5 * (bb.min[2] + bb.max[2]),
    ];
    if map.get(&id).is_none() {
        println!("  coverage {id}: NOT IN MAP");
        return;
    }
    let (n, c) = carrier_plane(&map, id, center, 0.05 * diag);
    // In-plane basis.
    let up = if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let u = {
        let v = [
            n[1] * up[2] - n[2] * up[1],
            n[2] * up[0] - n[0] * up[2],
            n[0] * up[1] - n[1] * up[0],
        ];
        let l = dot3(v, v).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let v = [
        n[1] * u[2] - n[2] * u[1],
        n[2] * u[0] - n[0] * u[2],
        n[0] * u[1] - n[1] * u[0],
    ];
    // Plane origin: closest point of the plane to the AABB centre.
    let d = dot3(n, center) - c;
    let o = [center[0] - n[0] * d, center[1] - n[1] * d, center[2] - n[2] * d];
    let half = 0.5 * diag;
    let res = 240;
    let (mut on, mut owned) = (0usize, 0usize);
    for i in 0..res {
        for j in 0..res {
            let a = -half + 2.0 * half * (i as f32 + 0.5) / res as f32;
            let bq = -half + 2.0 * half * (j as f32 + 0.5) / res as f32;
            let p = [
                o[0] + u[0] * a + v[0] * bq,
                o[1] + u[1] * a + v[1] * bq,
                o[2] + u[2] * a + v[2] * bq,
            ];
            if p[0] < bb.min[0] || p[0] > bb.max[0] || p[1] < bb.min[1] || p[1] > bb.max[1]
                || p[2] < bb.min[2] || p[2] > bb.max[2]
            {
                continue;
            }
            let (f, cs) = bounded.evaluate(p, eps, crate::csg::EvalMode::Logical);
            if f.abs() <= eps {
                on += 1;
                if cs.as_slice().contains(&crate::csg::BranchId(id)) {
                    owned += 1;
                }
            }
        }
    }
    println!("  coverage branch {id}: on-boundary samples {on}, of which contributor {owned} (eps {eps:.5})");
}

