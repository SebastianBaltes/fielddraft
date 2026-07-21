//! #384 offset_capability tests — one per capability tier, plus the
//! field-bit-identity guarantee and the end-to-end Kernel wiring.
//!
//! The report-level tests drive the exact regen-time pass sequence
//! (`expand_shells → push_down_offsets → fold_offset_leaves`, then the
//! non-dual `collect_offset_capability` walk and the dual-seam
//! `synthesize_offset_channels_reported`) so they exercise the real refusal
//! paths, not a mock.

use serde_json::json;

use fielddraft_protocol::{FeatureCall, FeatureId, Primitive, Transform};

use crate::bvh::BoundedCsg;
use crate::csg::{CsgNode, EvalMode};
use crate::offset_capability::{CapabilityReport, CapabilityStatus};
use crate::offset_channel::{
    prune_dead_channels, synthesize_offset_channels, synthesize_offset_channels_reported,
    CHANNEL_TOL_RENDER,
};
use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

fn id(n: u32) -> FeatureId {
    FeatureId::from_u32(n)
}

fn leaf_box(fid: u32, h: [f32; 3]) -> CsgNode {
    CsgNode::Leaf {
        id: id(fid),
        primitive: Primitive::Box { center: [0.0; 3], half_extents: h },
    }
}

/// Run the real regen-time offset pipeline on a hand-built tree and return the
/// collected capability report (mirror of `build_bounded_for_body_reported`
/// without the feature-decode wrapping).
fn report_for(node: CsgNode) -> CapabilityReport {
    compile_for(node).1
}

/// Like [`report_for`] but also returns the compiled (channel-synthesised,
/// pruned) `BoundedCsg` field — for #395 field-probe tests.
fn compile_for(node: CsgNode) -> (BoundedCsg, CapabilityReport) {
    let csg = node.expand_shells().push_down_offsets().fold_offset_leaves();
    let mut report = CapabilityReport::new();
    csg.collect_offset_capability(&mut report);
    let mut bounded = synthesize_offset_channels_reported(
        BoundedCsg::from_csg(&csg),
        CHANNEL_TOL_RENDER,
        &mut report,
    );
    prune_dead_channels(&mut bounded, CHANNEL_TOL_RENDER);
    (bounded, report)
}

/// A closed sweep profile that contains a quadratic Bézier segment (the family
/// `fold_offset_leaves` refuses).
fn quad_bezier_path() -> Sketch2d {
    Sketch2d::Prim(Sketch2dPrimitive::Path {
        segments: vec![
            SketchSegment::Line { start: [0.0, 0.0], end: [10.0, 0.0] },
            SketchSegment::Line { start: [10.0, 0.0], end: [5.0, 10.0] },
            SketchSegment::QuadBezier { p0: [5.0, 10.0], p1: [0.0, 10.0], p2: [0.0, 0.0] },
        ],
        closed: true,
    })
}

/// Union dihedral with an air wedge (mirror of `offset_channel_tests::
/// notch_union_node`): its root stays `Offset(Union, −δ)`, the dual stop the
/// channel synthesis corrects.
fn notch_union_node(alpha_deg: f64, delta: f64) -> CsgNode {
    let alpha = alpha_deg.to_radians();
    let gamma = std::f64::consts::PI - alpha;
    let b1 = CsgNode::Transform(
        Transform {
            translate: [0.0, -50.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(1, [100.0, 50.0, 50.0])),
    );
    let b2 = CsgNode::Transform(
        Transform {
            translate: [(50.0 * gamma.sin()) as f32, (-50.0 * gamma.cos()) as f32, 0.0],
            rotation: [0.0, 0.0, (gamma / 2.0).sin() as f32, (gamma / 2.0).cos() as f32],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [100.0, 50.0, 50.0])),
    );
    CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(b1), Box::new(b2))),
        -(delta as f32),
    )
}

// ─── (a) closed box shell → exact ────────────────────────────────────────

