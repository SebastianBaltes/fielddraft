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

/// Replay a recorded call list, returning `(per-call milliseconds, body count)`.
fn replay(json: &str) -> (Vec<f64>, usize) {
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
    (timings, kernel.body_count())
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
#[test]
fn onshape_419_revolve_remove_stays_bounded() {
    let (timings, bodies) = replay(include_str!("testdata/onshape419_revolve_boolean.json"));

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
}
