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

use crate::Kernel;
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
    // (3 → 5). The rest of those facets' boundaries — the vertical seams
    // against their neighbours — are a THIRD family: measured on pair
    // (131111, 131112), the seam line has no active interval anywhere
    // (`both_branches_active` is false along its whole length), so no amount
    // of seeding reaches it.
    assert_eq!(
        open_faces(&mut kernel),
        vec![0, 0, 0, 0, 0, 0, 2, 0, 0, 1, 8, 0],
        "faces without a traversable boundary (#436/#437); was [.., 6, ..] on body 10",
    );
}