#[test]
fn closed_box_shell_is_exact() {
    // A closed shell dualises to `Difference(box, InnerWall(Offset(box, −t)))`;
    // the erosion folds into a shrunk box → no residual f−δ, no channel.
    let node = CsgNode::Shell(Box::new(leaf_box(1, [20.0, 20.0, 20.0])), 2.0, Vec::new());
    let report = report_for(node);
    assert!(report.is_empty(), "reasons: {:?}", report.reasons());
    assert_eq!(report.status(), CapabilityStatus::Exact);
}

// ─── (b) offset on a Bézier extrude → conservative (with reason) ──────────

#[test]
fn offset_on_quadbezier_extrude_is_conservative() {
    let ext = CsgNode::Extrude {
        id: id(1),
        sketch: Box::new(quad_bezier_path()),
        half_height: 5.0,
        wall_subs: None,
    };
    let report = report_for(CsgNode::Offset(Box::new(ext), -1.0));
    assert_eq!(report.status(), CapabilityStatus::Conservative);
    assert!(
        report.reasons().iter().any(|r| r.contains("Bézier")),
        "expected a Bézier reason, got {:?}",
        report.reasons()
    );
}

// ─── (c) offset over a boolean union → tolerance_bounded ─────────────────

#[test]
fn offset_over_union_is_tolerance_bounded() {
    let report = report_for(notch_union_node(90.0, 2.0));
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "reasons: {:?}",
        report.reasons()
    );
    assert!(
        report.reasons().iter().any(|r| r.contains("Arc-Spline")),
        "expected an arc-spline channel reason, got {:?}",
        report.reasons()
    );
}

// ─── #395 Kontakt-Prisma (Befund F2, Fix): koplanarer Kontakt ────────────

/// Zwei bündig gestapelte Boxen mit UNGLEICHEM Querschnitt (submergierte
/// interne Grenzfläche) unter Erosion. Vor #395 meldete der Kernel das ehrlich
/// als `conservative` (#394); seit dem Kontakt-Prisma + ∂P-Rim ist der Fehler
/// getilgt → `tolerance_bounded` (Prism exakt bis auf den δ·(1/sin(α/2)−1)-
/// Sliver am Rand, den der Arc-Kanal schließt). Feld-Probe: der frühere
/// Phantom-Schlitz ist geschlossen.
#[test]
fn coplanar_stacked_boxes_erosion_is_tolerance_bounded() {
    let b1 = leaf_box(1, [10.0, 10.0, 10.0]);
    // Bündig an x = 10 (b1.+x-Fläche ≡ b2.−x-Fläche), aber schmaler in y/z
    // → die gemeinsame Fläche ist eine echte, submergierte 2D-Grenzfläche.
    let b2 = CsgNode::Transform(
        Transform {
            translate: [18.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [8.0, 7.0, 6.0])),
    );
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(b1.clone()), Box::new(b2.clone()))),
        -1.5,
    );
    let (field, report) = compile_for(node);
    assert!(
        matches!(
            report.status(),
            CapabilityStatus::ToleranceBounded | CapabilityStatus::Exact
        ),
        "koplanarer Stapel muss seit #395 korrigiert sein — reasons: {:?}",
        report.reasons()
    );
    assert!(
        !report.reasons().iter().any(|r| r.contains("unbeschränkt")),
        "der Rest darf nicht mehr unbeschränkt sein — reasons: {:?}",
        report.reasons()
    );

    // Feld-Probe: der Phantom-Schlitz lag im submergierten Kern (|y|≤7,|z|≤6)
    // bei x knapp jenseits der naiven Erosions-Wand (x = 10 − δ = 8.5). Dort war
    // das naive Feld `min(A,B)+δ` fälschlich > 0 (Loch); mit dem Prisma muss es
    // jetzt < 0 sein (solides Material).
    let phantom = [9.5f32, 0.0, 0.0];
    let naive_min = b1_b2_min(&b1, &b2, phantom);
    let naive = naive_min + 1.5; // min − delta, delta = −1.5
    assert!(naive > 0.0, "Testaufbau: naiver Phantom-Punkt sollte >0 sein (war {naive})");
    let corrected = field.sdf(phantom, EvalMode::Visual);
    assert!(
        corrected < 0.0,
        "Phantom-Schlitz nicht geschlossen: f({phantom:?}) = {corrected} (soll < 0)"
    );
}

/// Feld-Ground-Truth-Helfer: das naive `min(f_A, f_B)` an einem Punkt.
fn b1_b2_min(b1: &CsgNode, b2: &CsgNode, p: [f32; 3]) -> f32 {
    let f1 = BoundedCsg::from_csg(b1).sdf(p, EvalMode::Visual);
    let f2 = BoundedCsg::from_csg(b2).sdf(p, EvalMode::Visual);
    f1.min(f2)
}

/// GLEICHER Querschnitt (Union = eine größere Box): das Kontakt-Prisma IST die
/// Union (S = U), die Erosion ist metrisch **exakt** — kein Toleranz-Downgrade.
#[test]
fn coplanar_equal_boxes_erosion_is_exact() {
    let b1 = leaf_box(1, [10.0, 10.0, 10.0]);
    let b2 = CsgNode::Transform(
        Transform {
            translate: [20.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [10.0, 10.0, 10.0])),
    );
    let node = CsgNode::Offset(Box::new(CsgNode::Union(Box::new(b1), Box::new(b2))), -2.0);
    let report = report_for(node);
    assert_eq!(
        report.status(),
        CapabilityStatus::Exact,
        "gleicher Querschnitt → S = U → exakt; reasons: {:?}",
        report.reasons()
    );
}

/// Gekrümmter Kontakt (Kugel bündig auf Box-Fläche): kein planarer Carrier
/// koinzident mit der Box-Fläche → das Kontakt-Prisma feuert NICHT, der Status
/// bleibt unverändert (keine Falschkorrektur, Design §1.5).
#[test]
fn sphere_flush_on_box_does_not_trigger_prism() {
    let b = leaf_box(1, [15.0, 15.0, 15.0]);
    let sph = CsgNode::Leaf {
        id: id(2),
        primitive: Primitive::Sphere { center: [0.0, 0.0, 15.0 + 8.0], radius: 8.0 },
    };
    let node = CsgNode::Offset(Box::new(CsgNode::Union(Box::new(b), Box::new(sph))), -2.0);
    let report = report_for(node);
    // Der Prisma-Pfad darf hier nie eine Toleranz-/Exakt-Zusage aus einem
    // koplanaren Prisma erfinden; die Kugel-Naht bleibt wie gehabt (die
    // Kanal-Synthese entscheidet — nie eine koplanar-Prisma-Notiz).
    assert!(
        !report
            .reasons()
            .iter()
            .any(|r| r.contains("Kontakt-Prisma")),
        "gekrümmter Kontakt darf kein Kontakt-Prisma auslösen — reasons: {:?}",
        report.reasons()
    );
}

/// Gegenprobe: eine TRANSVERSALE Dieder-Union (Luftkeil, echte Nahtkurve)
/// unter Erosion wird von der Kanal-Synthese korrigiert und darf NICHT von
/// der Ehrlichkeits-Heuristik degradiert werden — die Normalen an der Naht
/// sind nicht anti-parallel, es gibt keinen koplanaren internen Kontakt.
#[test]
fn transversal_union_erosion_is_not_degraded() {
    let report = report_for(notch_union_node(90.0, 2.0));
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "transversale Union darf nicht conservative werden — reasons: {:?}",
        report.reasons()
    );
    assert!(
        !report
            .reasons()
            .iter()
            .any(|r| r.contains("Koplanar") || r.contains("submergierte")),
        "transversale Naht fälschlich als koplanar gemeldet: {:?}",
        report.reasons()
    );
}

/// Gegenprobe: DISJUNKTE Kinder (kein Kontakt) erodieren unabhängig und
/// metrisch exakt — kein dualer Naht-Fehler, keine Degradierung.
#[test]
fn disjoint_union_erosion_is_exact() {
    let b1 = leaf_box(1, [10.0, 10.0, 10.0]);
    let b2 = CsgNode::Transform(
        Transform {
            translate: [40.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [10.0, 10.0, 10.0])),
    );
    let node = CsgNode::Offset(Box::new(CsgNode::Union(Box::new(b1), Box::new(b2))), -1.5);
    let report = report_for(node);
    assert_eq!(
        report.status(),
        CapabilityStatus::Exact,
        "disjunkte Kinder dürfen nicht degradiert werden — reasons: {:?}",
        report.reasons()
    );
}

/// S ⊆ U (Design §4, beweistragende Eigenschaft): das Kontakt-Prisma darf kein
/// Material AUSSERHALB der Union hinzufügen. Feld-Test: an jedem Punkt, der
/// klar (> |δ|) außerhalb von U = min(A,B) liegt, muss das korrigierte
/// Erosions-Feld positiv (außen) bleiben — sonst hätte das Prisma über eine
/// Nachbarfläche hinaus gefüllt.
#[test]
fn contact_prism_stays_inside_union() {
    let b1 = leaf_box(1, [12.0, 12.0, 12.0]);
    // In y/z versetzt + schmaler → echtes Schnitt-Sub-Rechteck, Stufen an ∂P.
    let b2 = CsgNode::Transform(
        Transform {
            translate: [22.0, 5.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [10.0, 9.0, 8.0])),
    );
    let delta = -2.0f32;
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(b1.clone()), Box::new(b2.clone()))),
        delta,
    );
    let (field, _report) = compile_for(node);
    let fb1 = BoundedCsg::from_csg(&b1);
    let fb2 = BoundedCsg::from_csg(&b2);
    let guard = 0.05f32;
    let da = delta.abs();
    // Gitter über die gepolsterte Szene.
    let (lo, hi) = ([-16.0f32, -18.0, -18.0], [36.0f32, 18.0, 14.0]);
    let n = 40;
    let mut violations = 0;
    for ix in 0..=n {
        for iy in 0..=n {
            for iz in 0..=n {
                let p = [
                    lo[0] + (hi[0] - lo[0]) * ix as f32 / n as f32,
                    lo[1] + (hi[1] - lo[1]) * iy as f32 / n as f32,
                    lo[2] + (hi[2] - lo[2]) * iz as f32 / n as f32,
                ];
                let u = fb1
                    .sdf(p, EvalMode::Visual)
                    .min(fb2.sdf(p, EvalMode::Visual));
                if u > da + guard {
                    // Klar außerhalb U (und damit außerhalb der Erosion U⊖δ):
                    // das korrigierte Feld muss außen sein.
                    if field.sdf(p, EvalMode::Visual) <= 0.0 {
                        violations += 1;
                    }
                }
            }
        }
    }
    assert_eq!(violations, 0, "S ⊄ U: Prisma fügt Material außerhalb der Union hinzu");
}

// ─── (d) axis-touching revolve → conservative (with reason) ──────────────

#[test]
fn offset_on_axis_touching_revolve_is_conservative() {
    // Rectangle meridian touching the rotation axis (min x = 0) → the #390
    // fold gate refuses, leaving f−δ standing.
    let sketch = Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
        center: [1.0, 0.0],
        half_extents: [1.0, 5.0],
    });
    let rev = CsgNode::Revolve { id: id(1), sketch: Box::new(sketch), wall_subs: None };
    let report = report_for(CsgNode::Offset(Box::new(rev), -1.0));
    assert_eq!(report.status(), CapabilityStatus::Conservative);
    assert!(
        report.reasons().iter().any(|r| r.contains("Achse") || r.contains("#390")),
        "expected an axis reason, got {:?}",
        report.reasons()
    );
}

// ─── dilation of an exact leaf is exact (f−δ IS the offset SDF) ───────────

#[test]
fn dilation_of_exact_leaf_is_exact() {
    let report = report_for(CsgNode::Offset(Box::new(leaf_box(1, [10.0, 10.0, 10.0])), 2.0));
    assert!(report.is_empty(), "reasons: {:?}", report.reasons());
    assert_eq!(report.status(), CapabilityStatus::Exact);
}

// ─── erosion of an un-foldable primitive family → conservative ───────────

#[test]
fn erosion_of_unfoldable_family_is_conservative() {
    // A hex prism is outside the analytic fold family (offset_fold.rs) → the
    // erosion refuses and stays f−δ.
    let hex = CsgNode::Leaf {
        id: id(1),
        primitive: Primitive::HexPrism { apothem: 10.0, half_height: 5.0 },
    };
    let report = report_for(CsgNode::Offset(Box::new(hex), -1.0));
    assert_eq!(report.status(), CapabilityStatus::Conservative);
    assert!(
        report.reasons().iter().any(|r| r.contains("Sechskant")),
        "reasons: {:?}",
        report.reasons()
    );
}

// ─── dilation on an INEXACT leaf field → conservative ────────────────────

#[test]
fn dilation_of_inexact_leaf_is_conservative() {
    // A gyroid field is `|trig| − t`, not a distance — even dilation `f − δ`
    // only approximates the offset surface.
    let gyroid = CsgNode::Leaf {
        id: id(1),
        primitive: Primitive::Gyroid { cell_size: 10.0, thickness: 1.0 },
    };
    let report = report_for(CsgNode::Offset(Box::new(gyroid), 2.0));
    assert_eq!(report.status(), CapabilityStatus::Conservative);
    assert!(
        report.reasons().iter().any(|r| r.contains("keine exakte Distanz")),
        "reasons: {:?}",
        report.reasons()
    );
}

// ─── open shells: exact leaf stays exact, composite body degrades ────────

#[test]
fn open_shell_on_exact_leaf_is_exact() {
    // Open shell wall = zero set of `d + t`; on an exact leaf SDF that IS the
    // true erosion surface.
    let node = CsgNode::Shell(Box::new(leaf_box(1, [20.0, 20.0, 20.0])), 2.0, vec![crate::csg::OpenSurface {
        branch: crate::csg::BranchId::from_feature_sub(&id(1), 4).as_u32(),
        surface: fielddraft_protocol::HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 20.0 },
        placement: Transform::IDENTITY,
    }]);
    let report = report_for(node);
    assert!(report.is_empty(), "reasons: {:?}", report.reasons());
    assert_eq!(report.status(), CapabilityStatus::Exact);
}

#[test]
fn open_shell_on_composite_body_is_conservative() {
    // Over a union the child field is no distance near the seam — the wall
    // thickness drifts exactly like f−δ. Must be reported, not silent.
    let b1 = leaf_box(1, [20.0, 20.0, 20.0]);
    let b2 = CsgNode::Transform(
        Transform {
            translate: [15.0, 15.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [20.0, 20.0, 20.0])),
    );
    let node = CsgNode::Shell(
        Box::new(CsgNode::Union(Box::new(b1), Box::new(b2))),
        2.0,
        vec![crate::csg::OpenSurface {
        branch: crate::csg::BranchId::from_feature_sub(&id(1), 4).as_u32(),
        surface: fielddraft_protocol::HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 20.0 },
        placement: Transform::IDENTITY,
    }],
    );
    let report = report_for(node);
    assert_eq!(report.status(), CapabilityStatus::Conservative);
    assert!(
        report.reasons().iter().any(|r| r.contains("Offene Shell")),
        "reasons: {:?}",
        report.reasons()
    );
}

// ─── status aggregation ordering ─────────────────────────────────────────

#[test]
fn status_takes_the_worst_downgrade() {
    let mut r = CapabilityReport::new();
    assert_eq!(r.status(), CapabilityStatus::Exact);
    r.note_tolerance("t");
    assert_eq!(r.status(), CapabilityStatus::ToleranceBounded);
    r.note_conservative("c");
    assert_eq!(r.status(), CapabilityStatus::Conservative);
    // conservative reasons are listed first.
    assert_eq!(r.reasons(), vec!["c".to_string(), "t".to_string()]);
    // dedup.
    r.note_conservative("c");
    assert_eq!(r.reasons().iter().filter(|x| *x == "c").count(), 1);
}

// ─── field bit-identity: report collection never changes the field ───────

#[test]
fn field_bit_identical_with_report() {
    let node = notch_union_node(90.0, 2.0);
    let plain = synthesize_offset_channels(BoundedCsg::from_csg(&node), CHANNEL_TOL_RENDER);
    let mut report = CapabilityReport::new();
    let reported = synthesize_offset_channels_reported(
        BoundedCsg::from_csg(&node),
        CHANNEL_TOL_RENDER,
        &mut report,
    );
    // The report must have observed the tolerance-bounded correction …
    assert_eq!(report.status(), CapabilityStatus::ToleranceBounded);
    // … while the evaluated field is bit-for-bit identical to the plain build.
    let mut checked = 0usize;
    let mut x = -110.0f32;
    while x <= 110.0 {
        let mut y = -60.0f32;
        while y <= 60.0 {
            let mut z = -60.0f32;
            while z <= 60.0 {
                let p = [x, y, z];
                let a = plain.sdf(p, EvalMode::Visual);
                let b = reported.sdf(p, EvalMode::Visual);
                assert_eq!(
                    a.to_bits(),
                    b.to_bits(),
                    "field diverged at {p:?}: {a} vs {b}"
                );
                checked += 1;
                z += 17.0;
            }
            y += 17.0;
        }
        x += 17.0;
    }
    assert!(checked > 100, "expected a dense grid, sampled {checked}");
}

// ─── (e) end-to-end: Twist in scope → FeatureError + Unsupported ─────────

#[test]
fn twisted_offset_is_unsupported_end_to_end() {
    let mut k = crate::Kernel::new();
    k.document.feature_calls = vec![
        FeatureCall::new(
            "box",
            id(1),
            json!({ "half_extents": [10.0, 10.0, 10.0], "twist_rate": 0.4 }),
        ),
        FeatureCall::new(
            "offset",
            id(2),
            json!({ "targets": "qCreatedBy(1)", "delta": { "value": 2.0, "unit": "mm" } }),
        ),
    ];
    k.bootstrap_features_from_calls().unwrap();
    // The offset still refuses the twisted body (existing D4 guard) …
    assert!(k.feature_errors.contains_key(&id(2)), "offset must error on a twisted body");
    // … and its capability is surfaced as unsupported.
    let cap = k.feature_capabilities.get(&id(2)).expect("offset capability present");
    assert_eq!(cap.status, CapabilityStatus::Unsupported);
}

#[test]
fn plain_offset_is_exact_end_to_end() {
    let mut k = crate::Kernel::new();
    k.document.feature_calls = vec![
        FeatureCall::new("box", id(1), json!({ "half_extents": [10.0, 10.0, 10.0] })),
        FeatureCall::new(
            "offset",
            id(2),
            json!({ "targets": "qCreatedBy(1)", "delta": { "value": 2.0, "unit": "mm" } }),
        ),
    ];
    k.bootstrap_features_from_calls().unwrap();
    assert!(!k.feature_errors.contains_key(&id(2)));
    let cap = k.feature_capabilities.get(&id(2)).expect("offset capability present");
    assert_eq!(cap.status, CapabilityStatus::Exact, "reasons: {:?}", cap.reasons);
}

