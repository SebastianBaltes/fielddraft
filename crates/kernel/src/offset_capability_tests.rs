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

// ─── #395 Stufe 3: Kontakt-Prisma für den DILATIONS-Fall (Difference) ────

/// Bündig ausgeschnittene Tasche: A minus B, wobei B's +x-Fläche koplanar mit
/// A's +x-Fläche liegt (flush cut). Unter DILATION +δ legt `max(f_A, −f_B) − δ`
/// einen Phantom-Fin über den Taschenmund P (versiegelt ihn). Das Kontakt-Prisma
/// als Schnitt-Constraint `max(f_A, −f_B, −f_S) − δ` (Komplement-Dualität) tilgt
/// ihn → tolerance_bounded, und die Feldprobe zeigt: der Punkt über P ist nach
/// der Korrektur außen (statt fälschlich solide).
fn flush_pocket_difference(delta: f32) -> (CsgNode, CsgNode, CsgNode) {
    let a = leaf_box(1, [10.0, 10.0, 10.0]); // x,y,z ∈ [−10,10]
    // B: he[5,4,4] @ (5,0,0) → x∈[0,10], y,z∈[−4,4]; +x-Fläche bei x=10 (bündig,
    // parallele wahre Normale zu A's +x-Fläche → Difference-Kontakt).
    let b = CsgNode::Transform(
        Transform { translate: [5.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(leaf_box(2, [5.0, 4.0, 4.0])),
    );
    let node = CsgNode::Offset(Box::new(CsgNode::Difference(Box::new(a.clone()), Box::new(b.clone()))), delta);
    (node, a, b)
}

#[test]
fn dilation_flush_pocket_difference_is_tolerance_bounded() {
    let delta = 2.0f32;
    let (node, a, b) = flush_pocket_difference(delta);
    let (field, report) = compile_for(node);
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "koplanarer Dilations-Kontakt muss seit #395 Stufe 3 korrigiert sein — reasons: {:?}",
        report.reasons()
    );
    assert!(
        report.reasons().iter().any(|r| r.contains("Kontakt-Prisma") && r.contains("Dilations-Stopp")),
        "erwartete Dilations-Prisma-Notiz, got {:?}",
        report.reasons()
    );
    assert!(
        !report.reasons().iter().any(|r| r.contains("Stufe 3, Offset")),
        "die alte Stufe-3-conservative-Notiz darf hier nicht mehr feuern — {:?}",
        report.reasons()
    );

    // Feld-Probe: der Phantom-Fin lag über dem Taschenmund (P = y,z∈[−4,4]) knapp
    // außerhalb der Kontaktebene x=10. Dort war `max(f_A,−f_B)−δ` fälschlich < 0
    // (Fin versiegelt die Tasche); mit dem Prisma muss es jetzt > 0 sein (außen).
    let fa = BoundedCsg::from_csg(&a);
    let fb = BoundedCsg::from_csg(&b);
    let phantom = [10.5f32, 0.0, 0.0];
    let naive = fa.sdf(phantom, EvalMode::Visual).max(-fb.sdf(phantom, EvalMode::Visual)) - delta;
    assert!(naive < 0.0, "Testaufbau: naiver Phantom-Fin-Punkt sollte < 0 sein (war {naive})");
    let corrected = field.sdf(phantom, EvalMode::Visual);
    assert!(
        corrected > 0.0,
        "Phantom-Fin nicht getilgt: f({phantom:?}) = {corrected} (soll > 0, wahre Dilation ist dort außen)"
    );
}

/// Gegenprobe „kein Fehlfeuern": ein Difference, dessen Subtrahend AUSSERHALB von
/// A liegt und A nur an einer Fläche berührt (anti-parallele wahre Normalen),
/// erzeugt KEINEN submergierten Kontakt → das Dilations-Prisma feuert nicht.
#[test]
fn dilation_touching_difference_does_not_trigger_prism() {
    let a = leaf_box(1, [10.0, 10.0, 10.0]); // x∈[−10,10]
    // B @ (−20,0,0): x∈[−30,−10]; B's +x-Fläche bei x=−10 ≡ A's −x-Fläche
    // (anti-parallele WAHRE Normalen → im Komplement PARALLEL → kein Kontakt).
    let b = CsgNode::Transform(
        Transform { translate: [-20.0, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(leaf_box(2, [10.0, 10.0, 10.0])),
    );
    let node = CsgNode::Offset(Box::new(CsgNode::Difference(Box::new(a), Box::new(b))), 2.0);
    let report = report_for(node);
    assert!(
        !report.reasons().iter().any(|r| r.contains("Kontakt-Prisma")),
        "berührender (nicht submergierter) Difference-Kontakt darf kein Dilations-Prisma auslösen — {:?}",
        report.reasons()
    );
    // Ehrlichkeits-Fallback (#394): der tangentiale Kontakt hat KEINE Prisma-
    // Korrektur bekommen — dann muss die conservative-Degradierung bleiben
    // (wie vor Stufe 3), nicht still entfallen.
    assert_eq!(
        report.status(),
        CapabilityStatus::Conservative,
        "tangentialer Dilations-Kontakt ohne Prisma muss conservative bleiben — {:?}",
        report.reasons()
    );
    assert!(
        report.reasons().iter().any(|r| r.contains("ohne Prisma-Korrektur")),
        "erwartete den Ehrlichkeits-Fallback am Dilations-Stopp — {:?}",
        report.reasons()
    );
}

/// Duale Sicherheit (Analogon zu `contact_prism_stays_inside_union`, 40³): das
/// Dilations-Prisma (und die Kanäle) dürfen NIRGENDS Material entfernen, das der
/// innere Körper hat. Formal: wo das korrigierte Feld gegenüber dem naiven
/// `max(f_A,−f_B)−δ` Material entfernt hat (corrected > naive), muss der Punkt
/// AUSSERHALB des inneren Solids liegen (`max(f_A,−f_B) ≥ −guard`).
#[test]
fn dilation_prism_removes_no_inner_material() {
    let delta = 2.0f32;
    let (node, a, b) = flush_pocket_difference(delta);
    let (field, _report) = compile_for(node);
    let fa = BoundedCsg::from_csg(&a);
    let fb = BoundedCsg::from_csg(&b);
    let guard = 0.05f32;
    let (lo, hi) = ([-14.0f32, -14.0, -14.0], [22.0f32, 14.0, 14.0]);
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
                let f_inner = fa.sdf(p, EvalMode::Visual).max(-fb.sdf(p, EvalMode::Visual));
                let naive = f_inner - delta;
                let corrected = field.sdf(p, EvalMode::Visual);
                // Dilation-Korrekturen sind rein subtraktiv (corrected ≥ naive).
                if corrected > naive + 1e-3 && f_inner < -guard {
                    // Material entfernt, obwohl der Punkt klar im inneren Solid liegt.
                    violations += 1;
                }
            }
        }
    }
    assert_eq!(violations, 0, "Dilations-Prisma entfernt Material aus dem inneren Solid (S ⊄ ¬inner)");
}

/// Gekrümmter Subtrahend (Kugel schneidet A) unter Dilation: keine planare
/// Box-Kontaktfläche → das Dilations-Prisma feuert NICHT (keine Falschkorrektur).
#[test]
fn dilation_curved_subtrahend_does_not_trigger_prism() {
    let a = leaf_box(1, [12.0, 12.0, 12.0]);
    let sph = CsgNode::Leaf {
        id: id(2),
        primitive: Primitive::Sphere { center: [12.0, 0.0, 0.0], radius: 8.0 },
    };
    let node = CsgNode::Offset(Box::new(CsgNode::Difference(Box::new(a), Box::new(sph))), 2.0);
    let report = report_for(node);
    assert!(
        !report.reasons().iter().any(|r| r.contains("Kontakt-Prisma")),
        "gekrümmter Subtrahend darf kein Dilations-Prisma auslösen — {:?}",
        report.reasons()
    );
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
        branch: crate::csg::BranchId::from_feature_sub(&id(1), 4).as_u64(),
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
        branch: crate::csg::BranchId::from_feature_sub(&id(1), 4).as_u64(),
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

// ─── #397 sign-bewusste Koplanaritäts-Heuristik am Dilations-Stopp ────────

fn rect_extrude(fid: u32, cx: f32, cy: f32, hx: f32, hy: f32, hh: f32) -> CsgNode {
    CsgNode::Extrude {
        id: id(fid),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [cx, cy],
            half_extents: [hx, hy],
        })),
        half_height: hh,
        wall_subs: None,
    }
}

/// Bündig ausgeschnittene Tasche aus NICHT-Box-Carriern (Extrude − Extrude): B's
/// +x-Seitenwand (x=10) ist koplanar mit A's +x-Seitenwand. Die WAHREN Normalen
/// sind PARALLEL (beide +x); effektiv (De-Morgan-Komplement, Difference-
/// Rechtskind trägt −1) sind sie anti-parallel und KOINZIDENT (|O_i+O_j|≈0 — ein
/// echter submergierter Flush-Cut, KEINE versetzte Dünnwand). Vor #398/P2 sah die
/// Prisma-Synthese diesen Kontakt nicht (nur Box-Loops) → ehrliche conservative-
/// Notiz. Seit FC Phase 2 liefert `member_contact_faces` die Extrude-Mantel-
/// Facetten, der Verschnitt baut das Kontakt-Prisma (Komplement-Dualität) → der
/// Status steigt auf die ehrliche `tolerance_bounded` (Design §0).
fn extrude_flush_pocket(delta: f32) -> CsgNode {
    // A: x,y,z ∈ [−10,10]; B: x∈[0,10], y,z∈[−4,4].
    let a = rect_extrude(1, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = rect_extrude(2, 5.0, 0.0, 5.0, 4.0, 4.0);
    CsgNode::Offset(Box::new(CsgNode::Difference(Box::new(a), Box::new(b))), delta)
}

#[test]
fn extrude_flush_pocket_dilation_prism_p2() {
    let delta = 2.0f32;
    let node = extrude_flush_pocket(delta);
    let report = report_for(node);
    // FC Phase 2 (#398): der koinzidente Extrude-Flush-Cut bekommt jetzt ein
    // Kontakt-Prisma (Komplement-Dualität) → `tolerance_bounded` statt der
    // früheren `conservative`-Degradierung (design-gewollter Statusaufstieg §0).
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "koinzidenter Extrude-Flush-Cut muss seit P2 per Prisma tolerance_bounded sein — reasons: {:?}",
        report.reasons()
    );
    assert!(
        report.reasons().iter().any(|r| r.contains("Kontakt-Prisma")),
        "erwartete die Dilations-Prisma-Notiz — {:?}",
        report.reasons()
    );

    // Feld-Probe: BELEG, dass unter der (vorher zu guten) Zusage real ein
    // Phantom-Grat sitzt. Am Taschenmund knapp jenseits x=10 ist `max(f_A,−f_B)−δ`
    // fälschlich < 0 (naiver Fin versiegelt die Tasche), obwohl die WAHRE Dilation
    // dort außen ist: der nächste echte Körperpunkt (Mundrand [10,±4,·]) liegt
    // ≈ 4 mm entfernt, also > δ = 2 mm → der ganze δ-Ball um den Punkt ist Luft.
    let a = rect_extrude(1, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = rect_extrude(2, 5.0, 0.0, 5.0, 4.0, 4.0);
    let fa = BoundedCsg::from_csg(&a);
    let fb = BoundedCsg::from_csg(&b);
    let phantom = [10.5f32, 0.0, 0.0];
    let naive =
        fa.sdf(phantom, EvalMode::Visual).max(-fb.sdf(phantom, EvalMode::Visual)) - delta;
    assert!(
        naive < 0.0,
        "Testaufbau: naiver Phantom-Fin sollte < 0 (fälschlich solide) sein, war {naive}"
    );
    // Der δ-Ball um den Punkt ist restlos außerhalb des wahren Solids A−B (die
    // CSG-Vorzeichen von max(f_A,−f_B) sind exakt, auch wenn der Betrag
    // unterschätzt): das belegt d(phantom, A−B) > δ ⇒ wahre Dilation = Luft.
    let m = 6;
    for ix in -m..=m {
        for iy in -m..=m {
            for iz in -m..=m {
                let d = [
                    ix as f32 / m as f32,
                    iy as f32 / m as f32,
                    iz as f32 / m as f32,
                ];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 1.0 {
                    continue;
                }
                let r = 0.98 * delta;
                let q = [phantom[0] + d[0] * r, phantom[1] + d[1] * r, phantom[2] + d[2] * r];
                let inside = fa
                    .sdf(q, EvalMode::Visual)
                    .max(-fb.sdf(q, EvalMode::Visual));
                assert!(
                    inside > 0.0,
                    "der δ-Ball um den Phantom-Punkt darf kein wahres Material enthalten (q={q:?}, f={inside}) — sonst wäre die naive Zusage nicht falsch"
                );
            }
        }
    }
}

/// PFLICHT-Kalibrierung #397: Dünnwand-Shells (Difference outer − inner) dürfen
/// durch die sign-bewusste Verallgemeinerung NICHT ihre (bessere) Zusage
/// verlieren. Die EXTRUDE-Spalte ist der eigentliche #397-Gegenstand: sie
/// erreicht den eff-Fallback und belegt, dass die enge Koinzidenz-Toleranz
/// (0.02·δ) jede Wand ≥ 0.3·δ verwirft — sie steht unverändert auf der
/// HEAD-Baseline.
///
/// **Korrektur der BOX-Spalte (#384, v0.56.3).** Für w < 0.5·δ stand dort
/// `tolerance_bounded` — und das war KEINE bessere Zusage, sondern eine
/// falsche: die Box-Shell lief über die Dilations-Prisma-Synthese, deren
/// unsigniertes Fenster den vergrabenen Hohlraum (`s = −w`) für einen bündigen
/// Kontakt hielt. Das Prisma trug daraufhin die Wand ab, während die Zusage
/// ≤ 0.01 mm lautete; gemessen gegen die exakte Distanz zu `A \ B` fielen
/// 60 % aller Innenpunkte der wahren Dilatation weg (Fehler ≥ 2.000 mm = 1.0·δ,
/// s. `dilation_thin_wall_shell_zero_set_is_exact_384`). Die WAHRE Zusage
/// dieser Familie ist `exact` — dieselbe Messung zeigt für JEDE Wandstärke
/// (auch 0.1·δ) null Nullmengen-Fehler, und die Zeilen ≥ 0.8·δ standen schon
/// immer so.
#[test]
fn thin_wall_shell_status_unchanged_397() {
    let delta = 2.0f32;
    let ho = 10.0f32;
    // (w/δ, erwarteter BOX-Status, erwarteter EXTRUDE-Status).
    let expect = [
        (0.3f32, CapabilityStatus::Exact, CapabilityStatus::Exact),
        (0.5, CapabilityStatus::Exact, CapabilityStatus::Exact),
        (0.8, CapabilityStatus::Exact, CapabilityStatus::Exact),
        (1.0, CapabilityStatus::Exact, CapabilityStatus::Exact),
        (2.0, CapabilityStatus::Exact, CapabilityStatus::Exact),
    ];
    for (f, want_box, want_ext) in expect {
        let w = f * delta;
        let box_shell = CsgNode::Offset(
            Box::new(CsgNode::Difference(
                Box::new(leaf_box(1, [ho, ho, ho])),
                Box::new(leaf_box(2, [ho - w, ho - w, ho - w])),
            )),
            delta,
        );
        let ext_shell = CsgNode::Offset(
            Box::new(CsgNode::Difference(
                Box::new(rect_extrude(1, 0.0, 0.0, ho, ho, ho)),
                Box::new(rect_extrude(2, 0.0, 0.0, ho - w, ho - w, ho - w)),
            )),
            delta,
        );
        assert_eq!(
            report_for(box_shell).status(),
            want_box,
            "Box-Dünnwand w/δ={f} Status ≠ Soll"
        );
        assert_eq!(
            report_for(ext_shell).status(),
            want_ext,
            "Extrude-Dünnwand w/δ={f} Status ≠ HEAD-Baseline (eff-Fallback darf nicht feuern)"
        );
    }
}

// ─── #399 Offset auf Add-gedocktem Mehr-Member-Body ──────────────────────
//
// Ein Offset, dessen Ziel-Body per Add-Primitive-Docking aus MEHREREN
// Features besteht, muss den GESAMTEN Body-Baum einfalten — nicht nur den
// Teilbaum des im `targets`-Query genannten Features. Vor dem Fix wrappte
// `wrap_offset` nur den Union-Zweig des benannten Features (A), der
// Add-gedockte Member (B) blieb in Originalgröße stehen; die Capability log
// zudem „exact" über einer objektiv falschen Teilmengen-Erosion.
#[test]
fn offset_on_add_docked_multi_member_body_erodes_whole_body_399() {
    let mut k = crate::Kernel::new();
    k.document.feature_calls = vec![
        // A: New, he 10³ → x,y,z ∈ [−10,10].
        FeatureCall::new("box", id(1), json!({ "half_extents": [10.0, 10.0, 10.0] })),
        // B: Add gedockt in A, he [8,7,6] @ +18 → x ∈ [10,26] (bündiger Stack).
        FeatureCall::new(
            "box",
            id(2),
            json!({
                "half_extents": [8.0, 7.0, 6.0],
                "translate": [18.0, 0.0, 0.0],
                "op": "add",
                "targets": "qCreatedBy(1)"
            }),
        ),
        // Offset −1.5 auf den Body (per qCreatedBy(A) benannt).
        FeatureCall::new(
            "offset",
            id(3),
            json!({ "targets": "qCreatedBy(1)", "delta": { "value": -1.5, "unit": "mm" } }),
        ),
    ];
    k.bootstrap_features_from_calls().unwrap();
    // Leere Bodies → Index-Panik in bodies_mesh.rs; erst nach Mesh-Rebuild
    // steht das kompilierte SDF.
    k.rebuild_mesh_internal();
    assert!(!k.feature_errors.contains_key(&id(3)), "offset darf nicht fehlern");

    // (a) Feldproben: die GESAMTE Union ist erodiert.
    //  • Connector innen (x∈[8.5,11.5] um die Mittellinie): d(9.25,0,0) < 0.
    let d_conn = k.body_sdf_eval(0, 9.25, 0.0, 0.0);
    assert!(
        d_conn < 0.0,
        "Connector muss innen sein (d(9.25,0,0)<0), war {d_conn} — B nicht eingefaltet?"
    );
    //  • B's +x-Fläche bei 24.5 statt 26: d(24,0,0) ≈ −0.5 (innen, 0.5 von der Fläche).
    let d_bx = k.body_sdf_eval(0, 24.0, 0.0, 0.0);
    assert!(
        d_bx < 0.0 && (d_bx - (-0.5)).abs() < 0.25,
        "B's +x-Fläche muss bei 24.5 liegen (d(24,0,0)≈−0.5), war {d_bx}"
    );

    // (b) Capability darf NIE „exact" sein: Erosion eines bündigen Stacks
    //     erzeugt Kontakt-Prisma + Rand-Fillet (tolerance_bounded).
    let cap = k.feature_capabilities.get(&id(3)).expect("offset capability present");
    assert_ne!(
        cap.status,
        CapabilityStatus::Exact,
        "Erosion eines zusammenhängenden Mehr-Member-Bodies ist nicht exakt — reasons: {:?}",
        cap.reasons
    );
    // Beim bündigen Stack läuft der Fall regulär durch die Pipeline und landet
    // ehrlich bei ToleranceBounded (Kontakt-Prisma + ∂P-Rand-Fillet).
    assert_eq!(
        cap.status,
        CapabilityStatus::ToleranceBounded,
        "bündiger Stack → tolerance_bounded — reasons: {:?}",
        cap.reasons
    );
    // Das kompilierte Tape trägt den Rand-Fillet-Block (OP_RIM_FILLET 0x43).
    let has_rim = k.bodies[0]
        .compiled_sdf
        .as_ref()
        .map(|s| s.opcodes.iter().any(|&o| o.to_bits() == crate::sdf_compiler::OP_RIM_FILLET))
        .unwrap_or(false);
    assert!(has_rim, "Tape muss den Rand-Fillet-Block (OP_RIM_FILLET) enthalten");
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

// ─── #384 Penetrations-Befund: der Phantom-Schlitz an durchdringenden Membern ─
//
// Zwei achsparallel gestapelte Boxen, die sich um `s` DURCHDRINGEN (statt sich
// bündig zu berühren), unter Erosion `−δ`. Der Kontakt-Prisma-Pfad testete die
// zwei anti-parallelen Kontaktebenen vor #384 mit einem UNSIGNIERTEN Fenster
// (`|o_a + o_b| ≤ 0.5·δ`) und verankerte das Prisma an EINER der beiden Ebenen.
// Damit vermengte er drei physikalisch verschiedene Lagen und log in allen
// dreien `exact` (gemessen bei δ = 2 mm):
//
//   * `s < 0` (Luftspalt 0.1 mm): das Prisma überbrückte die LUFT → 8 mm
//     Phantom-Material, wo die wahre Erosion leer ist;
//   * `0 < s ≤ 0.5·δ`: das Prisma ragte um genau `s` über die Gegenseite
//     hinaus → bis 1 mm Phantom-Material über der wahren Erosionsfläche;
//   * `0.5·δ < s < 2·δ`: gar kein Prisma → die naive Erosion `min(f_i) + δ`
//     riss einen Phantom-Schlitz der Dicke `2δ − s`, Nullmengenfehler bis
//     1.47 mm (≈0.735·δ).
//
// Seit #384 trennt `face_pair_penetration` die drei Fälle signiert und das
// Prisma spannt von Member i's bis zu Member j's Rückseite.

/// Baut den Stapel: Box A (he 10³, Ursprung) + Box B (he `[hx,hy,10]`), deren
/// Unterseite `overlap` mm UNTER A's Oberseite liegt (negativ = Luftspalt),
/// unter Erosion `−delta`.
fn stacked_pair(overlap: f32, hx: f32, hy: f32, delta: f32) -> (CsgNode, CsgNode, CsgNode) {
    let b1 = leaf_box(1, [10.0, 10.0, 10.0]);
    let b2 = CsgNode::Transform(
        Transform {
            translate: [0.0, 0.0, 20.0 - overlap],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [hx, hy, 10.0])),
    );
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(b1.clone()), Box::new(b2.clone()))),
        -delta,
    );
    (b1, b2, node)
}

/// **Kern-Regression.** Überlappung `s` mit `0 < s < 2·δ` (δ = 2 mm): die naiv
/// erodierten Kinder sind DISJUNKT (A↓δ endet bei z = 8, B↓δ beginnt bei
/// z = 12 − s), die wahre Erosion der Vereinigung ist an der Naht massiv. Vor
/// #384 stand dort ein `2δ − s` mm dicker Phantom-Schlitz — mit dem Stempel
/// `exact`. Der Sweep überstreicht die ganze Bandbreite, insbesondere das Band
/// oberhalb der alten `0.5·δ`-Klemme (dort fehlte das Prisma vollständig) UND
/// deren Rand (dort stand ein überschießendes Prisma).
#[test]
fn penetrating_stack_erosion_has_no_phantom_slit_384() {
    for overlap in [0.5f32, 1.0, 1.05, 1.5, 2.0, 3.0, 3.9] {
        let (b1, b2, node) = stacked_pair(overlap, 10.0, 10.0, 2.0);
        let (field, report) = compile_for(node);

        // Testaufbau-Beweis: das NAIVE Feld `min(f_A,f_B) + δ` reißt den Schlitz
        // (Dicke 2δ − s, Nullmengenfehler (2δ − s)/2 in der Naht-Mitte).
        let seam = [0.0f32, 0.0, 10.0 - overlap / 2.0];
        let naive = b1_b2_min(&b1, &b2, seam) + 2.0;
        assert!(
            (naive - (4.0 - overlap) / 2.0).abs() < 1e-3,
            "Testaufbau (Überlappung {overlap}): naive Erosion soll um \
             {} klaffen, war {naive}",
            (4.0 - overlap) / 2.0
        );

        // Wahre Erosion: U = Box x,y ∈ [−10,10], z ∈ [−10, 30−s] ⇒ U⊖2 hat
        // x,y ∈ [−8,8], z ∈ [−8, 28−s]. An der Naht ist sie 8 mm tief.
        let f = field.sdf(seam, EvalMode::Visual);
        assert!(
            f < -7.5,
            "Überlappung {overlap}: Phantom-Schlitz an der Naht, f{seam:?} = {f} (soll ≈ −8)"
        );

        // Der ganze naive Schlitz-Bereich muss massiv sein.
        let mut z = 8.0f32;
        while z <= 12.0 - overlap {
            let v = field.sdf([0.0, 0.0, z], EvalMode::Visual);
            assert!(v < 0.0, "Überlappung {overlap}: Schlitz-Rest bei z = {z}: f = {v}");
            z += 0.05;
        }

        // Gleicher Querschnitt ⇒ das Prisma IST die Union (S = U) ⇒ exakt.
        assert_eq!(
            report.status(),
            CapabilityStatus::Exact,
            "Überlappung {overlap}: Gleich-Querschnitt ist S = U ⇒ exakt — reasons: {:?}",
            report.reasons()
        );
    }
}

/// Das Prisma darf am durchdringenden Kontakt NICHT über die Gegenseite
/// hinausragen. Vor #384 war es an Member i's Ebene verankert und trug Member
/// j's volle Dicke → bei Überlappung `s` lag seine Rückseite `s` mm zu hoch,
/// und die Erosion meldete dort Material, wo die wahre Erosion Luft hat.
/// (Der frühere Fehler saß im Band `0 < s ≤ 0.5·δ`, das die alte Klemme als
/// „bündig" behandelte.)
#[test]
fn penetrating_stack_prism_does_not_overshoot_384() {
    for overlap in [0.1f32, 0.5, 1.0, 1.5, 3.0] {
        let (_, _, node) = stacked_pair(overlap, 10.0, 10.0, 2.0);
        let (field, _) = compile_for(node);
        // Wahre Union-Oberkante: z = 30 − overlap; wahre Erosionsdecke 2 mm
        // darunter. 0.05 mm darüber MUSS das Feld außen sein.
        let top = 30.0 - overlap - 2.0;
        let f = field.sdf([0.0, 0.0, top + 0.05], EvalMode::Visual);
        assert!(
            f > 0.0,
            "Überlappung {overlap}: Prisma ragt über die wahre Erosionsdecke \
             (f(0,0,{}) = {f}, soll > 0)",
            top + 0.05
        );
        // Und 0.05 mm darunter innen (die Decke sitzt wirklich dort).
        let g = field.sdf([0.0, 0.0, top - 0.05], EvalMode::Visual);
        assert!(g < 0.0, "Überlappung {overlap}: Erosionsdecke fehlt (f = {g})");
    }
}

/// Gegenprobe LUFTSPALT: zwei Boxen mit 0.1 mm Abstand berühren sich NICHT.
/// Ihre Erosionen sind unabhängig und exakt — das Kontakt-Prisma darf den
/// Spalt nicht überbrücken. Vor #384 tat es genau das (der unsignierte
/// 0.5·δ-Test sah den Spalt als Kontakt) und erfand 8 mm Material.
#[test]
fn air_gap_stack_erosion_is_not_bridged_384() {
    for gap in [0.1f32, 0.5, 1.0] {
        let (_, _, node) = stacked_pair(-gap, 10.0, 10.0, 2.0);
        let (field, report) = compile_for(node);
        // Mitte des (2δ + gap) dicken Zwischenraums: muss AUSSEN sein.
        let f = field.sdf([0.0, 0.0, 10.0 + gap / 2.0], EvalMode::Visual);
        assert!(
            f > 0.0,
            "Luftspalt {gap} mm überbrückt: f = {f} (soll > 0 — getrennte Körper)"
        );
        // Getrennte Körper erodieren exakt; kein Verdachts-Downgrade.
        assert_eq!(
            report.status(),
            CapabilityStatus::Exact,
            "Luftspalt {gap} mm: getrennte Körper erodieren exakt — reasons: {:?}",
            report.reasons()
        );
    }
}

/// Pin: der BÜNDIGE Fall (Überlappung exakt 0) läuft unverändert über die
/// Prisma-Synthese — gleicher Querschnitt `exact`, schmalerer Aufsatz
/// `tolerance_bounded` mit dem ∂P-Rand-Fillet auf Bau-Toleranz. Diese
/// Kalibrierung darf sich durch #384 NICHT bewegen.
#[test]
fn flush_stack_erosion_unchanged_384() {
    let (_, _, equal) = stacked_pair(0.0, 10.0, 10.0, 2.0);
    let (field, report) = compile_for(equal);
    assert_eq!(
        report.status(),
        CapabilityStatus::Exact,
        "bündig + gleicher Querschnitt bleibt exakt — reasons: {:?}",
        report.reasons()
    );
    assert!(field.sdf([0.0, 0.0, 10.0], EvalMode::Visual) < -7.5, "bündige Naht muss massiv sein");

    let (_, _, narrow) = stacked_pair(0.0, 7.0, 7.0, 2.0);
    let (_, report) = compile_for(narrow);
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "bündig + schmaler Aufsatz bleibt toleranzbeschränkt — reasons: {:?}",
        report.reasons()
    );
    assert!(
        report.reasons().iter().any(|r| r.contains("Rand-Fillet")),
        "der bündige Kontakt muss weiter über Prisma + ∂P-Rand-Fillet laufen: {:?}",
        report.reasons()
    );
}

/// Pin: ab Überlappung ≥ 2·δ überlappen sich schon die erodierten Kinder —
/// es entsteht gar kein Schlitz, `min(f_i) + δ` ist dort zero-set-exakt und
/// die Synthese lässt den Stopp bewusst in Ruhe.
#[test]
fn deep_overlap_stack_erosion_stays_exact_384() {
    for overlap in [4.0f32, 6.0, 10.0] {
        let (_, _, node) = stacked_pair(overlap, 10.0, 10.0, 2.0);
        let (field, report) = compile_for(node);
        assert_eq!(
            report.status(),
            CapabilityStatus::Exact,
            "Überlappung {overlap} ≥ 2δ: kein Schlitz, kein Downgrade — reasons: {:?}",
            report.reasons()
        );
        let top = 30.0 - overlap - 2.0;
        assert!(field.sdf([0.0, 0.0, top - 0.05], EvalMode::Visual) < 0.0);
        assert!(field.sdf([0.0, 0.0, top + 0.05], EvalMode::Visual) > 0.0);
        assert!(field.sdf([0.0, 0.0, 9.0], EvalMode::Visual) < 0.0, "Naht muss massiv sein");
    }
}

/// Durchdringung mit UNGLEICHEM Querschnitt: das Prisma tilgt den flächigen
/// Schlitz, am Kontaktrand ∂P steht eine echte transversale Dieder-Naht. Die
/// darf NICHT (wie am bündigen Kontakt) unterdrückt werden — die SSI-Kanäle
/// sind dort die richtige Korrektur —, und die Capability muss die weitere
/// B0-Schranke nennen statt der engen `channel_tol` eines Rand-Fillets, den
/// es an dieser Geometrie nicht gibt.
#[test]
fn penetrating_stack_unequal_cross_section_is_honestly_bounded_384() {
    let (_, _, node) = stacked_pair(1.0, 7.0, 7.0, 2.0);
    let (field, report) = compile_for(node);
    assert_eq!(
        report.status(),
        CapabilityStatus::ToleranceBounded,
        "reasons: {:?}",
        report.reasons()
    );
    assert!(
        report.reasons().iter().any(|r| r.contains("Durchdringender Kontakt")),
        "die Durchdringung muss benannt sein: {:?}",
        report.reasons()
    );
    assert!(
        !report.reasons().iter().any(|r| r.contains("unbeschränkt")),
        "der Rest darf nicht unbeschränkt sein: {:?}",
        report.reasons()
    );
    // Der flächige Schlitz ist weg: über dem schmalen Aufsatz massiv.
    let f = field.sdf([0.0, 0.0, 9.5], EvalMode::Visual);
    assert!(f < -4.5, "Phantom-Schlitz unter dem schmalen Aufsatz: f = {f}");
    // Und nichts ragt über die wahre Erosionsdecke (z = 27) hinaus.
    assert!(field.sdf([0.0, 0.0, 27.05], EvalMode::Visual) > 0.0);
}

/// Ende-zu-Ende über die echte API-Kette (Dokument → Features → kompiliertes
/// Feld via `body_sdf_eval`): der Befund und sein Fix am Produkt, nicht nur an
/// der internen Pass-Kette.
#[test]
fn penetrating_stack_erosion_end_to_end_384() {
    let mut k = crate::Kernel::new();
    k.document.feature_calls = vec![
        FeatureCall::new("box", id(1), json!({ "half_extents": [10.0, 10.0, 10.0] })),
        // 1 mm Durchdringung: B's Unterseite bei z = 9, A's Oberseite bei z = 10.
        FeatureCall::new(
            "box",
            id(2),
            json!({
                "half_extents": [10.0, 10.0, 10.0],
                "translate": [0.0, 0.0, 19.0],
                "op": "add",
                "targets": "qCreatedBy(1)"
            }),
        ),
        FeatureCall::new(
            "offset",
            id(3),
            json!({ "targets": "qCreatedBy(1)", "delta": { "value": -2.0, "unit": "mm" } }),
        ),
    ];
    k.bootstrap_features_from_calls().unwrap();
    k.rebuild_mesh_internal();
    assert!(!k.feature_errors.contains_key(&id(3)), "offset darf nicht fehlern");

    // U = Box z ∈ [−10,29] ⇒ U⊖2: z ∈ [−8,27], x,y ∈ [−8,8].
    let d_seam = k.body_sdf_eval(0, 0.0, 0.0, 9.5);
    assert!(d_seam < -7.5, "Phantom-Schlitz an der Naht: d(0,0,9.5) = {d_seam}");
    let d_below_top = k.body_sdf_eval(0, 0.0, 0.0, 26.5);
    assert!(d_below_top < 0.0, "unter der Erosionsdecke muss Material sein: {d_below_top}");
    let d_above_top = k.body_sdf_eval(0, 0.0, 0.0, 27.5);
    assert!(d_above_top > 0.0, "über der Erosionsdecke darf kein Material sein: {d_above_top}");

    // Kein `exact`-Lug und kein Fehlalarm: gleicher Querschnitt ⇒ S = U ⇒ exakt,
    // und das Feld oben ist nachweislich korrekt.
    let cap = k.feature_capabilities.get(&id(3)).expect("offset capability present");
    assert_eq!(cap.status, CapabilityStatus::Exact, "reasons: {:?}", cap.reasons);
}


// ─── #384 Dilations-Duale: die signierte Lage im Komplement ───────────────
//
// Der DILATIONS-Prisma-Pfad (`synthesize_contact_prisms_dilation`) prüfte die
// Lage zweier anti-paralleler Komplement-Facetten bis v0.56.2 mit demselben
// UNSIGNIERTEN Fenster (`|O_i + O_j| ≤ 0.5·δ`), das der Erosions-Pfad in
// #384 verloren hat. Die signierte Größe bedeutet hier aber etwas ANDERES
// (Komplement-Dualität, Design §8d):
//
//   `s = O_i + O_j`,  `O = −sign·o`  ⇒  für `A − B` ist `s = o_B − o_A`
//
//   * `s = 0`   bündig ausgeschnittene Tasche (Taschenmund koplanar mit A's
//               Außenfläche) — der Fall, für den die Korrektur gebaut wurde;
//   * `s > 0`   B ragt um `s` aus A heraus (Taschenboden liegt `s` höher);
//   * `s < 0`   B ist um `|s|` VERGRABEN — ein geschlossener Hohlraum hinter
//               einem Deckel/Steg der Dicke `|s|`. **Gar kein Kontakt.**
//
// Das Vorzeichen dreht sich also gegenüber der Erosion: dort war `s < 0` der
// harmlose Luftspalt, hier ist es der gefährliche Fall. Gemessen (δ = 2 mm,
// Referenz = exakte Distanz zu `A \ B`):
//
// | Lage | Status (alt) | zugesagt (alt) | real (alt) | nach dem Fix |
// |---|---|---|---|---|
// | `s < 0`, Deckel ≤ 0.5·δ | `tolerance_bounded` | ≤ 0.01 mm | **≥ 2.000 mm** (der Deckel wird ABGETRAGEN) | `exact`, 0.000 mm |
// | `s = 0` | `tolerance_bounded` | ≤ 0.01 mm | 0.000 mm | unverändert |
// | `0 < s ≤ 0.5·δ` | `tolerance_bounded` | ≤ 0.01 mm | **= s** (bis 1.000 mm) | ≤ 1.5 mm zugesagt, 0.395 mm real |
// | `0.5·δ < s < 2·δ` | `tolerance_bounded` | ≤ 0.01 mm | **5.159 mm** (kein Prisma, Phantom-Fin) | ≤ 1.5 mm zugesagt, 0.395 mm real |
//
// Der `s < 0`-Fall ist der schwerste: die dünnwandige Box-Shell
// (`Difference(box, box)`, Wand `w < 0.5·δ`) IST dieser Fall mit `s = −w` auf
// allen sechs Achsen. Das Prisma spannte dort vom Deckel-Außenrand quer durch
// die Wand bis zur Rückseite des Hohlraums und subtrahierte sie — 60 % aller
// Innenpunkte der wahren Dilatation fielen weg, 5766 davon lagen im echten
// Solid selbst. Zugesagt waren 0.01 mm.

/// Exakte Distanz zu `A \ B` (achsparallele Boxen) über die ≤6-Box-Zerlegung
/// des Differenzkörpers — die Ground-Truth-Referenz dieser Sonden. Die wahre
/// Dilatation um δ ist genau `{x : dist(x, A\B) ≤ δ}`.
fn dist_to_box_minus_box(p: [f64; 3], a: ([f64; 3], [f64; 3]), b: ([f64; 3], [f64; 3])) -> f64 {
    let mut c0 = [0.0f64; 3];
    let mut c1 = [0.0f64; 3];
    for k in 0..3 {
        c0[k] = b.0[k].max(a.0[k]);
        c1[k] = b.1[k].min(a.1[k]);
    }
    let mut boxes: Vec<([f64; 3], [f64; 3])> = Vec::new();
    if (0..3).any(|k| c1[k] <= c0[k]) {
        boxes.push(a); // B trifft A gar nicht
    } else {
        if c0[0] > a.0[0] {
            boxes.push(([a.0[0], a.0[1], a.0[2]], [c0[0], a.1[1], a.1[2]]));
        }
        if c1[0] < a.1[0] {
            boxes.push(([c1[0], a.0[1], a.0[2]], [a.1[0], a.1[1], a.1[2]]));
        }
        if c0[1] > a.0[1] {
            boxes.push(([c0[0], a.0[1], a.0[2]], [c1[0], c0[1], a.1[2]]));
        }
        if c1[1] < a.1[1] {
            boxes.push(([c0[0], c1[1], a.0[2]], [c1[0], a.1[1], a.1[2]]));
        }
        if c0[2] > a.0[2] {
            boxes.push(([c0[0], c0[1], a.0[2]], [c1[0], c1[1], c0[2]]));
        }
        if c1[2] < a.1[2] {
            boxes.push(([c0[0], c0[1], c1[2]], [c1[0], c1[1], a.1[2]]));
        }
    }
    let mut best = f64::INFINITY;
    for (lo, hi) in boxes {
        let mut d2 = 0.0;
        for k in 0..3 {
            let dk = (lo[k] - p[k]).max(0.0).max(p[k] - hi[k]);
            d2 += dk * dk;
        }
        best = best.min(d2.sqrt());
    }
    best
}

/// Misst die Nullmenge des kompilierten Feldes gegen die wahre Dilatation von
/// `A \ B` auf einem Gitter über `[lo,hi]³`. Rückgabe: (Zahl der
/// Fehlklassifikationen mit messbarem Abstand zur wahren Grenze, größter
/// solcher Abstand, dessen Ort). `|d − δ|` ist wegen der 1-Lipschitz-Stetigkeit
/// von `d` eine UNTERE Schranke des wahren Nullmengen-Fehlers — die Zahlen
/// unten sind also konservativ. Punkte, die exakt auf der Grenzfläche liegen
/// (`|d − δ| ≤ 1e-3`), zählen nicht: dort ist das Vorzeichen per Definition
/// unbestimmt.
fn dilation_zero_set_error(
    field: &BoundedCsg,
    a: ([f64; 3], [f64; 3]),
    b: ([f64; 3], [f64; 3]),
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
) -> (usize, f64, [f64; 3]) {
    let n = 60;
    let (mut miss, mut worst, mut worst_p) = (0usize, 0.0f64, [0.0f64; 3]);
    for ix in 0..=n {
        for iy in 0..=n {
            for iz in 0..=n {
                let t = [ix, iy, iz].map(|i| i as f64 / n as f64);
                let p = [
                    lo[0] + (hi[0] - lo[0]) * t[0],
                    lo[1] + (hi[1] - lo[1]) * t[1],
                    lo[2] + (hi[2] - lo[2]) * t[2],
                ];
                let d = dist_to_box_minus_box(p, a, b);
                let err = (d - delta as f64).abs();
                if err <= 1e-3 {
                    continue; // exakt auf der wahren Grenzfläche
                }
                let got = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                if (d <= delta as f64) != (got < 0.0) {
                    miss += 1;
                    if err > worst {
                        worst = err;
                        worst_p = p;
                    }
                }
            }
        }
    }
    (miss, worst, worst_p)
}

/// A = Box he 10³ (Ursprung), B = Box he [7,7,5] mit Mittelpunkt `z = 5 + s`,
/// unter Dilation `+delta`. `s > 0`: B ragt oben aus A heraus; `s = 0`:
/// bündige Tasche; `s < 0`: vergrabener Hohlraum mit Deckel der Dicke `|s|`.
fn pocket_dilation(s: f32, delta: f32) -> CsgNode {
    let a = leaf_box(1, [10.0, 10.0, 10.0]);
    let b = CsgNode::Transform(
        Transform {
            translate: [0.0, 0.0, 5.0 + s],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [7.0, 7.0, 5.0])),
    );
    CsgNode::Offset(Box::new(CsgNode::Difference(Box::new(a), Box::new(b))), delta)
}

fn pocket_boxes(s: f32) -> (([f64; 3], [f64; 3]), ([f64; 3], [f64; 3])) {
    (
        ([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]),
        ([-7.0, -7.0, s as f64], [7.0, 7.0, s as f64 + 10.0]),
    )
}

/// **Kern-Regression (Dilations-Duale).** Ein VERGRABENER Hohlraum (`s < 0`)
/// ist kein koplanarer Kontakt: das naive `max(f_A, −f_B) − δ` ist dort
/// zero-set-exakt (kein Phantom-Fin, es gibt keinen Taschenmund). Vor dem Fix
/// hielt das unsignierte Fenster jeden Deckel dünner als 0.5·δ für einen
/// bündigen Kontakt und trug ihn per Prisma ab — unter der Zusage ≤ 0.01 mm.
#[test]
fn dilation_buried_cavity_is_not_a_contact_384() {
    let delta = 2.0f32;
    for s in [-1.0f32, -0.9, -0.5, -0.2, -0.05] {
        let (field, report) = compile_for(pocket_dilation(s, delta));
        let (a, b) = pocket_boxes(s);
        let (miss, worst, at) =
            dilation_zero_set_error(&field, a, b, delta, [-14.0; 3], [14.0, 14.0, 16.0]);
        assert_eq!(
            miss, 0,
            "Deckel {} mm: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?} — \
             der vergrabene Hohlraum darf kein Kontakt-Prisma auslösen",
            -s
        );
        assert_eq!(
            report.status(),
            CapabilityStatus::Exact,
            "Deckel {} mm: getrennte Grenzflächen ⇒ das naive Feld ist zero-set-exakt — reasons: {:?}",
            -s,
            report.reasons()
        );
    }
}

/// Dieselbe Lage in ihrer praktisch wichtigsten Form: die geschlossene
/// Box-Shell `Difference(box, box)` unter Dilation. Ihre sechs Wandpaare sind
/// `s = −w`. Vor dem Fix verschwand für `w < 0.5·δ` der halbe Körper (60 % der
/// Innenpunkte, darunter Punkte im echten Solid) — bei zugesagten 0.01 mm.
/// Die Wandstärken ≥ 0.8·δ belegen mit derselben Referenz, dass `exact` die
/// WAHRE Zusage dieser Familie ist (dort lief sie schon immer so).
#[test]
fn dilation_thin_wall_shell_zero_set_is_exact_384() {
    let delta = 2.0f32;
    let ho = 10.0f32;
    for f in [0.1f32, 0.3, 0.5, 0.8, 1.0] {
        let w = f * delta;
        let node = CsgNode::Offset(
            Box::new(CsgNode::Difference(
                Box::new(leaf_box(1, [ho, ho, ho])),
                Box::new(leaf_box(2, [ho - w, ho - w, ho - w])),
            )),
            delta,
        );
        let (field, report) = compile_for(node);
        let hi = (ho - w) as f64;
        let (miss, worst, at) = dilation_zero_set_error(
            &field,
            ([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]),
            ([-hi, -hi, -hi], [hi, hi, hi]),
            delta,
            [-14.0; 3],
            [14.0; 3],
        );
        assert_eq!(
            miss, 0,
            "Wand w/δ={f}: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?}"
        );
        assert_eq!(
            report.status(),
            CapabilityStatus::Exact,
            "Wand w/δ={f}: die Shell-Dilatation ist nachweislich zero-set-exakt — reasons: {:?}",
            report.reasons()
        );
    }
}

/// Pin: der BÜNDIGE Schnitt (`s = 0`) läuft unverändert über Prisma +
/// ∂P-Rand-Fillet und ist an dieser Geometrie messbar fehlerfrei. Diese
/// Kalibrierung darf sich durch die signierte Klassifikation NICHT bewegen.
#[test]
fn dilation_flush_pocket_unchanged_384() {
    let delta = 2.0f32;
    let (field, report) = compile_for(pocket_dilation(0.0, delta));
    let (a, b) = pocket_boxes(0.0);
    let (miss, worst, at) =
        dilation_zero_set_error(&field, a, b, delta, [-14.0; 3], [14.0, 14.0, 16.0]);
    assert_eq!(miss, 0, "bündige Tasche: {miss} Fehler, größter {worst:.4} mm bei {at:?}");
    assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
    assert!(
        report.reasons().iter().any(|r| r.contains("Rand-Fillet (OP_RIM_FILLET)")),
        "der bündige Schnitt muss weiter über Prisma + ∂P-Rand-Fillet laufen: {:?}",
        report.reasons()
    );
}

/// Ragt der Subtrahend um `0 < s < 2·δ` heraus, verankert das Prisma seit dem
/// Fix an Member j's echter Rückseite (`depth_pos − pen`) statt an der eigenen
/// Ebene, und die Zusage nennt die weitere B0-Schranke statt der Bau-Toleranz
/// eines Rand-Fillets, den es an dieser Geometrie nicht gibt. Vor dem Fix
/// sank der Taschenboden um genau `s` (`s ≤ 0.5·δ`) bzw. stand über der Tasche
/// ein 5.159 mm dicker Phantom-Fin (`s > 0.5·δ`, gar kein Prisma) — beides
/// unter der Zusage ≤ 0.01 mm.
#[test]
fn dilation_protruding_pocket_is_honestly_bounded_384() {
    let delta = 2.0f32;
    for s in [0.05f32, 0.2, 0.5, 0.9, 1.0, 1.5, 1.9, 3.0, 3.9] {
        let (field, report) = compile_for(pocket_dilation(s, delta));
        let (a, b) = pocket_boxes(s);
        let (_, worst, at) =
            dilation_zero_set_error(&field, a, b, delta, [-14.0; 3], [14.0, 14.0, 16.0]);
        assert!(
            worst <= 0.75 * delta as f64,
            "Überstand {s}: Nullmengen-Fehler {worst:.4} mm bei {at:?} verletzt die \
             zugesagte B0-Schranke {:.3} mm",
            0.75 * delta
        );
        assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
        assert!(
            report.reasons().iter().any(|r| r.contains("Durchdringender koplanarer Kontakt")),
            "Überstand {s} muss als Durchdringung benannt sein: {:?}",
            report.reasons()
        );
        assert!(
            !report.reasons().iter().any(|r| r.contains("Rand-Fillet (OP_RIM_FILLET)")),
            "Überstand {s}: der ∂P-Rand-Fillet hat hier keine gemeinsame Ebene Π und darf \
             seine enge Bau-Toleranz nicht zusagen: {:?}",
            report.reasons()
        );
    }
}

// ─── #384 §8f: das dritte Gate im Kanal-Opcode ───────────────────────────
//
// Bei `s ≥ 2·δ` entsteht KEIN Kontakt-Prisma (die Komplemente überlappen
// nicht mehr, §8d) — die Mündungs-Naht der Tasche korrigieren die regulären
// `OP_ARC_CHANNEL`-Kanäle. Deren Endpunkt-Ball-Kappe wirkte bis `K ≈ 1.95·δ`
// ÜBER das Nahtende hinaus: an der Außenkante des Trägerkörpers, wo das
// naive Feld exakt ist, trug sie 0.760 mm ab (worst bei
// `[−10.73, 6.53, 11.0]`, δ = 2) — unter der Zusage ≤ 0.01 mm. §8e schloss
// die Kappe synthese-seitig (Kegel-Gate des terminalen Bogens auf `∓t̂`) und
// kam auf 0.632 mm / 204 Fehlklassifikationen — den Rest trug die
// NACHBAR-Naht der Mündungs-Ecke, die näher lag als die eigene.
//
// Seit §8f hat der Opcode ein DRITTES Gate: je offenem Spine-Ende einen
// affinen Halbraum. An einer Ecke, in der eine Nachbar-Naht anschließt, ist
// das der Voronoi-Bisektor zwischen beiden Nähten — damit korrigiert immer
// genau der Kanal, dessen Naht wirklich am nächsten liegt. Gemessen gegen
// dieselbe exakte ≤6-Box-Referenz: **0 Fehlklassifikationen, 0.000 mm.**

/// **Kern-Regression (§8f).** Der Taschen-Stopp mit `s ≥ 2·δ` ist an der
/// exakten Referenz messbar fehlerfrei — und sagt das jetzt auch zu.
#[test]
fn dilation_channel_end_gate_is_exact_384() {
    let delta = 2.0f32;
    for s in [4.0f32, 5.0, 6.0] {
        let (field, report) = compile_for(pocket_dilation(s, delta));
        let (a, b) = pocket_boxes(s);
        let (miss, worst, at) =
            dilation_zero_set_error(&field, a, b, delta, [-14.0; 3], [14.0, 14.0, 16.0]);
        // Vor §8e: 0.760 mm / 544 Fehler. Nach §8e: 0.632 mm / 204.
        // Mit dem dritten Gate: 0.
        assert_eq!(
            miss, 0,
            "Überstand {s}: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?}"
        );
        // Der Punkt der alten §8d-Fehlstelle liegt richtig.
        let outer = [-10.7333f32, 6.5333, 11.0];
        assert!(
            field.sdf(outer, EvalMode::Visual) < 0.0,
            "die Außenkante des Trägerkörpers muss materiell bleiben (war +0.014)"
        );
        // Und das Randpunkt-Zeugen-Downgrade aus §8e ist WEG, weil der
        // Fehler weg ist — nicht, weil die Sonde weggeschaut hätte.
        assert!(
            !report.reasons().iter().any(|r| r.contains("NICHT auf die Bau-Toleranz")),
            "Überstand {s}: das dritte Gate schließt den Fehler, das Downgrade muss fallen: {:?}",
            report.reasons()
        );
        assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
    }
}

/// Der Spiegel am EROSIONS-Stopp: am `notch_union`-Dieder endet die Naht in
/// den Deckflächen `z = ±50`. Dort gibt es keine Nachbar-Naht, sondern eine
/// fremde FLÄCHE — das dritte Gate ist die um δ nach innen geschobene
/// Endebene. Gemessen wird gegen die exakte 3D-Referenz
/// `dist(x, ∁Körper)`: ein Punkt, den das Feld als IN der Erosion meldet,
/// dessen δ-Kugel aber aus dem Körper ragt, ist ein bewiesener Fehler.
/// Vor §8f: 0.496 mm (α = 90°) bzw. 0.998 mm (α = 60°).
#[test]
fn erosion_notch_seam_end_is_exact_384() {
    let delta = 2.0f32;
    for alpha in [90.0f64, 60.0] {
        let (field, report) = compile_for(notch_union_node(alpha, delta as f64));
        let (mut worst, mut at) = (0.0f64, [0.0f64; 3]);
        let n = 40;
        for ix in 0..=n {
            for iy in 0..=n {
                for iz in 0..=n {
                    let p = [
                        -10.0 + 20.0 * ix as f64 / n as f64,
                        -10.0 + 20.0 * iy as f64 / n as f64,
                        42.0 + 12.0 * iz as f64 / n as f64,
                    ];
                    let got = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                    if got >= 0.0 {
                        continue; // „nicht in der Erosion" — nichts zu beweisen
                    }
                    let u = dist_to_notch_complement(alpha, p);
                    if delta as f64 - u > worst {
                        worst = delta as f64 - u;
                        at = p;
                    }
                }
            }
        }
        assert!(
            worst <= 1e-2,
            "notch α={alpha}: das Feld behauptet Erosion bei {at:?}, die δ-Kugel ragt aber \
             {worst:.4} mm heraus"
        );
        assert!(
            !report.reasons().iter().any(|r| r.contains("NICHT auf die Bau-Toleranz")),
            "notch α={alpha}: das §8e-Downgrade muss fallen: {:?}",
            report.reasons()
        );
    }
}

/// Obere Schranke für `dist(p, ∁(Box₁ ∪ Box₂))` des `notch_union`-Körpers:
/// je Richtung exakt (Slab-Intervalle beider OBBs, verschmolzene Komponente
/// um `t = 0`), über eine dichte Fibonacci-Richtungsmenge minimiert. Eine
/// OBERE Schranke genügt: sie kann einen Fehler nur unterschätzen, nie
/// erfinden.
fn dist_to_notch_complement(alpha_deg: f64, p: [f64; 3]) -> f64 {
    let gamma = std::f64::consts::PI - alpha_deg.to_radians();
    let boxes: [([f64; 3], [f64; 3], f64); 2] = [
        ([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
        (
            [50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0],
            [100.0, 50.0, 50.0],
            gamma,
        ),
    ];
    let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let n = 600usize;
    let mut best = f64::INFINITY;
    for i in 0..n {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let th = ga * i as f64;
        let d = [r * th.cos(), r * th.sin(), z];
        let mut iv: Vec<(f64, f64)> = Vec::new();
        for (c, h, rot) in boxes {
            // In das Box-Frame drehen (Rotation um z um −rot).
            let (sn, cs) = (-rot).sin_cos();
            let rel = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            let o = [rel[0] * cs - rel[1] * sn, rel[0] * sn + rel[1] * cs, rel[2]];
            let dl = [d[0] * cs - d[1] * sn, d[0] * sn + d[1] * cs, d[2]];
            let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
            let mut hit = true;
            for k in 0..3 {
                if dl[k].abs() < 1e-12 {
                    if o[k].abs() > h[k] {
                        hit = false;
                    }
                } else {
                    let a = (-h[k] - o[k]) / dl[k];
                    let b = (h[k] - o[k]) / dl[k];
                    t0 = t0.max(a.min(b));
                    t1 = t1.min(a.max(b));
                }
            }
            if hit && t1 >= t0 && t1 > 0.0 {
                iv.push((t0.max(0.0), t1));
            }
        }
        if iv.is_empty() {
            return 0.0;
        }
        iv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if iv[0].0 > 1e-12 {
            return 0.0; // p liegt außerhalb des Körpers
        }
        let mut end = 0.0f64;
        for (a, b) in iv {
            if a <= end + 1e-9 {
                end = end.max(b);
            } else {
                break;
            }
        }
        best = best.min(end);
    }
    best
}



// ─── #384 §8g: Vertex-Kugel-Exposition und Junction-Anker ────────────────
//
// §8f schloss die Kanal-Enden mit dem dritten Gate. Zwei benachbarte
// Verdachtsflächen blieben UNGEMESSEN — beide brachen, beide gemessen an
// exakten Referenzen:
//
// 1. **Vertex-Kugel gegen Nachbar-Ecke.** Der C4-Term behauptet
//    `dist(x, ∁Ω) = |x−v|` im ganzen Trieder-Kegel bis `K = (c_v+0.5)·δ`.
//    Sitzt eine ZWEITE konkave Ecke näher als `K + δ`, erreicht deren
//    Komplement-Oktant Kegelpunkte früher als `v` — und weil der konkave
//    Term ein `min` bildet, GEWINNT die falsche, weiter entfernte Ecke
//    (dieselbe Fehlerklasse wie §8f an den Nahtenden, nur mit Punkt-Spine).
//    Gemessen an der dünnen Ecken-Schale (drei Platten der Dicke `t`,
//    δ = 2 mm), Referenz `dist(x, ∁(∪Boxen))` als Strahl-Minimum:
//
//    | `t/δ` | vorher | Zusage vorher | nachher |
//    |---|---|---|---|
//    | 3.0 | 510 Fehlklassifikationen, worst 0.523 mm | ≤ 0.01 mm (STILL) | **0**, ≤ 0.01 wahr |
//    | 3.5 | 19, worst 0.267 mm | ≤ 0.868 mm | **0**, ≤ 0.01 |
//    | 4.4 | 28, worst 0.407 mm | ≤ 0.868 mm | **0**, ≤ 0.01 |
//    | 5.0 | 25, worst 0.500 mm | ≤ 0.868 mm | **0**, ≤ 0.01 |
//    | ≤ 2.5 | bis 1.945 mm | ≤ 1.867 mm (GEBROCHEN) | `conservative`, keine Zusage |
//
//    Die Sonde war zugleich der Beleg, dass der Randpunkt-Zeugen-Wächter
//    aus §8e/§8f eine blinde Stelle hatte: er lief nur um KANAL-Enden,
//    nie um Vertex-Kugeln. Bei `t/δ = 3.0` meldete er darum gar nichts.
//
// 2. **Junction-Anker.** Der Bisektor wurde im MITTELPUNKT der beiden
//    Trace-Enden verankert. Der Versatz zur echten Ecke ist nicht der
//    Rückstand, sondern dessen DIFFERENZ — und die Trace-Schrittweite ist
//    2 % der Überlappungs-AABB, wächst also mit dem Bauteil, während die
//    Junction-Toleranz mit `0.5·δ` körpergrößen-blind war. Am um `f`
//    skalierten Taschen-Fixture (δ = 2 mm fest):
//
//    | `f` | Schritt | vorher | nachher |
//    |---|---|---|---|
//    | 1 | 0.28 mm | 0 Fehler, ≤ 0.01 | unverändert |
//    | 4 | 1.12 mm | 0 Fehler, ≤ 0.01 | unverändert |
//    | 8 | 2.24 mm | **23 Fehler, worst 0.436 mm** bei ≤ 0.01 | **0**, ≤ 0.01 |
//    | 16 | 4.48 mm | **35 Fehler, worst 0.436 mm** bei ≤ 0.01 | **0**, ≤ 0.01 |

/// Obere Schranke für `dist(p, ∁(∪ Boxen))` (achsparallel) — Spiegel von
/// [`dist_to_notch_complement`] ohne Rotation und für beliebig viele Boxen:
/// je Richtung exakt (verschmolzene Slab-Intervalle ab `t = 0`), über eine
/// dichte Fibonacci-Richtungsmenge minimiert. Eine OBERE Schranke genügt —
/// sie kann einen Fehler nur unterschätzen, nie erfinden.
fn dist_to_boxes_complement(boxes: &[([f64; 3], [f64; 3])], p: [f64; 3], ndir: usize) -> f64 {
    let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let mut best = f64::INFINITY;
    for i in 0..ndir {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / ndir as f64;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let th = ga * i as f64;
        let d = [r * th.cos(), r * th.sin(), z];
        let mut iv: Vec<(f64, f64)> = Vec::new();
        for (lo, hi) in boxes {
            let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
            let mut hit = true;
            for k in 0..3 {
                if d[k].abs() < 1e-12 {
                    if p[k] < lo[k] || p[k] > hi[k] {
                        hit = false;
                    }
                } else {
                    let a = (lo[k] - p[k]) / d[k];
                    let b = (hi[k] - p[k]) / d[k];
                    t0 = t0.max(a.min(b));
                    t1 = t1.min(a.max(b));
                }
            }
            if hit && t1 >= t0 && t1 > 0.0 {
                iv.push((t0.max(0.0), t1));
            }
        }
        if iv.is_empty() {
            return 0.0;
        }
        iv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if iv[0].0 > 1e-12 {
            return 0.0; // p liegt außerhalb des Körpers
        }
        let mut end = 0.0f64;
        for (a, b) in iv {
            if a <= end + 1e-9 {
                end = end.max(b);
            } else {
                break;
            }
        }
        best = best.min(end);
    }
    best
}

/// Drei Platten der Dicke `t` bilden eine konkave Ecke im Ursprung
/// (Material lokal `{−t≤x≤0} ∪ {−t≤y≤0} ∪ {−t≤z≤0}`, Luft = +Oktant). Die
/// acht Würfelecken von `[−t, 0]³` sind acht konkave Trieder — mit `t` lässt
/// sich ihr Abstand exakt einstellen und damit die Exposition der
/// Vertex-Kugeln durchfahren.
fn thin_corner_node(t: f32, l: f32, delta: f32) -> CsgNode {
    let plate = |fid: u32, c: [f32; 3], h: [f32; 3]| {
        CsgNode::Transform(
            Transform { translate: c, rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
            Box::new(leaf_box(fid, h)),
        )
    };
    let h = 0.5 * t;
    CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Union(
                Box::new(plate(1, [0.0, -h, 0.0], [l, h, l])),
                Box::new(plate(2, [-h, 0.0, 0.0], [h, l, l])),
            )),
            Box::new(plate(3, [0.0, 0.0, -h], [l, l, h])),
        )),
        -delta,
    )
}

fn thin_corner_boxes(t: f64, l: f64) -> Vec<([f64; 3], [f64; 3])> {
    vec![
        ([-l, -t, -l], [l, 0.0, l]),
        ([-t, -l, -l], [0.0, l, l]),
        ([-l, -l, -t], [l, l, 0.0]),
    ]
}

/// Zahl der Punkte, die das Feld als IN der Erosion meldet, deren δ-Kugel
/// aber nachweislich aus dem Körper ragt — plus der größte solche Nachweis.
/// Das Gitter ist um einen irrationalen Bruchteil versetzt: ein
/// achsen-ausgerichtetes Raster landet sonst genau auf den Kegel-Randebenen
/// der Vertex-Kugeln, wo das Feld exakt 0 ist und das Vorzeichen Rauschen ist.
fn erosion_zero_set_error(
    field: &BoundedCsg,
    boxes: &[([f64; 3], [f64; 3])],
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
    ndir: usize,
) -> (usize, f64, [f64; 3]) {
    let (mut miss, mut worst, mut at) = (0usize, 0.0f64, [0.0f64; 3]);
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let f = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * f[0],
                    lo[1] + (hi[1] - lo[1]) * f[1],
                    lo[2] + (hi[2] - lo[2]) * f[2],
                ];
                if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                    continue; // „nicht in der Erosion" — nichts zu beweisen
                }
                let u = dist_to_boxes_complement(boxes, p, ndir);
                let err = delta as f64 - u;
                if err <= 1e-3 {
                    continue;
                }
                miss += 1;
                if err > worst {
                    worst = err;
                    at = p;
                }
            }
        }
    }
    (miss, worst, at)
}

/// **Kern-Regression (§8g, Verdachtsfläche 1).** Solange die Schale dick
/// genug ist, dass die Erosion überhaupt überlebt (`t ≥ 3·δ`), ist der
/// Ecken-Stopp an der exakten Referenz messbar fehlerfrei — und sagt das
/// auch zu. Vor der Nachbar-Kegel-Klemme gewann bei `t = 3·δ` die
/// ÜBERNÄCHSTE Ecke den `min` und behauptete Material in wahrer Luft: 510
/// Fehlklassifikationen, worst 0.523 mm, gemeldet ≤ 0.01 mm.
#[test]
fn erosion_thin_corner_vertex_sphere_is_exact_384() {
    let delta = 2.0f32;
    for tf in [3.0f32, 4.0, 4.4, 6.0] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let lo = -(t as f64 + delta as f64 + 1.0);
        let (miss, worst, at) =
            erosion_zero_set_error(&field, &boxes, delta, [lo; 3], [1.0; 3], 26, 200);
        // Geprüft wird die ZUGESAGTE Schranke, nicht der Messboden: unter
        // `channel_tol` liegt der Arc-Spline-Fit selbst (0.0013 mm bei
        // t/δ = 4 — die Kurventoleranz, nicht die Ecken-Exposition).
        assert!(
            worst <= CHANNEL_TOL_RENDER as f64,
            "Schale t/δ={tf}: {miss} Fehlklassifikationen, größte {worst:.4} mm bei {at:?} — \
             die Vertex-Kugel darf über die Nachbar-Ecke hinaus nichts behaupten \
             (vor §8g: 510 Stück bis 0.523 mm)"
        );
        assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
        assert!(
            !report.reasons().iter().any(|r| r.contains("NICHT auf die Bau-Toleranz")),
            "Schale t/δ={tf}: der Fehler ist weg, also muss auch das Downgrade fallen: {:?}",
            report.reasons()
        );
    }
}

/// Der Gegenpol: eine Schale unter `3·δ` erodiert zu (fast) nichts und die
/// Ecken-Kegel überlappen einander vollständig. Dort darf der Stopp KEINE
/// Bau-Toleranz zusagen — und was immer er zusagt, muss gegen die exakte
/// Referenz stimmen.
///
/// **Pin-Bewegung §8i.** Bis §8h meldete diese Schale `conservative`: der
/// Randpunkt-Zeuge wies eine Fehlklassifikation ÜBER der strukturellen
/// Gate-Schranke `G` nach, also war gar keine Schranke mehr belegbar. Das
/// war der Kaskaden-Effekt — mehrere Terme addierten sich über `G` hinaus.
/// Seit das Gate gegen den naiven Stoppwert `d₀` rechnet, ist `G` eine echte
/// globale Schranke; der Zeuge bleibt darunter, und der Stopp sagt wieder
/// `≤ G` zu. Gemessen an der exakten Referenz (34³ Punkte, 240 Richtungen):
/// t/δ = 2.0 → 0.841 mm, t/δ = 2.5 → 0.822 mm, beide unter den zugesagten
/// 0.868 mm. Der Pin prüft deshalb ab jetzt die DOKTRIN statt des Status.
///
/// **Pin-Bewegung §8k — die angesagte.** Der alte Wortlaut hatte eine
/// Vorbedingung: `miss > 0`, „die Sonde taugt nur, solange die kollabierende
/// Erosion überhaupt noch Fehlklassifikationen erzeugt". Sie erzeugt keine
/// mehr. Die Naht-Synthese lieferte an dieser Fixture nur 16 der 24 konkaven
/// Nahtstrahlen (verworfene Traces hatten die Seed-Sperrliste `claimed`
/// bereits gefüllt); seit `claimed` erst nach `build_channel` gefüllt wird,
/// sind es 24 und der Über-Claim ist bei BEIDEN Dicken exakt null. Die
/// Vorbedingung fror damit einen Fehler ein, den es nicht mehr gibt. An ihre
/// Stelle tritt die Bedingung, die der Pin eigentlich meint: die Sonde muss
/// ÜBERHAUPT etwas sehen (Punkte im Feld), und was der Stopp zusagt, muss
/// gegen die exakte Referenz halten — jetzt auf BEIDEN Seiten.
#[test]
fn erosion_collapsing_shell_promise_is_true_384() {
    let delta = 2.0f32;
    for tf in [2.0f32, 2.5] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field,
            delta,
            [lo; 3],
            [1.0; 3],
            26,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 200),
        );
        assert!(
            r.pts > 1000 && r.xmax <= 1e-6,
            "Schale t/δ={tf}: die Sonde muss ein dichtes Gitter gegen eine exakte Referenz \
             fahren ({} Punkte, Referenz-Drift {:.6})",
            r.pts,
            r.xmax
        );
        if let Some(b) = promised_bound(&report) {
            assert!(
                r.over.1 <= b + 1e-3,
                "Schale t/δ={tf}: {} Punkte Phantom-Material, größtes {:.4} mm bei {:?} — \
                 zugesagt sind aber nur {b:.3} mm. Reasons: {:?}",
                r.over.0,
                r.over.1,
                r.over.2,
                report.reasons()
            );
        }
        if r.under.1 > 1e-2 {
            assert!(
                report
                    .reasons()
                    .iter()
                    .any(|s| s.contains("FEHLENDES Material") || s.contains("keine Schranke")),
                "Schale t/δ={tf}: {} Punkte mit FEHLENDEM Material, größtes {:.4} mm bei {:?} — \
                 der Report muss die Unter-Seite ausdrücklich ausnehmen. Reasons: {:?}",
                r.under.0,
                r.under.1,
                r.under.2,
                report.reasons()
            );
        }
    }
}

/// Taschen-Fixture wie [`pocket_dilation`], um `f` skaliert — δ bleibt bei
/// 2 mm. Die Trace-Schrittweite ist 2 % der Überlappungs-AABB, mit `f`
/// wächst also der Rückstand der Trace-Enden vor der Mündungs-Ecke.
fn pocket_dilation_scaled(f: f32, s: f32, delta: f32) -> CsgNode {
    let a = leaf_box(1, [10.0 * f, 10.0 * f, 10.0 * f]);
    let b = CsgNode::Transform(
        Transform {
            translate: [0.0, 0.0, 5.0 * f + s],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [7.0 * f, 7.0 * f, 5.0 * f])),
    );
    CsgNode::Offset(Box::new(CsgNode::Difference(Box::new(a), Box::new(b))), delta)
}

/// **Kern-Regression (§8g, Verdachtsfläche 2).** Der Voronoi-Bisektor an der
/// Mündungs-Ecke muss unabhängig von der BAUTEILGRÖSSE sitzen. Vor §8g fiel
/// ab `f = 8` die Junction-Erkennung aus (`0.5·δ` gegen einen Rückstand von
/// 2.06 mm), der Bisektor wich der stumpferen δ-Endebene, und die Ecke riss
/// 0.436 mm Nullmengen-Fehler auf — bei zugesagten 0.01 mm.
#[test]
fn dilation_junction_bisector_is_scale_invariant_384() {
    let delta = 2.0f32;
    for f in [1.0f32, 4.0, 16.0] {
        let s = 3.0 * delta;
        let (field, report) = compile_for(pocket_dilation_scaled(f, s, delta));
        let a = (
            [-10.0 * f as f64, -10.0 * f as f64, -10.0 * f as f64],
            [10.0 * f as f64, 10.0 * f as f64, 10.0 * f as f64],
        );
        let b = (
            [-7.0 * f as f64, -7.0 * f as f64, s as f64],
            [7.0 * f as f64, 7.0 * f as f64, s as f64 + 10.0 * f as f64],
        );
        // Lokal um EINE Mündungs-Ecke — dort entscheidet der Bisektor.
        let c = [7.0 * f as f64, 7.0 * f as f64, 10.0 * f as f64];
        let r = 5.0 * delta as f64;
        let n = 30;
        let (mut miss, mut worst, mut at) = (0usize, 0.0f64, [0.0f64; 3]);
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let p = [
                        c[0] - r + 2.0 * r * (ix as f64 + 0.2113248654) / n as f64,
                        c[1] - r + 2.0 * r * (iy as f64 + 0.5773502692) / n as f64,
                        c[2] - r + 2.0 * r * (iz as f64 + 0.7886751346) / n as f64,
                    ];
                    let d = dist_to_box_minus_box(p, a, b);
                    let err = (d - delta as f64).abs();
                    if err <= 1e-3 {
                        continue;
                    }
                    let got = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                    if (d <= delta as f64) != (got < 0.0) {
                        miss += 1;
                        if err > worst {
                            worst = err;
                            at = p;
                        }
                    }
                }
            }
        }
        assert_eq!(
            miss, 0,
            "Skala f={f}: {miss} Nullmengen-Fehler an der Mündungs-Ecke, größter {worst:.4} mm \
             bei {at:?} — der Bisektor muss unabhängig von der Bauteilgröße sitzen"
        );
        assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
        assert!(
            !report.reasons().iter().any(|r| r.contains("NICHT auf die Bau-Toleranz")),
            "Skala f={f}: kein Fehler mehr, also auch kein Downgrade: {:?}",
            report.reasons()
        );
    }
}


// ─── #384 §8h: der Kaskaden-Effekt sequenzieller Terme ───────────────────
//
// Die Kanäle/Vertex-Kugeln eines Stopps werden SEQUENZIELL angewandt, und das
// Gate jedes Terms rechnet gegen den LAUFENDEN Wert: `gate = G ∓ d`. Hat ein
// Term `d` schon bewegt, öffnet das Gate des nächsten weiter als beim naiven
// `d` — die Ein-Term-Schranke `G = (c−1)·δ + 4·tol` ist damit KEINE globale
// Schranke. §8f/§8g hatten das notiert, aber an keiner Fixture reproduziert.
//
// Der Verstärker ist die march-sichere **Luftseiten-Klemme** `min(d, −w_geo)`:
// sie ist keine Addition (sie kann kein Vorzeichen kippen) und darf `d`
// beliebig weit herunterziehen — im Extremfall auf fast 0, direkt außerhalb
// des Trust-Radius (`−w_geo = de − K → 0⁺`). Danach steht das Gate des
// NÄCHSTEN Terms mit fast vollem `G` offen, obwohl das naive Feld 2 mm
// Abstand meldet. Gemessen an zwei Szenen (δ = 2, G = 0.868):
//
// | Szene | naiv d₀ | Klemme | Gate danach | Nullmengen-Fehler | vorher zugesagt |
// |---|---|---|---|---|---|
// | Stufen-Nabe (2 KREIS-Nähte) | +2.349 | 0.067 | +0.801 | **2.000 mm** | ≤ 0.01 mm |
// | Rippen-Kamm (4 Rippen) | +1.886 | 0.550 | +0.319 | **1.870 mm** | (schon conservative) |
//
// Die Stufen-Nabe war der stille Fall: BEIDE Nähte sind geschlossene Kreise,
// und der Randpunkt-Zeugen-Wächter übersprang geschlossene Spines ganz
// (`continue`) — er hat dort nie gesucht. Seit §8h sondiert er auch das
// Naht-INNERE und damit auch Kreise; die Zusage fällt auf `conservative`.

/// Die Schranke, die ein Report ZUSAGT: `None` = keine (conservative),
/// sonst das Maximum aller in der Prosa genannten „≤ X mm" (der schlimmste
/// Downgrade gewinnt), mindestens die Bau-Toleranz.
fn promised_bound(report: &CapabilityReport) -> Option<f64> {
    match report.status() {
        CapabilityStatus::Conservative | CapabilityStatus::Unsupported => return None,
        CapabilityStatus::Exact => return Some(1e-3),
        CapabilityStatus::ToleranceBounded => {}
    }
    let mut b = CHANNEL_TOL_RENDER as f64;
    for r in report.reasons() {
        for (i, _) in r.match_indices('≤') {
            let rest = &r[i + '≤'.len_utf8()..];
            let num: String = rest
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(x) = num.trim_end_matches('.').parse::<f64>() {
                b = b.max(x);
            }
        }
    }
    Some(b)
}

fn cyl_z(fid: u32, cz: f32, r: f32, hh: f32) -> CsgNode {
    // Primitive::Cylinder hat die Achse in Y — per R_x(90°) auf Z drehen.
    CsgNode::Transform(
        Transform {
            translate: [0.0, 0.0, cz],
            rotation: [
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            scale: [1.0; 3],
        },
        Box::new(CsgNode::Leaf {
            id: id(fid),
            primitive: Primitive::Cylinder { center: [0.0; 3], radius: r, half_height: hh },
        }),
    )
}

fn box_at(fid: u32, c: [f32; 3], h: [f32; 3]) -> CsgNode {
    CsgNode::Transform(
        Transform { translate: c, rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        Box::new(leaf_box(fid, h)),
    )
}

/// **Stufen-Nabe auf dünner Platte** (§8h-Sonde 1). Platte der Dicke `t`,
/// darauf zwei koaxiale Zylinder verschiedener Höhe/Einbettung. Beide
/// Basis-Nähte sind KREISE — geschlossene Spines ohne Enden.
fn stepped_hub_node(t: f32, r1: f32, r2: f32, zh1: f32, e1: f32, e2: f32, delta: f32) -> CsgNode {
    let plate = box_at(1, [0.0, 0.0, -0.5 * t], [20.0, 20.0, 0.5 * t]);
    let a = cyl_z(2, 0.5 * (zh1 - e1), r1, 0.5 * (zh1 + e1));
    let b = cyl_z(3, 0.5 * (6.0 - e2), r2, 0.5 * (6.0 + e2));
    CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(CsgNode::Union(Box::new(plate), Box::new(a))),
            Box::new(b),
        )),
        -delta,
    )
}

/// `dist(p, ∁Ω)` der Stufen-Nabe als Strahl-Minimum (je Richtung exakt:
/// Box-Slabs + Zylinder-Slabs). OBERE Schranke — kann einen Fehler nur
/// unterschätzen, nie erfinden.
fn dist_hub_complement(
    t: f64,
    cyls: &[(f64, f64, f64)], // (radius, z_lo, z_hi)
    p: [f64; 3],
    ndir: usize,
) -> f64 {
    let l = 20.0f64;
    let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let mut best = f64::INFINITY;
    for i in 0..ndir {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / ndir as f64;
        let rr = (1.0 - z * z).max(0.0).sqrt();
        let th = ga * i as f64;
        let d = [rr * th.cos(), rr * th.sin(), z];
        let mut iv: Vec<(f64, f64)> = Vec::new();
        {
            let (lo, hi) = ([-l, -l, -t], [l, l, 0.0]);
            let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
            let mut hit = true;
            for k in 0..3 {
                if d[k].abs() < 1e-12 {
                    if p[k] < lo[k] || p[k] > hi[k] {
                        hit = false;
                    }
                } else {
                    let a = (lo[k] - p[k]) / d[k];
                    let b = (hi[k] - p[k]) / d[k];
                    t0 = t0.max(a.min(b));
                    t1 = t1.min(a.max(b));
                }
            }
            if hit && t1 >= t0 && t1 > 0.0 {
                iv.push((t0.max(0.0), t1));
            }
        }
        for (r, zlo, zhi) in cyls.iter().copied() {
            let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
            let mut hit = true;
            if d[2].abs() < 1e-12 {
                if p[2] < zlo || p[2] > zhi {
                    hit = false;
                }
            } else {
                let a = (zlo - p[2]) / d[2];
                let b = (zhi - p[2]) / d[2];
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            }
            let qa = d[0] * d[0] + d[1] * d[1];
            let qb = 2.0 * (p[0] * d[0] + p[1] * d[1]);
            let qc = p[0] * p[0] + p[1] * p[1] - r * r;
            if qa < 1e-15 {
                if qc > 0.0 {
                    hit = false;
                }
            } else {
                let disc = qb * qb - 4.0 * qa * qc;
                if disc <= 0.0 {
                    hit = false;
                } else {
                    let sq = disc.sqrt();
                    t0 = t0.max((-qb - sq) / (2.0 * qa));
                    t1 = t1.min((-qb + sq) / (2.0 * qa));
                }
            }
            if hit && t1 >= t0 && t1 > 0.0 {
                iv.push((t0.max(0.0), t1));
            }
        }
        if iv.is_empty() {
            return 0.0;
        }
        iv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if iv[0].0 > 1e-12 {
            return 0.0;
        }
        let mut end = 0.0f64;
        for (a, b) in iv {
            if a <= end + 1e-9 {
                end = end.max(b);
            } else {
                break;
            }
        }
        best = best.min(end);
    }
    best
}

/// **Kern-Pin (§8h, Verdacht A).** Zwei koaxiale Naben auf einer 3-mm-Platte:
/// die äußere Kreisnaht klemmt (`de` knapp über `K`), die innere addiert
/// durch das aufgerissene Gate — das Feld behauptet Material 0.35 mm UNTER
/// der Platte, also in freier Luft. Zugesagt waren ≤ 0.01 mm.
///
/// Der Pin prüft die DOKTRIN, nicht eine Zahl: was der Stopp zusagt, muss
/// wahr sein. Er hält also auch dann noch, wenn ein späteres Paket die
/// Kaskade strukturell schließt (dann sinkt der Fehler, und die Zusage darf
/// wieder eng werden).
#[test]
fn erosion_stepped_hub_cascade_promise_is_true_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, report) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    assert!(
        crate::offset_channel::count_channels(&field).0 >= 2,
        "die Sonde taugt nur mit mindestens zwei Kanal-Termen — sonst gibt es \
         nichts zu kaskadieren"
    );
    let cyls = [
        (r1 as f64, -(e1 as f64), zh1 as f64),
        (r2 as f64, -(e2 as f64), 6.0f64),
    ];
    let n = 26usize;
    let (mut miss, mut worst, mut at) = (0usize, 0.0f64, [0.0f64; 3]);
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 18.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 18.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -(t as f64) - 1.5 + (t as f64 + 2.0) * (iz as f64 + 0.7886751346) / n as f64,
                ];
                if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                    continue;
                }
                let u = dist_hub_complement(t as f64, &cyls, p, 160);
                let err = delta as f64 - u;
                if err <= 1e-3 {
                    continue;
                }
                miss += 1;
                if err > worst {
                    worst = err;
                    at = p;
                }
            }
        }
    }
    match promised_bound(&report) {
        None => assert!(
            report.reasons().iter().any(|r| r.contains("keine Schranke mehr belegbar")),
            "wenn der Stopp nichts zusagt, muss er auch sagen WARUM: {:?}",
            report.reasons()
        ),
        Some(b) => assert!(
            worst <= b + 1e-3,
            "Stufen-Nabe: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?} — \
             zugesagt sind aber nur {b:.3} mm. Reasons: {:?}",
            report.reasons()
        ),
    }
}

/// **Zweiter Pin (§8h).** Derselbe Mechanismus an reiner Quader-Geometrie:
/// vier eingelassene Rippen im Abstand 0.8 mm. Hier klemmt die Nachbar-Naht
/// (`de = 1.45 < δ`, also `−w_geo = δ − de = 0.550`) und die ÜBERNÄCHSTE
/// addiert. Gemessen 1.870 mm Nullmengen-Fehler bei einer Ein-Term-Schranke
/// von 0.868 mm.
#[test]
fn erosion_rib_comb_cascade_promise_is_true_384() {
    let delta = 2.0f32;
    let (nribs, w, g, embed, ylen) = (4usize, 1.5f32, 0.8f32, 5.0f32, 22.0f32);
    let (h, plate, l) = (8.0f32, 12.0f32, 20.0f32);
    let pitch = w + g;
    let x0 = -0.5 * (nribs as f32 * pitch - g);
    let mut node = box_at(1, [0.0, 0.0, -0.5 * plate], [l, l, 0.5 * plate]);
    let mut boxes: Vec<([f64; 3], [f64; 3])> =
        vec![([-l as f64, -l as f64, -plate as f64], [l as f64, l as f64, 0.0])];
    for i in 0..nribs {
        let cx = x0 + 0.5 * w + i as f32 * pitch;
        node = CsgNode::Union(
            Box::new(node),
            Box::new(box_at(
                2 + i as u32,
                [cx, 0.0, 0.5 * (h - embed)],
                [0.5 * w, ylen, 0.5 * (h + embed)],
            )),
        );
        boxes.push((
            [(cx - 0.5 * w) as f64, -ylen as f64, -embed as f64],
            [(cx + 0.5 * w) as f64, ylen as f64, h as f64],
        ));
    }
    let (field, report) = compile_for(CsgNode::Offset(Box::new(node), -delta));
    assert!(
        crate::offset_channel::count_channels(&field).0 >= 2,
        "die Sonde braucht mindestens zwei Kanal-Terme"
    );
    let span = (0.5 * (nribs as f32 * pitch - g) + 3.0) as f64;
    let (miss, worst, at) = {
        let n = 28usize;
        let (mut m, mut wst, mut a) = (0usize, 0.0f64, [0.0f64; 3]);
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let p = [
                        -span + 2.0 * span * (ix as f64 + 0.2113248654) / n as f64,
                        -3.0 + 6.0 * (iy as f64 + 0.5773502692) / n as f64,
                        -6.0 + 6.5 * (iz as f64 + 0.7886751346) / n as f64,
                    ];
                    if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                        continue;
                    }
                    let u = dist_to_boxes_complement(&boxes, p, 160);
                    let err = delta as f64 - u;
                    if err <= 1e-3 {
                        continue;
                    }
                    m += 1;
                    if err > wst {
                        wst = err;
                        a = p;
                    }
                }
            }
        }
        (m, wst, a)
    };
    if let Some(b) = promised_bound(&report) {
        assert!(
            worst <= b + 1e-3,
            "Rippen-Kamm: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?} — \
             zugesagt sind aber nur {b:.3} mm. Reasons: {:?}",
            report.reasons()
        );
    }
}

// ─── #384 §8i: das Gate rechnet gegen den NAIVEN Stoppwert d₀ ────────────
//
// §8h hat den Kaskaden-Bruch vermessen und den Fix benannt: solange jedes
// Gate gegen den LAUFENDEN Wert `d` rechnet, kann ein Term das nächste Gate
// aufreißen — die Luftseiten-Klemme `min(d, −w_geo)` senkt `d` beliebig weit,
// ohne ein Vorzeichen zu kippen. Rechnet das Gate gegen den naiven Wert `d₀`,
// ist die Ein-Term-Schranke `G = (c−1)·δ + 4·tol` ein SATZ und damit zugleich
// die globale Schranke; als Nebenwirkung wird die Term-Reihenfolge
// feldwert-neutral. Im Tape trägt das der payload-lose `OP_STOP_BASE`.
//
// Gemessen (δ = 2, G = 0.868, exakte Strahl-Referenzen wie in §8h):
//
// | Szene | vorher (§8h) | nachher (§8i) |
// |---|---|---|
// | Stufen-Nabe | 957 Fehler, 2.000 mm, `conservative` | 661, 0.841 mm, `≤ 0.868` wahr |
// | Rippen-Kamm 4×1.5/0.8 | 2408, 1.987 mm | 1904, 0.819 mm |
// | Rippen-Kamm 4×2.0/1.0 | 1680, 1.052 mm | 1624, 0.819 mm |
// | Zapfen-Reihe (Spalt 1–3) | 0.787 mm | unverändert 0.787 mm |
// | Steinmetz-Zentrum (konvex) | 1.198·G Auslenkung | 1.000·G |

/// **§8i-Kern-Pin.** An der Stufen-Nabe kaskadierten zwei geschlossene
/// Kreisnähte bis 2.000 mm — dem Doppelten der Ein-Term-Schranke. Mit dem
/// `d₀`-Gate bleibt der Nullmengen-Fehler unter `G`; genau das ist die
/// Aussage des Pakets. `G` ist analytisch, nicht kalibriert: die Naht ist ein
/// rechter Winkel, also `c = 1/sin(45°) = √2` und `G = (√2−1)·2 + 4·0.01`.
#[test]
fn erosion_stepped_hub_stays_within_one_term_gate_384() {
    let delta = 2.0f32;
    let g_one_term = (2.0f64.sqrt() - 1.0) * delta as f64 + 4.0 * CHANNEL_TOL_RENDER as f64;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, report) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    assert!(
        crate::offset_channel::count_channels(&field).0 >= 2,
        "die Sonde taugt nur mit mindestens zwei Kanal-Termen"
    );
    let cyls = [
        (r1 as f64, -(e1 as f64), zh1 as f64),
        (r2 as f64, -(e2 as f64), 6.0f64),
    ];
    let n = 26usize;
    let (mut miss, mut worst, mut at) = (0usize, 0.0f64, [0.0f64; 3]);
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 18.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 18.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -(t as f64) - 1.5 + (t as f64 + 2.0) * (iz as f64 + 0.7886751346) / n as f64,
                ];
                if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                    continue;
                }
                let u = dist_hub_complement(t as f64, &cyls, p, 160);
                let err = delta as f64 - u;
                if err <= 1e-3 {
                    continue;
                }
                miss += 1;
                if err > worst {
                    worst = err;
                    at = p;
                }
            }
        }
    }
    assert!(
        worst <= g_one_term + 1e-3,
        "Stufen-Nabe: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?} — \
         die Ein-Term-Schranke {g_one_term:.4} mm MUSS seit §8i global gelten \
         (vor §8i: 2.000 mm). Reasons: {:?}",
        report.reasons()
    );
    // Der Zeuge liegt damit wieder unter der strukturellen Schranke, also
    // fällt das §8h-Downgrade auf `conservative`: der Stopp sagt wieder
    // etwas zu — und was er zusagt, ist wahr (das prüft der Doktrin-Pin
    // `erosion_stepped_hub_cascade_promise_is_true_384`).
    assert_ne!(
        report.status(),
        CapabilityStatus::Conservative,
        "Stufen-Nabe: ohne Kaskade ist `G` belegbar, der Stopp darf das sagen: {:?}",
        report.reasons()
    );
}

/// **§8i-Wert-Pin am konvexen Zweig** (der Golden-Zeuge). Im Zentrum des
/// dilatierten Steinmetz-Körpers greifen ZWEI konvexe Kanal-Terme; vor §8i
/// hob der erste `d` um das volle `G` an und öffnete dem zweiten ein
/// weiteres Stück Gate — zusammen 0.7918 mm bei einer Ein-Term-Schranke von
/// 0.6612 mm (1.198·G). Mit dem `d₀`-Gate bleibt die Gesamt-Auslenkung bei
/// genau `G`.
#[test]
fn dilation_steinmetz_center_stays_within_one_term_gate_384() {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let s1 = [1.0f32; 3];
    let cyl = |fid: u32, rot: [f32; 4], r: f32| {
        CsgNode::Transform(
            Transform { translate: [0.0; 3], rotation: rot, scale: s1 },
            Box::new(CsgNode::Leaf {
                id: id(fid),
                primitive: Primitive::Cylinder { center: [0.0; 3], radius: r, half_height: 18.0 },
            }),
        )
    };
    let stein = CsgNode::Offset(
        Box::new(CsgNode::Intersection(
            Box::new(cyl(1, [half, 0.0, 0.0, half], 9.0)),
            Box::new(cyl(2, [0.0, 0.0, -half, half], 6.0)),
        )),
        1.5,
    );
    let field =
        crate::offset_channel::synthesize_offset_channels(BoundedCsg::from_csg(&stein), 0.01);
    // Der naive Stoppwert im Zentrum und die größte Gate-Weite des Stopps.
    fn probe(n: &BoundedCsg, p: [f32; 3], out: &mut Vec<(f32, f32, usize)>) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, .. } => {
                let g = channels.iter().fold(0.0f32, |a, c| a.max(c.gate));
                out.push((inner.sdf(p, EvalMode::Visual), g, channels.len()));
                probe(inner, p, out);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                probe(left, p, out);
                probe(right, p, out);
            }
            BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => {
                probe(inner, p, out)
            }
            _ => {}
        }
    }
    let p = [0.0f32; 3];
    let mut stops = Vec::new();
    probe(&field, p, &mut stops);
    assert_eq!(stops.len(), 1, "Steinmetz: genau ein Offset-Stopp erwartet");
    let (d0, gate, n_ch) = stops[0];
    assert!(n_ch >= 2, "Steinmetz: die Kaskade braucht mindestens zwei Terme");
    let f = field.sdf(p, EvalMode::Visual);
    assert!(
        (f - d0).abs() <= gate + 1e-4,
        "Steinmetz-Zentrum: naiv {d0:.6}, korrigiert {f:.6} — Auslenkung {:.6} mm über \
         der Ein-Term-Schranke {gate:.6} mm (vor §8i: 0.791823 mm bei G = 0.661242)",
        (f - d0).abs()
    );
}

/// **§8i-Struktur-Pin: Reihenfolge-Unabhängigkeit.** Ohne Gate sind alle
/// Terme eines Stopps `min` (konkav) bzw. `max` (konvex) und damit
/// kommutativ; einzig das Gate gegen den laufenden Wert machte das Ergebnis
/// von der Term-Reihenfolge abhängig. Mit dem `d₀`-Gate ist jeder Term
/// `min(d, C(x))` bzw. `max(d, C(x))` mit einem von `d` unabhängigen `C` —
/// jede Permutation liefert BIT-identisch dasselbe Feld.
#[test]
fn stop_terms_are_order_independent_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let mut stops: Vec<(
        Vec<crate::offset_channel::OffsetChannel>,
        Vec<crate::offset_channel::VertexSphere>,
    )> = Vec::new();
    fn collect(
        n: &BoundedCsg,
        out: &mut Vec<(
            Vec<crate::offset_channel::OffsetChannel>,
            Vec<crate::offset_channel::VertexSphere>,
        )>,
    ) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, verts, .. } => {
                out.push((channels.clone(), verts.clone()));
                collect(inner, out);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                collect(left, out);
                collect(right, out);
            }
            BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => {
                collect(inner, out)
            }
            _ => {}
        }
    }
    for node in [
        stepped_hub_node(t, r1, r2, zh1, e1, e2, delta),
        thin_corner_node(4.0 * delta, 40.0, delta),
    ] {
        let (field, _) = compile_for(node);
        collect(&field, &mut stops);
    }
    let mut checked = 0usize;
    let mut active = 0usize;
    for (channels, verts) in &stops {
        if channels.len() + verts.len() < 2 {
            continue;
        }
        // Permutationen: umgekehrt und um eins rotiert.
        let mut rev_ch = channels.clone();
        rev_ch.reverse();
        let mut rot_ch = channels.clone();
        rot_ch.rotate_left(1);
        let mut rev_vs = verts.clone();
        rev_vs.reverse();
        let n = 12usize;
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let p = [
                        -10.0 + 20.0 * (ix as f32 + 0.2113248654) / n as f32,
                        -10.0 + 20.0 * (iy as f32 + 0.5773502692) / n as f32,
                        -10.0 + 20.0 * (iz as f32 + 0.7886751346) / n as f32,
                    ];
                    for d0 in [-6.0f32, -2.0, -0.5, 0.0, 0.5, 2.0, 6.0] {
                        let base =
                            crate::offset_channel::stop_apply(d0, p, channels, verts, None);
                        if base != d0 {
                            active += 1;
                        }
                        for (c, v) in
                            [(&rev_ch, verts), (&rot_ch, verts), (channels, &rev_vs)]
                        {
                            let got = crate::offset_channel::stop_apply(d0, p, c, v, None);
                            assert_eq!(
                                got.to_bits(),
                                base.to_bits(),
                                "Term-Reihenfolge ist NICHT feldwert-neutral bei p={p:?}, \
                                 d₀={d0}: {base} vs {got}"
                            );
                        }
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 0, "kein Stopp mit mindestens zwei Termen gefunden");
    assert!(
        active > 0,
        "die Sonde traf keinen einzigen Punkt, an dem ein Term überhaupt wirkt — \
         dann beweist die Permutations-Gleichheit nichts"
    );
}



// ─── #384 §8j: das vierte Gate — Fremdgeometrie QUER zur Naht ────────────
//
// §8i machte die Ein-Term-Schranke `G` global; übrig blieb an der Stufen-Nabe
// ein Restfehler von 0.841 mm, weil die 1.8 mm dünne Platte NÄHER liegt als
// der Trust-Radius `K = 3.908` und der Term durch sie hindurchgreift. Das
// Paket setzt zwei strukturell bewiesene Schärfungen dagegen — beide sind
// `min`/`max` über GÜLTIGE Schranken und können deshalb nur Über-Claims
// tilgen, nie welche erzeugen:
//
// * **Viertes Gate** (`face_planes`, `OP_ARC_CHANNEL` +16 Floats): je Kanal
//   bis zu vier LOKAL STÜTZENDE Fremdflächen-Halbräume `⟨x−F,n̂⟩ − δ` im
//   selben `min` wie das dritte Gate. Stützend heißt `Ω ∩ B(Naht, 2·K) ⊆
//   {h ≥ 0}`; dann liegt der Lotfußpunkt jedes bindenden Punkts auf oder
//   jenseits von `∂Ω`, `h` ist also eine gültige OBERE Schranke von
//   `dist(x,∁Ω)`, und `min(de, h)` ist die schärfere.
// * **Grenzwert-Klemme** (`OP_STOP_LIMIT`): jeder Naht-/Eckpunkt liegt auf
//   `∂Ω`, also ist `δ − de` eine gültige UNTERE Schranke des Zielfelds. Die
//   Terme liefern nur obere Schranken (`min`) — genau deshalb konnte in §8i
//   die WEITER entfernte Naht den `min` gewinnen. Die Klemme dreht das um.
//
// Gemessen (δ = 2, exakte Strahl-Referenzen wie §8h/§8i):
//
// | Szene | §8i | §8j |
// |---|---|---|
// | Stufen-Nabe | 661 Fehler, 0.841 mm, ≤ 0.868 | **0**, 0.000, ≤ 0.010 |
// | Ecken-Schale t/δ = 2.0 | 741, 0.806 | 114, 0.764 |
// | Ecken-Schale t/δ = 2.5 | 1594, 0.808 | 157, 0.808 |
// | Rippen-Kamm 4×1.5/0.8 | 1904, 0.819 | 644, 0.819 |
// | Rippen-Kamm 4×2.0/1.0 | 1624, 0.819 | 616, 0.819 |
//
// VERWORFEN (gemessen, nicht vermutet): die Trust-Klemme gegen die nächste
// fremde Naht (`K ← max(D−δ, δ)`, der §8g-Spiegel für Kanäle). Sie schließt
// zwar mehr Über-Claims (Nabe 0, Schale 2.5 → 0, Kamm 2.0/1.0 → 0), tauscht
// sie aber gegen GRÖSSERE Unter-Claims: Nabe 0.198 → 1.199 mm, Schale 2.5
// 1.248 → 1.436 mm, Kamm 2.0/1.0 0 → 0.093 mm — und der Stopp sagt darüber
// die Bau-Toleranz zu. §8h hatte das vorhergesagt; hier sind die Zahlen.

/// **§8j-Kern-Pin.** Die Stufen-Nabe ist an der exakten Referenz fehlerfrei
/// UND sagt die Bau-Toleranz zu. Vor §8j: 661 Fehlklassifikationen bis
/// 0.841 mm bei einer ehrlichen, aber weiten Zusage von ≤ 0.868 mm.
#[test]
fn erosion_stepped_hub_cross_gate_is_exact_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, report) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    assert!(
        crate::offset_channel::count_channels(&field).0 >= 2,
        "die Sonde taugt nur mit mindestens zwei Kanal-Termen"
    );
    let cyls = [
        (r1 as f64, -(e1 as f64), zh1 as f64),
        (r2 as f64, -(e2 as f64), 6.0f64),
    ];
    let n = 26usize;
    let (mut miss, mut worst, mut at) = (0usize, 0.0f64, [0.0f64; 3]);
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 18.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 18.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -(t as f64) - 1.5 + (t as f64 + 2.0) * (iz as f64 + 0.7886751346) / n as f64,
                ];
                if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                    continue;
                }
                let u = dist_hub_complement(t as f64, &cyls, p, 160);
                let err = delta as f64 - u;
                if err <= 1e-3 {
                    continue;
                }
                miss += 1;
                if err > worst {
                    worst = err;
                    at = p;
                }
            }
        }
    }
    assert_eq!(
        miss, 0,
        "Stufen-Nabe: {miss} Nullmengen-Fehler, größter {worst:.4} mm bei {at:?} — \
         das vierte Gate (Platten-Unterseite als stützende Fremdebene) plus die \
         Grenzwert-Klemme müssen den §8i-Rest von 0.841 mm schließen. Reasons: {:?}",
        report.reasons()
    );
    assert_eq!(report.status(), CapabilityStatus::ToleranceBounded, "{:?}", report.reasons());
    assert!(
        !report.reasons().iter().any(|r| r.contains("NICHT auf die Bau-Toleranz")),
        "Stufen-Nabe: kein Über-Claim mehr, also darf die Zusage eng sein: {:?}",
        report.reasons()
    );
}

/// **§8j-Struktur-Pin: jede Fremdebene stützt wirklich.** Die Gültigkeit des
/// vierten Gates hängt an EINER Bedingung — `Ω` darf die Ebene in der
/// Naht-Umgebung nicht durchschneiden. Ist sie verletzt, unterschätzt `h` die
/// wahre Distanz und der Term würde legitime Korrektur wegschneiden
/// (Unter-Claim). Geprüft wird sie hier direkt am Feld: kein Punkt IM Körper
/// darf innerhalb `2·K` der Naht auf der falschen Seite liegen.
#[test]
fn face_plane_gates_are_supporting_halfspaces_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let mut stops: Vec<(Vec<crate::offset_channel::OffsetChannel>, f32)> = Vec::new();
    fn collect(
        n: &BoundedCsg,
        out: &mut Vec<(Vec<crate::offset_channel::OffsetChannel>, f32)>,
        p: [f32; 3],
    ) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, .. } => {
                out.push((channels.clone(), inner.sdf(p, EvalMode::Visual)));
                collect(inner, out, p);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                collect(left, out, p);
                collect(right, out, p);
            }
            BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => {
                collect(inner, out, p)
            }
            _ => {}
        }
    }
    // Die Kanäle einmal einsammeln (der Feldwert im Tupel ist hier egal).
    collect(&field, &mut stops, [0.0; 3]);
    let channels: Vec<crate::offset_channel::OffsetChannel> =
        stops.into_iter().flat_map(|(c, _)| c).collect();
    let mut checked = 0usize;
    for ch in &channels {
        for g in &ch.face_planes {
            if g[3] <= -crate::offset_channel::END_PLANE_OFF * 0.5 {
                continue; // Sentinel
            }
            checked += 1;
            // Naht-AABB + 2·K — genau das Fenster, in dem die Synthese prüft.
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for arc in &ch.arcs {
                for k in 0..=8 {
                    let q = crate::offset_channel::arc_point_at(arc, k as f32 / 8.0);
                    for i in 0..3 {
                        lo[i] = lo[i].min(q[i]);
                        hi[i] = hi[i].max(q[i]);
                    }
                }
            }
            let r = 2.0 * ch.trust;
            let n = 22usize;
            for ix in 0..n {
                for iy in 0..n {
                    for iz in 0..n {
                        let f = [ix as f32, iy as f32, iz as f32];
                        let p = [
                            lo[0] - r + (hi[0] - lo[0] + 2.0 * r) * f[0] / (n - 1) as f32,
                            lo[1] - r + (hi[1] - lo[1] + 2.0 * r) * f[1] / (n - 1) as f32,
                            lo[2] - r + (hi[2] - lo[2] + 2.0 * r) * f[2] / (n - 1) as f32,
                        ];
                        // Punkt im Körper? (naiver Stopp-Wert = min der Member
                        // plus δ; `< δ` heißt „im Material").
                        if field.sdf(p, EvalMode::Visual) >= delta {
                            continue;
                        }
                        // h(x) = ⟨x,n̂⟩ − (c − δ)
                        let h = p[0] * g[0] + p[1] * g[1] + p[2] * g[2] - (g[3] - ch.delta);
                        assert!(
                            h >= -1e-2,
                            "Fremdebene {g:?} schneidet den Körper bei {p:?} (h = {h:.4}) — \
                             dann ist sie keine stützende Ebene und ihr Gate wäre ein \
                             UNTER-Claim"
                        );
                    }
                }
            }
        }
    }
    assert!(checked > 0, "die Sonde fand keine einzige belegte Fremdebene");
}

/// **§8j-Doktrin-Pin: die Klemme tilgt nur, sie erfindet nicht.** Für einen
/// konkaven Stopp ist das naive Feld konservativ (`d₀ ≥ f_wahr`), und jeder
/// Grenzwert-Beitrag `δ − de` liegt wegen `de ≥ dist(x,∁Ω)` unter `f_wahr`.
/// Der korrigierte Wert darf den naiven deshalb NIE übersteigen — sonst
/// behauptet die Klemme mehr Luft, als das konservative Ausgangsfeld kennt.
/// (Der naive Wert MUSS dabei der echte Teilbaum-Wert sein: mit einem frei
/// erfundenen `d₀` ist die Aussage gegenstandslos, weil `lim` aus der
/// Geometrie kommt und `d₀` dann nicht dazu passt.)
#[test]
fn stop_limit_never_exceeds_the_naive_field_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    fn walk(n: &BoundedCsg, p: [f32; 3], worst: &mut f32, at: &mut [f32; 3]) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, verts, lb, .. } => {
                if channels.first().map(|c| c.concave).unwrap_or(false) {
                    let d0 = inner.sdf(p, EvalMode::Visual);
                    let d = crate::offset_channel::stop_apply(d0, p, channels, verts, lb.as_ref());
                    if d - d0 > *worst {
                        *worst = d - d0;
                        *at = p;
                    }
                }
                walk(inner, p, worst, at);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                walk(left, p, worst, at);
                walk(right, p, worst, at);
            }
            BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => {
                walk(inner, p, worst, at)
            }
            _ => {}
        }
    }
    let mut worst = 0.0f32;
    let mut at = [0.0f32; 3];
    for node in [
        stepped_hub_node(t, r1, r2, zh1, e1, e2, delta),
        thin_corner_node(3.0 * delta, 40.0, delta),
    ] {
        let (field, _) = compile_for(node);
        let n = 24usize;
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let p = [
                        -12.0 + 24.0 * (ix as f32 + 0.2113248654) / n as f32,
                        -12.0 + 24.0 * (iy as f32 + 0.5773502692) / n as f32,
                        -12.0 + 24.0 * (iz as f32 + 0.7886751346) / n as f32,
                    ];
                    walk(&field, p, &mut worst, &mut at);
                }
            }
        }
    }
    assert!(
        worst <= 1e-3,
        "die Grenzwert-Klemme hebt den konkaven Stopp um {worst:.6} mm ÜBER den naiven \
         Wert bei {at:?} — sie darf nur Über-Claims zurücknehmen"
    );
}
// ─── #384 §8k: die UNTER-Seite — exakte Referenzen und die zweiseitige Sonde ──
//
// Alle Erosions-Sonden oben messen GENAU EINE Richtung: „das Feld behauptet
// Material, die Referenz widerlegt es" (Über-Claim / Phantom-Material). Die
// Gegenrichtung — „das Feld behauptet Luft, obwohl dort Material ist"
// (Unter-Claim / fehlendes Material) — kann ein Strahl-Minimum GAR NICHT
// beweisen: es ist eine OBERE Schranke von `dist(x,∁Ω)`, und ein Unter-Claim
// braucht eine UNTERE. Dafür sind die folgenden Referenzen EXAKT statt
// bloß beschränkend: sie zerlegen `∂Ω` analytisch in Flächenstücke.
//
// Beide Referenzen sind so gebaut, dass jede Degeneriertheit ein Randstück
// ZU VIEL behält (statt eines zu wenig) — die Distanz wird also höchstens
// unterschätzt, ein Unter-Claim damit nie erfunden.

/// Eine orientierte Box (Mittelpunkt, Halbmaße, orthonormale Achsen).
#[derive(Clone, Copy, Debug)]
struct Obb {
    c: [f64; 3],
    he: [f64; 3],
    u: [[f64; 3]; 3],
}

fn d3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn s3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn obb_aabb(lo: [f64; 3], hi: [f64; 3]) -> Obb {
    Obb {
        c: [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]), 0.5 * (lo[2] + hi[2])],
        he: [0.5 * (hi[0] - lo[0]), 0.5 * (hi[1] - lo[1]), 0.5 * (hi[2] - lo[2])],
        u: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    }
}

fn obb_rot_z(c: [f64; 3], he: [f64; 3], rot: f64) -> Obb {
    let (sn, cs) = rot.sin_cos();
    Obb { c, he, u: [[cs, sn, 0.0], [-sn, cs, 0.0], [0.0, 0.0, 1.0]] }
}

/// Sutherland–Hodgman gegen die Halbebene `A·α + B·β ≤ C` (`keep_le`) bzw.
/// `≥ C`. Konvexes Polygon rein, konvexes Polygon raus.
fn clip_poly(poly: &[[f64; 2]], a: f64, b: f64, c: f64, keep_le: bool) -> Vec<[f64; 2]> {
    let n = poly.len();
    let mut out: Vec<[f64; 2]> = Vec::new();
    if n == 0 {
        return out;
    }
    let val = |p: &[f64; 2]| {
        let v = a * p[0] + b * p[1] - c;
        if keep_le {
            -v
        } else {
            v
        }
    };
    for i in 0..n {
        let cur = poly[i];
        let nxt = poly[(i + 1) % n];
        let (vc, vn) = (val(&cur), val(&nxt));
        if vc >= 0.0 {
            out.push(cur);
        }
        if (vc >= 0.0) != (vn >= 0.0) {
            let t = vc / (vc - vn);
            out.push([cur[0] + t * (nxt[0] - cur[0]), cur[1] + t * (nxt[1] - cur[1])]);
        }
    }
    out
}

/// `poly \ int(Q)` als disjunkte konvexe Stücke; `q` sind die Halbebenen von
/// `Q` in der Form `(A,B,C)` mit „innen = `A·α + B·β ≤ C`".
fn sub_halfplanes(poly: &[[f64; 2]], q: &[(f64, f64, f64)]) -> Vec<Vec<[f64; 2]>> {
    let mut pieces: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut running: Vec<[f64; 2]> = poly.to_vec();
    for &(a, b, c) in q {
        if running.is_empty() {
            break;
        }
        let outside = clip_poly(&running, a, b, c, false);
        if outside.len() >= 3 {
            pieces.push(outside);
        }
        running = clip_poly(&running, a, b, c, true);
    }
    pieces
}

fn sub_all(poly: Vec<[f64; 2]>, qs: &[Vec<(f64, f64, f64)>]) -> Vec<Vec<[f64; 2]>> {
    let mut cur = vec![poly];
    for q in qs {
        let mut next: Vec<Vec<[f64; 2]>> = Vec::new();
        for p in &cur {
            next.extend(sub_halfplanes(p, q));
        }
        cur = next;
        if cur.is_empty() {
            break;
        }
    }
    cur
}

fn dist_point_poly(q: [f64; 2], poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    if n == 0 {
        return f64::INFINITY;
    }
    let mut best = f64::INFINITY;
    let mut all_pos = true;
    let mut all_neg = true;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let ll = ex * ex + ey * ey;
        let t = if ll > 0.0 { (((q[0] - a[0]) * ex + (q[1] - a[1]) * ey) / ll).clamp(0.0, 1.0) } else { 0.0 };
        let (px, py) = (a[0] + t * ex - q[0], a[1] + t * ey - q[1]);
        best = best.min((px * px + py * py).sqrt());
        let cr = ex * (q[1] - a[1]) - ey * (q[0] - a[0]);
        if cr < 0.0 {
            all_pos = false;
        }
        if cr > 0.0 {
            all_neg = false;
        }
    }
    if n >= 3 && (all_pos || all_neg) {
        return 0.0;
    }
    best
}

/// **EXAKTE** `dist(p, ∁(∪ Boxen))` für beliebig viele orientierte Boxen.
/// `∂Ω` wird analytisch zerlegt: jede Box-Fläche minus den INNEREN der
/// übrigen Boxen (konvexe Polygon-Subtraktion), Distanz je Stück in
/// geschlossener Form. Anders als [`dist_to_boxes_complement`] ist das keine
/// obere Schranke, sondern der Wert selbst — nur so lässt sich ein
/// UNTER-Claim (`dist ≥ δ`, wo das Feld Luft behauptet) überhaupt beweisen.
fn dist_obb_union_complement_exact(boxes: &[Obb], p: [f64; 3]) -> f64 {
    let inside =
        boxes.iter().any(|o| (0..3).all(|k| d3(s3(p, o.c), o.u[k]).abs() <= o.he[k] + 1e-12));
    if !inside {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for (i, o) in boxes.iter().enumerate() {
        for a in 0..3 {
            let (i1, i2) = ((a + 1) % 3, (a + 2) % 3);
            for s in [-1.0f64, 1.0] {
                let pp = [
                    o.c[0] + s * o.he[a] * o.u[a][0],
                    o.c[1] + s * o.he[a] * o.u[a][1],
                    o.c[2] + s * o.he[a] * o.u[a][2],
                ];
                let h = d3(s3(p, pp), o.u[a]).abs();
                if h >= best {
                    continue;
                }
                let (e1, e2) = (o.u[i1], o.u[i2]);
                let rect = vec![
                    [-o.he[i1], -o.he[i2]],
                    [o.he[i1], -o.he[i2]],
                    [o.he[i1], o.he[i2]],
                    [-o.he[i1], o.he[i2]],
                ];
                let mut qs: Vec<Vec<(f64, f64, f64)>> = Vec::new();
                for (j, ob) in boxes.iter().enumerate() {
                    if j == i {
                        continue;
                    }
                    let mut hp: Vec<(f64, f64, f64)> = Vec::new();
                    for m in 0..3 {
                        for sg in [1.0f64, -1.0] {
                            let nv = [sg * ob.u[m][0], sg * ob.u[m][1], sg * ob.u[m][2]];
                            hp.push((d3(e1, nv), d3(e2, nv), ob.he[m] - d3(s3(pp, ob.c), nv)));
                        }
                    }
                    qs.push(hp);
                }
                let q2 = [d3(s3(p, pp), e1), d3(s3(p, pp), e2)];
                for piece in sub_all(rect, &qs) {
                    let d2 = dist_point_poly(q2, &piece);
                    let d = (h * h + d2 * d2).sqrt();
                    if d < best {
                        best = d;
                    }
                }
            }
        }
    }
    best
}

fn subtract_intervals(lo: f64, hi: f64, cuts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut ivs = vec![(lo, hi)];
    for &(a, b) in cuts {
        let mut next: Vec<(f64, f64)> = Vec::new();
        for (x, y) in ivs {
            if b <= x || a >= y {
                next.push((x, y));
                continue;
            }
            if a > x {
                next.push((x, a));
            }
            if b < y {
                next.push((b, y));
            }
        }
        ivs = next;
    }
    ivs
}

/// **EXAKTE** `dist(p, ∁Ω)` der Stufen-Nabe: Platte `[−20,20]² × [−t,0]`
/// vereinigt mit koaxialen z-Zylindern `(r, z_lo, z_hi)`, alle Radien < 20.
/// `∂Ω` analytisch zerlegt: Plattenflächen minus konzentrische Scheiben,
/// Zylinder-Mäntel minus verdeckte z-Intervalle, Deckel als Kreisringe.
fn dist_hub_complement_exact(t: f64, cyls: &[(f64, f64, f64)], p: [f64; 3]) -> f64 {
    let l = 20.0f64;
    let rho = (p[0] * p[0] + p[1] * p[1]).sqrt();
    let in_plate = p[0].abs() <= l && p[1].abs() <= l && p[2] >= -t && p[2] <= 0.0;
    let in_cyl = cyls.iter().any(|&(r, zlo, zhi)| rho <= r && p[2] >= zlo && p[2] <= zhi);
    if !in_plate && !in_cyl {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    // 1) Platten-Deckflächen: Rechteck minus konzentrische Scheiben.
    for zc in [0.0f64, -t] {
        let mut rmax = 0.0f64;
        for &(r, zlo, zhi) in cyls {
            if zlo < zc && zc < zhi {
                rmax = rmax.max(r);
            }
        }
        let dz = (p[2] - zc).abs();
        let d2 = if rho >= rmax {
            let dx = (-l - p[0]).max(p[0] - l).max(0.0);
            let dy = (-l - p[1]).max(p[1] - l).max(0.0);
            (dx * dx + dy * dy).sqrt()
        } else {
            rmax - rho
        };
        best = best.min((dz * dz + d2 * d2).sqrt());
    }
    // 2) Platten-Seitenflächen (von Zylindern mit r < 20 nie berührt).
    for (ax, sgn) in [(0usize, -1.0f64), (0, 1.0), (1, -1.0), (1, 1.0)] {
        let other = 1 - ax;
        let hh = (p[ax] - sgn * l).abs();
        let dy = (-l - p[other]).max(p[other] - l).max(0.0);
        let dz = (-t - p[2]).max(p[2]).max(0.0);
        best = best.min((hh * hh + dy * dy + dz * dz).sqrt());
    }
    // 3) Zylinder-Mäntel minus Platten-/Nachbar-Inneres.
    for (i, &(r, zlo, zhi)) in cyls.iter().enumerate() {
        let mut cuts: Vec<(f64, f64)> = Vec::new();
        if r < l {
            cuts.push((-t, 0.0));
        }
        for (j, &(rj, zlj, zhj)) in cyls.iter().enumerate() {
            if j != i && r < rj {
                cuts.push((zlj, zhj));
            }
        }
        let dr = (rho - r).abs();
        for (a, b) in subtract_intervals(zlo, zhi, &cuts) {
            let dz = (a - p[2]).max(p[2] - b).max(0.0);
            best = best.min((dr * dr + dz * dz).sqrt());
        }
    }
    // 4) Zylinder-Deckel als Kreisringe.
    for (i, &(r, zlo, zhi)) in cyls.iter().enumerate() {
        for zc in [zlo, zhi] {
            if r < l && -t < zc && zc < 0.0 {
                continue; // ganz im Platten-Inneren
            }
            let mut rin = 0.0f64;
            for (j, &(rj, zlj, zhj)) in cyls.iter().enumerate() {
                if j != i && zlj < zc && zc < zhj {
                    rin = rin.max(rj);
                }
            }
            if rin >= r {
                continue;
            }
            let dz = (p[2] - zc).abs();
            let dr = (rin - rho).max(rho - r).max(0.0);
            best = best.min((dz * dz + dr * dr).sqrt());
        }
    }
    best
}

/// Zweiseitige Nullmengen-Messung über ein versetztes Gitter.
///
/// * **über** (Phantom-Material): Feld meldet `< 0`, die exakte Referenz
///   zeigt `dist < δ` ⇒ Fehler `δ − dist`.
/// * **unter** (fehlendes Material): Feld meldet `≥ 0`, die exakte Referenz
///   zeigt `dist > δ` ⇒ Fehler `dist − δ`.
///
/// `xmax` ist die größte beobachtete Abweichung `exakt − Strahlschranke`; sie
/// MUSS `≤ 0` bleiben (die Strahl-Referenz ist eine obere Schranke) und ist
/// damit der Selbsttest der exakten Referenz.
struct TwoSided {
    over: (usize, f64, [f64; 3]),
    under: (usize, f64, [f64; 3]),
    xmax: f64,
    pts: usize,
}

fn two_sided_scan(
    field: &BoundedCsg,
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
    exact: &dyn Fn([f64; 3]) -> f64,
    ray: &dyn Fn([f64; 3]) -> f64,
) -> TwoSided {
    let mut r = TwoSided {
        over: (0, 0.0, [0.0; 3]),
        under: (0, 0.0, [0.0; 3]),
        xmax: f64::NEG_INFINITY,
        pts: 0,
    };
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let f = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * f[0],
                    lo[1] + (hi[1] - lo[1]) * f[1],
                    lo[2] + (hi[2] - lo[2]) * f[2],
                ];
                r.pts += 1;
                let got = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                let e = exact(p);
                let u = ray(p);
                r.xmax = r.xmax.max(e - u);
                if got < 0.0 {
                    let err = delta as f64 - u;
                    if err > 1e-3 {
                        r.over.0 += 1;
                        if err > r.over.1 {
                            r.over.1 = err;
                            r.over.2 = p;
                        }
                    }
                } else {
                    let err = e - delta as f64;
                    if err > 1e-3 {
                        r.under.0 += 1;
                        if err > r.under.1 {
                            r.under.1 = err;
                            r.under.2 = p;
                        }
                    }
                }
            }
        }
    }
    r
}

fn rib_comb_node(nribs: usize, w: f32, g: f32, delta: f32) -> (CsgNode, Vec<([f64; 3], [f64; 3])>, f64) {
    let (embed, ylen) = (5.0f32, 22.0f32);
    let (h, plate, l) = (8.0f32, 12.0f32, 20.0f32);
    let pitch = w + g;
    let x0 = -0.5 * (nribs as f32 * pitch - g);
    let mut node = box_at(1, [0.0, 0.0, -0.5 * plate], [l, l, 0.5 * plate]);
    let mut boxes: Vec<([f64; 3], [f64; 3])> =
        vec![([-l as f64, -l as f64, -plate as f64], [l as f64, l as f64, 0.0])];
    for i in 0..nribs {
        let cx = x0 + 0.5 * w + i as f32 * pitch;
        node = CsgNode::Union(
            Box::new(node),
            Box::new(box_at(
                2 + i as u32,
                [cx, 0.0, 0.5 * (h - embed)],
                [0.5 * w, ylen, 0.5 * (h + embed)],
            )),
        );
        boxes.push((
            [(cx - 0.5 * w) as f64, -ylen as f64, -embed as f64],
            [(cx + 0.5 * w) as f64, ylen as f64, h as f64],
        ));
    }
    let span = (0.5 * (nribs as f32 * pitch - g) + 3.0) as f64;
    (CsgNode::Offset(Box::new(node), -delta), boxes, span)
}


/// **§8k-Selbsttest der exakten Referenz.** Das Strahl-Minimum ist eine OBERE
/// Schranke von `dist(x,∁Ω)`, die analytische Zerlegung liefert den Wert
/// selbst — also muss `exakt ≤ Strahl` an JEDEM Punkt gelten. Ein Verstoß
/// hieße, die Zerlegung erfindet Distanz, und dann wäre jeder Unter-Claim
/// unten frei erfunden. Zusätzlich geprüft: die beiden kommen sich auch
/// wirklich nahe (eine Referenz, die überall 0 liefert, bestünde den
/// Ungleichungs-Test ebenfalls).
#[test]
fn exact_complement_reference_is_below_the_ray_bound_384() {
    let boxes = thin_corner_boxes(6.0, 40.0);
    let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
    let (mut worst_gap, mut inside, mut max_exact) = (0.0f64, 0usize, 0.0f64);
    let n = 14usize;
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 10.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 10.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -9.0 + 10.0 * (iz as f64 + 0.7886751346) / n as f64,
                ];
                let e = dist_obb_union_complement_exact(&obbs, p);
                let u = dist_to_boxes_complement(&boxes, p, 400);
                assert!(
                    e <= u + 1e-6,
                    "die exakte Referenz {e:.6} übersteigt bei {p:?} die Strahl-Schranke \
                     {u:.6} — dann erfindet sie Distanz"
                );
                if e > 1e-6 {
                    inside += 1;
                    worst_gap = worst_gap.max(u - e);
                    max_exact = max_exact.max(e);
                }
            }
        }
    }
    assert!(inside > 100, "zu wenige Innenpunkte gesondet: {inside}");
    assert!(max_exact > 3.0, "die Referenz liefert nur Kleinkram: max {max_exact:.3}");
    // Der Strahl konvergiert von OBEN gegen die Wahrheit — am Punkt der
    // größten Restdifferenz muss ein dichteres Richtungsbündel sie also
    // schließen. Täte es das nicht, läge die exakte Zerlegung zu tief.
    let (mut gap_at, mut gap) = ([0.0f64; 3], 0.0f64);
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 10.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 10.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -9.0 + 10.0 * (iz as f64 + 0.7886751346) / n as f64,
                ];
                let e = dist_obb_union_complement_exact(&obbs, p);
                if e <= 1e-6 {
                    continue;
                }
                let d = dist_to_boxes_complement(&boxes, p, 400) - e;
                if d > gap {
                    gap = d;
                    gap_at = p;
                }
            }
        }
    }
    let e = dist_obb_union_complement_exact(&obbs, gap_at);
    let (u1, u2) = (
        dist_to_boxes_complement(&boxes, gap_at, 2_000),
        dist_to_boxes_complement(&boxes, gap_at, 40_000),
    );
    assert!(
        u2 <= u1 && u1 <= e + gap + 1e-9 && u2 - e < 0.4 * gap,
        "am Punkt der größten Differenz {gap_at:?} konvergiert der Strahl nicht gegen die \
         exakte Referenz {e:.4}: 400 → {:.4}, 2000 → {u1:.4}, 40000 → {u2:.4}",
        e + gap
    );
    // Und die Handrechnung am §8k-Referenzpunkt: p liegt in der x-Platte und
    // der z-Platte, die nächste Luft ist die konkave Kante (x=0, z=−6).
    let p = [-1.9956442825384606f64, -8.777942204153845, -4.081278794384615];
    let want = ((1.9956442825384606f64).powi(2) + (6.0 - 4.081278794384615f64).powi(2)).sqrt();
    assert!(
        (dist_obb_union_complement_exact(&obbs, p) - want).abs() < 1e-9,
        "exakte Referenz {:.6} statt der Handrechnung {want:.6}",
        dist_obb_union_complement_exact(&obbs, p)
    );
}

/// **§8k-Kern-Pin: die Ecken-Schale ist BEIDSEITIG exakt.**
///
/// Bis §8j maß jede Erosions-Sonde nur Phantom-Material. Die Gegenprobe fand
/// an dieser Fixture vorbestehend FEHLENDES Material — bei zugesagten 0.01 mm
/// bis zu 0.768 mm (t/δ = 3.0), und die Zusage sagte darüber nichts. Ursache
/// war weder die End-Ebene noch die `G`-Kappe, sondern eine Synthese-Lücke:
/// von den 24 konkaven Nahtstrahlen des Körpers entstanden nur 16, weil
/// verworfene Traces die Seed-Sperrliste `claimed` bereits gefüllt hatten.
/// Seit `claimed` erst NACH `build_channel` gefüllt wird, sind es 24 — und
/// die Fixture ist auf BEIDEN Seiten fehlerfrei.
#[test]
fn erosion_thin_corner_is_exact_on_both_sides_384() {
    let delta = 2.0f32;
    // `t/δ = 2` fehlt hier nur historisch: bis §8k behielt die KOLLABIERENDE
    // Schale einen Unter-Claim von 1.319 mm. Seit §8l ist auch sie beidseitig
    // exakt; geprüft wird sie von
    // `erosion_collapsing_shell_is_exact_on_both_sides_384` (dort zusammen mit
    // `t/δ = 2.25`, das den Composite-Rand-Wächter des Vertex-Snaps hält).
    for tf in [2.5f32, 3.0, 3.5, 4.0, 4.4, 5.0, 6.0] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field,
            delta,
            [lo; 3],
            [1.0; 3],
            22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        assert_eq!(
            r.under.0, 0,
            "Schale t/δ={tf}: {} Punkte mit FEHLENDEM Material, größtes {:.4} mm bei {:?} — \
             die Naht-Synthese muss alle 24 Strahlen liefern. Reasons: {:?}",
            r.under.0,
            r.under.1,
            r.under.2,
            report.reasons()
        );
        assert_eq!(
            r.over.0, 0,
            "Schale t/δ={tf}: {} Punkte Phantom-Material, größtes {:.4} mm bei {:?}",
            r.over.0, r.over.1, r.over.2
        );
    }
}

/// **§8k-Doktrin-Pin, beidseitig.** Was ein Stopp zusagt, muss auf der Seite
/// halten, für die er es zusagt — und über die ANDERE Seite darf er nicht
/// schweigen. Konkret: die genannte Schranke muss den Über-Claim decken, und
/// wo die exakte Referenz fehlendes Material nachweist, muss die Prosa die
/// Unter-Seite ausdrücklich ausnehmen. (Ein Unter-Claim-ZEUGE ist mit den
/// heutigen Mitteln unmöglich — er bräuchte eine UNTERE Distanzschranke, jede
/// verfügbare Quelle liefert eine obere. Deshalb kann der Stopp die Richtung
/// nur ausnehmen, nicht beziffern.)
#[test]
fn erosion_promise_names_the_side_it_covers_384() {
    let delta = 2.0f32;
    let mut cases: Vec<(String, TwoSided, CapabilityReport)> = Vec::new();
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, report) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        let r = two_sided_scan(
            &field,
            delta,
            [-9.0, -9.0, -(t as f64) - 1.5],
            [9.0, 9.0, 0.5],
            22,
            &|p| dist_hub_complement_exact(t as f64, &cyls, p),
            &|p| dist_hub_complement(t as f64, &cyls, p, 160),
        );
        cases.push(("Stufen-Nabe".into(), r, report));
    }
    {
        // Die KOLLABIERENDE Schale: Erosion frisst sie fast auf, die
        // Ecken-Kegel überlappen einander vollständig.
        let t = 2.0f32 * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field,
            delta,
            [lo; 3],
            [1.0; 3],
            22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        cases.push(("Ecken-Schale t/δ=2".into(), r, report));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, report) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let r = two_sided_scan(
            &field,
            delta,
            [-span, -3.0, -6.0],
            [span, 3.0, 0.5],
            24,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        cases.push((format!("Rippen-Kamm 4×{w}/{g}"), r, report));
    }
    for alpha in [90.0f64, 60.0] {
        let (field, report) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let r = two_sided_scan(
            &field,
            delta,
            [-10.0, -10.0, 42.0],
            [10.0, 10.0, 54.0],
            22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_notch_complement(alpha, p),
        );
        cases.push((format!("notch α={alpha}"), r, report));
    }
    for (name, r, report) in cases {
        assert!(
            r.xmax <= 1e-6,
            "{name}: die exakte Referenz übersteigt die Strahl-Schranke um {:.6} — \
             die Messung selbst ist kaputt",
            r.xmax
        );
        if let Some(b) = promised_bound(&report) {
            assert!(
                r.over.1 <= b + 1e-3,
                "{name}: {} Punkte Phantom-Material, größtes {:.4} mm bei {:?} — zugesagt sind \
                 aber nur {b:.3} mm. Reasons: {:?}",
                r.over.0,
                r.over.1,
                r.over.2,
                report.reasons()
            );
        }
        if r.under.1 > 1e-2 {
            assert!(
                report
                    .reasons()
                    .iter()
                    .any(|s| s.contains("FEHLENDES Material") || s.contains("keine Schranke")),
                "{name}: {} Punkte mit FEHLENDEM Material, größtes {:.4} mm bei {:?} — der Report \
                 muss die Unter-Seite ausdrücklich ausnehmen, sonst liest sich die Zusage \
                 zweiseitig. Reasons: {:?}",
                r.under.0,
                r.under.1,
                r.under.2,
                report.reasons()
            );
        }
    }
}

/// **§8l-Kern-Pin: die KOLLABIERENDE Ecken-Schale ist beidseitig exakt.**
///
/// Bei `t/δ = 2` erodiert jede Platte für sich zu nichts — das naive
/// `∪ Erosion(P_k)` ist im ganzen Eckwürfel `≥ 0`, das gesamte Material der
/// wahren Erosion muss also aus Kanälen und Vertex-Kugeln kommen. §8k mass
/// dort 2119 Punkte mit bis zu 1.319 mm FEHLENDEM Material bei zugesagten
/// 0.01 mm und benannte zwei Verdächtige. Die Term-Aufschlüsselung am
/// Worst-Punkt `[−1.832, −2.063, −1.986]` widerlegte beide: die Vertex-Kugel
/// am Würfeleck `(0,−4,0)` hatte den exakten Wert (`ρ − δ = 1.3247`, alle drei
/// Kegel-Gates positiv, `gate = 1.507` weit genug) und wurde ALLEIN von der
/// §8g-Nachbar-Kegel-Klemme abgeschaltet (`K = max(D−δ, δ) = δ`, weil die
/// Nachbarecken nur `D = 2δ` entfernt stehen).
///
/// Zwei der drei §8l-Eingriffe schließen das: die Klemme zählt nur noch
/// VERWEIGERTE Trieder (Ecken mit Term tragen seit §8j ihre eigene
/// Grenzwert-Klemme), und die freie End-Ebene verliert ihren δ-Versatz dort,
/// wo eine Vertex-Kugel oder eine stützende Fremdebene die Distanz-Rolle
/// nachweislich trägt.
#[test]
fn erosion_collapsing_shell_is_exact_on_both_sides_384() {
    let delta = 2.0f32;
    for tf in [2.0f32, 2.25] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field,
            delta,
            [lo; 3],
            [1.0; 3],
            24,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 200),
        );
        assert!(r.pts > 1000 && r.xmax <= 1e-6, "die Sonde selbst ist kaputt: {:?}", r.xmax);
        assert_eq!(
            r.under.0, 0,
            "kollabierende Schale t/δ={tf}: {} Punkte mit FEHLENDEM Material, größtes {:.4} mm \
             bei {:?} — die Vertex-Kugel am Würfeleck MUSS dort greifen (vor §8l: 2119 / 1.319). \
             Reasons: {:?}",
            r.under.0,
            r.under.1,
            r.under.2,
            report.reasons()
        );
        assert_eq!(
            r.over.0, 0,
            "kollabierende Schale t/δ={tf}: {} Punkte Phantom-Material, größtes {:.4} mm bei \
             {:?} — die gelockerte Klemme darf KEINEN Über-Claim eintauschen",
            r.over.0, r.over.1, r.over.2
        );
    }
}

/// **§8l-Pin: das Nahtende am notch ist beidseitig exakt.**
///
/// Die δ-verschobene freie End-Ebene sitzt am TRACE-Ende, und das bleibt
/// typisch einen Schritt vor der echten Fläche stehen. Am notch waren das
/// 0.9 mm: `s₃ = −0.238` schaltete den Term ab, während die stützende
/// Deckfläche `s₄ = +0.661` — den exakten Wert `dist − δ` — längst
/// bereitstellte. Seit §8l fällt der δ-Versatz genau dort, wo eine solche
/// Ebene belegt ist; die Reichweiten-Rolle bleibt als reine Ebene erhalten.
#[test]
fn erosion_notch_seam_end_is_exact_on_both_sides_384() {
    let delta = 2.0f32;
    for alpha in [90.0f64, 60.0] {
        let (field, report) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let r = two_sided_scan(
            &field,
            delta,
            [-10.0, -10.0, 42.0],
            [10.0, 10.0, 54.0],
            24,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_notch_complement(alpha, p),
        );
        assert!(r.xmax <= 1e-6, "notch α={alpha}: die Referenz driftet um {:.6}", r.xmax);
        assert_eq!(
            r.under.0, 0,
            "notch α={alpha}: {} Punkte mit FEHLENDEM Material, größtes {:.4} mm bei {:?} — \
             die Endfläche trägt die Distanz-Rolle, der Trace-Rückstand darf sie nicht \
             verkürzen (vor §8l: 9 bzw. 14 Punkte bis 0.559). Reasons: {:?}",
            r.under.0,
            r.under.1,
            r.under.2,
            report.reasons()
        );
        assert_eq!(
            r.over.0, 0,
            "notch α={alpha}: {} Punkte Phantom-Material, größtes {:.4} mm bei {:?}",
            r.over.0, r.over.1, r.over.2
        );
    }
}

/// **§8l-Struktur-Pin: die Endfläche einer Naht IST ein Fremdflächen-Kandidat.**
///
/// Der Trägerflächen-Filter in `set_face_plane_gates` verlangte, die Naht
/// müsse STRIKT auf der Materialseite der Kandidaten-Ebene liegen
/// (`h_seam > 0`). Damit fiel jede Ebene durch, auf der die Naht ENDET — und
/// an der Ecken-Schale traf das genau die vier Kanäle, deren Trace um ein
/// Trieder-Eck herum zwei Deckflächen erreicht: sie bekamen GAR KEINE
/// Fremdebene. Das war der Grund, warum §8k den Wegfall des δ-Versatzes
/// verwerfen musste (0.868 mm Über-Claim an der Deckfläche z = 40).
///
/// Der richtige Test ist die Naht-ENTHALTUNG (`h ≡ 0` auf der ganzen Naht),
/// nicht die Berührung. Dieser Pin friert das Ergebnis ein: JEDER konkave
/// Kanal der Ecken-Schale trägt mindestens eine stützende Fremdebene
/// (vorher: 4 von 24 ohne).
#[test]
fn face_plane_gates_cover_seam_end_faces_384() {
    let delta = 2.0f32;
    fn walk(n: &BoundedCsg, out: &mut Vec<usize>) {
        match n {
            BoundedCsg::ArcChannel { inner, channels, .. } => {
                for ch in channels.iter().filter(|c| c.concave) {
                    out.push(
                        ch.face_planes
                            .iter()
                            .filter(|g| g[0].abs() + g[1].abs() + g[2].abs() > 0.5)
                            .count(),
                    );
                }
                walk(inner, out);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                walk(left, out);
                walk(right, out);
            }
            BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => walk(inner, out),
            _ => {}
        }
    }
    for tf in [2.0f32, 3.0] {
        let (field, _) = compile_for(thin_corner_node(tf * delta, 40.0, delta));
        let mut counts = Vec::new();
        walk(&field, &mut counts);
        assert_eq!(counts.len(), 24, "Schale t/δ={tf}: 24 konkave Nahtstrahlen erwartet");
        let blind = counts.iter().filter(|c| **c == 0).count();
        assert_eq!(
            blind, 0,
            "Schale t/δ={tf}: {blind} von {} Kanälen ohne stützende Fremdebene — die Endfläche \
             der Naht darf nicht am Trägerflächen-Filter scheitern (vor §8l: 4)",
            counts.len()
        );
    }
}



/// **§8m-Struktur-Pin: JEDES konkave Trieder bekommt seine Vertex-Kugel.**
///
/// Die Ecken-Schale hat acht konkave Trieder — die acht Ecken des Würfels
/// `[−t, 0]³`. §8l fand, dass nicht jede von ihnen einen Term bekam, und
/// vermutete den Trace: `trace_seam` biege am Eck auf die Nachbar-Naht ab und
/// liefere kein blockiertes Ende. **Die Sonde widerlegt das** — alle acht
/// Enden werden blockiert, alle acht Trieder-Kandidaten entstehen. Verloren
/// gingen sie erst in [`project_to_vertex`]: dessen Newton landet auf der
/// Fläche `x = −t`, wo der Dual-Gradient wegen der `safe_sqrt`-Regularisierung
/// **exakt** `(0,0,0)` wird — und der Entartungs-Wächter (`‖g‖ < 1e−6`) warf
/// den bereits gelösten Punkt weg.
///
/// Warum das an der PHASENLAGE hing: die Flächen bei Koordinate `0` erreicht
/// die gedämpfte Newton nur asymptotisch (nahe null ist das f32-Raster dicht,
/// die Konvergenzschwelle `1e−5·scale` greift lange vorher); die Flächen bei
/// `−t` liegen dagegen auf einem f32-Raster der Weite `≈ 5e−7`, auf das ein
/// einziger Schritt aus einem Residuum von `4e−5` bit-exakt einrastet. Ob er
/// das tut, hängt daran, wo der Trace blockiert wurde — also an der
/// Schrittweiten-Phase gegen die Wandstärke. Gemessen fielen genau `t/δ = 2.1`
/// und `2.4` durch (dort 4 von 8 Kugeln, bis 0.975 mm fehlendes Material),
/// `2.0`/`2.25`/`2.5` nicht.
///
/// Dieser Pin friert die STRUKTUR ein statt der Symptome: acht Kugeln, an den
/// acht Würfelecken, über den ganzen Dicken-Fächer. Fällt er, ist die
/// Trieder-Erkennung wieder phasenabhängig geworden.
#[test]
fn vertex_spheres_cover_every_trihedron_384() {
    let delta = 2.0f32;
    let mut fan: Vec<f32> = Vec::new();
    let mut v = 2.0f32;
    while v <= 2.6001 {
        fan.push((v * 100.0).round() / 100.0);
        v += 0.05;
    }
    fan.extend_from_slice(&[2.7, 2.8, 2.9, 3.0, 3.2, 3.5, 4.0, 4.4, 5.0, 6.0]);
    fn anchors(n: &BoundedCsg, out: &mut Vec<[f32; 3]>) {
        match n {
            BoundedCsg::ArcChannel { inner, verts, .. } => {
                out.extend(verts.iter().map(|v| v.v));
                anchors(inner, out);
            }
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                anchors(left, out);
                anchors(right, out);
            }
            BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => anchors(inner, out),
            _ => {}
        }
    }
    for tf in fan {
        let t = tf * delta;
        let (field, _) = compile_for(thin_corner_node(t, 40.0, delta));
        let mut got = Vec::new();
        anchors(&field, &mut got);
        assert_eq!(
            got.len(),
            8,
            "Schale t/δ={tf}: {} Vertex-Kugeln statt 8 — jede der acht Würfelecken ist ein \
             konkaves Trieder und braucht ihren Term (§8m: bei 2.1/2.4 waren es 4). Anker: {:?}",
            got.len(),
            got
        );
        // …und zwar je EINE an jeder Ecke, nicht acht an derselben.
        for cx in [0.0f32, -t] {
            for cy in [0.0f32, -t] {
                for cz in [0.0f32, -t] {
                    let c = [cx, cy, cz];
                    let hit = got
                        .iter()
                        .filter(|v| {
                            (v[0] - c[0]).abs().max((v[1] - c[1]).abs()).max((v[2] - c[2]).abs())
                                < 1e-2
                        })
                        .count();
                    assert_eq!(
                        hit, 1,
                        "Schale t/δ={tf}: {hit} Kugeln an der Würfelecke {c:?} statt genau einer. \
                         Anker: {got:?}"
                    );
                }
            }
        }
    }
}

/// **§8m-Kern-Pin: die Ecken-Schale ist über den ganzen Dicken-Fächer
/// beidseitig exakt.**
///
/// §8l pinnte `t/δ = 2.0`/`2.25` (kollabierendes Regime) und `2.5…6.0`
/// (`erosion_thin_corner_is_exact_on_both_sides_384`) — und ließ dazwischen
/// zwei gemessene Löcher stehen: `2.1` mit 624 Punkten bis 0.877 mm und `2.4`
/// mit 65 bis 0.933 mm fehlendem Material. Beide sind reine PHASENLAGEN
/// (Nachbarn wie `2.15`/`2.2`/`2.45` waren sauber), also taugt kein einzelner
/// Stützpunkt als Wächter: der Fächer selbst ist der Pin.
///
/// Geprüft werden die Zwischen-Phasen in 0.05er-Schritten; `2.0`/`2.25` und
/// `2.5+` bleiben bei ihren bestehenden Pins, damit hier keine Erwartung
/// doppelt geführt wird.
#[test]
fn erosion_shell_is_exact_across_the_thickness_fan_384() {
    let delta = 2.0f32;
    for tf in [2.05f32, 2.1, 2.15, 2.2, 2.3, 2.35, 2.4, 2.45] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field,
            delta,
            [lo; 3],
            [1.0; 3],
            22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        assert!(r.pts > 1000 && r.xmax <= 1e-6, "die Sonde selbst ist kaputt: {:?}", r.xmax);
        assert_eq!(
            r.under.0, 0,
            "Schale t/δ={tf}: {} Punkte mit FEHLENDEM Material, größtes {:.4} mm bei {:?} — \
             die Phasenlage von Trace-Schritt zu Wandstärke darf keine Vertex-Kugel kosten \
             (§8m, vor dem Fix: 2.1 → 624 / 0.877, 2.4 → 65 / 0.933). Reasons: {:?}",
            r.under.0,
            r.under.1,
            r.under.2,
            report.reasons()
        );
        assert_eq!(
            r.over.0, 0,
            "Schale t/δ={tf}: {} Punkte Phantom-Material, größtes {:.4} mm bei {:?} — die \
             wiedergewonnenen Kugeln dürfen KEINEN Über-Claim eintauschen",
            r.over.0, r.over.1, r.over.2
        );
    }
}

// ══════════════ #384 §8n — die UNTERE Distanzschranke ══════════════
//
// Alle Referenzen der §8-Reihe (Strahl-Minimum, `de`, jede stützende Ebene,
// die Grenzwert-Klemme) liefern OBERE Schranken von `dist(x,∁Ω)`. Für die
// Unter-Seite braucht es das Gegenteil. Der Bauplan steht in §8k: `∁Ω` ist
// der Durchschnitt der Member-Komplemente, und jedes Member-Komplement ist
// die VEREINIGUNG seiner Austritts-Halbräume. Für eine Teilmenge `S` der
// Member ist deshalb
//
//     dist(x, ∁(⋃_{k∈S} P_k)) = min über alle Auswahlen (i_k)_{k∈S}
//                                   dist(x, ⋂_k S_{k,i_k})
//
// ein Minimum über `∏_{k∈S} n_k` konvexe KEILE — und weil eine kleinere
// Vereinigung ein grösseres Komplement hat, ist der Wert für JEDE Teilmenge
// eine gültige UNTERE Schranke von `dist(x,∁Ω)`. Die triviale Teilmenge
// `S = {k}` liefert genau die Member-Tiefe, also `δ − d₀`; jede grössere
// Teilmenge ist schärfer.

/// Halbebene im (r,z)-Profil: `n_r·r + n_z·z >= d`.
type Hp2 = [f64; 3];
/// Halbraum: `n·q >= d`, `|n| = 1`.
type Hp3 = [f64; 4];

fn dist_halfplanes2(x: [f64; 2], hs: &[Hp2]) -> f64 {
    let feas = |q: [f64; 2]| hs.iter().all(|h| h[0] * q[0] + h[1] * q[1] - h[2] >= -1e-7);
    if feas(x) {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for h in hs {
        let s = h[2] - (h[0] * x[0] + h[1] * x[1]);
        if s <= 0.0 {
            continue;
        }
        let q = [x[0] + s * h[0], x[1] + s * h[1]];
        if feas(q) {
            best = best.min(s);
        }
    }
    for i in 0..hs.len() {
        for j in (i + 1)..hs.len() {
            let (a, b) = (hs[i], hs[j]);
            let det = a[0] * b[1] - a[1] * b[0];
            if det.abs() < 1e-12 {
                continue;
            }
            let q = [(a[2] * b[1] - a[1] * b[2]) / det, (a[0] * b[2] - a[2] * b[0]) / det];
            if feas(q) {
                best = best.min(((q[0] - x[0]).powi(2) + (q[1] - x[1]).powi(2)).sqrt());
            }
        }
    }
    best
}

/// Lot auf den Schnitt der aktiven Ebenen: `q = x + Nᵀλ`, `(N Nᵀ)λ = b`.
fn solve_active(x: [f64; 3], a: &[Hp3]) -> Option<[f64; 3]> {
    let m = a.len();
    let mut g = vec![vec![0.0f64; m + 1]; m];
    for i in 0..m {
        for j in 0..m {
            g[i][j] = a[i][0] * a[j][0] + a[i][1] * a[j][1] + a[i][2] * a[j][2];
        }
        g[i][m] = a[i][3] - (a[i][0] * x[0] + a[i][1] * x[1] + a[i][2] * x[2]);
    }
    for c in 0..m {
        let mut piv = c;
        for r in c + 1..m {
            if g[r][c].abs() > g[piv][c].abs() {
                piv = r;
            }
        }
        if g[piv][c].abs() < 1e-10 {
            return None;
        }
        g.swap(c, piv);
        for r in 0..m {
            if r == c {
                continue;
            }
            let f = g[r][c] / g[c][c];
            for k in c..=m {
                g[r][k] -= f * g[c][k];
            }
        }
    }
    let mut q = x;
    for i in 0..m {
        let l = g[i][m] / g[i][i];
        for k in 0..3 {
            q[k] += l * a[i][k];
        }
    }
    Some(q)
}

fn dist_halfspaces3(x: [f64; 3], hs: &[Hp3]) -> f64 {
    let feas = |q: [f64; 3]| {
        hs.iter().all(|h| h[0] * q[0] + h[1] * q[1] + h[2] * q[2] - h[3] >= -1e-7)
    };
    if feas(x) {
        return 0.0;
    }
    let m = hs.len();
    let mut best = f64::INFINITY;
    let dd = |q: [f64; 3]| {
        ((q[0] - x[0]).powi(2) + (q[1] - x[1]).powi(2) + (q[2] - x[2]).powi(2)).sqrt()
    };
    for i in 0..m {
        if let Some(q) = solve_active(x, &[hs[i]]) {
            if feas(q) {
                best = best.min(dd(q));
            }
        }
        for j in i + 1..m {
            if let Some(q) = solve_active(x, &[hs[i], hs[j]]) {
                if feas(q) {
                    best = best.min(dd(q));
                }
            }
            for k in j + 1..m {
                if let Some(q) = solve_active(x, &[hs[i], hs[j], hs[k]]) {
                    if feas(q) {
                        best = best.min(dd(q));
                    }
                }
            }
        }
    }
    best
}

fn obb_exits(o: &Obb) -> Vec<Hp3> {
    let mut v = Vec::new();
    for a in 0..3 {
        for s in [-1.0f64, 1.0] {
            let cu = d3(o.c, o.u[a]);
            v.push([s * o.u[a][0], s * o.u[a][1], s * o.u[a][2], s * cu + o.he[a]]);
        }
    }
    v
}

/// Die drei Member der Stufen-Nabe als AUSTRITTS-Halbebenen im (r,z)-Profil.
/// Die Platte ist keine Rotationsfigur; ihre vier Seitenflächen werden durch
/// `r >= 20` ersetzt — eine OBERMENGE der echten Austrittsmenge
/// (`|x| >= 20` ⇒ `r >= 20`). Eine grössere Komplement-Menge liefert eine
/// KLEINERE Distanz, die Schranke bleibt also gültig.
fn hub_members(t: f64, cyls: &[(f64, f64, f64)], half: f64) -> Vec<Vec<Hp2>> {
    let mut m = vec![vec![[0.0, 1.0, 0.0], [0.0, -1.0, t], [1.0, 0.0, half]]];
    for &(r, zlo, zhi) in cyls {
        m.push(vec![[1.0, 0.0, r], [0.0, 1.0, zhi], [0.0, -1.0, -zlo]]);
    }
    m
}

fn complement_lb2(x: [f64; 3], members: &[Vec<Hp2>], sub: &[usize]) -> f64 {
    let q = [(x[0] * x[0] + x[1] * x[1]).sqrt(), x[2]];
    let mut best = f64::INFINITY;
    let mut idx = vec![0usize; sub.len()];
    loop {
        let hs: Vec<Hp2> = sub.iter().enumerate().map(|(k, &m)| members[m][idx[k]]).collect();
        best = best.min(dist_halfplanes2(q, &hs));
        let mut k = 0;
        loop {
            if k == sub.len() {
                return best;
            }
            idx[k] += 1;
            if idx[k] < members[sub[k]].len() {
                break;
            }
            idx[k] = 0;
            k += 1;
        }
    }
}

fn complement_lb3(x: [f64; 3], members: &[Vec<Hp3>], sub: &[usize]) -> f64 {
    let mut best = f64::INFINITY;
    let mut idx = vec![0usize; sub.len()];
    loop {
        let hs: Vec<Hp3> = sub.iter().enumerate().map(|(k, &m)| members[m][idx[k]]).collect();
        best = best.min(dist_halfspaces3(x, &hs));
        let mut k = 0;
        loop {
            if k == sub.len() {
                return best;
            }
            idx[k] += 1;
            if idx[k] < members[sub[k]].len() {
                break;
            }
            idx[k] = 0;
            k += 1;
        }
    }
}

fn pair_max3(x: [f64; 3], members: &[Vec<Hp3>]) -> f64 {
    let mut best: f64 = 0.0;
    for i in 0..members.len() {
        for j in i + 1..members.len() {
            best = best.max(complement_lb3(x, members, &[i, j]));
        }
    }
    best
}

// ---- Feld-Mutation: gate = max(G − d₀, L(x) − δ) im KONKAVEN Zweig ----

fn channel_apply_mut(
    base: f32,
    d: f32,
    p: [f32; 3],
    ch: &crate::offset_channel::OffsetChannel,
    extra: f32,
) -> f32 {
    let Some((de, q, m1, m2)) = crate::offset_channel::channel_nearest(p, ch) else {
        return d;
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let dq = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let (s1, s2) = (dot(dq, m1), dot(dq, m2));
    let gate =
        if ch.concave { (ch.gate - base).max(extra) } else { ch.gate + base };
    let w_geo = s1
        .min(s2)
        .min(crate::offset_channel::end_plane_gate(p, ch))
        .min(crate::offset_channel::face_plane_gate(p, ch))
        .min(de - ch.delta)
        .min(ch.trust - de);
    let w = w_geo.min(gate);
    if ch.concave {
        if w > 0.0 {
            d.min(-w)
        } else if w_geo <= 0.0 {
            d.min(-w_geo)
        } else {
            d
        }
    } else {
        d.max(w)
    }
}

fn vertex_apply_mut(
    base: f32,
    d: f32,
    p: [f32; 3],
    vs: &crate::offset_channel::VertexSphere,
    extra: f32,
) -> f32 {
    let dq = [p[0] - vs.v[0], p[1] - vs.v[1], p[2] - vs.v[2]];
    let rho = (dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2]).sqrt();
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let gate =
        if vs.concave { (vs.gate - base).max(extra) } else { vs.gate + base };
    let w_geo = dot(dq, vs.gates[0])
        .min(dot(dq, vs.gates[1]))
        .min(dot(dq, vs.gates[2]))
        .min(rho - vs.delta)
        .min(vs.trust - rho);
    let w = w_geo.min(gate);
    if vs.concave {
        if w > 0.0 {
            d.min(-w)
        } else if w_geo <= 0.0 {
            d.min(-w_geo)
        } else {
            d
        }
    } else {
        d.max(w)
    }
}

fn eval_mut(n: &BoundedCsg, p: [f32; 3], lb: &dyn Fn([f32; 3]) -> f32) -> f32 {
    match n {
        BoundedCsg::ArcChannel { inner, channels, verts, rims, .. } => {
            let d0 = eval_mut(inner, p, lb);
            let delta = channels
                .first()
                .map(|c| c.delta)
                .or_else(|| verts.first().map(|v| v.delta))
                .unwrap_or(0.0);
            let extra = lb(p) - delta;
            let mut d = d0;
            let mut lim = -crate::offset_channel::END_PLANE_OFF;
            for ch in channels {
                lim = lim.max(crate::offset_channel::channel_limit(p, ch));
                d = channel_apply_mut(d0, d, p, ch, extra);
            }
            for vs in verts {
                lim = lim.max(crate::offset_channel::vertex_limit(p, vs));
                d = vertex_apply_mut(d0, d, p, vs, extra);
            }
            let concave =
                channels.first().map(|c| c.concave).or_else(|| verts.first().map(|v| v.concave));
            d = match concave {
                Some(true) => d.max(lim),
                Some(false) => d.min(-lim),
                None => d,
            };
            for block in rims {
                d = crate::offset_rim::apply_rims_f32(d, p, block.concave, &block.recs);
                d = crate::offset_rim::apply_rims_arc_f32(d, p, block.concave, &block.arc_recs);
            }
            d
        }
        BoundedCsg::Union { left, right, .. } => eval_mut(left, p, lb).min(eval_mut(right, p, lb)),
        BoundedCsg::Intersection { left, right, .. } => {
            eval_mut(left, p, lb).max(eval_mut(right, p, lb))
        }
        BoundedCsg::Difference { left, right, .. } => {
            eval_mut(left, p, lb).max(-eval_mut(right, p, lb))
        }
        BoundedCsg::Offset { inner, delta, .. } => eval_mut(inner, p, lb) - delta,
        _ => n.sdf(p, EvalMode::Visual),
    }
}

struct Row {
    name: String,
    over: (usize, f64),
    under: (usize, f64),
    worst_under: [f64; 3],
    lb_viol: f64,
}

#[allow(clippy::too_many_arguments)]
fn scan_mut(
    name: &str,
    field: &BoundedCsg,
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
    exact: &dyn Fn([f64; 3]) -> f64,
    ray: &dyn Fn([f64; 3]) -> f64,
    lb: &dyn Fn([f64; 3]) -> f64,
    mutate: bool,
) -> Row {
    let mut row = Row {
        name: name.into(),
        over: (0, 0.0),
        under: (0, 0.0),
        worst_under: [0.0; 3],
        lb_viol: 0.0,
    };
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let f = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * f[0],
                    lo[1] + (hi[1] - lo[1]) * f[1],
                    lo[2] + (hi[2] - lo[2]) * f[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let l = lb(p);
                row.lb_viol = row.lb_viol.max(l - exact(p));
                let got = if mutate {
                    eval_mut(field, pf, &|q| {
                        lb([q[0] as f64, q[1] as f64, q[2] as f64]) as f32
                    })
                } else {
                    field.sdf(pf, EvalMode::Visual)
                };
                let (e, u) = (exact(p), ray(p));
                if got < 0.0 {
                    let err = delta as f64 - u;
                    if err > 1e-3 {
                        row.over.0 += 1;
                        row.over.1 = row.over.1.max(err);
                    }
                } else {
                    let err = e - delta as f64;
                    if err > 1e-3 {
                        row.under.0 += 1;
                        if err > row.under.1 {
                            row.under.1 = err;
                            row.worst_under = p;
                        }
                    }
                }
            }
        }
    }
    row
}

/// **§8n-Selbsttest der unteren Schranke.** Sie ist nur dann etwas wert,
/// wenn sie NIE über der Wahrheit liegt — eine erfundene untere Schranke
/// würde das Gate öffnen, wo es schliessen muss, und damit Phantom-Material
/// erzeugen. Geprüft wird gegen dieselben exakten Referenzen wie in §8k
/// (`dist_hub_complement_exact`, `dist_obb_union_complement_exact`), an
/// Nabe, Ecken-Schale, notch und Rippen-Kamm.
///
/// Zwei Richtungen, beide nötig:
/// * **Solide** — `LB ≤ exakt` an jedem Gitterpunkt. Ohne diese Ungleichung
///   ist jede Zusage, die auf `LB` baut, gelogen.
/// * **Scharf** — `LB` darf nicht trivial 0 sein (eine Funktion, die überall
///   0 liefert, bestünde die Ungleichung ebenfalls). Geprüft an der Nabe:
///   der VOLLE Member-Schnitt trifft die exakte Distanz am Worst-Punkt
///   AUF DEN MIKROMETER.
#[test]
fn complement_lower_bound_is_sound_384() {
    let (t, r1, r2, zh1, e1, e2) = (3.0f64, 6.0f64, 4.0f64, 0.5f64, 1.2f64, 1.8f64);
    let cyls = [(r1, -e1, zh1), (r2, -e2, 6.0f64)];
    let hub = hub_members(t, &cyls, 20.0);
    let (mut worst_viol, mut best_sharp) = (0.0f64, 0.0f64);
    let n = 13usize;
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 18.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 18.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -t - 1.5 + (t + 2.0) * (iz as f64 + 0.7886751346) / n as f64,
                ];
                let e = dist_hub_complement_exact(t, &cyls, p);
                for sub in [&[0usize, 1][..], &[0, 2], &[1, 2], &[0, 1, 2]] {
                    let l = complement_lb2(p, &hub, sub);
                    worst_viol = worst_viol.max(l - e);
                    best_sharp = best_sharp.max(l);
                }
            }
        }
    }
    // Box-Fixturen: Ecken-Schale, notch, Rippen-Kamm.
    let mut box_sets: Vec<Vec<Obb>> = Vec::new();
    for tf in [2.0f64, 3.0] {
        box_sets.push(
            thin_corner_boxes(tf * 2.0, 40.0).iter().map(|&(l, h)| obb_aabb(l, h)).collect(),
        );
    }
    {
        let gamma = std::f64::consts::PI - 90.0f64.to_radians();
        box_sets.push(vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ]);
    }
    let (_, comb_boxes, _) = rib_comb_node(4, 1.5, 0.8, 2.0);
    box_sets.push(comb_boxes.iter().map(|&(l, h)| obb_aabb(l, h)).collect());
    for obbs in &box_sets {
        let mem: Vec<Vec<Hp3>> = obbs.iter().map(obb_exits).collect();
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for o in obbs {
            for k in 0..3 {
                let ext = (0..3).map(|a| (o.u[a][k] * o.he[a]).abs()).sum::<f64>();
                lo[k] = lo[k].min(o.c[k] - ext);
                hi[k] = hi[k].max(o.c[k] + ext);
            }
        }
        let m = 9usize;
        for ix in 0..m {
            for iy in 0..m {
                for iz in 0..m {
                    let f = [
                        (ix as f64 + 0.2113248654) / m as f64,
                        (iy as f64 + 0.5773502692) / m as f64,
                        (iz as f64 + 0.7886751346) / m as f64,
                    ];
                    let p = [
                        lo[0] + (hi[0] - lo[0]) * f[0],
                        lo[1] + (hi[1] - lo[1]) * f[1],
                        lo[2] + (hi[2] - lo[2]) * f[2],
                    ];
                    let e = dist_obb_union_complement_exact(obbs, p);
                    let l = pair_max3(p, &mem);
                    worst_viol = worst_viol.max(l - e);
                    best_sharp = best_sharp.max(l);
                }
            }
        }
    }
    assert!(
        worst_viol <= 1e-9,
        "die untere Schranke übersteigt die exakte Distanz um {worst_viol:.3e} — sie erfindet \
         Distanz, und jedes Gate, das auf ihr steht, erzeugt Phantom-Material"
    );
    assert!(best_sharp > 3.0, "die Schranke ist trivial (max {best_sharp:.4})");

    // Handrechnung am §8l-Worst-Punkt der Nabe.
    let x = [-1.463461473763636, -1.163986143381818, -0.6843920148636364];
    let exact = dist_hub_complement_exact(t, &cyls, x);
    let full = complement_lb2(x, &hub, &[0, 1, 2]);
    // Nächste Luft ist die Platten-UNTERSEITE `z = −3`, senkrecht unter `x`.
    assert!((exact - (t + x[2])).abs() < 1e-9, "exakte Referenz: {exact:.6}");
    assert!(
        (full - exact).abs() < 1e-6,
        "der VOLLE Member-Schnitt muss die exakte Distanz treffen: {full:.6} vs {exact:.6}"
    );
}

/// **§8n-Kern-Befund: das Nabe-Gate braucht den VOLLEN Member-Schnitt, das
/// Paar reicht nicht.**
///
/// §8k und §8l haben den Weg aus der `G`-Kappe zweimal benannt und beide Male
/// als PAAR beziffert („`∁(P_a ∪ P_b)` ist die Vereinigung von höchstens 6×6
/// konvexen Keilen"), §8l sogar mit einer Zahl („das Paar (Platte, Zylinder A)
/// liefert genau die 2.306 mm der Plattenunterseite"). Beides ist falsch, und
/// dieser Pin hält fest, warum:
///
/// | Schranke | Wert am Worst-Punkt | Keil, der gewinnt |
/// |---|---|---|
/// | Paar (Platte, A) | 1.184392 | `z ≥ 0.5` — A's Deckel ist für dieses Paar Luft |
/// | Paar (Platte, B) | 2.237332 | `z ≥ 0 ∧ r ≥ 4` — die Plattenoberseite ausserhalb B |
/// | Paar (A, B) | 1.115608 | `z ≤ −1.8` — B's Boden |
/// | **VOLL (Platte, A, B)** | **2.315608** | `z ≤ −3` — die Plattenunterseite |
/// | exakt (§8k-Referenz) | 2.315608 | dieselbe Fläche |
///
/// Der Grund ist strukturell: der Keil, der das beste Paar (Platte, B)
/// niederhält, ist die Plattenoberseite ausserhalb von B — und die ist von
/// Zylinder A verdeckt. Ein Paar kann eine Verdeckung durch einen DRITTEN
/// Member nicht sehen. Der Bedarf am Worst-Punkt ist `d₀ − f_wahr = 1.200`
/// gegen `G = 0.868`; das Paar deckt davon 1.121, der volle Schnitt 1.200.
#[test]
fn hub_gate_needs_the_full_member_complement_384() {
    let (t, r1, r2, zh1, e1, e2) = (3.0f64, 6.0f64, 4.0f64, 0.5f64, 1.2f64, 1.8f64);
    let cyls = [(r1, -e1, zh1), (r2, -e2, 6.0f64)];
    let mem = hub_members(t, &cyls, 20.0);
    let x = [-1.463461473763636, -1.163986143381818, -0.6843920148636364];
    let exact = dist_hub_complement_exact(t, &cyls, x);
    let lb = |sub: &[usize]| complement_lb2(x, &mem, sub);
    let pair_max = [&[0usize, 1][..], &[0, 2], &[1, 2]].iter().map(|s| lb(s)).fold(0.0f64, f64::max);
    for (sub, want) in [
        (&[0usize, 1][..], 1.184392f64),
        (&[0, 2], 2.237332),
        (&[1, 2], 1.115608),
        (&[0, 1, 2], 2.315608),
    ] {
        let got = lb(sub);
        assert!(
            (got - want).abs() < 1e-5,
            "Member-Teilmenge {sub:?}: {got:.6} statt {want:.6} — die Keil-Zerlegung hat sich \
             bewegt, die §8n-Bilanz ist damit ungültig"
        );
    }
    assert!(
        pair_max < exact - 0.05,
        "das Paar-Maximum {pair_max:.6} müsste UNTER der exakten Distanz {exact:.6} bleiben — \
         genau das ist der Befund, der §8k/§8l korrigiert"
    );
    // Und der Bedarf: das Paar reicht über δ (das Vorzeichen kippt), aber
    // nicht bis an den exakten Wert (der Restfehler bleibt).
    assert!(pair_max > 2.0 && (lb(&[0, 1, 2]) - exact).abs() < 1e-6);
}

// ── einmalige §8n-Messung (Kosten + Deckenmessung), NICHT im Fast-Tier ──
#[test]
#[ignore]
fn probe_384_pair_distance_gate() {
    let delta = 2.0f32;
    let mut rows: Vec<Row> = Vec::new();
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, _rep) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        let mem = hub_members(t as f64, &cyls, 20.0);
        let exact = |p: [f64; 3]| dist_hub_complement_exact(t as f64, &cyls, p);
        let ray = |p: [f64; 3]| dist_hub_complement(t as f64, &cyls, p, 160);
        let pair = |p: [f64; 3]| {
            [[0usize, 1], [0, 2], [1, 2]]
                .iter()
                .map(|s| complement_lb2(p, &mem, s))
                .fold(0.0f64, f64::max)
        };
        let full = |p: [f64; 3]| complement_lb2(p, &mem, &[0, 1, 2]);
        let (lo, hi) = ([-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5]);
        let t0 = std::time::Instant::now();
        rows.push(scan_mut(
            "Nabe BASIS", &field, delta, lo, hi, 22, &exact, &ray, &|_| f64::NEG_INFINITY, false,
        ));
        rows.push(scan_mut("Nabe PAAR", &field, delta, lo, hi, 22, &exact, &ray, &pair, true));
        rows.push(scan_mut("Nabe VOLL", &field, delta, lo, hi, 22, &exact, &ray, &full, true));
        rows.push(scan_mut("Nabe NULLMUT", &field, delta, lo, hi, 22, &exact, &ray, &|_| -1e30, true));
        println!("Nabe-Block {:?}", t0.elapsed());
        // Kosten EINER L-Auswertung.
        let probe = [1.0, 1.0, -1.0];
        let t1 = std::time::Instant::now();
        let mut acc = 0.0;
        for _ in 0..20000 {
            acc += full(probe);
        }
        println!("  L_voll (3 Member, 27 Keile): {:?}/Auswertung (acc {acc:.1})", t1.elapsed() / 20000);
        let t2 = std::time::Instant::now();
        let mut acc = 0.0;
        for _ in 0..20000 {
            acc += pair(probe);
        }
        println!("  L_paar (3 Paare, 27 Keile):  {:?}/Auswertung (acc {acc:.1})", t2.elapsed() / 20000);
        // Welcher Keil gewinnt wo?
        let xw = [-1.463461473763636, -1.163986143381818, -0.6843920148636364];
        for sub in [&[0usize, 2][..], &[0, 1, 2]] {
            println!("  Teilmenge {sub:?}: L = {:.6}", complement_lb2(xw, &mem, sub));
        }
    }
    for tf in [2.0f32, 2.1, 2.25, 2.4, 2.5, 3.0, 4.0, 6.0] {
        let t = tf * delta;
        let (field, _rep) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let mem: Vec<Vec<Hp3>> = obbs.iter().map(obb_exits).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_boxes_complement(&boxes, p, 160);
        let pair = |p: [f64; 3]| pair_max3(p, &mem);
        let full = |p: [f64; 3]| complement_lb3(p, &mem, &[0, 1, 2]);
        if tf == 2.0 {
            let probe = [-1.0, -1.0, -1.0];
            let t1 = std::time::Instant::now();
            let mut acc = 0.0;
            for _ in 0..5000 {
                acc += full(probe);
            }
            println!(
                "  L_voll 3D (3 Boxen, 216 Keile): {:?}/Auswertung (acc {acc:.1})",
                t1.elapsed() / 5000
            );
            let t2 = std::time::Instant::now();
            let mut acc = 0.0;
            for _ in 0..5000 {
                acc += pair(probe);
            }
            println!(
                "  L_paar 3D (3 Paare, 108 Keile): {:?}/Auswertung (acc {acc:.1})",
                t2.elapsed() / 5000
            );
        }
        let lo = -(t as f64 + delta as f64 + 1.0);
        let (l3, h3) = ([lo; 3], [1.0; 3]);
        rows.push(scan_mut(
            &format!("Schale {tf} BASIS"), &field, delta, l3, h3, 22, &exact, &ray,
            &|_| f64::NEG_INFINITY, false,
        ));
        rows.push(scan_mut(&format!("Schale {tf} PAAR"), &field, delta, l3, h3, 22, &exact, &ray, &pair, true));
        rows.push(scan_mut(&format!("Schale {tf} VOLL"), &field, delta, l3, h3, 22, &exact, &ray, &full, true));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, _rep) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let mem: Vec<Vec<Hp3>> = obbs.iter().map(obb_exits).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_boxes_complement(&boxes, p, 160);
        let pair = |p: [f64; 3]| pair_max3(p, &mem);
        let (l3, h3) = ([-span, -3.0, -6.0], [span, 3.0, 0.5]);
        rows.push(scan_mut(
            &format!("Kamm {w}/{g} BASIS"), &field, delta, l3, h3, 24, &exact, &ray,
            &|_| f64::NEG_INFINITY, false,
        ));
        rows.push(scan_mut(&format!("Kamm {w}/{g} PAAR"), &field, delta, l3, h3, 24, &exact, &ray, &pair, true));
    }
    for alpha in [90.0f64, 60.0] {
        let (field, _rep) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let mem: Vec<Vec<Hp3>> = obbs.iter().map(obb_exits).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_notch_complement(alpha, p);
        let pair = |p: [f64; 3]| pair_max3(p, &mem);
        let (l3, h3) = ([-10.0, -10.0, 42.0], [10.0, 10.0, 54.0]);
        rows.push(scan_mut(
            &format!("notch {alpha} BASIS"), &field, delta, l3, h3, 22, &exact, &ray,
            &|_| f64::NEG_INFINITY, false,
        ));
        rows.push(scan_mut(&format!("notch {alpha} PAAR"), &field, delta, l3, h3, 22, &exact, &ray, &pair, true));
    }
    println!("\n{:<22} {:>6} {:>9} {:>6} {:>9} {:>10}", "Fixture", "über", "worst", "unter", "worst", "LB-Verstoß");
    for r in &rows {
        println!(
            "{:<22} {:>6} {:>9.4} {:>6} {:>9.4} {:>10.2e}   {:?}",
            r.name, r.over.0, r.over.1, r.under.0, r.under.1, r.lb_viol, r.worst_under
        );
    }
}

// ══════════════ #384 §8o — die untere Distanzschranke im Tape ══════════════
//
// §8n hat gemessen, dass `gate = max(G − d₀, L(x) − δ)` die Nabe schliesst,
// wenn `L` der VOLLE Member-Schnitt ist (das Paar genügt nicht), und dass die
// Keil-Form dafür exponentiell wäre. §8o baut den linearen Weg: `L` als
// **zugeschnittene Vereinigungs-B-Rep** im Tape (`crate::offset_lb`).
//
// Die Pins darunter decken die vier Richtungen ab, in denen das schiefgehen
// kann: (a) `L` ist zu GROSS (⇒ Phantom-Material), (b) `L` ist zu klein (⇒ die
// Nabe bleibt offen), (c) ohne Block bewegt sich irgendetwas (⇒ Regression an
// jeder Fixture ohne `L`), (d) die Über-Seiten-Fixturen reissen auf.

/// Entfernt jeden `L`-Block aus dem Baum — der Kontroll-Pfad „leeres `L`".
fn strip_lb(n: &BoundedCsg) -> BoundedCsg {
    match n {
        BoundedCsg::ArcChannel { inner, channels, verts, rims, aabb, .. } => {
            BoundedCsg::ArcChannel {
                inner: Box::new(strip_lb(inner)),
                channels: channels.clone(),
                verts: verts.clone(),
                rims: rims.clone(),
                lb: None,
                aabb: *aabb,
            }
        }
        BoundedCsg::Union { left, right, aabb } => BoundedCsg::Union {
            left: Box::new(strip_lb(left)),
            right: Box::new(strip_lb(right)),
            aabb: *aabb,
        },
        BoundedCsg::Intersection { left, right, aabb } => BoundedCsg::Intersection {
            left: Box::new(strip_lb(left)),
            right: Box::new(strip_lb(right)),
            aabb: *aabb,
        },
        BoundedCsg::Difference { left, right, aabb } => BoundedCsg::Difference {
            left: Box::new(strip_lb(left)),
            right: Box::new(strip_lb(right)),
            aabb: *aabb,
        },
        BoundedCsg::Offset { inner, delta, aabb } => BoundedCsg::Offset {
            inner: Box::new(strip_lb(inner)),
            delta: *delta,
            aabb: *aabb,
        },
        other => other.clone(),
    }
}

/// Sammelt alle `L`-Blöcke eines Baums.
fn collect_lb(n: &BoundedCsg, out: &mut Vec<crate::offset_lb::LbBlock>) {
    match n {
        BoundedCsg::ArcChannel { inner, lb, .. } => {
            if let Some(b) = lb {
                out.push(b.clone());
            }
            collect_lb(inner, out);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            collect_lb(left, out);
            collect_lb(right, out);
        }
        BoundedCsg::Offset { inner, .. } => collect_lb(inner, out),
        _ => {}
    }
}

/// **§8o-Sicherheits-Pin: `L` liegt NIE über der Wahrheit.**
///
/// Das ist die eine Ungleichung, an der alles hängt. `gate = max(G − d₀, L − δ)`
/// ist genau dann harmlos, wenn `L ≤ dist(x,∁Ω)` — überschätzt `L` die Distanz,
/// öffnet das Gate weiter als die Geometrie erlaubt und der Term behauptet
/// Material in echter Luft. Geprüft gegen dieselben exakten Referenzen wie
/// §8k/§8n (`dist_hub_complement_exact`, `dist_obb_union_complement_exact`) an
/// Nabe, Ecken-Schale, notch und Rippen-Kamm.
///
/// Und die Gegenrichtung, ohne die eine „überall 0"-Funktion bestünde: an der
/// Nabe muss `L` am §8l-Worst-Punkt den EXAKTEN Wert 2.315608 treffen — das ist
/// der VOLLE Member-Schnitt aus §8n, den das beste PAAR (2.237332) verfehlt.
#[test]
fn lb_block_is_sound_and_reaches_the_full_member_complement_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let mut blocks = Vec::new();
    collect_lb(&field, &mut blocks);
    assert_eq!(
        blocks.len(),
        1,
        "die Stufen-Nabe muss GENAU EINEN L-Block tragen (Box + zwei Zylinder sind \
         darstellbar) — sonst misst dieser Pin nichts"
    );
    let blk = &blocks[0];
    let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];

    // (1) Solide: L ≤ exakt an jedem Gitterpunkt IN Ω.
    let mut worst_viol = 0.0f64;
    let n = 15usize;
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -9.0 + 18.0 * (ix as f64 + 0.2113248654) / n as f64,
                    -9.0 + 18.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -(t as f64) - 1.5 + (t as f64 + 2.0) * (iz as f64 + 0.7886751346) / n as f64,
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                // `base = δ + min_k f_k` ist der naive Stoppwert; hier direkt
                // aus der exakten Member-Geometrie, damit der Pin nicht an der
                // Feld-Kette hängt.
                let rho = (p[0] * p[0] + p[1] * p[1]).sqrt();
                let f_plate = (p[0].abs() - 20.0)
                    .max(p[1].abs() - 20.0)
                    .max((p[2] + 1.5).abs() - 1.5);
                let mut fmin = f_plate;
                for &(r, zlo, zhi) in &cyls {
                    let fc = (rho - r).max((p[2] - 0.5 * (zlo + zhi)).abs() - 0.5 * (zhi - zlo));
                    fmin = fmin.min(fc);
                }
                let base = (delta as f64 + fmin) as f32;
                let l = crate::offset_lb::lb_value(pf, base, blk) as f64;
                if l <= -1.0e29 {
                    continue; // neutral (ausserhalb Ω / ausserhalb der Term-Kugel)
                }
                worst_viol = worst_viol.max(l - dist_hub_complement_exact(t as f64, &cyls, p));
            }
        }
    }
    assert!(
        worst_viol <= 1e-3,
        "die Stück-Schranke übersteigt die exakte Distanz um {worst_viol:.3e} mm — sie \
         ERFINDET Distanz, und das Gate, das auf ihr steht, erzeugt Phantom-Material"
    );

    // (2) Scharf: am §8l-Worst-Punkt der EXAKTE Wert (§8n: VOLL 2.315608,
    //     bestes Paar nur 2.237332).
    let xw = [-1.463461473763636f32, -1.163986143381818, -0.6843920148636364];
    let l = crate::offset_lb::lb_value(xw, 0.884392, blk);
    assert!(
        (l - 2.315608).abs() < 5e-3,
        "L am Nabe-Worst-Punkt: {l:.6} statt 2.315608 — genau dieser Wert ist der VOLLE \
         Member-Schnitt aus §8n; 2.237 wäre das Paar, 1.184 der ungeschnittene A-Deckel"
    );

    // (3) Dieselbe Ungleichung an den Box-Fixturen.
    let mut box_sets: Vec<Vec<Obb>> = Vec::new();
    for tf in [2.0f64, 3.0] {
        box_sets.push(thin_corner_boxes(tf * 2.0, 40.0).iter().map(|&(l, h)| obb_aabb(l, h)).collect());
    }
    let (_, comb_boxes, _) = rib_comb_node(4, 1.5, 0.8, 2.0);
    box_sets.push(comb_boxes.iter().map(|&(l, h)| obb_aabb(l, h)).collect());
    let nodes: Vec<CsgNode> = vec![
        thin_corner_node(4.0, 40.0, delta),
        thin_corner_node(6.0, 40.0, delta),
        rib_comb_node(4, 1.5, 0.8, delta).0,
    ];
    let mut worst_box = 0.0f64;
    let mut seen = 0usize;
    for (node, obbs) in nodes.into_iter().zip(box_sets.iter()) {
        let (f, _) = compile_for(node);
        let mut bs = Vec::new();
        collect_lb(&f, &mut bs);
        let Some(blk) = bs.first() else { continue };
        seen += 1;
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for o in obbs {
            for k in 0..3 {
                let ext = (0..3).map(|a| (o.u[a][k] * o.he[a]).abs()).sum::<f64>();
                lo[k] = lo[k].min(o.c[k] - ext);
                hi[k] = hi[k].max(o.c[k] + ext);
            }
        }
        let m = 11usize;
        for ix in 0..m {
            for iy in 0..m {
                for iz in 0..m {
                    let fr = [
                        (ix as f64 + 0.2113248654) / m as f64,
                        (iy as f64 + 0.5773502692) / m as f64,
                        (iz as f64 + 0.7886751346) / m as f64,
                    ];
                    let p = [
                        lo[0] + (hi[0] - lo[0]) * fr[0],
                        lo[1] + (hi[1] - lo[1]) * fr[1],
                        lo[2] + (hi[2] - lo[2]) * fr[2],
                    ];
                    let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                    // Naiver Stoppwert aus den Boxen selbst.
                    let mut fmin = f64::INFINITY;
                    for o in obbs {
                        let mut q = [0.0f64; 3];
                        for a in 0..3 {
                            let d: f64 = (0..3).map(|k| (p[k] - o.c[k]) * o.u[a][k]).sum();
                            q[a] = d.abs() - o.he[a];
                        }
                        let outside = (0..3).map(|a| q[a].max(0.0).powi(2)).sum::<f64>().sqrt();
                        let inside = q[0].max(q[1]).max(q[2]).min(0.0);
                        fmin = fmin.min(outside + inside);
                    }
                    let base = (delta as f64 + fmin) as f32;
                    let l = crate::offset_lb::lb_value(pf, base, blk) as f64;
                    if l <= -1.0e29 {
                        continue;
                    }
                    worst_box = worst_box.max(l - dist_obb_union_complement_exact(obbs, p));
                }
            }
        }
    }
    assert!(seen >= 2, "zu wenige Box-Fixturen mit L-Block ({seen})");
    assert!(
        worst_box <= 1e-3,
        "Box-Fixturen: die Stück-Schranke übersteigt die exakte Distanz um {worst_box:.3e} mm"
    );
}

/// **§8o-Kern-Pin: die Stufen-Nabe ist jetzt ZWEISEITIG exakt.**
///
/// Vorher (§8k…§8n, unverändert über vier Pakete): `0 / 59 · 0.3156` auf dem
/// 22³-Gitter — kein Phantom-Material, aber 59 Punkte, an denen das Feld die
/// Erosion für LEER hält, obwohl sie dort massiv ist. Ursache war
/// ausschliesslich die Ein-Term-Kappe `G = 0.868` gegen einen Bedarf von
/// `d₀ − f_wahr = 1.200`; der Term hatte den exakten Wert (`w_geo = 0.3156`)
/// längst. Mit `L = 2.315608` wird `gate = L − δ = 0.3156` und der Term geht
/// durch — auf den Mikrometer.
#[test]
fn erosion_stepped_hub_is_exact_on_both_sides_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
    let exact = |p: [f64; 3]| dist_hub_complement_exact(t as f64, &cyls, p);
    let ray = |p: [f64; 3]| dist_hub_complement(t as f64, &cyls, p, 160);
    let (lo, hi) = ([-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5]);
    let r = scan_mut(
        "Nabe", &field, delta, lo, hi, 22, &exact, &ray, &|_| f64::NEG_INFINITY, false,
    );
    assert_eq!(
        r.over.0, 0,
        "Stufen-Nabe: {} Punkte Phantom-Material, größtes {:.4} mm — die untere \
         Distanzschranke darf NIE einen Über-Claim eintauschen (§8n-Satz: wo `L − δ` \
         bindet, ist `f ≥ δ − L ≥ e`)",
        r.over.0, r.over.1
    );
    assert!(
        r.under.0 == 0 && r.under.1 <= 0.010,
        "Stufen-Nabe: {} Punkte fehlendes Material, größtes {:.4} mm bei {:?} — §8o \
         schliesst diese Seite (vorher 59 / 0.3156)",
        r.under.0, r.under.1, r.worst_under
    );
}

/// **§8o-Kontroll-Pin: ohne `L`-Block ist das Feld bit-identisch zu vor §8o.**
///
/// Das ist keine Messung, sondern eine Identität: `L = −1e30` macht
/// `max(G − d₀, L − δ)` zu `G − d₀`. Der Pin hält sie trotzdem fest, weil sie
/// die Grundlage jeder Aussage „Fixture X hat sich nicht bewegt" ist — und
/// nagelt gleichzeitig die BASIS-Zahl der Nabe fest, die §8o ablöst.
#[test]
fn stop_lb_absent_reproduces_the_g_cap_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let basis = strip_lb(&field);
    let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
    let exact = |p: [f64; 3]| dist_hub_complement_exact(t as f64, &cyls, p);
    let ray = |p: [f64; 3]| dist_hub_complement(t as f64, &cyls, p, 160);
    let (lo, hi) = ([-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5]);
    let r = scan_mut(
        "Nabe BASIS", &basis, delta, lo, hi, 22, &exact, &ray, &|_| f64::NEG_INFINITY, false,
    );
    assert_eq!(
        (r.over.0, r.under.0),
        (0, 59),
        "ohne L-Block muss die Nabe EXAKT den §8n-BASIS-Befund reproduzieren \
         (0 über / 59 unter), sonst misst der §8o-Vergleich zwei verschiedene Maschinen"
    );
    assert!(
        (r.under.1 - 0.3156).abs() < 1e-3,
        "BASIS-Restfehler {:.4} statt 0.3156 (§8n)",
        r.under.1
    );
    // Und der eigentliche Beweis: der Baum OHNE Block liefert an jedem Punkt
    // exakt den Feldwert, den `stop_apply` mit `lb = None` rechnet.
    let mut rng = 12345u64;
    for _ in 0..3000 {
        let mut nx = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((rng >> 33) as f64 / (1u64 << 31) as f64 - 0.5) as f32
        };
        let p = [nx() * 20.0, nx() * 20.0, nx() * 14.0];
        assert_eq!(
            basis.sdf(p, EvalMode::Visual).to_bits(),
            strip_lb(&field).sdf(p, EvalMode::Visual).to_bits(),
            "Kontroll-Pfad nicht deterministisch bei {p:?}"
        );
    }
}

/// **§8p-Kern-Pin: wo `L` da ist, ist die ÜBER-Seite leer.**
///
/// Der Vorgänger dieses Pins (`lb_gate_leaves_the_over_side_fixtures_unmoved_384`,
/// §8o) hat festgehalten, dass §8o die Kamm-Über-Seiten um NULL Punkte bewegt —
/// 144 · 0.5378 beim Kamm 4×2.0/1.0, und das mit einem AKTIVEN 140-Stück-Block.
/// Genau diese Zahl war der Befund: `L` lag im Tape, wusste am Worst-Punkt die
/// Wahrheit (`L = 1.4528` gegen `dist = 1.4531`) — und das Gate
/// `max(G − d₀, L − δ)` warf sie weg, weil die Schätzung `G − d₀ = +0.3215`
/// grösser war als die Wahrheit `L − δ = −0.5472`.
///
/// §8p dreht das um: wo `L` verfügbar ist, gatet `L` allein. Damit ist die
/// Über-Seite kein Messergebnis mehr, sondern ein Satz
/// (`f ≥ δ − L ≥ δ − dist`, s. [`crate::offset_channel::concave_gate`]) — der
/// Pin misst, dass der Satz auch im gebauten Feld gilt.
///
/// Die zweite Zeile war bis zur Hälfte dieses Pakets die ehrliche Gegenprobe:
/// der Kamm 4×1.5/0.8 trägt drei Extrude-Kontaktprismen, `lb_solid` kannte nur
/// Box und Zylinder, also verweigerte die Synthese den GANZEN Block (§8o-Regel
/// 3) und die `G`-Kappe blieb bei 168 · 0.8103. Seit §8p sind konvexe
/// Geraden-Extrudes darstellbar (296 Stücke), und damit fällt auch diese Zeile
/// auf 0/0. Die Gegenprobe „ohne `L` bleibt die `G`-Kappe" hält jetzt
/// `non_convex_extrude_member_refuses_the_lb_block_384`.
#[test]
fn lb_only_gate_closes_the_comb_over_side_384() {
    let delta = 2.0f32;
    for ((w, g), want_over, want_worst) in
        [((1.5f32, 0.8f32), 0usize, 0.0f64), ((2.0, 1.0), 0, 0.0)]
    {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, _) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_boxes_complement(&boxes, p, 160);
        let (l3, h3) = ([-span, -3.0, -6.0], [span, 3.0, 0.5]);
        let r = scan_mut(
            &format!("Kamm {w}/{g}"), &field, delta, l3, h3, 24, &exact, &ray,
            &|_| f64::NEG_INFINITY, false,
        );
        assert_eq!(
            r.over.0, want_over,
            "Rippen-Kamm {w}/{g}: {} statt {want_over} Über-Claim-Punkte. 0 ist hier ein \
             SATZ, keine Messung: wo L bindet, ist f ≥ δ − L ≥ δ − dist. War vor §8p \
             168 · 0.8103 (1.5/0.8) bzw. 144 · 0.5378 (2.0/1.0)",
            r.over.0
        );
        assert!(
            (r.over.1 - want_worst).abs() < 1e-3,
            "Rippen-Kamm {w}/{g}: größter Über-Claim {:.4} statt {want_worst}",
            r.over.1
        );
        assert_eq!(r.under.0, 0, "Rippen-Kamm {w}/{g}: unter-Seite aufgerissen");
    }
    // notch: beidseitig 0/0, unverändert.
    for alpha in [90.0f64, 60.0] {
        let (field, _) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_notch_complement(alpha, p);
        let (l3, h3) = ([-10.0, -10.0, 42.0], [10.0, 10.0, 54.0]);
        let r = scan_mut(
            &format!("notch {alpha}"), &field, delta, l3, h3, 22, &exact, &ray,
            &|_| f64::NEG_INFINITY, false,
        );
        assert_eq!(
            (r.over.0, r.under.0),
            (0, 0),
            "notch {alpha}: {} über / {} unter — war 0/0",
            r.over.0, r.under.0
        );
    }
}

/// **§8o-Tape-Pin: der neue Opcode ist im Strom und der Strom rechnet dasselbe.**
///
/// Ein `L`-Block, den nur der Baum-Auswerter sieht, wäre schlimmer als keiner:
/// Mesher (C++), GPU (WGSL) und Intervall-DC (fidget) würden ein ANDERES Feld
/// zeigen als Pick/Query. Der Pin prüft beides — Präsenz des Opcodes und
/// Feldgleichheit Baum ↔ Tape.
#[test]
fn stop_lb_opcode_is_emitted_and_tape_matches_the_tree_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let compiled = crate::sdf_compiler::compile_body(&field);
    assert!(
        compiled.opcodes.iter().any(|v| v.to_bits() == crate::sdf_compiler::OP_STOP_LB),
        "kompiliertes Tape der Stufen-Nabe enthält kein OP_STOP_LB"
    );
    let mut rng = 777u64;
    let mut max_diff = 0.0f32;
    for _ in 0..4000 {
        let mut nx = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((rng >> 33) as f64 / (1u64 << 31) as f64 - 0.5) as f32
        };
        let p = [nx() * 24.0, nx() * 24.0, nx() * 16.0];
        let d_tree = field.sdf(p, EvalMode::Visual);
        let d_tape = crate::sdf_compiler::eval_opcodes(&compiled.opcodes, p);
        max_diff = max_diff.max((d_tree - d_tape).abs());
    }
    assert!(
        max_diff < 2e-5,
        "Tape-Parität des L-Blocks verletzt: max |Δ| = {max_diff:.3e}"
    );
}

// ── einmalige §8o-Kostenmessung, NICHT im Fast-Tier ──
#[test]
#[ignore]
fn probe_384_lb_cost() {
    let delta = 2.0f32;
    let cases: Vec<(String, CsgNode)> = vec![
        ("Nabe".into(), stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta)),
        ("Schale t/δ=2".into(), thin_corner_node(4.0, 40.0, delta)),
        ("notch 90".into(), notch_union_node(90.0, delta as f64)),
        ("Kamm 4×1.5/0.8".into(), rib_comb_node(4, 1.5, 0.8, delta).0),
        ("Kamm 4×2.0/1.0".into(), rib_comb_node(4, 2.0, 1.0, delta).0),
    ];
    println!("\n{:<16} {:>7} {:>8} {:>10} {:>12} {:>12}", "Fixture", "Stücke", "Worte", "Tape ges.", "L ns/Punkt", "Feld ns/Pkt");
    for (name, node) in cases {
        let (field, _) = compile_for(node);
        let mut bs = Vec::new();
        collect_lb(&field, &mut bs);
        let compiled = crate::sdf_compiler::compile_body(&field);
        let np: usize = bs.iter().map(|b| b.pieces.len()).sum();
        let words = bs.len() * (1 + crate::offset_lb::LB_HEADER) + np * crate::offset_lb::LB_STRIDE;
        let probe = [1.0f32, 1.0, -1.0];
        let (mut tl, mut tf) = (0u128, 0u128);
        if let Some(b) = bs.first() {
            let t0 = std::time::Instant::now();
            let mut acc = 0.0f32;
            for k in 0..200000 {
                let q = [probe[0] + (k % 7) as f32 * 1e-4, probe[1], probe[2]];
                acc += crate::offset_lb::lb_value(q, 0.5, b);
            }
            tl = t0.elapsed().as_nanos() / 200000;
            std::hint::black_box(acc);
        }
        {
            let t0 = std::time::Instant::now();
            let mut acc = 0.0f32;
            for k in 0..50000 {
                let q = [probe[0] + (k % 7) as f32 * 1e-4, probe[1], probe[2]];
                acc += field.sdf(q, EvalMode::Visual);
            }
            tf = t0.elapsed().as_nanos() / 50000;
            std::hint::black_box(acc);
        }
        println!("{name:<16} {np:>7} {words:>8} {:>10} {tl:>12} {tf:>12}", compiled.opcodes.len());
    }
}


// ══════════════ #384 §8p — Naht gegen Naht: erst messen ══════════════
//
// Verdächtig ist laut §8j/§8o, dass zwei Nähte näher beieinander liegen als
// ihre Trust-Regionen und jeder Kanal rechnet, als wäre seine Naht das nächste
// Feature. Diese Sonde misst statt zu verdächtigen: sie zerlegt den
// Über-Seiten-Worst-Punkt beider Kamm-Fixtures Term für Term.

/// Alle Stopp-Knoten eines Baums (Referenzen, damit `inner` auswertbar bleibt).
fn stop_nodes<'a>(n: &'a BoundedCsg, out: &mut Vec<&'a BoundedCsg>) {
    match n {
        BoundedCsg::ArcChannel { inner, .. } => {
            out.push(n);
            stop_nodes(inner, out);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            stop_nodes(left, out);
            stop_nodes(right, out);
        }
        BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => {
            stop_nodes(inner, out)
        }
        _ => {}
    }
}

/// EIN Kanal-Term im gewählten Gate-Modus — Spiegel von `channel_apply`, damit
/// die Zerlegung auch den §8o-Zustand nachstellen kann (die Zahlen im Doc).
fn eval_term(
    d0: f32,
    d: f32,
    p: [f32; 3],
    ch: &crate::offset_channel::OffsetChannel,
    lbv: f32,
    mode: P8pMode,
) -> f32 {
    use crate::offset_channel as oc;
    if oc::channel_skip(d, p, ch) {
        return d;
    }
    let Some((de, q, m1, m2)) = oc::channel_nearest(p, ch) else { return d };
    let dt = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let dq = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let w_geo = dt(dq, m1)
        .min(dt(dq, m2))
        .min(oc::end_plane_gate(p, ch))
        .min(oc::face_plane_gate(p, ch))
        .min(de - ch.delta)
        .min(ch.trust - de);
    let w = w_geo.min(p8p_gate(ch.gate, d0, lbv - ch.delta, mode));
    if w > 0.0 {
        d.min(-w)
    } else if w_geo <= 0.0 {
        d.min(-w_geo)
    } else {
        d
    }
}

/// Term-für-Term-Zerlegung EINES Punktes; gibt `(feld, zeilen)` zurück.
fn decompose_at(field: &BoundedCsg, pf: [f32; 3], mode: P8pMode) -> String {
    use crate::offset_channel as oc;
    let mut s = String::new();
    let mut stops = Vec::new();
    stop_nodes(field, &mut stops);
    s += &format!("  Stopps: {}\n", stops.len());
    for (si, node) in stops.iter().enumerate() {
        let BoundedCsg::ArcChannel { inner, channels, verts, rims, lb, .. } = node else {
            continue;
        };
        let d0 = inner.sdf(pf, EvalMode::Visual);
        let delta = channels
            .first()
            .map(|c| c.delta)
            .or_else(|| verts.first().map(|v| v.delta))
            .unwrap_or(0.0);
        let lbv = oc::stop_lb_value(d0, pf, lb.as_ref());
        s += &format!(
            "  Stopp #{si}: d0={d0:.6} δ={delta:.3} L={} (L−δ={}) Kanäle={} Kugeln={} Rims={}\n",
            if lbv <= -1.0e29 { "—".into() } else { format!("{lbv:.6}") },
            if lbv <= -1.0e29 { "—".into() } else { format!("{:.6}", lbv - delta) },
            channels.len(),
            verts.len(),
            rims.len()
        );
        let mut d = d0;
        let mut lim = -oc::END_PLANE_OFF;
        let mut best_de = f32::INFINITY;
        for (ci, ch) in channels.iter().enumerate() {
            let skip = oc::channel_skip(d, pf, ch);
            let lc = oc::channel_limit(pf, ch);
            lim = lim.max(lc);
            let before = d;
            d = eval_term(d0, d, pf, ch, lbv, mode);
            let Some((de, q, m1, m2)) = oc::channel_nearest(pf, ch) else {
                s += &format!("   K{ci}: kein nearest\n");
                continue;
            };
            best_de = best_de.min(de);
            let dq = [pf[0] - q[0], pf[1] - q[1], pf[2] - q[2]];
            let dt = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            let (s1, s2) = (dt(dq, m1), dt(dq, m2));
            let e3 = oc::end_plane_gate(pf, ch);
            let e4 = oc::face_plane_gate(pf, ch);
            let w_geo = s1.min(s2).min(e3).min(e4).min(de - ch.delta).min(ch.trust - de);
            let gate = p8p_gate(ch.gate, d0, lbv - ch.delta, mode);
            let w = w_geo.min(gate);
            s += &format!(
                "   K{ci}{}: de={de:.4} q=({:.3},{:.3},{:.3}) s1={s1:.4} s2={s2:.4} \
                 s3={} s4={} de−δ={:.4} K−de={:.4} | w_geo={w_geo:.4} G−d0={:.4} \
                 L−δ={} gate={} w={} | lim={} d:{before:.4}→{d:.4}\n",
                if skip { "(skip)" } else { "" },
                q[0], q[1], q[2],
                if e3 > 1.0e29 { "∞".into() } else { format!("{e3:.4}") },
                if e4 > 1.0e29 { "∞".into() } else { format!("{e4:.4}") },
                de - ch.delta,
                ch.trust - de,
                ch.gate - d0,
                if lbv <= -1.0e29 { "—".into() } else { format!("{:.4}", lbv - ch.delta) },
                if gate < -1.0e29 { "—".into() } else { format!("{gate:.4}") },
                if w < -1.0e29 { "—".into() } else { format!("{w:.4}") },
                if lc <= -1.0e29 { "—".into() } else { format!("{lc:.4}") },
            );
        }
        for (vi, vs) in verts.iter().enumerate() {
            let lc = oc::vertex_limit(pf, vs);
            lim = lim.max(lc);
            let before = d;
            d = oc::vertex_apply(d0, d, pf, vs, lbv - vs.delta);
            let _ = mode; // Kugeln tragen an den Kamm-Fixturen nichts bei
            let rho = ((pf[0] - vs.v[0]).powi(2)
                + (pf[1] - vs.v[1]).powi(2)
                + (pf[2] - vs.v[2]).powi(2))
            .sqrt();
            s += &format!(
                "   V{vi}: ρ={rho:.4} K−ρ={:.4} d:{before:.4}→{d:.4} lim={}\n",
                vs.trust - rho,
                if lc <= -1.0e29 { "—".into() } else { format!("{lc:.4}") }
            );
        }
        let concave = channels.first().map(|c| c.concave).or_else(|| verts.first().map(|v| v.concave));
        let after = match concave {
            Some(true) => d.max(lim),
            Some(false) => d.min(-lim),
            None => d,
        };
        s += &format!(
            "  ⇒ nach Termen {d:.6}, lim={} ⇒ {after:.6}  (min_j de_j = {best_de:.4})\n",
            if lim <= -1.0e29 { "—".into() } else { format!("{lim:.4}") }
        );
    }
    s += &format!(
        "  Feld gesamt: {:.6} (Produktion {:.6})\n",
        eval_p8p(field, pf, mode),
        field.sdf(pf, EvalMode::Visual)
    );
    s
}

#[test]
#[ignore]
fn probe_384_seam_vs_seam() {
    let delta = 2.0f32;
    for (w, g) in [(1.5f32, 0.8f32), (2.0f32, 1.0f32)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, _rep) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_boxes_complement(&boxes, p, 160);
        let (lo, hi) = ([-span, -3.0, -6.0], [span, 3.0, 0.5]);
        let n = 24usize;
        let mut over: Vec<([f64; 3], f64, f32)> = Vec::new();
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let f = [
                        (ix as f64 + 0.2113248654) / n as f64,
                        (iy as f64 + 0.5773502692) / n as f64,
                        (iz as f64 + 0.7886751346) / n as f64,
                    ];
                    let p = [
                        lo[0] + (hi[0] - lo[0]) * f[0],
                        lo[1] + (hi[1] - lo[1]) * f[1],
                        lo[2] + (hi[2] - lo[2]) * f[2],
                    ];
                    let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                    // Gemessen wird der §8o-Zustand — die Zerlegung, die §8p
                    // erklärt. Produktion (LB-ONLY) hat hier 0 Punkte.
                    let got = eval_p8p(&field, pf, P8pMode::Basis);
                    if got < 0.0 {
                        let err = delta as f64 - ray(p);
                        if err > 1e-3 {
                            over.push((p, err, got));
                        }
                    }
                }
            }
        }
        over.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        println!("\n════ Kamm 4×{w}/{g}: {} Über-Punkte, worst {:.4}", over.len(), over[0].1);
        // Statistik über ALLE Über-Punkte: Feldwert, ray, exakt.
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        println!("  Stopp-Knoten: {}", stops.len());
        for (i, nd) in stops.iter().enumerate() {
            if let BoundedCsg::ArcChannel { channels, verts, rims, lb, .. } = nd {
                println!(
                    "   #{i}: {} Kanäle (concave {:?}), {} Kugeln, {} Rims, L-Block {}",
                    channels.len(),
                    channels.first().map(|c| c.concave),
                    verts.len(),
                    rims.len(),
                    lb.as_ref().map(|b| b.pieces.len()).map_or("—".into(), |k| k.to_string())
                );
                for (ci, ch) in channels.iter().enumerate().take(30) {
                    println!(
                        "      K{ci}: δ={:.2} G={:.4} K={:.4} bc=({:.2},{:.2},{:.2}) br={:.3} \
                         arcs={} s3-Slots={} s4-Slots={}",
                        ch.delta, ch.gate, ch.trust, ch.bound_c[0], ch.bound_c[1], ch.bound_c[2],
                        ch.bound_r, ch.arcs.len(),
                        ch.end_planes.iter().filter(|p| p[3] > -1.0e29).count(),
                        ch.face_planes.iter().filter(|p| p[3] > -1.0e29).count(),
                    );
                }
            }
        }
        for (k, (p, err, got)) in over.iter().take(3).enumerate() {
            println!(
                "\n  ── Über-Punkt #{k}: ({:.6},{:.6},{:.6}) err={err:.6} feld={got:.6} \
                 ray={:.6} exakt={:.6}",
                p[0], p[1], p[2], ray(*p), exact(*p)
            );
            print!(
                "{}",
                decompose_at(&field, [p[0] as f32, p[1] as f32, p[2] as f32], P8pMode::Basis)
            );
        }
    }
}

/// §8p-Experiment: der Gate-Zweig, den der Term nimmt.
#[derive(Clone, Copy, PartialEq)]
enum P8pMode {
    /// Produktion: `gate = max(G − d₀, L − δ)`.
    Basis,
    /// Kandidat A: wo `L` VERFÜGBAR ist, gatet `L` ALLEIN
    /// (`gate = L − δ`); sonst wie bisher `G − d₀`.
    LbOnly,
}

fn p8p_gate(g: f32, base: f32, extra: f32, mode: P8pMode) -> f32 {
    match mode {
        P8pMode::Basis => (g - base).max(extra),
        // Die multiplikative Maske ist die Form, die auch im Tape trägt:
        // `0 · (−1e30) = −0` ist exakt, `x + 1e30` löscht die Mantisse.
        P8pMode::LbOnly => {
            let m = if extra < -1.0e29 { 1.0f32 } else { 0.0f32 };
            m * (g - base) + (1.0 - m) * extra
        }
    }
}

fn eval_p8p(n: &BoundedCsg, p: [f32; 3], mode: P8pMode) -> f32 {
    use crate::offset_channel as oc;
    match n {
        BoundedCsg::ArcChannel { inner, channels, verts, rims, lb, .. } => {
            let d0 = eval_p8p(inner, p, mode);
            let lbv = oc::stop_lb_value(d0, p, lb.as_ref());
            let mut d = d0;
            let mut lim = -oc::END_PLANE_OFF;
            for ch in channels {
                lim = lim.max(oc::channel_limit(p, ch));
                if oc::channel_skip(d, p, ch) {
                    continue;
                }
                let Some((de, q, m1, m2)) = oc::channel_nearest(p, ch) else { continue };
                let dt = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let dq = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                let w_geo = dt(dq, m1)
                    .min(dt(dq, m2))
                    .min(oc::end_plane_gate(p, ch))
                    .min(oc::face_plane_gate(p, ch))
                    .min(de - ch.delta)
                    .min(ch.trust - de);
                let w = w_geo.min(p8p_gate(ch.gate, d0, lbv - ch.delta, mode));
                d = if w > 0.0 {
                    d.min(-w)
                } else if w_geo <= 0.0 {
                    d.min(-w_geo)
                } else {
                    d
                };
            }
            for vs in verts {
                lim = lim.max(oc::vertex_limit(p, vs));
                let dq = [p[0] - vs.v[0], p[1] - vs.v[1], p[2] - vs.v[2]];
                let rho = (dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2]).sqrt();
                if vs.trust - rho <= if vs.concave { -d } else { d } {
                    continue;
                }
                let dt = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let w_geo = dt(dq, vs.gates[0])
                    .min(dt(dq, vs.gates[1]))
                    .min(dt(dq, vs.gates[2]))
                    .min(rho - vs.delta)
                    .min(vs.trust - rho);
                let w = w_geo.min(p8p_gate(vs.gate, d0, lbv - vs.delta, mode));
                d = if w > 0.0 {
                    d.min(-w)
                } else if w_geo <= 0.0 {
                    d.min(-w_geo)
                } else {
                    d
                };
            }
            let concave =
                channels.first().map(|c| c.concave).or_else(|| verts.first().map(|v| v.concave));
            d = match concave {
                Some(true) => d.max(lim),
                Some(false) => d.min(-lim),
                None => d,
            };
            for block in rims {
                d = crate::offset_rim::apply_rims_f32(d, p, block.concave, &block.recs);
                d = crate::offset_rim::apply_rims_arc_f32(d, p, block.concave, &block.arc_recs);
            }
            d
        }
        // Die AABB-Prunes der Produktion MÜSSEN mitlaufen — ohne sie weicht
        // der Kontroll-Pfad an genau den Punkten ab, an denen der Prune
        // greift (gemessen: 2 von 4000 am Kamm 1.5/0.8).
        BoundedCsg::Union { left, right, .. } => {
            let dl = crate::bvh::distance_point_to_aabb(p, left.aabb());
            let dr = crate::bvh::distance_point_to_aabb(p, right.aabb());
            let (first, second, bound) =
                if dl <= dr { (&**left, &**right, dr) } else { (&**right, &**left, dl) };
            let d_first = eval_p8p(first, p, mode);
            if bound > 0.0 && d_first < bound {
                d_first
            } else {
                d_first.min(eval_p8p(second, p, mode))
            }
        }
        BoundedCsg::Intersection { left, right, .. } => {
            eval_p8p(left, p, mode).max(eval_p8p(right, p, mode))
        }
        BoundedCsg::Difference { left, right, .. } => {
            let d_left = eval_p8p(left, p, mode);
            let dr = crate::bvh::distance_point_to_aabb(p, right.aabb());
            if dr > 0.0 && d_left >= -dr {
                d_left
            } else {
                d_left.max(-eval_p8p(right, p, mode))
            }
        }
        BoundedCsg::Offset { inner, delta, .. } => eval_p8p(inner, p, mode) - delta,
        _ => n.sdf(p, EvalMode::Visual),
    }
}

/// Beidseitiger Scan mit dem §8p-Auswerter.
#[allow(clippy::too_many_arguments)]
fn scan_p8p(
    field: &BoundedCsg,
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
    exact: &dyn Fn([f64; 3]) -> f64,
    ray: &dyn Fn([f64; 3]) -> f64,
    mode: P8pMode,
) -> (usize, f64, [f64; 3], usize, f64, [f64; 3]) {
    let (mut oc, mut ow, mut op) = (0usize, 0.0f64, [0.0f64; 3]);
    let (mut uc, mut uw, mut up) = (0usize, 0.0f64, [0.0f64; 3]);
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let f = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * f[0],
                    lo[1] + (hi[1] - lo[1]) * f[1],
                    lo[2] + (hi[2] - lo[2]) * f[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let got = eval_p8p(field, pf, mode);
                if got < 0.0 {
                    let err = delta as f64 - ray(p);
                    if err > 1e-3 {
                        oc += 1;
                        if err > ow {
                            ow = err;
                            op = p;
                        }
                    }
                } else {
                    let err = exact(p) - delta as f64;
                    if err > 1e-3 {
                        uc += 1;
                        if err > uw {
                            uw = err;
                            up = p;
                        }
                    }
                }
            }
        }
    }
    (oc, ow, op, uc, uw, up)
}

#[test]
#[ignore]
fn probe_384_lb_only_gate() {
    let delta = 2.0f32;
    println!(
        "\n{:<20} {:>7} {:>5} {:>9} {:>5} {:>9}",
        "Fixture/Modus", "Stücke", "über", "worst", "unter", "worst"
    );
    let mut run = |name: &str,
                   field: &BoundedCsg,
                   lo: [f64; 3],
                   hi: [f64; 3],
                   n: usize,
                   exact: &dyn Fn([f64; 3]) -> f64,
                   ray: &dyn Fn([f64; 3]) -> f64| {
        let mut bs = Vec::new();
        collect_lb(field, &mut bs);
        let np: usize = bs.iter().map(|b| b.pieces.len()).sum();
        for (tag, mode) in [("BASIS", P8pMode::Basis), ("LB-ONLY", P8pMode::LbOnly)] {
            let r = scan_p8p(field, delta, lo, hi, n, exact, ray, mode);
            println!(
                "{:<20} {np:>7} {:>5} {:>9.4} {:>5} {:>9.4}   über@{:?} unter@{:?}",
                format!("{name} {tag}"),
                r.0,
                r.1,
                r.3,
                r.4,
                r.2.map(|v| (v * 1e4).round() / 1e4),
                r.5.map(|v| (v * 1e4).round() / 1e4)
            );
        }
    };
    // Stufen-Nabe
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        let exact = |p: [f64; 3]| dist_hub_complement_exact(t as f64, &cyls, p);
        let ray = |p: [f64; 3]| dist_hub_complement(t as f64, &cyls, p, 160);
        run("Nabe", &field, [-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5], 22, &exact, &ray);
    }
    // Ecken-Schale, ganzer Dicken-Fächer
    for tf in [2.0f32, 2.1, 2.25, 2.4, 2.5, 3.0, 4.0, 6.0] {
        let t = tf * delta;
        let (field, _) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_boxes_complement(&boxes, p, 160);
        let lo = -(t as f64 + delta as f64 + 1.0);
        run(&format!("Schale {tf}"), &field, [lo; 3], [1.0; 3], 22, &exact, &ray);
    }
    // Rippen-Kämme
    for (w, g) in [(1.5f32, 0.8f32), (2.0f32, 1.0f32)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, _) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_boxes_complement(&boxes, p, 160);
        run(&format!("Kamm {w}/{g}"), &field, [-span, -3.0, -6.0], [span, 3.0, 0.5], 24, &exact, &ray);
    }
    // notch
    for alpha in [90.0f64, 60.0] {
        let (field, _) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let exact = |p: [f64; 3]| dist_obb_union_complement_exact(&obbs, p);
        let ray = |p: [f64; 3]| dist_to_notch_complement(alpha, p);
        run(&format!("notch {alpha}"), &field, [-10.0, -10.0, 42.0], [10.0, 10.0, 54.0], 22, &exact, &ray);
    }
}

/// Kontroll-Äquivalenz des §8p-Auswerters: `Basis` MUSS die Produktion
/// bit-genau reproduzieren, sonst misst das Experiment zwei Maschinen.
#[test]
#[ignore]
fn probe_384_p8p_control() {
    let delta = 2.0f32;
    let cases: Vec<(String, CsgNode)> = vec![
        ("Nabe".into(), stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta)),
        ("Schale 2".into(), thin_corner_node(4.0, 40.0, delta)),
        ("notch 90".into(), notch_union_node(90.0, delta as f64)),
        ("Kamm 1.5/0.8".into(), rib_comb_node(4, 1.5, 0.8, delta).0),
        ("Kamm 2/1".into(), rib_comb_node(4, 2.0, 1.0, delta).0),
    ];
    for (name, node) in cases {
        let (field, _) = compile_for(node);
        let mut rng = 4242u64;
        let (mut bad, mut moved) = (0usize, 0usize);
        for _ in 0..4000 {
            let mut nx = || {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((rng >> 33) as f64 / (1u64 << 31) as f64 - 0.5) as f32
            };
            let p = [nx() * 26.0, nx() * 26.0, nx() * 18.0];
            // Seit §8p IST die Produktion der LB-ONLY-Modus; `Basis` bleibt der
            // §8o-Vergleichspfad der A/B-Messung.
            if field.sdf(p, EvalMode::Visual).to_bits()
                != eval_p8p(&field, p, P8pMode::LbOnly).to_bits()
            {
                bad += 1;
            }
            if field.sdf(p, EvalMode::Visual) != eval_p8p(&field, p, P8pMode::Basis) {
                moved += 1;
            }
        }
        println!("{name:<14} Kontroll-Abweichungen: {bad} / 4000, gegen §8o bewegt: {moved}");
    }
}

/// §8p: WARUM bekommt der Kamm 4×1.5/0.8 keinen `L`-Block? Die Member des
/// Stopps, benannt.
#[test]
#[ignore]
fn probe_384_lb_refusal() {
    fn name(n: &BoundedCsg) -> String {
        match n {
            BoundedCsg::Leaf { primitive, .. } => format!("Leaf({})", match primitive {
                Primitive::Box { .. } => "Box",
                Primitive::Cylinder { .. } => "Cylinder",
                Primitive::Sphere { .. } => "Sphere",
                _ => "andere",
            }),
            BoundedCsg::Union { .. } => "Union".into(),
            BoundedCsg::Intersection { .. } => "Intersection".into(),
            BoundedCsg::Difference { .. } => "Difference".into(),
            BoundedCsg::Offset { .. } => "Offset".into(),
            BoundedCsg::Extrude { .. } => "Extrude".into(),
            BoundedCsg::Revolve { .. } => "Revolve".into(),
            BoundedCsg::ArcChannel { .. } => "ArcChannel".into(),
            other => format!("{:?}", std::mem::discriminant(other)),
        }
    }
    fn flat<'a>(n: &'a BoundedCsg, out: &mut Vec<&'a BoundedCsg>) {
        match n {
            BoundedCsg::Union { left, right, .. } => {
                flat(left, out);
                flat(right, out);
            }
            // Die L-Synthese sieht den Baum INNERHALB des Offset-Stopps.
            BoundedCsg::Offset { inner, .. } => flat(inner, out),
            other => out.push(other),
        }
    }
    let delta = 2.0f32;
    for (w, g) in [(1.5f32, 0.8f32), (2.0f32, 1.0f32)] {
        let (node, _b, _s) = rib_comb_node(4, w, g, delta);
        let (field, rep) = compile_for(node);
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        println!("\n── Kamm {w}/{g}: {} Stopp(s), Status {:?}", stops.len(), rep.status());
        for nd in &stops {
            if let BoundedCsg::ArcChannel { inner, lb, .. } = nd {
                let mut ms = Vec::new();
                flat(inner, &mut ms);
                println!(
                    "   L-Block: {}, {} Union-Member:",
                    lb.as_ref().map_or("—".into(), |b| format!("{} Stücke", b.pieces.len())),
                    ms.len()
                );
                for m in ms {
                    println!("     {} aabb={:?}", name(m), m.aabb());
                }
            }
        }
    }
}

// ══════════════ #384 §8p — die Gate-Basis: `L` schlägt `G` ══════════════
//
// Die Zerlegung des Kamm-Worst-Punktes (Sonde `probe_384_seam_vs_seam`) hat
// zwei Vermutungen widerlegt und eine Ursache gefunden:
//
// * Der bindende Anteil von `w_geo` ist `K − de` (die Trust-Radius-Flanke),
//   nicht `de − δ` und kein Kegel-Gate: der Term sitzt mit `de = 3.7285` fast
//   am Rand seiner Trust-Region `K = 3.9084` und behauptet dort noch 0.18 mm.
// * `U = min_j de_j` (die §8j-Empfehlung) ist bereits gebaut — `OP_STOP_LIMIT`
//   klemmt genau `d ≥ δ − U` — und reicht am Worst-Punkt um 0.5378 mm nicht
//   (`U = 2.0413` gegen `dist = 1.4531`).
// * Die Ursache ist das `max` in `gate = max(G − d₀, L − δ)`: es nimmt die
//   Schätzung (+0.3215) statt der Wahrheit (−0.5472).

/// **§8p-Vertrags-Pin von [`crate::offset_channel::concave_gate`].**
///
/// Drei Aussagen, jede eine eigene Fehlerklasse:
/// 1. Ist `L` verfügbar, gilt `gate = L − δ` — AUCH wenn `G − d₀` grösser ist.
///    Genau dieser Fall ist der Kamm (0.3215 gegen −0.5472).
/// 2. Ist `L` nicht verfügbar (`−1e30`), gilt `gate = G − d₀` — bit-identisch
///    zum Zustand vor §8p. Das ist die Kontroll-Äquivalenz auf Term-Ebene.
/// 3. Die Sentinel-Arithmetik ist EXAKT: kein `x + 1e30`, das die Mantisse
///    löscht (Präzedenzfall 1.435 mm, §8j-fidget-Falle).
#[test]
fn concave_gate_prefers_the_lower_bound_384() {
    use crate::offset_channel::{concave_gate, LB_AVAIL};
    let (g, d0, delta) = (0.8684f32, 0.546933f32, 2.0f32);
    // (1) L da, aber KLEINER als die G-Kappe → L gewinnt trotzdem.
    let l = 1.452760f32;
    assert_eq!(
        concave_gate(g, d0, l - delta),
        l - delta,
        "L verfügbar, aber das Gate nahm die G-Kappe — genau der §8o-Fehler, der \
         am Rippen-Kamm 144 Punkte Phantom-Material erzeugte"
    );
    assert!(g - d0 > l - delta, "die Sonde misst den falschen Fall");
    // (2) L da und GRÖSSER als die G-Kappe (Stufen-Nabe) → L gewinnt auch da.
    assert_eq!(concave_gate(g, 2.884f32, 2.3156 - delta), 2.3156 - delta);
    // (3) kein L → G-Kappe, exakt wie vor §8p.
    let none = crate::offset_lb::LB_NONE - delta;
    assert!(none < LB_AVAIL, "der Sentinel muss unter der Schwelle liegen");
    assert_eq!(concave_gate(g, d0, none), g - d0);
    assert_eq!(concave_gate(g, -3.5, none), g + 3.5);
    // (3b) und keine Mantissen-Löschung: der Sentinel darf das Ergebnis nicht
    // um ein einziges ULP verschieben.
    assert_eq!(concave_gate(g, d0, none).to_bits(), (g - d0).to_bits());
}

/// **§8p-Satz-Pin: `f ≥ min(d₀, δ − L, 0)`, gemessen statt geglaubt.**
///
/// Der Satz aus [`crate::offset_channel::concave_gate`] listet genau drei
/// untere Schranken für den Stoppwert, eine je Term-Zweig: `d₀` (kein Term
/// bindet), `δ − L` (der addierende Zweig, gekappt vom Gate) und `0` (die
/// Luftseiten-Klemme `min(d, −w_geo)`, die nie unter Null geht). Der Stoppwert
/// ist ein `min` über diese drei.
///
/// **Die Über-Seiten-Leere ist die Folgerung, nicht die Ungleichung selbst:**
/// ist `dist < δ`, so sind alle drei Schranken `≥ 0` (`d₀ ≥ δ − dist > 0`,
/// `δ − L ≥ δ − dist > 0`), also `f ≥ 0` — kein Phantom-Material. Umgekehrt
/// darf `f` sehr wohl unter `δ − L` liegen, wo `L < δ` ist: dort greift die
/// Luftseiten-Klemme, und die ist absichtlich pessimistisch (march-sicher).
/// Eine erste Fassung dieses Pins forderte `f ≥ δ − L` und schlug prompt fehl —
/// die Klemme, nicht das Gate, war der Grund.
///
/// Der Pin prüft die Ungleichung direkt am Feld — an der Fixture, an der das
/// Gate 144 Punkte gerettet hat.
#[test]
fn field_never_undercuts_the_lower_bound_384() {
    let delta = 2.0f32;
    let (node, _boxes, span) = rib_comb_node(4, 2.0, 1.0, delta);
    let (field, _) = compile_for(node);
    let mut bs = Vec::new();
    collect_lb(&field, &mut bs);
    assert_eq!(bs.len(), 1, "der Kamm 4×2.0/1.0 muss GENAU EINEN L-Block tragen");
    let blk = &bs[0];
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    assert_eq!(stops.len(), 1);
    let BoundedCsg::ArcChannel { inner, .. } = stops[0] else { unreachable!() };
    let (mut worst, mut checked) = (0.0f32, 0usize);
    let n = 22usize;
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    (-span + 2.0 * span * (ix as f64 + 0.2113248654) / n as f64) as f32,
                    (-3.0 + 6.0 * (iy as f64 + 0.5773502692) / n as f64) as f32,
                    (-6.0 + 6.5 * (iz as f64 + 0.7886751346) / n as f64) as f32,
                ];
                let d0 = inner.sdf(p, EvalMode::Visual);
                let l = crate::offset_lb::lb_value(p, d0, blk);
                if l <= -1.0e29 {
                    continue; // ausserhalb Ω oder ausserhalb der Term-Kugel
                }
                checked += 1;
                let bound = d0.min(delta - l).min(0.0);
                worst = worst.max(bound - field.sdf(p, EvalMode::Visual));
            }
        }
    }
    assert!(checked > 500, "zu wenige Punkte mit gültigem L: {checked}");
    assert!(
        worst <= 1e-4,
        "das Feld unterschreitet min(d₀, δ − L, 0) um {worst:.3e} mm an {checked} geprüften \
         Punkten — der §8p-Satz gilt im Feld nicht, und damit ist die Über-Seiten-Zusage keine"
    );
}

// ══════════════ #384 §8p — konvexe Extrude-Member im `L`-Block ══════════════

/// Ein L-förmiges (NICHT konvexes) Extrude-Profil als Union-Member.
fn l_shape_extrude(fid: u32, hh: f32) -> CsgNode {
    let v = [
        [0.0f32, 0.0],
        [6.0, 0.0],
        [6.0, 2.0],
        [2.0, 2.0],
        [2.0, 6.0],
        [0.0, 6.0],
    ];
    let segments: Vec<SketchSegment> = (0..v.len())
        .map(|i| SketchSegment::Line { start: v[i], end: v[(i + 1) % v.len()] })
        .collect();
    CsgNode::Extrude {
        id: id(fid),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Path { segments, closed: true })),
        half_height: hh,
        wall_subs: None,
    }
}

/// **§8p-Sicherheits-Pin: ein NICHT-konvexes Extrude-Profil verweigert den
/// ganzen Block.**
///
/// Die Austritts-Aufzählung von `LbSolid::Poly` setzt
/// `int P = ⋂{n_i·q < c_i}` voraus. Für ein L-Profil ist der Schnitt der sechs
/// Facetten-Halbräume die konvexe HÜLLE, also echt grösser als `P` — ein
/// Randstück im fehlenden Quadranten läge im Schnitt und würde für vergraben
/// gehalten. Es fiele aus dem `min`, `L` überschätzte die Distanz, und das
/// §8p-Gate (das jetzt ALLEIN auf `L` steht) erlaubte Phantom-Material. Der
/// einzige sichere Ausgang ist die Verweigerung des GANZEN Blocks — dann
/// bindet wie vor §8p die `G`-Kappe.
#[test]
fn non_convex_extrude_member_refuses_the_lb_block_384() {
    let delta = 2.0f32;
    let node = CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(box_at(1, [0.0, 0.0, -6.0], [20.0, 20.0, 6.0])),
            Box::new(l_shape_extrude(2, 5.0)),
        )),
        -delta,
    );
    let (field, _) = compile_for(node);
    let mut blocks = Vec::new();
    collect_lb(&field, &mut blocks);
    assert!(
        blocks.is_empty(),
        "ein L-förmiges Extrude-Profil hat einen L-Block bekommen ({} Stücke) — die \
         Konvexitäts-Prüfung greift nicht, und `⋂{{h_i ≤ 0}} ⊋ P` macht jedes \
         Vergraben-Urteil ungültig",
        blocks.first().map_or(0, |b| b.pieces.len())
    );
    // Gegenprobe, damit der Pin nicht bloss „Extrudes werden generell
    // verweigert" misst: derselbe Syntheseweg baut sehr wohl einen Block, wenn
    // die Extrude-Member KONVEX sind — der Rippen-Kamm 4×1.5/0.8 trägt drei
    // konvexe Kontakt-Prismen und bekommt 296 Stücke
    // (`lb_block_covers_convex_extrude_members_384` misst dort zusätzlich die
    // Sicherheits-Ungleichung).
    let (comb, _b, _s) = rib_comb_node(4, 1.5, 0.8, delta);
    let (cf, _) = compile_for(comb);
    let mut cb = Vec::new();
    collect_lb(&cf, &mut cb);
    assert_eq!(
        cb.len(),
        1,
        "der Kamm mit KONVEXEN Extrude-Prismen bekommt keinen Block — dann prüft die \
         Verweigerung oben nur, dass Extrudes generell scheitern"
    );
}

/// **§8p-Sicherheits-Pin: der Block mit Extrude-Membern lügt nicht.**
///
/// Der Rippen-Kamm 4×1.5/0.8 trägt acht Union-Member — fünf Boxen und drei
/// Extrude-Kontaktprismen. Seit §8p baut die Synthese daraus einen Block; die
/// eine Ungleichung, an der alles hängt, ist unverändert `L ≤ dist(x,∁Ω)`.
/// Überschätzt `L` die Distanz, öffnet das Gate (das seit §8p ALLEIN auf `L`
/// steht) weiter, als die Geometrie erlaubt.
///
/// Referenz ist die exakte Zerlegung über die fünf BOXEN. Die Prismen liegen
/// in deren Vereinigung (`prism_subset_of_union`, geprüft bei der Synthese)
/// und ragen nur um die f32-Rundung ihrer Kanten hinaus (gemessen: 5·10⁻⁷ mm),
/// also ist die Box-Referenz bis auf diese Grössenordnung auch die Referenz
/// des Feld-Körpers.
#[test]
fn lb_block_covers_convex_extrude_members_384() {
    let delta = 2.0f32;
    let (node, boxes, span) = rib_comb_node(4, 1.5, 0.8, delta);
    let (field, _) = compile_for(node);
    let mut blocks = Vec::new();
    collect_lb(&field, &mut blocks);
    assert_eq!(
        blocks.len(),
        1,
        "der Kamm 4×1.5/0.8 muss seit §8p GENAU EINEN L-Block tragen (fünf Boxen + drei \
         konvexe Extrude-Prismen sind darstellbar) — sonst misst dieser Pin nichts"
    );
    let blk = &blocks[0];
    assert!(
        blk.pieces.len() > 200,
        "nur {} Stücke — acht Member liefern deutlich mehr; ein zu kleiner Satz heisst \
         fehlende Randstücke",
        blk.pieces.len()
    );
    let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let BoundedCsg::ArcChannel { inner, .. } = stops[0] else { unreachable!() };
    let (mut worst, mut checked) = (0.0f64, 0usize);
    let n = 20usize;
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let p = [
                    -span + 2.0 * span * (ix as f64 + 0.2113248654) / n as f64,
                    -3.0 + 6.0 * (iy as f64 + 0.5773502692) / n as f64,
                    -6.0 + 6.5 * (iz as f64 + 0.7886751346) / n as f64,
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let d0 = inner.sdf(pf, EvalMode::Visual);
                let l = crate::offset_lb::lb_value(pf, d0, blk) as f64;
                if l <= -1.0e29 {
                    continue;
                }
                checked += 1;
                worst = worst.max(l - dist_obb_union_complement_exact(&obbs, p));
            }
        }
    }
    assert!(checked > 500, "zu wenige Punkte mit gültigem L: {checked}");
    assert!(
        worst <= 1e-3,
        "die Stück-Schranke übersteigt die exakte Distanz um {worst:.3e} mm an {checked} \
         Punkten — sie ERFINDET Distanz, und das §8p-Gate steht allein auf ihr"
    );
}

/// **§8q-Inventar (einmalig): Zusage gegen Wirklichkeit, Fixture für Fixture.**
///
/// Druckt je Fixture den Status, die daraus abgeleitete zugesagte Schranke
/// ([`promised_bound`]), die vollständigen Report-Gründe, die zweiseitige
/// Messung und die `L`-Blöcke des Baums. Nur so ist entscheidbar, welche
/// Zusage heute STUMPFER ist als das ausgelieferte Feld.
#[test]
#[ignore]
fn probe_384_promise_inventory() {
    let delta = 2.0f32;
    let mut rows: Vec<(String, TwoSided, CapabilityReport, Vec<usize>)> = Vec::new();
    let lb_of = |field: &BoundedCsg| {
        let mut b = Vec::new();
        collect_lb(field, &mut b);
        b.iter().map(|x| x.pieces.len()).collect::<Vec<_>>()
    };
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, report) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        let r = two_sided_scan(
            &field, delta, [-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5], 22,
            &|p| dist_hub_complement_exact(t as f64, &cyls, p),
            &|p| dist_hub_complement(t as f64, &cyls, p, 160),
        );
        let l = lb_of(&field);
        rows.push(("Stufen-Nabe".into(), r, report, l));
    }
    for tf in [2.0f32, 2.1, 2.25, 2.4, 2.5, 3.0, 4.0, 6.0] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field, delta, [lo; 3], [1.0; 3], 22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        let l = lb_of(&field);
        rows.push((format!("Ecken-Schale t/d={tf}"), r, report, l));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, report) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let r = two_sided_scan(
            &field, delta, [-span, -3.0, -6.0], [span, 3.0, 0.5], 24,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        let l = lb_of(&field);
        rows.push((format!("Rippen-Kamm 4x{w}/{g}"), r, report, l));
    }
    for alpha in [90.0f64, 60.0] {
        let (field, report) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let r = two_sided_scan(
            &field, delta, [-10.0, -10.0, 42.0], [10.0, 10.0, 54.0], 22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_notch_complement(alpha, p),
        );
        let l = lb_of(&field);
        rows.push((format!("notch a={alpha}"), r, report, l));
    }
    // Die Kaskaden-Pins messen auf FEINEREN Gittern (26³ Nabe, 28³ Kamm) und
    // nur gegen die Strahl-Schranke. Eine geschärfte Zusage muss auch DORT
    // halten — deshalb hier dieselbe Rechnung.
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        let n = 26usize;
        let (mut miss, mut worst) = (0usize, 0.0f64);
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let p = [
                        -9.0 + 18.0 * (ix as f64 + 0.2113248654) / n as f64,
                        -9.0 + 18.0 * (iy as f64 + 0.5773502692) / n as f64,
                        -(t as f64) - 1.5
                            + (t as f64 + 2.0) * (iz as f64 + 0.7886751346) / n as f64,
                    ];
                    if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                        continue;
                    }
                    let err = delta as f64 - dist_hub_complement(t as f64, &cyls, p, 160);
                    if err > 1e-3 {
                        miss += 1;
                        worst = worst.max(err);
                    }
                }
            }
        }
        println!("\n## FEIN Stufen-Nabe 26^3: miss={miss} worst={worst:.4}");
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, _) = compile_for(node);
        let n = 28usize;
        let (mut miss, mut worst) = (0usize, 0.0f64);
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let p = [
                        -span + 2.0 * span * (ix as f64 + 0.2113248654) / n as f64,
                        -3.0 + 6.0 * (iy as f64 + 0.5773502692) / n as f64,
                        -6.0 + 6.5 * (iz as f64 + 0.7886751346) / n as f64,
                    ];
                    if field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) >= 0.0 {
                        continue;
                    }
                    let err = delta as f64 - dist_to_boxes_complement(&boxes, p, 160);
                    if err > 1e-3 {
                        miss += 1;
                        worst = worst.max(err);
                    }
                }
            }
        }
        println!("## FEIN Rippen-Kamm 4x{w}/{g} 28^3: miss={miss} worst={worst:.4}");
    }
    println!("\n=== §8q INVENTAR ===");
    for (name, r, report, l) in &rows {
        println!(
            "\n## {name}\n  status={:?} promised={:?} L-Bloecke={:?}\n  ueber={} / {:.4}  unter={} / {:.4}",
            report.status(),
            promised_bound(report),
            l,
            r.over.0, r.over.1, r.under.0, r.under.1
        );
        for s in report.reasons() {
            println!("  REASON: {s}");
        }
    }
}

// ─── #384 §8q: die Zusage sieht `L` ──────────────────────────────────────────
//
// Bis §8p rief der Randpunkt-Zeuge `stop_apply(…, None)` — der `L`-Block
// entstand erst NACH dem Audit. Die Zusage war dadurch nie falsch (`G` ist die
// SCHWÄCHERE Aussage), aber am Rippen-Kamm 87-fach stumpfer als das Feld:
//
// | Fixture | Zusage vorher | Zusage nachher | zweiseitig gemessen | `L`-Stücke |
// |---|---|---|---|---|
// | Stufen-Nabe | ≤ 0.010 | ≤ 0.010 | 0 / 0 | 15 |
// | Ecken-Schale (8 Dicken) | ≤ 0.010 | ≤ 0.010 | 0 / 0 | 72 |
// | Rippen-Kamm 4×1.5/0.8 | **≤ 0.868** | **≤ 0.010** | 0 / 0 | 296 |
// | Rippen-Kamm 4×2.0/1.0 | **≤ 0.868** | **≤ 0.010** | 0 / 0 | 140 |
// | notch 90° / 60° | ≤ 0.010 | ≤ 0.010 | 0 / 0 | 28 |

/// Alle Fixturen der §8q-Matrix als (Name, Feld, Report, zweiseitige Messung).
fn promise_matrix(delta: f32) -> Vec<(String, BoundedCsg, CapabilityReport, TwoSided)> {
    let mut out: Vec<(String, BoundedCsg, CapabilityReport, TwoSided)> = Vec::new();
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, report) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        let r = two_sided_scan(
            &field, delta, [-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5], 22,
            &|p| dist_hub_complement_exact(t as f64, &cyls, p),
            &|p| dist_hub_complement(t as f64, &cyls, p, 160),
        );
        out.push(("Stufen-Nabe".into(), field, report, r));
    }
    for tf in [2.0f32, 2.25, 3.0, 6.0] {
        let t = tf * delta;
        let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        let r = two_sided_scan(
            &field, delta, [lo; 3], [1.0; 3], 22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        out.push((format!("Ecken-Schale t/δ={tf}"), field, report, r));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, report) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let r = two_sided_scan(
            &field, delta, [-span, -3.0, -6.0], [span, 3.0, 0.5], 24,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_boxes_complement(&boxes, p, 160),
        );
        out.push((format!("Rippen-Kamm 4×{w}/{g}"), field, report, r));
    }
    for alpha in [90.0f64, 60.0] {
        let (field, report) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        let r = two_sided_scan(
            &field, delta, [-10.0, -10.0, 42.0], [10.0, 10.0, 54.0], 22,
            &|p| dist_obb_union_complement_exact(&obbs, p),
            &|p| dist_to_notch_complement(alpha, p),
        );
        out.push((format!("notch α={alpha}"), field, report, r));
    }
    out
}

/// **§8q-Kern-Pin: die Zusage ist die Wirklichkeit — in BEIDE Richtungen.**
///
/// Der bestehende Pin `erosion_promise_names_the_side_it_covers_384` prüft nur
/// die eine Hälfte des Vertrags: die zugesagte Schranke darf nicht ÜBERSCHRITTEN
/// werden. Das ist gegen eine Lüge gerichtet — und gegen Stumpfheit blind: eine
/// Zusage `≤ 1000 mm` bestünde ihn an jeder Fixture. Genau in dieser Lücke saß
/// der Rippen-Kamm: gemeldet `≤ 0.868 mm`, gemessen NULL Phantom-Punkte, weil
/// der Zeuge das Feld ohne `L` sah.
///
/// Dieser Pin schließt sie: an jeder Fixture, die einen `L`-Block trägt, MUSS
/// die Zusage die enge sein (Ziel-Toleranz-Klasse), nicht die `G`-Kappe. Er
/// wäre vor §8q an beiden Kämmen mit `0.868` gefallen.
#[test]
fn promise_is_as_sharp_as_the_field_384() {
    let delta = 2.0f32;
    let tol_class = CHANNEL_TOL_RENDER as f64 + 1e-9;
    for (name, field, report, r) in promise_matrix(delta) {
        let mut blocks = Vec::new();
        collect_lb(&field, &mut blocks);
        let b = promised_bound(&report).unwrap_or_else(|| {
            panic!("{name}: der Stopp sagt gar nichts mehr zu. Reasons: {:?}", report.reasons())
        });
        // (1) Die Zusage muss WAHR sein — die Über-Seite hält sie ein.
        assert!(
            r.over.1 <= b + 1e-3,
            "{name}: {} Punkte Phantom-Material, größtes {:.4} mm bei {:?} — zugesagt sind aber \
             nur {b:.3} mm. Reasons: {:?}",
            r.over.0, r.over.1, r.over.2, report.reasons()
        );
        // (2) Und sie darf nicht grob STUMPFER sein als das Feld. Wo eine
        //     Stück-Schranke im Baum liegt, gatet seit §8p `L` allein, der
        //     §8p-Satz schließt Phantom-Material strukturell aus — dann ist die
        //     `G`-Kappe als Zusage schlicht die falsche Zahl.
        if !blocks.is_empty() {
            assert!(
                b <= tol_class,
                "{name}: {} L-Block/Blöcke mit {:?} Randstücken im Baum, gemessen {} Punkte / \
                 {:.4} mm Phantom-Material — aber zugesagt werden {b:.3} mm statt der \
                 Ziel-Toleranz. Genau so sah es aus, als der Zeuge `stop_apply(…, None)` rief. \
                 Reasons: {:?}",
                blocks.len(),
                blocks.iter().map(|x| x.pieces.len()).collect::<Vec<_>>(),
                r.over.0, r.over.1, report.reasons()
            );
            assert!(
                report.reasons().iter().any(|s| s.contains("Stück-Schranke L")),
                "{name}: der Baum trägt eine Stück-Schranke, die Zusage nennt sie aber nicht — \
                 dann steht dort noch die tote `G`-Kappen-Begründung. Reasons: {:?}",
                report.reasons()
            );
        }
        // (3) Die Unter-Seite bleibt ausdrücklich ausgenommen, solange sie es
        //     ist: für sie gibt es weiterhin keinen Satz, nur Messungen.
        assert!(
            report
                .reasons()
                .iter()
                .any(|s| s.contains("FEHLENDES Material") || s.contains("keine Schranke")),
            "{name}: der Report muss die Unter-Seite ausdrücklich ausnehmen, sonst liest sich \
             die Zusage zweiseitig. Reasons: {:?}",
            report.reasons()
        );
        assert!(
            r.under.0 == 0,
            "{name}: {} Punkte FEHLENDES Material, größtes {:.4} mm bei {:?} — die Matrix ist \
             seit §8p beidseitig leer",
            r.under.0, r.under.1, r.under.2
        );
    }
}

/// **§8q-Lockstep-Pin: Zeuge und Laufzeit sehen DASSELBE `L`.**
///
/// Der Zeuge darf nichts validieren, was die Laufzeit nicht liefert. Die
/// Absicherung ist keine Ähnlichkeits-Prüfung zweier Nachbildungen, sondern
/// **Identität**: [`crate::offset_channel::stop_lb_block`] baut den Block
/// EINMAL, vor dem Audit; das Audit bekommt ihn als `lb.as_ref()`, und derselbe
/// `Option` wandert unverändert in den `ArcChannel`-Knoten. Damit teilen sich
/// beide Seiten automatisch jede Klemme: Innen-Indikator `d₀ ≤ δ`,
/// Fernfeld-Kugel, `slack`, [`crate::offset_channel::LB_AVAIL`]-Schwelle.
///
/// Testbar ist die Identität so: der Block IM Baum muss bit-genau das sein, was
/// die Produktions-Synthese aus dem AUSGELIEFERTEN Baum ergibt. Baut irgendwer
/// je eine zweite Fassung (andere Quelle, andere Kugel, andere Reihenfolge),
/// fällt dieser Pin — und mit ihm die Grundlage jeder Zusage, die auf `L` steht.
#[test]
fn witness_and_runtime_share_the_same_lower_bound_384() {
    let delta = 2.0f32;
    let mut checked = 0usize;
    for (name, field, _report, _) in promise_matrix(delta) {
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        for st in stops {
            let BoundedCsg::ArcChannel { inner, channels, verts, lb, .. } = st else {
                continue;
            };
            let BoundedCsg::Offset { inner: src, delta: d, .. } = &**inner else {
                continue;
            };
            // Der ausgelieferte Baum trägt die Kontakt-Prismen bereits; deshalb
            // hier die leere Prisma-Liste — `attach_prisms` ist dann die
            // Identität, und übrig bleibt exakt der Aufruf, den das Audit sah.
            let rebuilt = crate::offset_channel::stop_lb_block(src, &[], channels, verts, *d);
            match (lb, &rebuilt) {
                (None, None) => {}
                (Some(a), Some(b)) => {
                    assert_eq!(
                        a.pieces.len(),
                        b.pieces.len(),
                        "{name}: der Baum trägt {} Randstücke, die Synthese liefert {} — \
                         Zeuge und Laufzeit sehen verschiedene Felder",
                        a.pieces.len(),
                        b.pieces.len()
                    );
                    for (i, (pa, pb)) in a.pieces.iter().zip(b.pieces.iter()).enumerate() {
                        assert_eq!(
                            pa.to_words().map(f32::to_bits),
                            pb.to_words().map(f32::to_bits),
                            "{name}: Randstück {i} weicht ab"
                        );
                    }
                    assert_eq!(
                        [a.delta, a.slack, a.center[0], a.center[1], a.center[2], a.skip_r]
                            .map(f32::to_bits),
                        [b.delta, b.slack, b.center[0], b.center[1], b.center[2], b.skip_r]
                            .map(f32::to_bits),
                        "{name}: Kopf des L-Blocks weicht ab (δ / slack / Fernfeld-Kugel) — \
                         genau die Klemmen, die in Lockstep sein müssen"
                    );
                    checked += 1;
                }
                (a, b) => panic!(
                    "{name}: Baum hat L={}, Synthese liefert L={} — die eine Seite gatet mit \
                     einer Schranke, die die andere nicht kennt",
                    a.is_some(),
                    b.is_some()
                ),
            }
        }
    }
    assert!(
        checked >= 8,
        "nur {checked} Stopps mit L-Block geprüft — der Pin misst nichts"
    );
}

// ══════════════ #384 §8r — der Unter-Claim-Zeuge ══════════════
//
// **Die Vorzeichen, hergeleitet statt vermutet.** Die Harness misst die
// NULLMENGE (s. [`two_sided_scan`]):
//
//     Unter-Claim bei x  ⟺  f(x) ≥ 0  ∧  dist(x,∁Ω) − δ > tol
//     Über-Claim  bei x  ⟺  f(x) < 0  ∧  δ − dist(x,∁Ω) > tol
//
// `L` ist eine UNTERE Schranke von `dist(x,∁Ω)` (§8o). Damit gilt:
//
// * `L ≥ δ` beweist „x liegt tief genug" — das stützt die ÜBER-Seite (und
//   genau das ist der §8p-Satz, nur strukturell statt punktweise).
// * Für die ABWESENHEIT eines Unter-Claims braucht man das Gegenteil, eine
//   OBERE Schranke `U ≥ dist(x,∁Ω)`: nur sie kann `dist ≤ δ + tol` belegen.
//   **`L` kann das nicht — in keiner Umformung.**
// * Was `L` dagegen kann, ist die ANWESENHEIT beweisen: `f(x) ≥ 0 ∧
//   L(x) > δ + tol` ⇒ `dist ≥ L > δ + tol` ⇒ der Punkt MUSS Material sein,
//   das Feld behauptet Luft. Das ist ein Zeuge im Wortsinn — seine Aussage
//   ist ein Beweis, sein Schweigen keiner.
//
// Die obere Schranke liefert der **Fuss-Zeuge**: `lb_foot` gibt den Fusspunkt
// `q*` des `min`-gewinnenden Randstücks; um `CERT_PUSH` von `x` weg geschoben
// und im naiven Vereinigungsfeld als AUSSEN nachgewiesen (`d₀(q_out) > δ`, der
// SELBE Innen-Indikator, den `lb_value` benutzt), ist `q_out ∈ ∁Ω` und damit
// `U = |x − q_out| ≥ dist(x,∁Ω)`. Schlägt der Nachweis fehl, ist das genau der
// Fall „das Stück ist von einem anderen Member verschluckt" — dort ist `L`
// stumpf, und es gibt kein Zertifikat.

/// Toleranz-Schwelle der Nullmengen-Messung — identisch zu [`two_sided_scan`].
const CERT_TOL: f64 = 1e-3;
/// Ausschub des Fuss-Zeugen aus `∂Ω` heraus (mm). Er geht 1:1 in die Breite
/// des unentschiedenen Bandes ein und muss deutlich unter [`CERT_TOL`] bleiben.
const CERT_PUSH: f64 = 2e-4;
/// Mindestwert des naiven Vereinigungsfeldes am ausgeschobenen Punkt, damit er
/// BEWEISBAR ausserhalb `Ω` liegt. Das f32-Rauschen der Auswertung liegt eine
/// Zehnerpotenz darunter; ein zu kleiner Wert kostet nur Abdeckung, ein zu
/// grosser wäre der eine tödliche Fehler.
const CERT_MARGIN: f32 = 5e-5;

/// Das Unter-Claim-Zertifikat an EINEM Punkt.
#[derive(Debug, Clone, Copy, PartialEq)]
enum UnderCert {
    /// Das Feld behauptet Material — fehlendes Material ist per Definition aus.
    FieldSolid,
    /// `d₀ > δ` ⇒ `min_k f_k > 0` ⇒ `x ∉ Ω` ⇒ `dist(x,∁Ω) = 0`.
    OutsideOmega,
    /// Fuss-Zeuge auf dem `min`-Stück: `U ≤ δ + tol`, der Punkt gehört
    /// nachweislich nicht in die Erosion.
    Foot,
    /// Der Aussen-Punkt kam aus dem Newton-Marsch ([`march_outside`]).
    March,
    /// `L > δ + tol` bei stehendem Feld — BEWIESENER Unter-Claim.
    Proven,
    /// `L` neutral: kein Block, oder ausserhalb der Fernfeld-Kugel.
    NoLb,
    /// Der Fusspunkt liegt NICHT nachweisbar in `∁Ω` (Stück verschluckt) —
    /// dort ist `L` stumpf.
    FootSwallowed,
    /// `L ≤ δ + tol < U`: das Band der Breite `slack + CERT_PUSH` um
    /// `dist = δ + tol`.
    Band,
}

/// Der duale Stopp eines Fixture-Baums: der `Offset`-Knoten (er liefert den
/// naiven Stoppwert `d₀`) und der `L`-Block darüber.
fn stop_with_lb(field: &BoundedCsg) -> (&BoundedCsg, &crate::offset_lb::LbBlock) {
    let mut stops = Vec::new();
    stop_nodes(field, &mut stops);
    for st in stops {
        if let BoundedCsg::ArcChannel { inner, lb: Some(b), .. } = st {
            return (inner.as_ref(), b);
        }
    }
    panic!("Fixture ohne L-Block — das Zertifikat hat dort nichts zu messen");
}

/// **Rigoroser Aussen-Zeuge per gedämpftem Newton-Marsch.** Gesucht wird ein
/// Punkt `y` mit `f_inner(y) > CERT_MARGIN` — der liegt BEWEISBAR in `∁Ω`
/// (Vorzeichen-Invariante jeder Leaf-SDF), also ist `|x − y|` eine obere
/// Schranke von `dist(x,∁Ω)`. Anders als [`crate::offset_channel`]s
/// `boundary_witness` wird hier NICHT `|f(y)|` aufaddiert: das wäre nur für
/// `f(y) ≥ 0` gültig, und der Marsch endet dort sonst knapp INNEN.
///
/// Der Marsch ist eine reine Schärfe-Frage — jeder gefundene Punkt ist gültig,
/// ein nicht gefundener kostet nur Abdeckung.
fn march_outside(stop: &BoundedCsg, delta: f32, start: [f64; 3], reach: f64) -> Option<[f64; 3]> {
    let h = 1e-3f32;
    let mut y = start;
    for _ in 0..24 {
        let yf = [y[0] as f32, y[1] as f32, y[2] as f32];
        let v = stop.sdf(yf, EvalMode::Visual) - delta;
        if !v.is_finite() {
            return None;
        }
        if v > CERT_MARGIN {
            return Some(y);
        }
        let g = [
            stop.sdf([yf[0] + h, yf[1], yf[2]], EvalMode::Visual)
                - stop.sdf([yf[0] - h, yf[1], yf[2]], EvalMode::Visual),
            stop.sdf([yf[0], yf[1] + h, yf[2]], EvalMode::Visual)
                - stop.sdf([yf[0], yf[1] - h, yf[2]], EvalMode::Visual),
            stop.sdf([yf[0], yf[1], yf[2] + h], EvalMode::Visual)
                - stop.sdf([yf[0], yf[1], yf[2] - h], EvalMode::Visual),
        ];
        let n = ((g[0] * g[0] + g[1] * g[1] + g[2] * g[2]) as f64).sqrt();
        if !(n > 1e-12) {
            return None;
        }
        let step = (-v as f64 + 4.0 * CERT_MARGIN as f64) * 2.0 * h as f64 / n;
        for k in 0..3 {
            y[k] += g[k] as f64 / n * step;
        }
        let d = ((y[0] - start[0]).powi(2) + (y[1] - start[1]).powi(2) + (y[2] - start[2]).powi(2))
            .sqrt();
        if d > reach {
            return None;
        }
    }
    None
}

/// Das Zertifikat: Klassifikation plus `L` und (falls belegt) `U`.
fn under_cert(
    stop: &BoundedCsg,
    blk: &crate::offset_lb::LbBlock,
    delta: f32,
    f: f32,
    p: [f64; 3],
) -> (UnderCert, Option<f64>, Option<f64>) {
    if f < 0.0 {
        return (UnderCert::FieldSolid, None, None);
    }
    let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
    let d0 = stop.sdf(pf, EvalMode::Visual);
    if d0 > delta {
        return (UnderCert::OutsideOmega, None, Some(0.0));
    }
    let l = crate::offset_lb::lb_value(pf, d0, blk);
    if l <= crate::offset_channel::LB_AVAIL {
        return (UnderCert::NoLb, None, None);
    }
    let l64 = l as f64;
    if l64 > delta as f64 + CERT_TOL {
        return (UnderCert::Proven, Some(l64), None);
    }
    // Die Schranke, die zu schlagen ist. Alles darüber zertifiziert nichts,
    // also darf jede Suche dort abbrechen.
    let cap = delta as f64 + CERT_TOL;
    let dist_to = |y: [f64; 3]| {
        ((y[0] - p[0]).powi(2) + (y[1] - p[1]).powi(2) + (y[2] - p[2]).powi(2)).sqrt()
    };
    // Ein Stück-Fusspunkt, um `CERT_PUSH` von `x` weg geschoben.
    let pushed = |q: [f32; 3]| -> Option<[f64; 3]> {
        let dir = [q[0] as f64 - p[0], q[1] as f64 - p[1], q[2] as f64 - p[2]];
        let n = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        (n > 1e-9).then(|| {
            [
                q[0] as f64 + CERT_PUSH * dir[0] / n,
                q[1] as f64 + CERT_PUSH * dir[1] / n,
                q[2] as f64 + CERT_PUSH * dir[2] / n,
            ]
        })
    };
    let is_outside = |y: [f64; 3]| {
        stop.sdf([y[0] as f32, y[1] as f32, y[2] as f32], EvalMode::Visual) > delta + CERT_MARGIN
    };
    let mut best: Option<f64> = None;
    // (1) Der Fusspunkt des `min`-Stücks — der billigste und schärfste Fall.
    let foot = crate::offset_lb::lb_foot(pf, d0, blk);
    if let Some(y) = foot.and_then(pushed).filter(|y| is_outside(*y)) {
        let u = dist_to(y);
        if u <= cap {
            return (UnderCert::Foot, Some(l64), Some(u));
        }
        best = Some(u);
    }
    // (2) Ist er verschluckt, liegt der wahre Randpunkt oft auf DEMSELBEN
    //     Stück, nur ausserhalb des verschluckten Teils — ein Marsch vom
    //     Fusspunkt aus findet ihn, ein Stück-Wechsel nicht.
    for start in [foot.map(|q| [q[0] as f64, q[1] as f64, q[2] as f64]), Some(p)]
        .into_iter()
        .flatten()
    {
        if let Some(y) = march_outside(stop, delta, start, 2.0 * cap) {
            let u = dist_to(y);
            if u <= cap {
                return (UnderCert::March, Some(l64), Some(u));
            }
            best = Some(best.map_or(u, |b: f64| b.min(u)));
        }
    }
    // Ein dritter Zweig — die ganze Stück-Liste in wachsender Distanz
    // abzuklappern — war an keiner der dreizehn Fixturen je der entscheidende:
    // was der `min`-Fusspunkt nicht schafft, holt der Marsch. Er steht deshalb
    // nicht im Code.
    match best {
        Some(u) => (UnderCert::Band, Some(l64), Some(u)),
        None => (UnderCert::FootSwallowed, Some(l64), None),
    }
}

/// Abdeckungs-Zeile einer Fixture.
#[derive(Default, Clone)]
struct CertRow {
    name: String,
    pts: usize,
    /// Die RISIKO-Menge: `f ≥ 0` UND `x ∈ Ω` — nur dort kann fehlendes
    /// Material überhaupt sitzen.
    risk: usize,
    foot: usize,
    march: usize,
    proven: usize,
    no_lb: usize,
    swallowed: usize,
    band: usize,
    field_solid: usize,
    outside: usize,
    /// Grösste Verletzung `L − exakt` (muss `≤ 0` bleiben).
    l_viol: f64,
    /// Grösste Verletzung `exakt − U` (muss `≤ 0` bleiben).
    u_viol: f64,
    /// Grösste Klammer-Weite `U − L`, wo beide da sind.
    bracket: f64,
    /// Der `slack` des Blocks — die Klammer kann nicht enger werden als
    /// `slack + CERT_PUSH`.
    slack: f64,
}

impl CertRow {
    fn certified(&self) -> usize {
        self.foot + self.march
    }
    fn open(&self) -> usize {
        self.no_lb + self.swallowed + self.band
    }
}

type ExactFn = Box<dyn Fn([f64; 3]) -> f64>;
/// Eine Zeile der Zertifikats-Matrix: Name, Feld, exakte Referenz, Gitter.
type CertFixture = (String, BoundedCsg, ExactFn, [f64; 3], [f64; 3], usize);

/// Die volle §8p-Fixture-Matrix mit exakter Komplement-Referenz, Gitter und
/// Grenzen — dieselben Fixtures, dieselben Gitter wie im Doktrin-Pin.
fn cert_matrix(delta: f32) -> Vec<CertFixture> {
    let mut out: Vec<CertFixture> = Vec::new();
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
        out.push((
            "Stufen-Nabe".into(),
            field,
            Box::new(move |p| dist_hub_complement_exact(t as f64, &cyls, p)),
            [-9.0, -9.0, -(t as f64) - 1.5],
            [9.0, 9.0, 0.5],
            22,
        ));
    }
    for tf in [2.0f32, 2.1, 2.25, 2.4, 2.5, 3.0, 4.0, 6.0] {
        let t = tf * delta;
        let (field, _) = compile_for(thin_corner_node(t, 40.0, delta));
        let boxes = thin_corner_boxes(t as f64, 40.0);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        let lo = -(t as f64 + delta as f64 + 1.0);
        out.push((
            format!("Ecken-Schale t/δ={tf}"),
            field,
            Box::new(move |p| dist_obb_union_complement_exact(&obbs, p)),
            [lo; 3],
            [1.0; 3],
            22,
        ));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        let (node, boxes, span) = rib_comb_node(4, w, g, delta);
        let (field, _) = compile_for(node);
        let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
        out.push((
            format!("Rippen-Kamm 4×{w}/{g}"),
            field,
            Box::new(move |p| dist_obb_union_complement_exact(&obbs, p)),
            [-span, -3.0, -6.0],
            [span, 3.0, 0.5],
            24,
        ));
    }
    for alpha in [90.0f64, 60.0] {
        let (field, _) = compile_for(notch_union_node(alpha, delta as f64));
        let gamma = std::f64::consts::PI - alpha.to_radians();
        let obbs = vec![
            obb_rot_z([0.0, -50.0, 0.0], [100.0, 50.0, 50.0], 0.0),
            obb_rot_z([50.0 * gamma.sin(), -50.0 * gamma.cos(), 0.0], [100.0, 50.0, 50.0], gamma),
        ];
        out.push((
            format!("notch α={alpha}"),
            field,
            Box::new(move |p| dist_obb_union_complement_exact(&obbs, p)),
            [-10.0, -10.0, 42.0],
            [10.0, 10.0, 54.0],
            22,
        ));
    }
    out
}

/// Ein Gitterlauf des Zertifikats über eine Fixture.
#[allow(clippy::too_many_arguments)]
fn cert_scan(
    name: &str,
    field: &BoundedCsg,
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
    exact: &dyn Fn([f64; 3]) -> f64,
    no_skip: bool,
) -> CertRow {
    let (stop, blk0) = stop_with_lb(field);
    // Die Fernfeld-Kugel ist eine reine KOSTEN-Optimierung des Gates; für das
    // Zertifikat darf sie aus. Das ist keine zweite Fassung von `lb_value`,
    // sondern derselbe Aufruf mit `skip_r = ∞` (§8q-Lockstep bleibt intakt).
    let blk = if no_skip {
        crate::offset_lb::LbBlock { skip_r: f32::INFINITY, ..blk0.clone() }
    } else {
        blk0.clone()
    };
    let mut row = CertRow {
        name: name.into(),
        slack: blk0.slack as f64,
        l_viol: f64::NEG_INFINITY,
        u_viol: f64::NEG_INFINITY,
        ..Default::default()
    };
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let fr = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * fr[0],
                    lo[1] + (hi[1] - lo[1]) * fr[1],
                    lo[2] + (hi[2] - lo[2]) * fr[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let f = field.sdf(pf, EvalMode::Visual);
                row.pts += 1;
                let (c, l, u) = under_cert(stop, &blk, delta, f, p);
                let e = exact(p);
                if let Some(l) = l {
                    row.l_viol = row.l_viol.max(l - e);
                }
                if let Some(u) = u {
                    row.u_viol = row.u_viol.max(e - u);
                    if let Some(l) = l {
                        row.bracket = row.bracket.max(u - l);
                    }
                }
                match c {
                    UnderCert::FieldSolid => row.field_solid += 1,
                    UnderCert::OutsideOmega => row.outside += 1,
                    other => {
                        row.risk += 1;
                        match other {
                            UnderCert::Foot => row.foot += 1,
                            UnderCert::March => row.march += 1,
                            UnderCert::Proven => row.proven += 1,
                            UnderCert::NoLb => row.no_lb += 1,
                            UnderCert::FootSwallowed => row.swallowed += 1,
                            UnderCert::Band => row.band += 1,
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
    }
    row
}

/// **§8r-Sonde: die Abdeckungs-Tabelle.** Nicht im Fast-Tier — sie fährt die
/// volle Matrix zweimal (mit und ohne Fernfeld-Kugel).
#[test]
#[ignore]
fn probe_384_under_certificate_coverage() {
    let delta = 2.0f32;
    println!(
        "| Fixture | Punkte | Risiko | zert. (min-Stück) | zert. (Marsch) | zert. (Liste) | \
         Rest | bewiesener Unter-Claim | kein L | verschluckt | Band | L−exakt | exakt−U | U−L |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for no_skip in [false, true] {
        println!("**Fernfeld-Kugel {}**", if no_skip { "AUS" } else { "an (Laufzeit)" });
        for (name, field, exact, lo, hi, n) in cert_matrix(delta) {
            let r = cert_scan(&name, &field, delta, lo, hi, n, &*exact, no_skip);
            println!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.2e} | {:.2e} | \
                 {:.2e} |",
                r.name,
                r.pts,
                r.risk,
                r.foot,
                r.march,
                r.open(),
                r.proven,
                r.no_lb,
                r.swallowed,
                r.band,
                r.l_viol,
                r.u_viol,
                r.bracket
            );
        }
    }
}

/// **§8r-Gegenprobe: der Kandidaten-Weg des Hauptagenten, widerlegt.**
///
/// Die Hypothese lautete: `L(x) ≥ f(x) + δ − tol` (also `f ≤ L − δ + tol`)
/// zertifiziere Unter-Claim-Freiheit. Sie hat das Vorzeichen verdreht — die
/// Ungleichung ist an einem echten Unter-Claim erst recht erfüllt, weil ein
/// grosses `L` gerade heisst, dass der Punkt Material SEIN MUSS. Der Pin
/// misst das am BASIS-Feld der Nabe (`L`-Block entfernt, §8n-Stand
/// 0 / 59 · 0.3156): wie viele der bewiesenen Unter-Claim-Punkte das
/// Kriterium „zertifiziert".
#[test]
fn the_naive_lb_criterion_certifies_real_under_claims_384() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let basis = strip_lb(&field);
    let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
    let (stop, blk) = stop_with_lb(&field);
    let (lo, hi, n) = ([-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5], 22usize);
    let (mut under, mut falsely_certified, mut worst) = (0usize, 0usize, (0.0f64, [0.0f64; 3]));
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let fr = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * fr[0],
                    lo[1] + (hi[1] - lo[1]) * fr[1],
                    lo[2] + (hi[2] - lo[2]) * fr[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let f = basis.sdf(pf, EvalMode::Visual) as f64;
                let e = dist_hub_complement_exact(t as f64, &cyls, p);
                if !(f >= 0.0 && e - delta as f64 > CERT_TOL) {
                    continue;
                }
                under += 1;
                let d0 = stop.sdf(pf, EvalMode::Visual);
                let l = crate::offset_lb::lb_value(pf, d0, blk) as f64;
                if l <= crate::offset_channel::LB_AVAIL as f64 {
                    continue;
                }
                // Das Kriterium der Hypothese.
                if f <= l - delta as f64 + CERT_TOL {
                    falsely_certified += 1;
                    if e - delta as f64 > worst.0 {
                        worst = (e - delta as f64, p);
                    }
                }
            }
        }
    }
    assert_eq!(
        under, 59,
        "die BASIS-Nabe muss die §8n-Zahl reproduzieren (59 Unter-Claim-Punkte), \
         gemessen {under} — sonst misst die Gegenprobe eine andere Maschine"
    );
    assert!(
        falsely_certified > 0,
        "die Hypothese `f ≤ L − δ + tol` hat an dieser Fixture keinen einzigen \
         Unter-Claim fälschlich zertifiziert — dann ist die Widerlegung nicht belegt"
    );
    println!(
        "§8r-Gegenprobe: {falsely_certified} von {under} bewiesenen Unter-Claim-Punkten \
         werden vom Kandidaten-Kriterium ZERTIFIZIERT, schlimmster {:.4} mm bei {:?}",
        worst.0, worst.1
    );
}

/// **§8r-Gültigkeits-Pin: der Zeuge erkennt einen ECHTEN Unter-Claim.**
///
/// Ein Zeuge, der nie etwas findet, ist von einem `return None` nicht zu
/// unterscheiden. Die Gegenprobe braucht deshalb ein Feld, in dem nachweislich
/// Material fehlt — und es gibt eins, ohne dass man etwas erfinden müsste: das
/// BASIS-Feld der Stufen-Nabe (`L`-Block entfernt, §8n-Stand `0 / 59 · 0.3156`,
/// festgenagelt von `stop_lb_absent_reproduces_the_g_cap_384`).
///
/// Geprüft wird die Produktions-Funktion selbst
/// ([`crate::offset_channel::under_claim_witness`]), nicht ein Nachbau — und
/// zwar in beide Richtungen:
///
/// * **Er spricht**, wo Material fehlt: am §8n-Worst-Punkt mindestens 0.31 mm.
/// * **Er lügt nie**: an JEDEM Punkt, an dem er anschlägt, bestätigt die exakte
///   Referenz mindestens denselben Fehlbetrag.
/// * **Er schweigt** am ausgelieferten Feld (seit §8o ist die Nabe beidseitig
///   leer).
#[test]
fn under_witness_proves_the_basis_regression_384() {
    let delta = 2.0f32;
    let tol = CHANNEL_TOL_RENDER;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let basis = strip_lb(&field);
    let cyls = [(r1 as f64, -(e1 as f64), zh1 as f64), (r2 as f64, -(e2 as f64), 6.0f64)];
    let (stop, blk) = stop_with_lb(&field);
    let (lo, hi, n) = ([-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5], 22usize);
    let (mut basis_hits, mut prod_hits, mut worst) = (0usize, 0usize, (0.0f32, [0.0f32; 3]));
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let fr = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * fr[0],
                    lo[1] + (hi[1] - lo[1]) * fr[1],
                    lo[2] + (hi[2] - lo[2]) * fr[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let d0 = stop.sdf(pf, EvalMode::Visual);
                for (prod, hits) in
                    [(false, &mut basis_hits), (true, &mut prod_hits)]
                {
                    let corr = if prod { &field } else { &basis }.sdf(pf, EvalMode::Visual);
                    let (found, _) = crate::offset_channel::under_claim_witness(
                        None,
                        d0,
                        pf,
                        corr,
                        Some(blk),
                        delta,
                        tol,
                    );
                    let Some((v, at)) = found else { continue };
                    *hits += 1;
                    // Kein Falsch-Positiv: die exakte Referenz muss den
                    // behaupteten Fehlbetrag mindestens bestätigen.
                    let e = dist_hub_complement_exact(t as f64, &cyls, p);
                    assert!(
                        e - delta as f64 >= v as f64 - 1e-4,
                        "Unter-Zeuge behauptet {v:.4} mm bei {at:?}, die exakte Referenz \
                         gibt aber nur {:.4} mm her — ein Falsch-Positiv wäre das Ende \
                         jeder Aussage, die auf L steht",
                        e - delta as f64
                    );
                    if !prod && v > worst.0 {
                        worst = (v, at);
                    }
                }
            }
        }
    }
    assert!(
        basis_hits >= 40,
        "der Zeuge findet am BASIS-Feld nur {basis_hits} der 59 bewiesenen \
         Unter-Claim-Punkte — dann misst er zu wenig, um etwas zu belegen"
    );
    assert!(
        worst.0 > 0.31 && worst.0 < 0.3156,
        "schlimmster nachgewiesener Fehlbetrag {:.4} mm — erwartet knapp unter dem \
         exakten §8n-Wert 0.3156 (die Differenz ist der `slack`)",
        worst.0
    );
    assert_eq!(
        prod_hits, 0,
        "am AUSGELIEFERTEN Feld weist der Zeuge {prod_hits} Unter-Claims nach — \
         die Nabe ist seit §8o beidseitig leer"
    );
}

/// **§8r-Abdeckungs-Pin: das Zertifikat deckt die Risiko-Menge.**
///
/// Die Risiko-Menge ist `{f ≥ 0} ∩ Ω` — nur dort kann fehlendes Material
/// überhaupt sitzen. Der Pin fährt ein grobes Gitter (das feine steht in der
/// Sonde `probe_384_under_certificate_coverage` und im Doc §8r) und hält drei
/// Dinge fest, die alle drei fallen könnten:
///
/// * **Beide Schranken sind solide** — `L ≤ dist ≤ U` an JEDEM Punkt, gemessen
///   gegen die exakten §8k-Referenzen. Ohne das ist das Zertifikat wertlos.
/// * **Die Klammer ist eng** — `U − L` bleibt unter der Mess-Toleranz; das
///   Zertifikat entscheidet also, statt bloss zu existieren.
/// * **Die Abdeckung ist vollständig** — kein Punkt der Risiko-Menge bleibt
///   offen, und keiner ist ein bewiesener Unter-Claim.
#[test]
fn under_certificate_covers_the_risk_set_384() {
    let delta = 2.0f32;
    let mut checked = 0usize;
    for (name, field, exact, lo, hi, _) in cert_matrix(delta) {
        if !(name.starts_with("Stufen")
            || name == "Ecken-Schale t/δ=2"
            || name == "Rippen-Kamm 4×2/1"
            || name == "notch α=90")
        {
            continue;
        }
        let r = cert_scan(&name, &field, delta, lo, hi, 10, &*exact, true);
        assert!(
            r.l_viol <= 0.0,
            "{name}: L übersteigt die exakte Distanz um {:.2e} — die untere Schranke lügt",
            r.l_viol
        );
        assert!(
            r.u_viol <= 0.0,
            "{name}: die exakte Distanz übersteigt U um {:.2e} — der Aussen-Zeuge ist kein \
             Aussen-Punkt, und dann zertifiziert er Unter-Claims weg",
            r.u_viol
        );
        assert!(
            r.bracket <= r.slack + CERT_PUSH + 1e-4,
            "{name}: Klammer U − L = {:.2e} über `slack + push` = {:.2e} plus die f32-Rundung \
             des Fusspunkts (an allen dreizehn Fixturen ≤ 4.4·10⁻⁵) — dann kommt die \
             Unschärfe nicht mehr aus den drei bekannten Quellen",
            r.bracket,
            r.slack + CERT_PUSH
        );
        assert_eq!(
            r.proven, 0,
            "{name}: {} bewiesene Unter-Claims im Feld — die Matrix ist seit §8p \
             beidseitig leer",
            r.proven
        );
        assert_eq!(
            r.open(),
            0,
            "{name}: {} von {} Risiko-Punkten bleiben unzertifiziert ({} kein L, {} Stück \
             verschluckt, {} Band) — die Restmenge muss in der Zusage benannt sein",
            r.open(),
            r.risk,
            r.no_lb,
            r.swallowed,
            r.band
        );
        assert!(r.risk > 100, "{name}: nur {} Risiko-Punkte — der Pin misst nichts", r.risk);
        checked += 1;
    }
    assert_eq!(checked, 4, "nur {checked} Fixtures geprüft");
}

/// **§8r/§8s-Zusagen-Pin: die Unter-Seiten-Ausnahme benennt ihren Zeugen — und
/// der schweigt jetzt an allen neun Fixturen.**
///
/// Bis §8q lautete die Ausnahme „für diese Richtung ist keine Schranke
/// belegbar" und damit basta: niemand hatte je hingesehen, und die Begründung
/// stützte sich auf zwölf leere Messgitter. §8r sah hin — mit einem Zeugen,
/// dessen Fund ein BEWEIS ist (`L > δ + tol` bei stehendem Feld) — und fand an
/// vier der neun Matrix-Fixturen echtes fehlendes Material, alles am freien
/// NAHT-ENDE:
///
/// | Fixture | §8r (Fund) | §8s (nachher) |
/// |---|---|---|
/// | Ecken-Schale `t/δ = 2` | 0.200 mm | — |
/// | Ecken-Schale `t/δ = 2.25` | 0.499 mm | — |
/// | Rippen-Kamm 4×1.5/0.8 | 0.110 mm | — |
/// | Rippen-Kamm 4×2/1 | 0.199 mm | — |
///
/// §8s hat die zwei Ursachen geschlossen (Phantom-Junction zwischen parallelen
/// Naht-Enden; δ-Versatz der freien End-Ebene ohne Stützebenen-Beweis). Das
/// Inventar ist damit leer — und genau das nagelt dieser Pin jetzt fest.
///
/// **Ein leeres Inventar ist von einem kaputten Zeugen nicht zu unterscheiden.**
/// Deshalb steht daneben `seam_end_fix_is_load_bearing_384`: es dreht beide
/// Hälften des Fixes künstlich zurück und weist die alten Funde wieder nach.
#[test]
fn promise_names_the_under_claim_witness_384() {
    let delta = 2.0f32;
    let tol_class = CHANNEL_TOL_RENDER as f64 + 1e-9;
    // Fixture → nachgewiesenes FEHLENDES Material (`None` = der Zeuge schweigt).
    // §8s: alle neun leer. Die §8r-Funde (0.200 / 0.499 / 0.110 / 0.199 mm)
    // stehen als Historie im Doc-Kommentar; die Rückdreh-Gegenprobe
    // `seam_end_fix_is_load_bearing_384` weist sie wieder nach.
    let inventory: &[(&str, Option<f64>)] = &[
        ("Stufen-Nabe", None),
        ("Ecken-Schale t/δ=2", None),
        ("Ecken-Schale t/δ=2.25", None),
        ("Ecken-Schale t/δ=3", None),
        ("Ecken-Schale t/δ=6", None),
        ("Rippen-Kamm 4×1.5/0.8", None),
        ("Rippen-Kamm 4×2/1", None),
        ("notch α=90", None),
        ("notch α=60", None),
    ];
    let mut seen = 0usize;
    for (name, field, report, _) in promise_matrix(delta) {
        let mut blocks = Vec::new();
        collect_lb(&field, &mut blocks);
        assert!(!blocks.is_empty(), "{name}: Fixture ohne L-Block");
        let reasons = report.reasons();
        assert!(
            reasons.iter().any(|s| s.contains("Unter-Claim-Zeuge (#384 §8r)")),
            "{name}: die Zusage nennt ihren Unter-Claim-Zeugen nicht. Reasons: {reasons:?}"
        );
        // Die Zusage selbst darf sich durch den Zeugen NICHT bewegen: er ist
        // Text, keine Schranke. Stünde seine Messung als Zusage da, wäre das
        // der §8g-Zirkelschluss.
        assert!(
            promised_bound(&report).is_some_and(|b| b <= tol_class),
            "{name}: die Zusage hat sich durch den Zeugen bewegt ({:?})",
            promised_bound(&report)
        );
        let (_, want) = inventory
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("{name}: nicht im §8r-Inventar"));
        seen += 1;
        let note = reasons
            .iter()
            .find(|s| s.contains("§8r"))
            .unwrap_or_else(|| panic!("{name}: keine §8r-Notiz"));
        match want {
            None => {
                assert!(
                    note.contains("Sondier-Punkten dieses Stopps keinen")
                        && note.contains("Stichprobe, kein Satz"),
                    "{name}: der Zeuge schweigt — dann muss die Notiz genau das sagen, samt \
                     der Zahl der Sondier-Punkte, und das Schweigen darf nicht wie ein Beweis \
                     klingen. Notiz: {note}"
                );
            }
            Some(v) => {
                assert!(
                    note.contains("mindestens") && note.contains("FEHLENDES Material nach"),
                    "{name}: der Zeuge hat {v:.3} mm nachgewiesen — die Notiz muss den Fund \
                     benennen, nicht das Schweigen. Notiz: {note}"
                );
                let got: f64 = note
                    .split("mindestens ")
                    .nth(1)
                    .and_then(|t| t.split(' ').next())
                    .and_then(|t| t.parse().ok())
                    .unwrap_or_else(|| panic!("{name}: Fehlbetrag nicht lesbar: {note}"));
                assert!(
                    (got - v).abs() < 0.011,
                    "{name}: nachgewiesen {got:.3} mm statt {v:.3} — das Naht-Ende hat sich \
                     bewegt, §8r gehört fortgeschrieben"
                );
            }
        }
    }
    assert_eq!(seen, inventory.len(), "nur {seen} Fixtures geprüft");
}

/// **§8s-Rückdreh-Gegenprobe: der Fix trägt, und der Zeuge lebt.**
///
/// `promise_names_the_under_claim_witness_384` steht ab §8s auf einem LEEREN
/// Inventar. Leer heisst aber nur dann etwas, wenn dasselbe Messwerkzeug am
/// selben Feld mit zurückgedrehtem Fix wieder anschlägt. Genau das tut dieser
/// Pin — für beide Hälften des §8s-Fixes, jede an ihrer eigenen Fixture.
///
/// **Hälfte 1 — die Phantom-Junction (Ecken-Schale).** Bis §8r verlängerte
/// `snap_channel_ends_to_junctions` ein Naht-Ende bis zum
/// [`crate::offset_channel::junction_anchor`]; für (anti)parallele Tangenten
/// war dessen Ergebnis der MITTELPUNKT der beiden Enden. An der Stirnfläche
/// `z = −l` enden vier paarweise PARALLELE Reflex-Kanten; ihr Mittelpunkt liegt
/// `t/2` mm tief IM MATERIAL. Der Pin rechnet genau diesen alten Mittelpunkt
/// nach und misst seine Eindringtiefe im naiven Vereinigungsfeld: sie ist
/// gross. Damit war [`crate::offset_channel::channel_limit`] dort unsauber —
/// seine Voraussetzung ist „jeder Naht-Punkt liegt auf `∂Ω`" (§8j).
///
/// **Hälfte 2 — der δ-Versatz der freien End-Ebene (Rippen-Kamm).** Der Pin
/// nimmt das AUSGELIEFERTE Feld und schiebt jede freie End-Ebene wieder um `δ`
/// nach innen — bit-genau die Operation, die §8s gestrichen hat. Danach weist
/// der Produktions-Zeuge ([`crate::offset_channel::under_claim_witness`]) den
/// §8r-Fund wieder nach: ≥ 0.19 mm fehlendes Material am Kamm 4×2/1.
#[test]
fn seam_end_fix_is_load_bearing_384() {
    use crate::offset_channel as oc;
    let delta = 2.0f32;
    let tol = CHANNEL_TOL_RENDER;

    // ── Hälfte 1: der alte Mittelpunkt-Anker lag im Material ──────────────
    let t = 2.25f32 * delta;
    let (shell, _) = compile_for(thin_corner_node(t, 40.0, delta));
    let mut stops = Vec::new();
    stop_nodes(&shell, &mut stops);
    let mut deepest = 0.0f32;
    let mut deepest_at = [0.0f32; 3];
    for node in stops.iter() {
        let BoundedCsg::ArcChannel { inner, channels, .. } = node else { continue };
        // Alle offenen Enden mit ihrer Innen-Tangente (wie set_end_plane_gates).
        let mut ends: Vec<(usize, [f32; 3], [f32; 3])> = Vec::new();
        for (ci, ch) in channels.iter().enumerate() {
            if ch.arcs.is_empty() {
                continue;
            }
            let s0 = oc::arc_point_at(&ch.arcs[0], 0.0);
            let e0 = oc::arc_point_at(ch.arcs.last().unwrap(), 1.0);
            if norm3(sub3(s0, e0)) < 1e-4 {
                continue;
            }
            if let Some(tg) = oc::arc_tangent_at(&ch.arcs[0], 0.0) {
                ends.push((ci, s0, tg));
            }
            if let Some(tg) = oc::arc_tangent_at(ch.arcs.last().unwrap(), 1.0) {
                ends.push((ci, e0, [-tg[0], -tg[1], -tg[2]]));
            }
        }
        for (a, e1, t1) in ends.iter() {
            for (b, e2, t2) in ends.iter() {
                if a == b {
                    continue;
                }
                let d = norm3(sub3(*e1, *e2));
                // Das alte Fenster: max(0.5·δ, 3·step); step = 2 % der
                // Überlappungs-AABB, hier 1.6 mm ⇒ 4.8 mm.
                if d > 4.8 {
                    continue;
                }
                // (anti)parallel ⇒ der alte Code nahm den Mittelpunkt.
                if (dot3(*t1, *t2).abs() - 1.0).abs() > 1e-3 {
                    continue;
                }
                let mid = [
                    0.5 * (e1[0] + e2[0]),
                    0.5 * (e1[1] + e2[1]),
                    0.5 * (e1[2] + e2[2]),
                ];
                // Eindringtiefe im naiven Vereinigungsfeld (d0 = sdf_∪ + δ).
                let depth = -(inner.sdf(mid, EvalMode::Visual) - delta);
                if depth > deepest {
                    deepest = depth;
                    deepest_at = mid;
                }
            }
        }
    }
    assert!(
        deepest > 0.9,
        "der alte Mittelpunkt-Anker lag nur {deepest:.4} mm im Material (bei {deepest_at:?})          — dann ist die Ecken-Schale nicht mehr die Fixture, an der §8s gemessen wurde"
    );
    // Und genau dafür gibt es ihn nicht mehr.
    assert!(
        oc::junction_anchor([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [4.5, 0.0, 0.0], [0.0, 0.0, 1.0], 0.004)
            .is_none(),
        "parallele Naht-Enden dürfen keine Junction bilden"
    );

    // ── Hälfte 2: der δ-Versatz zurückgedreht ⇒ der Fund ist wieder da ────
    let (node, boxes, span) = rib_comb_node(4, 2.0, 1.0, delta);
    let (comb, _) = compile_for(node);
    let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
    let mut rolled = comb.clone();
    let shifted = shift_free_end_planes(&mut rolled, delta);
    assert!(
        shifted > 0,
        "keine einzige freie End-Ebene gefunden — die Rückdrehung misst nichts"
    );
    let (stop, blk) = stop_with_lb(&comb);
    let _ = span;
    // Feines Fenster um das freie Naht-Ende (Rippen-Flanke ∩ Platten-Stirn,
    // `x = ±(w/2) + cx`, `y = 20`, Ende auf der Rippen-Unterseite `z = −5`).
    let (lo, hi, n) = ([-6.5, 17.0, -5.5], [-2.5, 20.5, -2.0], 44usize);
    let mut worst = 0.0f32;
    let mut worst_at = [0.0f32; 3];
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let fr = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * fr[0],
                    lo[1] + (hi[1] - lo[1]) * fr[1],
                    lo[2] + (hi[2] - lo[2]) * fr[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let d0 = stop.sdf(pf, EvalMode::Visual);
                let (found, _) = oc::under_claim_witness(
                    None,
                    d0,
                    pf,
                    rolled.sdf(pf, EvalMode::Visual),
                    Some(blk),
                    delta,
                    tol,
                );
                let Some((v, at)) = found else { continue };
                // Kein Falsch-Positiv: die exakte Referenz muss mitgehen.
                let e = dist_obb_union_complement_exact(&obbs, p);
                assert!(
                    e - delta as f64 >= v as f64 - 1e-4,
                    "Unter-Zeuge behauptet {v:.4} mm bei {at:?}, exakt sind es nur {:.4}",
                    e - delta as f64
                );
                if v > worst {
                    worst = v;
                    worst_at = at;
                }
            }
        }
    }
    assert!(
        worst > 0.19,
        "mit zurückgedrehtem δ-Versatz weist der Zeuge nur {worst:.4} mm nach (bei          {worst_at:?}) — §8r mass am Kamm 4×2/1 0.199 mm; findet die Rückdrehung das          nicht mehr, misst der Pin nicht mehr, was er behauptet"
    );
    // Und am ausgelieferten Feld schweigt derselbe Zeuge.
    let mut prod_hits = 0usize;
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                let fr = [
                    (ix as f64 + 0.2113248654) / n as f64,
                    (iy as f64 + 0.5773502692) / n as f64,
                    (iz as f64 + 0.7886751346) / n as f64,
                ];
                let p = [
                    lo[0] + (hi[0] - lo[0]) * fr[0],
                    lo[1] + (hi[1] - lo[1]) * fr[1],
                    lo[2] + (hi[2] - lo[2]) * fr[2],
                ];
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let d0 = stop.sdf(pf, EvalMode::Visual);
                let (found, _) = oc::under_claim_witness(
                    None,
                    d0,
                    pf,
                    comb.sdf(pf, EvalMode::Visual),
                    Some(blk),
                    delta,
                    tol,
                );
                if found.is_some() {
                    prod_hits += 1;
                }
            }
        }
    }
    assert_eq!(
        prod_hits, 0,
        "am AUSGELIEFERTEN Kamm-Feld weist der Zeuge {prod_hits} Unter-Claims nach —          §8s hat das Naht-Ende geschlossen"
    );
}

/// **§8s-Skew-Pin: windschiefe Geraden treffen sich so wenig wie parallele.**
///
/// Die erste Fassung von §8s strich nur den PARALLEL-Rückfall. Für nicht
/// parallele Geraden rechnete `junction_anchor` weiter die Fusspunkte `q₁`, `q₂`
/// der kürzesten Verbindung aus und lieferte deren Mittelpunkt — ohne zu prüfen,
/// ob die Geraden sich überhaupt schneiden. Bei windschiefen Geraden liegt
/// dieser Mittelpunkt auf KEINER der beiden: dieselbe Phantom-Junction-Klasse,
/// nur eine Stufe subtiler. (Befund des externen Mathe-Reviews zu §8s.)
///
/// **Die Schranke ist hergeleitet, nicht kalibriert.** Der Anker behauptet,
/// Punkt BEIDER Nähte zu sein, und liegt als Mittelpunkt `gap/2` neben jeder;
/// die Synthese verlangt von jedem Punkt, den sie als Naht-Punkt behandelt,
/// `∂Ω`-Nähe innerhalb `gate_eps` (die Schwelle des Traces). Also
/// `gap/2 ≤ gate_eps`, d. h. `gap ≤ 2·gate_eps` — bei δ = 2 sind das 0.004 mm.
/// Gemessen über alle Fixturen liegt die Schwelle in einer LEEREN Lücke der
/// Verteilung: grösster akzeptierter Rest 0.003728, kleinster verworfener
/// 0.004230; verworfen werden 4100 von 21760 Ankern, mit Resten bis 4.80 mm.
///
/// Der Pin prüft drei Dinge: die Verwerfung eines windschiefen Paars, die
/// ANNAHME eines Paars knapp unterhalb der Schranke (der Test ist keine
/// Pauschal-Verweigerung) und — als Rückdreh-Gegenprobe — dass der ungetestete
/// Mittelpunkt tatsächlich neben beiden Geraden liegt.
#[test]
fn junction_anchor_refuses_skew_lines_384() {
    use crate::offset_channel as oc;
    // δ = 2 ⇒ gate_eps = max(δ·1e−3, 1e−4) = 2e−3 ⇒ Schranke 2·gate_eps.
    const SKEW: f32 = 0.004;

    // Zwei windschiefe Geraden: x-Achse durch den Ursprung, y-Achse um `gap`
    // in z versetzt. Sie schneiden sich nie; die Fusspunkte der kürzesten
    // Verbindung sind (0,0,0) und (0,0,gap).
    for gap in [0.01f32, 0.05, 0.5, 4.0] {
        let e1 = [3.0f32, 0.0, 0.0];
        let t1 = [1.0f32, 0.0, 0.0]; // Innen-Tangente ⇒ Gerade läuft nach −x
        let e2 = [0.0f32, 3.0, gap];
        let t2 = [0.0f32, 1.0, 0.0];
        assert!(
            oc::junction_anchor(e1, t1, e2, t2, SKEW).is_none(),
            "windschiefes Paar mit Rest {gap} wurde als Junction akzeptiert"
        );
        // Rückdrehung: ohne Skew-Test ist der Anker der Mittelpunkt der
        // Fusspunkte — und der liegt `gap/2` neben BEIDEN Geraden.
        let phantom = [0.0f32, 0.0, 0.5 * gap];
        let off1 = phantom[2].abs(); // Abstand zur x-Achse (y=z=0)
        let off2 = (phantom[2] - gap).abs(); // Abstand zur versetzten y-Achse
        assert!(
            (off1 - 0.5 * gap).abs() < 1e-6 && (off2 - 0.5 * gap).abs() < 1e-6,
            "die Rückdreh-Rechnung stimmt nicht: {off1} / {off2} statt {}",
            0.5 * gap
        );
        assert!(
            off1 > SKEW,
            "Rest {gap}: der Phantom-Anker läge nur {off1} neben der Naht — dann \
             misst der Pin nicht, was er behauptet"
        );
    }

    // Und knapp UNTERHALB der Schranke wird angenommen: der Test verweigert
    // nicht pauschal, er trennt.
    let ok = oc::junction_anchor(
        [3.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 3.0, 0.5 * SKEW],
        [0.0, 1.0, 0.0],
        SKEW,
    );
    assert!(ok.is_some(), "ein Paar mit Rest {} unter der Schranke muss eine Junction sein", 0.5 * SKEW);

    // Auf ECHTER Geometrie: unter den Naht-Enden der Matrix-Fixturen gibt es
    // Paare im Kopplungs-Fenster, die nicht parallel sind UND windschief —
    // ohne den Test bekämen sie einen Phantom-Anker.
    let delta = 2.0f32;
    let mut cases: Vec<BoundedCsg> = vec![
        compile_for(stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta)).0,
        compile_for(thin_corner_node(2.25 * delta, 40.0, delta)).0,
        compile_for(rib_comb_node(4, 2.0, 1.0, delta).0).0,
    ];
    cases.push(compile_for(notch_union_node(90.0, delta as f64)).0);
    let mut skew_pairs = 0usize;
    let mut worst = 0.0f32;
    for field in cases.iter() {
        let mut stops = Vec::new();
        stop_nodes(field, &mut stops);
        for node in stops.iter() {
            let BoundedCsg::ArcChannel { channels, .. } = node else { continue };
            let mut ends: Vec<([f32; 3], [f32; 3])> = Vec::new();
            for ch in channels.iter() {
                if ch.arcs.is_empty() {
                    continue;
                }
                let s0 = oc::arc_point_at(&ch.arcs[0], 0.0);
                let e0 = oc::arc_point_at(ch.arcs.last().unwrap(), 1.0);
                if norm3(sub3(s0, e0)) < 1e-4 {
                    continue;
                }
                if let Some(tg) = oc::arc_tangent_at(&ch.arcs[0], 0.0) {
                    ends.push((s0, tg));
                }
                if let Some(tg) = oc::arc_tangent_at(ch.arcs.last().unwrap(), 1.0) {
                    ends.push((e0, [-tg[0], -tg[1], -tg[2]]));
                }
            }
            for (e1, t1) in ends.iter() {
                for (e2, t2) in ends.iter() {
                    if norm3(sub3(*e1, *e2)) < 1e-6 || norm3(sub3(*e1, *e2)) > 4.8 {
                        continue;
                    }
                    if (dot3(*t1, *t2).abs() - 1.0).abs() < 1e-3 {
                        continue; // parallel — das war der erste §8s-Fund
                    }
                    let Some(gap) = skew_gap(*e1, *t1, *e2, *t2) else { continue };
                    if gap > SKEW {
                        skew_pairs += 1;
                        worst = worst.max(gap);
                        assert!(
                            oc::junction_anchor(*e1, *t1, *e2, *t2, SKEW).is_none(),
                            "windschiefes Paar (Rest {gap:.6}) an echter Geometrie akzeptiert"
                        );
                    }
                }
            }
        }
    }
    assert!(
        skew_pairs > 0 && worst > 0.05,
        "nur {skew_pairs} windschiefe Paare (schlimmster Rest {worst:.6}) an echter \
         Geometrie — dann wäre der Skew-Test eine tote Ausnahme"
    );
}

/// Rest der kürzesten Verbindung zweier Spine-Geraden (`t` zeigt ins
/// Spine-Innere, die Gerade läuft also nach `−t`). `None` bei (anti)parallel.
fn skew_gap(e1: [f32; 3], t1: [f32; 3], e2: [f32; 3], t2: [f32; 3]) -> Option<f32> {
    let d1 = [-t1[0], -t1[1], -t1[2]];
    let d2 = [-t2[0], -t2[1], -t2[2]];
    let b = dot3(d1, d2);
    let den = 1.0 - b * b;
    if den.abs() < 1e-6 {
        return None;
    }
    let w0 = sub3(e1, e2);
    let dd = dot3(d1, w0);
    let ee = dot3(d2, w0);
    let s = (b * ee - dd) / den;
    let u = (ee - b * dd) / den;
    let q1 = [e1[0] + d1[0] * s, e1[1] + d1[1] * s, e1[2] + d1[2] * s];
    let q2 = [e2[0] + d2[0] * u, e2[1] + d2[1] * u, e2[2] + d2[2] * u];
    if !q1.iter().chain(q2.iter()).all(|c| c.is_finite()) {
        return None;
    }
    Some(norm3(sub3(q1, q2)))
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm3(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}

/// Dreht §8s Hälfte 2 zurück: schiebt jede FREIE End-Ebene (Normale = die
/// Innen-Tangente eines Spine-Endes, `c = ⟨E,t̂⟩`) um `δ` nach innen — exakt
/// die Form, die `set_end_plane_gates` bis §8r schrieb. Bisektor-Ebenen bleiben
/// unberührt. Liefert die Zahl der geschobenen Ebenen.
fn shift_free_end_planes(node: &mut BoundedCsg, delta: f32) -> usize {
    use crate::offset_channel as oc;
    let mut n = 0usize;
    match node {
        BoundedCsg::ArcChannel { inner, channels, .. } => {
            for ch in channels.iter_mut() {
                if ch.arcs.is_empty() {
                    continue;
                }
                let s0 = oc::arc_point_at(&ch.arcs[0], 0.0);
                let e0 = oc::arc_point_at(ch.arcs.last().unwrap(), 1.0);
                if norm3(sub3(s0, e0)) < 1e-4 {
                    continue; // geschlossen
                }
                let mut ends: Vec<([f32; 3], [f32; 3])> = Vec::new();
                if let Some(tg) = oc::arc_tangent_at(&ch.arcs[0], 0.0) {
                    ends.push((s0, tg));
                }
                if let Some(tg) = oc::arc_tangent_at(ch.arcs.last().unwrap(), 1.0) {
                    ends.push((e0, [-tg[0], -tg[1], -tg[2]]));
                }
                for g in ch.end_planes.iter_mut() {
                    if g[3] <= -1.0e29 {
                        continue;
                    }
                    let gn = [g[0], g[1], g[2]];
                    for (e, tg) in &ends {
                        if norm3(sub3(gn, *tg)) < 1e-4 && (g[3] - dot3(*e, *tg)).abs() < 1e-4 {
                            g[3] += delta;
                            n += 1;
                            break;
                        }
                    }
                }
            }
            n += shift_free_end_planes(inner, delta);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            n += shift_free_end_planes(left, delta);
            n += shift_free_end_planes(right, delta);
        }
        BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => {
            n += shift_free_end_planes(inner, delta);
        }
        _ => {}
    }
    n
}

#[test]
#[ignore]
fn probe_384_shell_under_claim_point() {
    let delta = 2.0f32;
    let t = 2.0f32 * delta;
    let (field, report) = compile_for(thin_corner_node(t, 40.0, delta));
    let boxes = thin_corner_boxes(t as f64, 40.0);
    let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
    let (stop, blk) = stop_with_lb(&field);
    for r in report.reasons() {
        println!("REASON: {r}");
    }
    let mut zs: Vec<f64> = Vec::new();
    let mut z = -39.5f64;
    while z <= 39.5 {
        let pf = [-1.78f32, -1.30, z as f32];
        if field.sdf(pf, EvalMode::Visual) >= 0.0 {
            zs.push(z);
        }
        z += 0.25;
    }
    println!("Feld ≥ 0 bei z ∈ {:?} … {:?} ({} von {} Abtastungen)",
        zs.first(), zs.last(), zs.len(), 317);
    for p in [
        [-1.78f64, -1.30, -37.78],
        [-1.78, -1.30, -20.0],
        [-1.78, -1.30, 0.0],
        [-1.78, -1.30, 37.78],
    ] {
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let d0 = stop.sdf(pf, EvalMode::Visual);
        let l = crate::offset_lb::lb_value(pf, d0, blk);
        println!(
            "p={p:?}  Feld={:.6}  d0={:.6}  L={:.6}  exakt={:.6}",
            field.sdf(pf, EvalMode::Visual),
            d0,
            l,
            dist_obb_union_complement_exact(&obbs, p)
        );
    }
}

#[test]
#[ignore]
fn probe_384_under_witness_inventory() {
    let delta = 2.0f32;
    let mut cases: Vec<(String, CsgNode)> = vec![(
        "Stufen-Nabe".into(),
        stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta),
    )];
    for tf in [2.0f32, 2.1, 2.25, 2.4, 2.5, 3.0, 4.0, 6.0] {
        cases.push((format!("Ecken-Schale t/δ={tf}"), thin_corner_node(tf * delta, 40.0, delta)));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        cases.push((format!("Rippen-Kamm 4×{w}/{g}"), rib_comb_node(4, w, g, delta).0));
    }
    for alpha in [90.0f64, 60.0] {
        cases.push((format!("notch α={alpha}"), notch_union_node(alpha, delta as f64)));
    }
    for (name, node) in cases {
        let (field, report) = compile_for(node);
        let mut blocks = Vec::new();
        collect_lb(&field, &mut blocks);
        let verdict = report
            .reasons()
            .iter()
            .find(|s| s.contains("§8r"))
            .map(|s| {
                let tail = s.split("§8r)").nth(1).unwrap_or("").trim().to_string();
                tail.chars().take(120).collect::<String>()
            })
            .unwrap_or_else(|| "KEINE §8r-Notiz".into());
        println!(
            "{name}: {} Stücke | {verdict}",
            blocks.first().map_or(0, |b| b.pieces.len())
        );
    }
}

/// **§8r-Naht-Ende-Sonde: das Gitter war das Problem, nicht das Feld.**
///
/// Die §8k–§8q-Gitter sitzen um die interessante Ecke herum (die Schale misst
/// `[−7,1]³` bei Platten, die bis `±40` reichen). Der Unter-Claim sitzt aber am
/// freien NAHT-ENDE. Diese Sonde legt ein Gitter genau dorthin, wo der
/// zertifizierte Zeuge angeschlagen hat, und misst beides: was das Zertifikat
/// sagt und was die exakte Referenz sagt.
#[test]
#[ignore]
fn probe_384_seam_end_grid() {
    let delta = 2.0f32;
    type SeamCase = (String, BoundedCsg, ExactFn, [f64; 3]);
    let cases: Vec<SeamCase> = {
        let mut v: Vec<SeamCase> = Vec::new();
        for tf in [2.0f32, 2.1, 2.25, 2.4] {
            let t = tf * delta;
            let (field, _) = compile_for(thin_corner_node(t, 40.0, delta));
            let boxes = thin_corner_boxes(t as f64, 40.0);
            let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
            v.push((
                format!("Ecken-Schale t/δ={tf}"),
                field,
                Box::new(move |p| dist_obb_union_complement_exact(&obbs, p)),
                [-(t as f64) * 0.5, -(t as f64) * 0.5, -37.5],
            ));
        }
        for (w, g, cx) in [(1.5f32, 0.8f32, 3.5f64), (2.0, 1.0, -4.4)] {
            let (node, boxes, _) = rib_comb_node(4, w, g, delta);
            let (field, _) = compile_for(node);
            let obbs: Vec<Obb> = boxes.iter().map(|&(lo, hi)| obb_aabb(lo, hi)).collect();
            v.push((
                format!("Rippen-Kamm 4×{w}/{g}"),
                field,
                Box::new(move |p| dist_obb_union_complement_exact(&obbs, p)),
                [cx, 18.0, -3.5],
            ));
        }
        v
    };
    println!("| Fixture | Zentrum | Risiko | zertifiziert | BEWIESENER Unter-Claim | offen | exakt max |");
    println!("|---|---|---|---|---|---|---|");
    for (name, field, exact, c) in cases {
        let lo = [c[0] - 2.0, c[1] - 2.0, c[2] - 2.0];
        let hi = [c[0] + 2.0, c[1] + 2.0, c[2] + 2.0];
        let r = cert_scan(&name, &field, delta, lo, hi, 16, &*exact, true);
        // Wieviel fehlendes Material sieht die EXAKTE Referenz im selben Fenster?
        let (stop, _) = stop_with_lb(&field);
        let _ = stop;
        let mut worst = 0.0f64;
        for ix in 0..16 {
            for iy in 0..16 {
                for iz in 0..16 {
                    let fr = [
                        (ix as f64 + 0.2113248654) / 16.0,
                        (iy as f64 + 0.5773502692) / 16.0,
                        (iz as f64 + 0.7886751346) / 16.0,
                    ];
                    let p = [
                        lo[0] + (hi[0] - lo[0]) * fr[0],
                        lo[1] + (hi[1] - lo[1]) * fr[1],
                        lo[2] + (hi[2] - lo[2]) * fr[2],
                    ];
                    let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                    if field.sdf(pf, EvalMode::Visual) >= 0.0 {
                        worst = worst.max(exact(p) - delta as f64);
                    }
                }
            }
        }
        println!(
            "| {} | {:?} | {} | {} | {} | {} | {:.4} |",
            r.name,
            c,
            r.risk,
            r.certified(),
            r.proven,
            r.open(),
            worst
        );
    }
}

/// **§8s-Invarianten-Pin: die abgetasteten Naht-Punkte liegen auf `∂Ω`.**
///
/// **Was der Pin misst — und was nicht.** Er tastet jeden Bogen an 21
/// Stützstellen ab und verlangt dort `|naives Vereinigungsfeld| ≤ fit_tol`.
/// Das ist eine STICHPROBE, kein Beweis: zwischen zwei Stützstellen kann die
/// Kurve ins Material eintauchen, ohne dass eine Probe es sieht (die
/// 1-Lipschitz-Eigenschaft des naiven Feldes beschränkt den FELDWERT zwischen
/// den Proben, nicht die geometrische Tiefe eines Extremums dazwischen). Ein
/// echter Beweis bräuchte eine Intervall-Auswertung der Member-Felder über dem
/// Bogen — dasselbe fehlende Werkzeug wie in §8r Intervall-Zweig 2.
///
/// Was er trotzdem leistet: der §8s-Fund war kein Aliasing-Artefakt, sondern
/// ein Anbau, der über seine GANZE Länge im Material lag (bis 1.126 mm tief).
/// Solche Brüche fängt die Stichprobe zuverlässig — und genau sie sind
/// §8g–§8r zwölf Pakete lang unbemerkt geblieben.
///
/// Geprüft wird die Modell-Annahme, auf der zwei Aussagen dieses Stopps
/// stehen:
///
/// * [`crate::offset_channel::channel_limit`] (§8j) macht aus `de` die untere
///   Feld-Schranke `δ − de`. Sie gilt, weil `q*` auf `∂Ω` liegt und damit
///   `dist(x,∁Ω) ≤ de` — bedingungslos. Liegt `q*` statt dessen `u` mm TIEF im
///   Material, hebt die Klemme das Feld um bis zu `u` über die Wahrheit: sie
///   erzeugt fehlendes Material, statt Phantom-Material zu tilgen.
/// * Der Term selbst liest `de − δ` als Distanz-Kandidaten in `w_geo`.
///
/// Der Trace hält die Annahme von sich aus ein — ebenfalls nur an seinen
/// Punkten (`project_to_seam` + `blocking_member` werten dort aus, nicht
/// dazwischen). Gebrochen haben sie die SNAPS: bis §8r verlängerte
/// `snap_channel_ends_to_junctions` ein Ende bis zum Mittelpunkt zweier
/// PARALLELER Naht-Enden — an der Ecken-Schale `t/2` mm ins Material hinein
/// (gemessen 1.126 mm bei `t/δ = 2.25`), an jedem einzelnen der 24 Kanäle.
///
/// Erlaubt ist nur, was der Bogen-Fit selbst kostet (`fit_tol = tol/2`, plus
/// f32-Rauschen).
#[test]
fn every_seam_point_lies_on_the_boundary_384() {
    let delta = 2.0f32;
    let mut cases: Vec<(String, BoundedCsg)> = Vec::new();
    cases.push((
        "Stufen-Nabe".into(),
        compile_for(stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta)).0,
    ));
    for tf in [2.0f32, 2.25, 3.0, 6.0] {
        cases.push((
            format!("Ecken-Schale t/δ={tf}"),
            compile_for(thin_corner_node(tf * delta, 40.0, delta)).0,
        ));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0, 1.0)] {
        cases.push((
            format!("Rippen-Kamm 4×{w}/{g}"),
            compile_for(rib_comb_node(4, w, g, delta).0).0,
        ));
    }
    for alpha in [90.0f64, 60.0] {
        cases.push((
            format!("notch α={alpha}"),
            compile_for(notch_union_node(alpha, delta as f64)).0,
        ));
    }
    // Der Greedy-Bogen-Fit darf `fit_tol = tol/2` von der Naht abweichen; dazu
    // das f32-Rauschen der Member-Auswertung an 40-mm-Geometrie.
    let allow = (CHANNEL_TOL_RENDER * 0.5 + 1e-3) as f32;
    let mut checked = 0usize;
    for (name, field) in cases {
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        assert!(!stops.is_empty(), "{name}: kein Stopp-Knoten");
        for node in stops.iter() {
            let BoundedCsg::ArcChannel { inner, channels, .. } = node else { continue };
            for (ci, ch) in channels.iter().enumerate() {
                for (ai, arc) in ch.arcs.iter().enumerate() {
                    for k in 0..=20 {
                        let q = crate::offset_channel::arc_point_at(arc, k as f32 / 20.0);
                        // naiver Stoppwert `d₀ = sdf_∪ + δ` ⇒ `sdf_∪ = d₀ − δ`.
                        let depth = -(inner.sdf(q, EvalMode::Visual) - ch.delta);
                        assert!(
                            depth <= allow,
                            "{name} K{ci}/arc{ai}: Naht-Punkt {q:?} liegt {depth:.4} mm TIEF im \
                             Material (erlaubt {allow:.4}). Damit ist `de` keine obere Schranke \
                             von dist(x,∁Ω) mehr, und `channel_limit` hebt das Feld über die \
                             Wahrheit — genau der §8s-Fund."
                        );
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 4_000, "nur {checked} Naht-Punkte geprüft — der Pin misst zu wenig");
}

// ══════════════ #384 §8t — erst messen: wo geht die Zeit im `L`-Block hin? ══════════════
//
// §8o/§8p haben das räumliche Pruning der Stück-Liste als „offensichtliche
// Optimierung" benannt und nicht gebaut. Bevor irgendein Format umgebaut wird,
// misst diese Sonde vier Dinge, die die Entscheidung tragen:
//
// 1. **Verteilung** — wie viele Stücke welcher Art je Fixture/Block.
// 2. **Kosten je Stück-Art** — `tri_dist` gegen `rev_dist`, isoliert.
// 3. **Realer Anteil** — Feldkosten mit und ohne `L`-Block über das ECHTE
//    Scan-Gitter der Doktrin-Pins (nicht nur am Worst-Case-Punkt): wie oft ist
//    der Block überhaupt lebendig?
// 4. **Ordnungs-Statistik + Simulation** — welcher Index gewinnt das `min`, wie
//    viele Stücke sind überhaupt konkurrenzfähig, und wie viele Stücke würde
//    ein Gruppen-Prune mit laufendem `best` tatsächlich noch anfassen.

/// Konservative Hüllkugel EINES Stücks (Welt-Raum, beide Arten).
///
/// * `Tri` — Umkreis-Ersatz: Schwerpunkt + grösster Ecken-Abstand. Jeder Punkt
///   des Dreiecks ist Konvexkombination der Ecken, liegt also in der Kugel.
/// * `Rev` — die Fläche ist `{ o + z·ax + r·û : (r,z) ∈ Segment, û ⊥ ax }`.
///   Relativ zu `c = o + z_mid·ax` ist der Offset `(z − z_mid)·ax + r·û` mit
///   `|z − z_mid| ≤ h` und `r ≤ R = max(r0,r1)`, und weil `ax ⊥ û` gilt
///   `|Offset|² = (z − z_mid)² + r² ≤ h² + R²`.
fn piece_sphere(p: &crate::offset_lb::LbPiece) -> ([f32; 3], f32) {
    match *p {
        crate::offset_lb::LbPiece::Tri { a, b, c, .. } => {
            let ctr = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            let d = |q: [f32; 3]| {
                ((q[0] - ctr[0]).powi(2) + (q[1] - ctr[1]).powi(2) + (q[2] - ctr[2]).powi(2)).sqrt()
            };
            (ctr, d(a).max(d(b)).max(d(c)))
        }
        crate::offset_lb::LbPiece::Rev { o, ax, r0, z0, r1, z1 } => {
            let zm = 0.5 * (z0 + z1);
            let ctr = [o[0] + ax[0] * zm, o[1] + ax[1] * zm, o[2] + ax[2] * zm];
            let h = 0.5 * (z1 - z0).abs();
            let rr = r0.max(r1);
            (ctr, (h * h + rr * rr).sqrt())
        }
    }
}

/// Hüllkugel einer Stück-GRUPPE (konservativ): Mittelpunkt = Schwerpunkt der
/// Stück-Zentren, Radius = `max_i (|c_i − c| + r_i)`.
fn group_sphere(ps: &[crate::offset_lb::LbPiece]) -> ([f32; 3], f32) {
    let sph: Vec<_> = ps.iter().map(piece_sphere).collect();
    let n = sph.len() as f32;
    let mut c = [0.0f32; 3];
    for (s, _) in &sph {
        for k in 0..3 {
            c[k] += s[k] / n;
        }
    }
    let mut r = 0.0f32;
    for (s, rr) in &sph {
        let d = ((s[0] - c[0]).powi(2) + (s[1] - c[1]).powi(2) + (s[2] - c[2]).powi(2)).sqrt();
        r = r.max(d + rr);
    }
    (c, r)
}

/// Die 13 Fixturen der §8p-Matrix mit ihren Doktrin-Gittern.
fn p8t_fixtures(delta: f32) -> Vec<(String, BoundedCsg, [f64; 3], [f64; 3], usize)> {
    let mut out: Vec<(String, BoundedCsg, [f64; 3], [f64; 3], usize)> = Vec::new();
    {
        let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
        let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
        out.push((
            "Nabe".into(), field,
            [-9.0, -9.0, -(t as f64) - 1.5], [9.0, 9.0, 0.5], 22,
        ));
    }
    for tf in [2.0f32, 2.1, 2.25, 2.4, 2.5, 3.0, 4.0, 6.0] {
        let t = tf * delta;
        let (field, _) = compile_for(thin_corner_node(t, 40.0, delta));
        let lo = -(t as f64 + delta as f64 + 1.0);
        out.push((format!("Schale {tf}"), field, [lo; 3], [1.0; 3], 22));
    }
    for (w, g) in [(1.5f32, 0.8f32), (2.0f32, 1.0f32)] {
        let (node, _, span) = rib_comb_node(4, w, g, delta);
        let (field, _) = compile_for(node);
        out.push((format!("Kamm {w}/{g}"), field, [-span, -3.0, -6.0], [span, 3.0, 0.5], 24));
    }
    for alpha in [90.0f64, 60.0] {
        let (field, _) = compile_for(notch_union_node(alpha, delta as f64));
        out.push((format!("notch {alpha}"), field, [-10.0, -10.0, 42.0], [10.0, 10.0, 54.0], 22));
    }
    out
}

/// Alle Stopps eines Baums mit ihrem `L`-Block und dem `inner` für `d₀`.
fn lb_stops<'a>(
    n: &'a BoundedCsg,
    out: &mut Vec<(&'a BoundedCsg, f32, &'a crate::offset_lb::LbBlock)>,
) {
    match n {
        BoundedCsg::ArcChannel { inner, channels, verts, lb, .. } => {
            if let Some(b) = lb {
                let d = channels
                    .first()
                    .map(|c| c.delta)
                    .or_else(|| verts.first().map(|v| v.delta))
                    .unwrap_or(0.0);
                out.push((inner, d, b));
            }
            lb_stops(inner, out);
        }
        BoundedCsg::Union { left, right, .. }
        | BoundedCsg::Intersection { left, right, .. }
        | BoundedCsg::Difference { left, right, .. } => {
            lb_stops(left, out);
            lb_stops(right, out);
        }
        BoundedCsg::Offset { inner, .. } | BoundedCsg::Shell { inner, .. } => lb_stops(inner, out),
        _ => {}
    }
}

#[test]
#[ignore]
fn probe_384_lb_pruning_cost() {
    let delta = 2.0f32;
    let fx = p8t_fixtures(delta);

    // ── (1) Verteilung ───────────────────────────────────────────────────────
    println!("\n── §8t (1) Stück-Verteilung ──");
    println!("{:<16} {:>7} {:>8} {:>6} {:>6} {:>26}", "Fixture", "Blöcke", "Stücke", "Tri", "Rev", "je Block");
    for (name, field, ..) in &fx {
        let mut bs = Vec::new();
        collect_lb(field, &mut bs);
        let np: usize = bs.iter().map(|b| b.pieces.len()).sum();
        let tri: usize = bs
            .iter()
            .map(|b| b.pieces.iter().filter(|p| matches!(p, crate::offset_lb::LbPiece::Tri { .. })).count())
            .sum();
        let per: Vec<usize> = bs.iter().map(|b| b.pieces.len()).collect();
        println!(
            "{name:<16} {:>7} {np:>8} {tri:>6} {:>6} {:>26}",
            bs.len(), np - tri, format!("{per:?}")
        );
    }

    // ── (2) Kosten je Stück-Art ──────────────────────────────────────────────
    println!("\n── §8t (2) Kosten je Stück-Art (ns/Aufruf) ──");
    {
        let tri = crate::offset_lb::LbPiece::tri(
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0; 3], 1e4,
        );
        let rev = crate::offset_lb::LbPiece::Rev {
            o: [0.0, 0.0, 0.0], ax: [0.0, 0.0, 1.0], r0: 1.0, z0: 0.0, r1: 1.0, z1: 2.0,
        };
        for (tag, pc) in [("Tri", tri), ("Rev", rev)] {
            let n = 4_000_000u32;
            let t0 = std::time::Instant::now();
            let mut acc = 0.0f32;
            for k in 0..n {
                let q = [0.3 + (k % 13) as f32 * 1e-4, 0.7, 2.5];
                acc += crate::offset_lb::piece_dist(q, &pc);
            }
            let el = t0.elapsed().as_nanos() as f64 / n as f64;
            std::hint::black_box(acc);
            println!("  {tag:<4} {el:>8.3} ns");
        }
    }

    // ── (3) Realer Anteil über das Doktrin-Gitter ────────────────────────────
    println!("\n── §8t (3) Feld-Kosten über das echte Scan-Gitter ──");
    println!(
        "{:<16} {:>8} {:>9} {:>9} {:>8} {:>10} {:>10} {:>9}",
        "Fixture", "Stücke", "Pkt", "live%", "L-Aufr", "mit L ns", "ohne L ns", "L-Anteil"
    );
    for (name, field, lo, hi, n) in &fx {
        let mut bs = Vec::new();
        collect_lb(field, &mut bs);
        let np: usize = bs.iter().map(|b| b.pieces.len()).sum();
        if np == 0 {
            println!("{name:<16} {np:>8}   (kein Block)");
            continue;
        }
        let bare = strip_lb(field);
        let mut pts: Vec<[f32; 3]> = Vec::with_capacity(n * n * n);
        for i in 0..*n {
            for j in 0..*n {
                for k in 0..*n {
                    let f = |a: f64, b: f64, t: usize| (a + (b - a) * t as f64 / (*n - 1) as f64) as f32;
                    pts.push([f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)]);
                }
            }
        }
        // live-Anteil + Anzahl echter min-Läufe
        let mut stops = Vec::new();
        lb_stops(field, &mut stops);
        let (mut live, mut calls) = (0usize, 0usize);
        for p in &pts {
            let mut any = false;
            for (inner, d, b) in &stops {
                let base = inner.sdf(*p, EvalMode::Visual) + *d;
                let dc = ((p[0] - b.center[0]).powi(2) + (p[1] - b.center[1]).powi(2)
                    + (p[2] - b.center[2]).powi(2)).sqrt();
                if base <= b.delta && dc <= b.skip_r {
                    calls += 1;
                    any = true;
                }
            }
            if any {
                live += 1;
            }
        }
        let bench = |f: &BoundedCsg| -> f64 {
            let mut acc = 0.0f32;
            let t0 = std::time::Instant::now();
            for p in &pts {
                acc += f.sdf(*p, EvalMode::Visual);
            }
            let el = t0.elapsed().as_nanos() as f64 / pts.len() as f64;
            std::hint::black_box(acc);
            el
        };
        // je zwei Läufe, Minimum (Cache-Warmlauf)
        let with = bench(field).min(bench(field));
        let without = bench(&bare).min(bench(&bare));
        println!(
            "{name:<16} {np:>8} {:>9} {:>8.1}% {calls:>8} {with:>10.0} {without:>10.0} {:>8.1}%",
            pts.len(),
            100.0 * live as f64 / pts.len() as f64,
            100.0 * (with - without) / with
        );
    }

    // ── (4) Wie viel KANN ein Prune überhaupt? (Orakel-Schranke) ─────────────
    //
    // Die Frage vor jedem Format-Umbau: gäbe es den PERFEKTEN laufenden `best`
    // (= das wahre Minimum, von Anfang an), wie viele Stücke dürfte eine
    // konservative Schranke `B(x,k) ≤ dist(x,Stück_k)` dann überspringen? Das
    // ist die OBERE Grenze jeder Prune-Strategie mit dieser Schranke — schafft
    // das Orakel es nicht, schafft es kein Tape-Format.
    //
    // Drei Schranken, alle gültig:
    //   `sph`   `max(0, |x−c_k| − r_k)`      (Hüllkugel, beide Stück-Arten)
    //   `pln`   `|n̂·x − n̂·a|`               (Trägerebene, nur `Tri`)
    //   `max`   das Maximum beider           (Maximum gültiger Schranken)
    println!("\n── §8t (4) Orakel-Prune: was eine perfekte Schranke überspringen dürfte ──");
    println!(
        "{:<16} {:>7} {:>7} {:>9} {:>9} {:>9} {:>8} {:>8} {:>8} {:>8}",
        "Fixture", "Stücke", "live", "r_med", "r_max", "best_med",
        "sph%", "pln%", "max%", "BVH%"
    );
    for (name, field, lo, hi, n) in &fx {
        let mut stops = Vec::new();
        lb_stops(field, &mut stops);
        if stops.is_empty() {
            continue;
        }
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for i in 0..*n {
            for j in 0..*n {
                for k in 0..*n {
                    let f = |a: f64, b: f64, t: usize| (a + (b - a) * t as f64 / (*n - 1) as f64) as f32;
                    pts.push([f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)]);
                }
            }
        }
        let np: usize = stops.iter().map(|(_, _, b)| b.pieces.len()).sum();
        let (mut cnt, mut sph, mut pln, mut mx, mut bvh) = (0usize, 0f64, 0f64, 0f64, 0f64);
        let mut bests: Vec<f32> = Vec::new();
        let mut radii: Vec<f32> = Vec::new();
        let mut tri_inside = (0usize, 0usize);
        for (_, _, b) in &stops {
            for q in &b.pieces {
                radii.push(piece_sphere(q).1);
            }
        }
        radii.sort_by(|a, c| a.partial_cmp(c).unwrap());
        for p in &pts {
            for (inner, d, b) in &stops {
                let base = inner.sdf(*p, EvalMode::Visual) + *d;
                let dc = ((p[0] - b.center[0]).powi(2) + (p[1] - b.center[1]).powi(2)
                    + (p[2] - b.center[2]).powi(2)).sqrt();
                if !(base <= b.delta && dc <= b.skip_r) {
                    continue;
                }
                let m = b.pieces.len();
                let ds: Vec<f32> = b.pieces.iter().map(|q| crate::offset_lb::piece_dist(*p, q)).collect();
                let bd = ds.iter().cloned().fold(f32::INFINITY, f32::min);
                cnt += 1;
                if bests.len() < 20000 {
                    bests.push(bd);
                }
                let (mut ns, mut npl, mut nm) = (0usize, 0usize, 0usize);
                for q in &b.pieces {
                    let (c, r) = piece_sphere(q);
                    let bs = (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()
                        - r).max(0.0);
                    let bp = match *q {
                        crate::offset_lb::LbPiece::Tri { a, b: bb, c: cc, .. } => {
                            let e1 = [bb[0] - a[0], bb[1] - a[1], bb[2] - a[2]];
                            let e2 = [cc[0] - a[0], cc[1] - a[1], cc[2] - a[2]];
                            let nr = [
                                e1[1] * e2[2] - e1[2] * e2[1],
                                e1[2] * e2[0] - e1[0] * e2[2],
                                e1[0] * e2[1] - e1[1] * e2[0],
                            ];
                            let ln = (nr[0] * nr[0] + nr[1] * nr[1] + nr[2] * nr[2]).sqrt();
                            if ln > 1e-20 {
                                ((p[0] - a[0]) * nr[0] + (p[1] - a[1]) * nr[1] + (p[2] - a[2]) * nr[2]).abs() / ln
                            } else {
                                0.0
                            }
                        }
                        _ => 0.0,
                    };
                    if bs >= bd { ns += 1; }
                    if bp >= bd { npl += 1; }
                    if bs.max(bp) >= bd { nm += 1; }
                }
                sph += ns as f64 / m as f64;
                pln += npl as f64 / m as f64;
                mx += nm as f64 / m as f64;
                // Echte BVH: Median-Split über die Stück-Kugeln, näheres Kind
                // zuerst, laufendes `best`.
                let idx: Vec<usize> = (0..m).collect();
                let mut visited = 0usize;
                let mut running = f32::INFINITY;
                bvh_visit(*p, &b.pieces, &idx, &mut running, &mut visited);
                assert!((running - bd).abs() <= 0.0, "{name}: BVH ändert das min");
                bvh += visited as f64 / m as f64;
                // Innen-Anteil von `tri_dist`
                for q in &b.pieces {
                    if let crate::offset_lb::LbPiece::Tri { a, b: bb, c: cc, .. } = *q {
                        tri_inside.1 += 1;
                        if tri_is_inside(*p, a, bb, cc) {
                            tri_inside.0 += 1;
                        }
                    }
                }
            }
        }
        if cnt == 0 {
            println!("{name:<16} {np:>7} {cnt:>7}  (nie live)");
            continue;
        }
        bests.sort_by(|a, c| a.partial_cmp(c).unwrap());
        let f = cnt as f64;
        println!(
            "{name:<16} {np:>7} {cnt:>7} {:>9.3} {:>9.3} {:>9.3} {:>7.1}% {:>7.1}% {:>7.1}% {:>7.1}%   Tri-innen {:.1}%",
            radii[radii.len() / 2], radii[radii.len() - 1], bests[bests.len() / 2],
            100.0 * sph / f, 100.0 * pln / f, 100.0 * mx / f, 100.0 * bvh / f,
            100.0 * tri_inside.0 as f64 / tri_inside.1.max(1) as f64
        );
    }
}

/// **§8t-Perf-Nachweis: flacher Lauf gegen Prune, an der Produktion.**
///
/// Drei Tabellen, weil eine einzelne Zahl hier lügt:
///
/// * **(A)** exakt die §8o/§8p-Methodik (EIN Sondenpunkt `[1,1,−1]`,
///   `base = 0.5`), damit die Zeilen mit den veröffentlichten 237 … 4142 ns
///   vergleichbar bleiben;
/// * **(B)** über die LEBENDIGEN Gitterpunkte der Doktrin-Gitter — die Menge,
///   an der `L` real ausgewertet wird;
/// * **(C)** das ganze Feld über das ganze Gitter, also inklusive aller
///   Punkte, an denen der Fernfeld-Skip den Block gar nicht erst betritt.
#[test]
#[ignore]
fn probe_384_lb_pruning_perf() {
    let delta = 2.0f32;
    let fx = p8t_fixtures(delta);
    let bench = |f: &dyn Fn([f32; 3]) -> f32, pts: &[[f32; 3]]| -> f64 {
        let mut acc = 0.0f32;
        let t0 = std::time::Instant::now();
        for q in pts {
            acc += f(*q);
        }
        let el = t0.elapsed().as_nanos() as f64 / pts.len() as f64;
        std::hint::black_box(acc);
        el
    };

    println!("\n── §8t (A) §8o-Methodik: EIN Sondenpunkt [1,1,−1], base 0.5 ──");
    println!("{:<16} {:>7} {:>10} {:>10} {:>8}", "Fixture", "Stücke", "flach ns", "prune ns", "Faktor");
    for (name, field, ..) in &fx {
        let mut bs = Vec::new();
        collect_lb(field, &mut bs);
        let Some(b) = bs.first() else { continue };
        let pts: Vec<[f32; 3]> = (0..200_000u32)
            .map(|k| [1.0 + (k % 7) as f32 * 1e-4, 1.0, -1.0])
            .collect();
        let flat = |q: [f32; 3]| crate::offset_lb::lb_value_flat(q, 0.5, b);
        let prun = |q: [f32; 3]| crate::offset_lb::lb_value(q, 0.5, b);
        let tf = bench(&flat, &pts).min(bench(&flat, &pts));
        let tp = bench(&prun, &pts).min(bench(&prun, &pts));
        let livemark = if crate::offset_lb::lb_value_flat([1.0, 1.0, -1.0], 0.5, b)
            == crate::offset_lb::LB_NONE
        {
            " (Block dort tot)"
        } else {
            ""
        };
        println!(
            "{name:<16} {:>7} {tf:>10.0} {tp:>10.0} {:>7.2}×{livemark}",
            b.pieces.len(),
            tf / tp.max(1e-9)
        );
    }

    println!("\n── §8t (B) `L` allein, über die LEBENDIGEN Gitterpunkte ──");
    println!(
        "{:<16} {:>7} {:>8} {:>10} {:>10} {:>8} {:>9}",
        "Fixture", "Stücke", "Punkte", "flach ns", "prune ns", "Faktor", "besucht%"
    );
    for (name, field, lo, hi, n) in &fx {
        let mut bs = Vec::new();
        collect_lb(field, &mut bs);
        let Some(b) = bs.first() else { continue };
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for i in 0..*n {
            for j in 0..*n {
                for k in 0..*n {
                    let f = |a: f64, c: f64, t: usize| (a + (c - a) * t as f64 / (*n - 1) as f64) as f32;
                    let q = [f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)];
                    if crate::offset_lb::lb_value_flat(q, -1e6, b) != crate::offset_lb::LB_NONE {
                        pts.push(q);
                    }
                }
            }
        }
        if pts.is_empty() {
            continue;
        }
        let (mut seen, mut tot) = (0usize, 0usize);
        for q in &pts {
            let mut best = f32::INFINITY;
            for pc in &b.pieces {
                tot += 1;
                if let crate::offset_lb::LbPiece::Tri { a, n, emax, .. } = *pc {
                    if crate::offset_lb::tri_cannot_beat(*q, a, n, emax, best) {
                        continue;
                    }
                }
                seen += 1;
                let v = crate::offset_lb::piece_dist(*q, pc);
                if v < best {
                    best = v;
                }
            }
        }
        let flat = |q: [f32; 3]| crate::offset_lb::lb_value_flat(q, -1e6, b);
        let prun = |q: [f32; 3]| crate::offset_lb::lb_value(q, -1e6, b);
        let tf = bench(&flat, &pts).min(bench(&flat, &pts)).min(bench(&flat, &pts));
        let tp = bench(&prun, &pts).min(bench(&prun, &pts)).min(bench(&prun, &pts));
        println!(
            "{name:<16} {:>7} {:>8} {tf:>10.0} {tp:>10.0} {:>7.2}× {:>8.1}%",
            b.pieces.len(),
            pts.len(),
            tf / tp.max(1e-9),
            100.0 * seen as f64 / tot.max(1) as f64
        );
    }

    println!("\n── §8t (C) FELD über das ganze Doktrin-Gitter ──");
    println!("{:<16} {:>7} {:>9} {:>10} {:>10} {:>8}", "Fixture", "Stücke", "live%", "vorher ns", "nachher ns", "Faktor");
    for (name, field, lo, hi, n) in &fx {
        let mut bs = Vec::new();
        collect_lb(field, &mut bs);
        let np: usize = bs.iter().map(|b| b.pieces.len()).sum();
        if np == 0 {
            continue;
        }
        let mut pts: Vec<[f32; 3]> = Vec::new();
        for i in 0..*n {
            for j in 0..*n {
                for k in 0..*n {
                    let f = |a: f64, c: f64, t: usize| (a + (c - a) * t as f64 / (*n - 1) as f64) as f32;
                    pts.push([f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)]);
                }
            }
        }
        let mut stops = Vec::new();
        lb_stops(field, &mut stops);
        let mut live = 0usize;
        for p in &pts {
            for (inner, d, b) in &stops {
                let base = inner.sdf(*p, EvalMode::Visual) + *d;
                let dc = ((p[0] - b.center[0]).powi(2) + (p[1] - b.center[1]).powi(2)
                    + (p[2] - b.center[2]).powi(2)).sqrt();
                if base <= b.delta && dc <= b.skip_r {
                    live += 1;
                }
            }
        }
        let fieldb = |f: &BoundedCsg| -> f64 {
            let mut acc = 0.0f32;
            let t0 = std::time::Instant::now();
            for p in &pts {
                acc += f.sdf(*p, EvalMode::Visual);
            }
            let el = t0.elapsed().as_nanos() as f64 / pts.len() as f64;
            std::hint::black_box(acc);
            el
        };
        let now = fieldb(field).min(fieldb(field));
        // Zusatzzeit des flachen Laufs an genau den lebendigen Punkten —
        // derselbe Baum, nur die `L`-Strategie getauscht.
        let mut extra = 0f64;
        for (inner, d, b) in &stops {
            let mut sub: Vec<[f32; 3]> = Vec::new();
            for p in &pts {
                let base = inner.sdf(*p, EvalMode::Visual) + *d;
                let dc = ((p[0] - b.center[0]).powi(2) + (p[1] - b.center[1]).powi(2)
                    + (p[2] - b.center[2]).powi(2)).sqrt();
                if base <= b.delta && dc <= b.skip_r {
                    sub.push(*p);
                }
            }
            if sub.is_empty() {
                continue;
            }
            let flat = |q: [f32; 3]| crate::offset_lb::lb_value_flat(q, -1e6, b);
            let prun = |q: [f32; 3]| crate::offset_lb::lb_value(q, -1e6, b);
            let tf = bench(&flat, &sub).min(bench(&flat, &sub));
            let tp = bench(&prun, &sub).min(bench(&prun, &sub));
            extra += (tf - tp) * sub.len() as f64;
        }
        let before = now + extra / pts.len() as f64;
        println!(
            "{name:<16} {np:>7} {:>8.1}% {before:>10.0} {now:>10.0} {:>7.2}×",
            100.0 * live as f64 / pts.len() as f64,
            before / now
        );
    }
}

/// Der Innen-Test von [`crate::offset_lb::tri_dist`] (für die Anteils-Messung).
fn tri_is_inside(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    let sub = |u: [f32; 3], v: [f32; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
    let cr = |u: [f32; 3], v: [f32; 3]| {
        [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
    };
    let dt = |u: [f32; 3], v: [f32; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let (ba, cb, ac) = (sub(b, a), sub(c, b), sub(a, c));
    let nor = cr(ba, ac);
    if dt(nor, nor) <= 1e-24 {
        return false;
    }
    let s = |e: [f32; 3], w: [f32; 3]| if dt(cr(e, nor), w) >= 0.0 { 1.0f32 } else { -1.0 };
    s(ba, sub(p, a)) + s(cb, sub(p, b)) + s(ac, sub(p, c)) >= 2.0
}

/// Rekursive BVH-Simulation (Median-Split auf der längsten Achse der
/// Stück-Zentren, näheres Kind zuerst).
fn bvh_visit(
    p: [f32; 3],
    pieces: &[crate::offset_lb::LbPiece],
    idx: &[usize],
    best: &mut f32,
    visited: &mut usize,
) {
    if idx.is_empty() {
        return;
    }
    // Knoten-Hülle
    let sub: Vec<crate::offset_lb::LbPiece> = idx.iter().map(|i| pieces[*i]).collect();
    let (c, r) = group_sphere(&sub);
    let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt();
    if (d - r).max(0.0) >= *best {
        return;
    }
    if idx.len() <= 4 {
        for i in idx {
            *visited += 1;
            let v = crate::offset_lb::piece_dist(p, &pieces[*i]);
            if v < *best {
                *best = v;
            }
        }
        return;
    }
    // Median-Split auf der längsten Ausdehnung der Zentren
    let ctrs: Vec<[f32; 3]> = idx.iter().map(|i| piece_sphere(&pieces[*i]).0).collect();
    let mut ax = 0usize;
    let mut span = -1.0f32;
    for k in 0..3 {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for q in &ctrs {
            lo = lo.min(q[k]);
            hi = hi.max(q[k]);
        }
        if hi - lo > span {
            span = hi - lo;
            ax = k;
        }
    }
    let mut order: Vec<usize> = (0..idx.len()).collect();
    order.sort_by(|a, b| ctrs[*a][ax].partial_cmp(&ctrs[*b][ax]).unwrap());
    let half = order.len() / 2;
    let left: Vec<usize> = order[..half].iter().map(|k| idx[*k]).collect();
    let right: Vec<usize> = order[half..].iter().map(|k| idx[*k]).collect();
    let near = |v: &[usize]| {
        let sub: Vec<crate::offset_lb::LbPiece> = v.iter().map(|i| pieces[*i]).collect();
        let (c, r) = group_sphere(&sub);
        (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt() - r).max(0.0)
    };
    if near(&left) <= near(&right) {
        bvh_visit(p, pieces, &left, best, visited);
        bvh_visit(p, pieces, &right, best, visited);
    } else {
        bvh_visit(p, pieces, &right, best, visited);
        bvh_visit(p, pieces, &left, best, visited);
    }
}

/// **§8t-Exaktheits-Pin: der Prune ändert `L` an keinem Punkt um ein Bit.**
///
/// Das ist die eine Zusage des Pakets. `L` ist eine BEWIESENE untere Schranke
/// von `dist(x,∁Ω)`; ein Stück wegzulassen, das das `min` gesenkt hätte, macht
/// `L` grösser als die Wahrheit und erlaubt Phantom-Material (§8o, Regel 1).
/// Der Prune ist deshalb nicht „nah genug", sondern **wertgleich**: geprüft
/// wird `to_bits()`, nicht eine Toleranz, an jedem Gitterpunkt aller 13
/// Fixturen, an dem der Block lebendig ist.
///
/// **Was dieser Pin beweist und was nicht.** Er ist die EMPIRISCHE Hälfte:
/// er deckt genau diese 13 Fixturen ab. Die allgemeine Aussage steht auf dem
/// Budget-Lemma in `offset_lb::LbPiece::tri` — und dass ein Pin allein hier
/// nicht reicht, ist keine Vermutung: die erste §8t-Fassung war auf denselben
/// 13 Fixturen grün und trotzdem unsound (`lb_old_emax_budget_was_unsound_384`
/// findet an einer Sliver-Familie Verletzungen bis 0.62 mm). Pin und Lemma
/// zusammen, nicht der Pin allein.
#[test]
fn lb_pruning_is_bit_exact_384() {
    let delta = 2.0f32;
    let (mut checked, mut diff, mut skipped) = (0usize, 0usize, 0usize);
    for (name, field, lo, hi, n) in p8t_fixtures(delta) {
        let mut bs = Vec::new();
        collect_lb(&field, &mut bs);
        for b in &bs {
            for i in 0..n {
                for j in 0..n {
                    for k in 0..n {
                        let f = |a: f64, c: f64, t: usize| {
                            (a + (c - a) * t as f64 / (n - 1) as f64) as f32
                        };
                        let p = [f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)];
                        // `base = −1e6` hält den Innen-Indikator offen: der Pin
                        // misst die Stück-Schleife, nicht das Gate davor.
                        let a = crate::offset_lb::lb_value(p, -1e6, b);
                        let c = crate::offset_lb::lb_value_flat(p, -1e6, b);
                        if a.to_bits() != c.to_bits() {
                            diff += 1;
                        }
                        if a != crate::offset_lb::LB_NONE {
                            checked += 1;
                            // Wie viele Stücke der Prune wirklich spart —
                            // wäre das 0, prüfte der Pin nichts.
                            let mut best = f32::INFINITY;
                            for pc in &b.pieces {
                                if let crate::offset_lb::LbPiece::Tri { a, n, emax, .. } = *pc {
                                    if crate::offset_lb::tri_cannot_beat(p, a, n, emax, best) {
                                        skipped += 1;
                                        continue;
                                    }
                                }
                                let v = crate::offset_lb::piece_dist(p, pc);
                                if v < best {
                                    best = v;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(diff, 0, "{name}: {diff} Bit-Abweichungen zwischen Prune und flachem Lauf");
    }
    assert!(checked > 20_000, "nur {checked} lebendige Punkte geprüft — der Pin misst zu wenig");
    assert!(
        skipped > 100_000,
        "der Prune hat nur {skipped} Stücke übersprungen — dann pinnt der Test die \
         Wertgleichheit einer Maschine, die gar nicht prunet"
    );
}

/// **§8t-Gegenprobe: eine künstlich zu kleine `emax` LÄSST den Pin fallen.**
///
/// Ohne sie wäre `lb_pruning_is_bit_exact_384` wertlos — er könnte auch dann
/// grün sein, wenn der Prune nie greift oder die Schranke beliebig wäre. Hier
/// wird `emax` (der Sicherheitsabzug, der das Rundungs-Residuum der genäherten
/// Normale deckt) auf `−0.05 mm` gesetzt, also die Schranke um 0.05 mm ZU
/// SCHARF gemacht. Dann darf der Prune Stücke verwerfen, die das `min` sehr
/// wohl gesenkt hätten — und genau das muss sichtbar werden.
#[test]
fn lb_pruning_counter_probe_too_tight_emax_384() {
    let delta = 2.0f32;
    let mut worst = 0usize;
    for (_, field, lo, hi, n) in p8t_fixtures(delta) {
        let mut bs = Vec::new();
        collect_lb(&field, &mut bs);
        for b in &bs {
            let mut bad = b.clone();
            for pc in bad.pieces.iter_mut() {
                if let crate::offset_lb::LbPiece::Tri { emax, .. } = pc {
                    *emax = -0.05;
                }
            }
            let mut diff = 0usize;
            for i in 0..n {
                for j in 0..n {
                    for k in 0..n {
                        let f = |a: f64, c: f64, t: usize| {
                            (a + (c - a) * t as f64 / (n - 1) as f64) as f32
                        };
                        let p = [f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)];
                        if crate::offset_lb::lb_value(p, -1e6, &bad).to_bits()
                            != crate::offset_lb::lb_value_flat(p, -1e6, b).to_bits()
                        {
                            diff += 1;
                        }
                    }
                }
            }
            worst = worst.max(diff);
        }
    }
    assert!(
        worst > 0,
        "die Gegenprobe blieb still: mit einer um 0.05 mm ZU SCHARFEN Schranke muss der \
         Prune messbar falsch werden, sonst prüft der Exaktheits-Pin nichts"
    );
}

// ══════════════ #384 §8t-Korrektur — das Budget gegen die Kondition ══════════
//
// Das externe Mathe-Review hat die Ebenen-Schranke geometrisch bestätigt und
// das RUNDUNGS-BUDGET verworfen: `emax = res + 64·2⁻²⁴·R` deckt nur Terme, die
// mit der Reichweite skalieren. `tri_dist` hat aber einen Anteil, der mit den
// KANTENLÄNGEN wächst, und einen, der mit der KONDITION `κ = E²/|N|` wächst.
// Die zwei Tests hier sind die Gegenprobe dazu: der erste zeigt, dass das ALTE
// Budget an einer konstruierbaren Familie REISST, der zweite, dass das neue
// sie trägt.

/// Das ALTE §8t-Budget (`res + 64·2⁻²⁴·R`) — nur für die Gegenprobe.
fn emax_old_384(a: [f32; 3], b: [f32; 3], c: [f32; 3], center: [f32; 3], skip_r: f32) -> f32 {
    let f = |v: [f32; 3]| [v[0] as f64, v[1] as f64, v[2] as f64];
    let (af, bf, cf) = (f(a), f(b), f(c));
    let s = |u: [f64; 3], v: [f64; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
    let d = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let (ba, ca) = (s(bf, af), s(cf, af));
    let nx = [
        ba[1] * ca[2] - ba[2] * ca[1],
        ba[2] * ca[0] - ba[0] * ca[2],
        ba[0] * ca[1] - ba[1] * ca[0],
    ];
    let ln = d(nx, nx).sqrt();
    if !(ln > 0.0) {
        return f32::INFINITY;
    }
    let n32 = [(nx[0] / ln) as f32, (nx[1] / ln) as f32, (nx[2] / ln) as f32];
    let nh = f(n32);
    let res = d(nh, ba).abs().max(d(nh, ca).abs());
    let reach = skip_r as f64 + d(s(af, f(center)), s(af, f(center))).sqrt();
    (res + 64.0 * (-24f64).exp2() * reach) as f32
}

/// Die Sliver-Familie: fast kollineare Dreiecke mit grosser Kantenlänge, bei
/// denen das f32-Kreuzprodukt in `tri_dist` Auslöschung erleidet — `κ` gross.
/// Der Punkt wird über die Fläche gelegt, damit der Ebenen-Zweig greift.
fn sliver_family_384() -> Vec<([f32; 3], [f32; 3], [f32; 3], Vec<[f32; 3]>)> {
    let mut out = Vec::new();
    let mut rng = 0x384_0084u64;
    let mut nx = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (rng >> 33) as f64 / (1u64 << 31) as f64 - 0.5
    };
    for k in 0..60 {
        // Zwei fast parallele Kanten von grosser Länge, schief im Raum —
        // genau die Lage, in der `cross` in f32 auslöscht.
        let len = 60.0 + 40.0 * k as f64;
        let dir: [f64; 3] = {
            let v = [0.6 + nx() * 0.2, 0.5 + nx() * 0.2, 0.62 + nx() * 0.2];
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / n, v[1] / n, v[2] / n]
        };
        // Senkrechte Richtung
        let perp: [f64; 3] = {
            let t: [f64; 3] = if dir[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            let v = [
                dir[1] * t[2] - dir[2] * t[1],
                dir[2] * t[0] - dir[0] * t[2],
                dir[0] * t[1] - dir[1] * t[0],
            ];
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / n, v[1] / n, v[2] / n]
        };
        let thin: f64 = 10f64.powf(-2.0 - 2.0 * (k % 7) as f64 / 6.0);
        let a: [f64; 3] = [nx() * 20.0, nx() * 20.0, nx() * 20.0];
        let b: [f64; 3] = [
            a[0] + dir[0] * len,
            a[1] + dir[1] * len,
            a[2] + dir[2] * len,
        ];
        let c: [f64; 3] = [
            a[0] + dir[0] * len * 0.5 + perp[0] * thin,
            a[1] + dir[1] * len * 0.5 + perp[1] * thin,
            a[2] + dir[2] * len * 0.5 + perp[2] * thin,
        ];
        let v32 = |q: [f64; 3]| [q[0] as f32, q[1] as f32, q[2] as f32];
        // Punkte über der Fläche, in der Mitte der langen Kante (dort schlägt
        // der Orientierungstest auf „innen").
        let mut pts = Vec::new();
        for m in 0..24 {
            let h = 0.05 + 0.6 * m as f64;
            let t = 0.2 + 0.6 * (m % 5) as f64 / 5.0;
            pts.push(v32([
                a[0] + dir[0] * len * t + perp[0] * thin * 0.4 + (-perp[0]) * 0.0 + dir[0] * 0.0
                    + h * (dir[1] * perp[2] - dir[2] * perp[1]),
                a[1] + dir[1] * len * t + perp[1] * thin * 0.4
                    + h * (dir[2] * perp[0] - dir[0] * perp[2]),
                a[2] + dir[2] * len * t + perp[2] * thin * 0.4
                    + h * (dir[0] * perp[1] - dir[1] * perp[0]),
            ]));
        }
        out.push((v32(a), v32(b), v32(c), pts));
    }
    out
}

/// Wertet die Prune-Bedingung mit einem GEGEBENEN Budget aus und meldet, ob
/// die Soundness verletzt ist: `|H| ≥ best + emax` behauptet
/// `tri_dist ≥ best` — verletzt ist es, wenn `tri_dist < best`.
///
/// Geprüft wird die schärfste Form (`best` = die Behauptung selbst,
/// `best := |H| − emax`): genau dann darf `tri_dist` nicht darunter liegen.
fn budget_violation_384(
    p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3], n: [f32; 3], emax: f32,
) -> Option<f32> {
    if !emax.is_finite() {
        return None;
    }
    let h = n[0] * (p[0] - a[0]) + n[1] * (p[1] - a[1]) + n[2] * (p[2] - a[2]);
    let claim = h.abs() - emax;
    if !(claim > 0.0) {
        return None;
    }
    let d = crate::offset_lb::tri_dist(p, a, b, c);
    if d < claim {
        Some(claim - d)
    } else {
        None
    }
}

/// **§8t-Korrektur, Gegenprobe 1: das ALTE Budget war unhaltbar.**
///
/// Nicht „vielleicht knapp", sondern konstruierbar falsch: an fast kollinearen
/// Dreiecken mit grosser Kantenlänge löscht das f32-Kreuzprodukt in `tri_dist`
/// aus, der Ebenen-Zweig liefert eine Distanz, die um ein Vielfaches des alten
/// Budgets unter der Behauptung liegt — der Prune hätte ein Stück verworfen,
/// das das `min` sehr wohl gesenkt hätte. `L` würde die Distanz ÜBERschätzen:
/// Phantom-Material.
///
/// Der Test ist damit der Beleg, dass die §8t-Korrektur tragend ist und nicht
/// Kosmetik.
#[test]
fn lb_old_emax_budget_was_unsound_384() {
    let (center, skip_r) = ([0.0f32; 3], 400.0f32);
    let (mut hits, mut worst) = (0usize, 0.0f32);
    for (a, b, c, pts) in sliver_family_384() {
        let f = |v: [f32; 3]| [v[0] as f64, v[1] as f64, v[2] as f64];
        let s = |u: [f64; 3], v: [f64; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
        let (ba, ca) = (s(f(b), f(a)), s(f(c), f(a)));
        let nx = [
            ba[1] * ca[2] - ba[2] * ca[1],
            ba[2] * ca[0] - ba[0] * ca[2],
            ba[0] * ca[1] - ba[1] * ca[0],
        ];
        let ln = (nx[0] * nx[0] + nx[1] * nx[1] + nx[2] * nx[2]).sqrt();
        if !(ln > 0.0) {
            continue;
        }
        let n32 = [(nx[0] / ln) as f32, (nx[1] / ln) as f32, (nx[2] / ln) as f32];
        let eold = emax_old_384(a, b, c, center, skip_r);
        for p in &pts {
            if let Some(v) = budget_violation_384(*p, a, b, c, n32, eold) {
                hits += 1;
                worst = worst.max(v);
            }
        }
    }
    assert!(
        hits > 0,
        "die Gegenprobe blieb still — dann wäre die §8t-Korrektur unbegründet. \
         Das alte Budget MUSS an der Sliver-Familie reissen."
    );
    println!("§8t alt: {hits} Soundness-Verletzungen, schlimmste {worst:.6} mm");
    assert!(
        worst > 1e-4,
        "nur {worst:.3e} mm Verletzung — das wäre im Rauschen, der Test träfe \
         nicht den beschriebenen Struktur-Fehler"
    );
}

/// **§8t-Korrektur, Gegenprobe 2: das NEUE Budget trägt dieselbe Familie.**
///
/// Entweder das Budget deckt den Fehler, oder `κ` ist so gross, dass
/// [`crate::offset_lb::LbPiece::tri`] den Prune für dieses Stück ganz abschaltet
/// (`emax = ∞`). Beides ist sound; verboten ist nur ein endliches Budget, das
/// zu klein ist.
#[test]
fn lb_new_emax_budget_covers_the_sliver_family_384() {
    let (center, skip_r) = ([0.0f32; 3], 400.0f32);
    let (mut checked, mut off, mut live) = (0usize, 0usize, 0usize);
    for (a, b, c, pts) in sliver_family_384() {
        let crate::offset_lb::LbPiece::Tri { n, emax, .. } =
            crate::offset_lb::LbPiece::tri(a, b, c, center, skip_r)
        else {
            unreachable!()
        };
        if !emax.is_finite() {
            off += 1;
        } else {
            live += 1;
        }
        for p in &pts {
            checked += 1;
            assert!(
                budget_violation_384(*p, a, b, c, n, emax).is_none(),
                "neues Budget verletzt: Dreieck {a:?}/{b:?}/{c:?}, Punkt {p:?}, emax {emax:e}"
            );
        }
    }
    println!("§8t neu: {checked} Punkte, {off} Stücke mit Prune AUS, {live} mit aktivem Budget");
    assert!(checked > 1000, "nur {checked} Punkte geprüft");
}

/// **§8t-Korrektur: das Budget hält auch an gut konditionierten Dreiecken.**
///
/// Die Sliver-Familie zeigt die Abschalt-Richtung; dieser Test zeigt die
/// andere: an gesunden Dreiecken (`κ ≈ 1`) bleibt `emax` endlich UND korrekt,
/// sonst hätte die Korrektur den Prune stillschweigend überall abgeschaltet
/// und der Perf-Gewinn wäre eine Illusion.
#[test]
fn lb_new_emax_stays_sharp_on_healthy_triangles_384() {
    let (center, skip_r) = ([0.0f32; 3], 120.0f32);
    let mut rng = 0xA5A5_1234u64;
    let mut nx = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((rng >> 33) as f64 / (1u64 << 31) as f64 - 0.5) as f32
    };
    let (mut live, mut checked) = (0usize, 0usize);
    for _ in 0..400 {
        let a = [nx() * 60.0, nx() * 60.0, nx() * 60.0];
        let b = [a[0] + nx() * 40.0, a[1] + nx() * 40.0, a[2] + nx() * 40.0];
        let c = [a[0] + nx() * 40.0, a[1] + nx() * 40.0, a[2] + nx() * 40.0];
        let crate::offset_lb::LbPiece::Tri { n, emax, .. } =
            crate::offset_lb::LbPiece::tri(a, b, c, center, skip_r)
        else {
            unreachable!()
        };
        if emax.is_finite() {
            live += 1;
            // Ein gesundes Dreieck darf kein millimetergrosses Budget haben —
            // sonst prunet nichts mehr.
            assert!(
                emax < 0.05,
                "gesundes Dreieck mit emax = {emax:e} mm — das Budget ist zu stumpf"
            );
        }
        for _ in 0..20 {
            let p = [nx() * 100.0, nx() * 100.0, nx() * 100.0];
            checked += 1;
            assert!(
                budget_violation_384(p, a, b, c, n, emax).is_none(),
                "Budget verletzt an gesundem Dreieck {a:?}/{b:?}/{c:?}, Punkt {p:?}"
            );
        }
    }
    assert!(live > 300, "nur {live} von 400 gesunden Dreiecken haben ein endliches Budget");
    assert!(checked > 5000, "nur {checked} Punkte geprüft");
}

/// §8t-Korrektur-Diagnose: Verteilung von `emax` und der Kondition `κ` je
/// Fixture — die Frage, warum die Budget-Verschärfung am 296-Stück-Kamm
/// Schärfe kostet.
#[test]
#[ignore]
fn probe_384_lb_emax_distribution() {
    let delta = 2.0f32;
    println!(
        "\n{:<16} {:>7} {:>6} {:>10} {:>10} {:>10} {:>10}",
        "Fixture", "Stücke", "aus", "emax med", "emax max", "κ med", "κ max"
    );
    for (name, field, ..) in p8t_fixtures(delta) {
        let mut bs = Vec::new();
        collect_lb(&field, &mut bs);
        let Some(b) = bs.first() else { continue };
        let (mut es, mut ks, mut off) = (Vec::new(), Vec::new(), 0usize);
        for pc in &b.pieces {
            if let crate::offset_lb::LbPiece::Tri { a, b: bb, c, emax, .. } = *pc {
                if emax.is_finite() {
                    es.push(emax);
                } else {
                    off += 1;
                }
                let f = |v: [f32; 3]| [v[0] as f64, v[1] as f64, v[2] as f64];
                let (af, bf, cf) = (f(a), f(bb), f(c));
                let s = |u: [f64; 3], v: [f64; 3]| [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
                let (ba, ca) = (s(bf, af), s(cf, af));
                let nx = [
                    ba[1] * ca[2] - ba[2] * ca[1],
                    ba[2] * ca[0] - ba[0] * ca[2],
                    ba[0] * ca[1] - ba[1] * ca[0],
                ];
                let ln = (nx[0] * nx[0] + nx[1] * nx[1] + nx[2] * nx[2]).sqrt();
                let l = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let e = l(ba).max(l(ca)).max(l(s(cf, bf)));
                ks.push(if ln > 0.0 { e * e / ln } else { f64::INFINITY });
            }
        }
        es.sort_by(|a, c| a.partial_cmp(c).unwrap());
        ks.sort_by(|a, c| a.partial_cmp(c).unwrap());
        if es.is_empty() {
            println!("{name:<16} {:>7} {off:>6}  (alle aus)", b.pieces.len());
            continue;
        }
        println!(
            "{name:<16} {:>7} {off:>6} {:>10.2e} {:>10.2e} {:>10.2} {:>10.2}",
            b.pieces.len(), es[es.len() / 2], es[es.len() - 1],
            ks[ks.len() / 2], ks[ks.len() - 1]
        );
    }
}

// ══════════════ #384 §8u — die Intervall-Stufe, Zell-Zertifizierung ══════════
//
// §8r hat den Zeugen gebaut und ihn Punkte sondieren lassen; §8u fragt nach
// ZELLEN. Zertifiziert wird „kein x ∈ C erfüllt f(x) ≥ 0 ∧ dist(x,∁Ω) > δ+tol",
// mit zwei hinreichenden Zweigen (s. `crate::offset_interval`):
//
//   Zweig 1  U(c) + ρ ≤ δ + tol       — 1-Lipschitz, ohne Intervall-Arithmetik
//   Zweig 2  sup_C f < 0              — Intervall-Auswertung des Feldes
//
// Der Zeuge bleibt daneben stehen: er sucht ANWESENHEIT, die Zertifizierung
// beweist ABWESENHEIT. Beide schweigen aus verschiedenen Gründen.

use crate::offset_channel::VertexSphere;
use crate::offset_interval as iv;
use crate::offset_interval::CellDomain as _;

/// **Die obere Distanzschranke `U(x) ≥ dist(x,∁Ω)`** — der §8r-Fuss-Zeuge,
/// aus [`under_cert`]s Kaskade herausgezogen, ohne die `cap`-Abbrüche.
///
/// Das Argument: ein Punkt `z` mit `f_inner(z) > δ + CERT_MARGIN` liegt
/// BEWEISBAR in `∁Ω` (Vorzeichen-Invariante des naiven Vereinigungsfeldes),
/// also ist `|x − z|` eine obere Schranke von `dist(x,∁Ω)`. `None` heisst:
/// kein Aussen-Zeuge gefunden — dann gibt es für diesen Punkt kein `U`, und
/// Zweig 1 ist stumm.
///
/// **§8u-Korrektur (zweiter blockierender Befund des vierten Mathe-Reviews),
/// drei Brücken:**
///
/// * **Der bewiesene Punkt ist `fl32(y)`, nicht `y`.** `stop.sdf` wird am
///   f32-gerundeten Punkt ausgewertet; ausserhalb `Ω` liegt also `fl32(y)`.
///   Gemessen wird die Distanz jetzt zu GENAU diesem Punkt.
/// * **`d₀ > δ` liefert nicht `U = 0`.** Es beweist `fl32(c) ∉ Ω` — für den
///   reellen Zellmittelpunkt `c` folgt daraus `dist(c,∁Ω) ≤ |c − fl32(c)|`,
///   nicht `0`. Die Rundungsdistanz ist winzig, aber sie ist nicht null, und
///   eine falsche Null ist genau der Fehler, den Zweig 1 nicht machen darf.
/// * **Die Norm wird nach aussen gerundet.** Jede Teilrechnung von `|c − z|`
///   geht durch `next_up`; `U` ist eine Zusage nach OBEN.
///
/// Zusätzlich wird für den Innen-/Aussen-Test durchgehend dieselbe Marge
/// `CERT_MARGIN` verlangt wie beim Marsch — vorher stand im `d₀`-Zweig ein
/// blosses `>`. Das kostet nur Abdeckung.
fn dist_up(a: [f64; 3], b: [f64; 3]) -> f64 {
    let mut s = 0.0f64;
    for k in 0..3 {
        let d = a[k] - b[k];
        s = crate::offset_interval::next_up(s + crate::offset_interval::next_up(d * d));
    }
    crate::offset_interval::next_up(s.sqrt())
}

/// Der f32-gerundete Punkt, an dem die Produktion tatsächlich auswertet.
fn as_f32_point(y: [f64; 3]) -> [f64; 3] {
    [y[0] as f32 as f64, y[1] as f32 as f64, y[2] as f32 as f64]
}

fn u_upper(
    stop: &BoundedCsg,
    blk: &crate::offset_lb::LbBlock,
    delta: f32,
    p: [f64; 3],
    reach: f64,
) -> Option<f64> {
    let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
    let d0 = stop.sdf(pf, EvalMode::Visual);
    if d0 > delta + CERT_MARGIN {
        // `fl32(p) ∉ Ω` — bewiesen ist die Aussenlage des GERUNDETEN Punktes.
        return Some(dist_up(p, as_f32_point(p)));
    }
    let pushed = |q: [f32; 3]| -> Option<[f64; 3]> {
        let dir = [q[0] as f64 - p[0], q[1] as f64 - p[1], q[2] as f64 - p[2]];
        let n = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        (n > 1e-9).then(|| {
            [
                q[0] as f64 + CERT_PUSH * dir[0] / n,
                q[1] as f64 + CERT_PUSH * dir[1] / n,
                q[2] as f64 + CERT_PUSH * dir[2] / n,
            ]
        })
    };
    let is_outside = |y: [f64; 3]| {
        stop.sdf([y[0] as f32, y[1] as f32, y[2] as f32], EvalMode::Visual) > delta + CERT_MARGIN
    };
    let mut best: Option<f64> = None;
    let foot = crate::offset_lb::lb_foot(pf, d0, blk);
    if let Some(y) = foot.and_then(pushed).filter(|y| is_outside(*y)) {
        best = Some(dist_up(p, as_f32_point(y)));
    }
    for start in [foot.map(|q| [q[0] as f64, q[1] as f64, q[2] as f64]), Some(p)]
        .into_iter()
        .flatten()
    {
        if let Some(y) = march_outside(stop, delta, start, reach) {
            let u = dist_up(p, as_f32_point(y));
            best = Some(best.map_or(u, |b: f64| b.min(u)));
        }
    }
    best
}

/// Eine Zeile der §8u-Zertifizierungs-Tabelle.
#[derive(Default, Clone)]
struct CellRow {
    name: String,
    cells: usize,
    b1: usize,
    b2: usize,
    reasons: std::collections::BTreeMap<&'static str, usize>,
}

impl CellRow {
    fn unknown(&self) -> usize {
        self.cells - self.b1 - self.b2
    }
    fn top_reason(&self) -> String {
        let mut v: Vec<_> = self.reasons.iter().collect();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        v.iter().take(2).map(|(k, n)| format!("{k}·{n}")).collect::<Vec<_>>().join(", ")
    }
}

/// Ein Zell-Lauf über eine Fixture: das Doktrin-Gitter, aber als ZELLEN statt
/// als Punkte gelesen (`n³` achsenparallele Kästen, lückenlos und
/// überschneidungsfrei — zusammen decken sie den ganzen Quader).
///
/// `depth > 0` schaltet die adaptive Unterteilung zu: eine unentschiedene
/// Zelle wird halbiert, und sie zählt nur dann als zertifiziert, wenn JEDE
/// Teilzelle es ist. Das Erreichen der Maximaltiefe ist `Unknown`, nie Erfolg.
fn cell_scan_at(
    name: &str,
    field: &BoundedCsg,
    u_src: &BoundedCsg,
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
    depth: u32,
) -> CellRow {
    let (stop, blk) = stop_with_lb(u_src);
    let cap = delta as f64 + CERT_TOL;
    let u_at = |c: [f64; 3]| u_upper(stop, blk, delta, c, 2.0 * cap);
    let mut row = CellRow { name: name.into(), ..Default::default() };
    for cell in fixture_cells(lo, hi, n) {
        let mut ctx = iv::Ctx::new();
        row.cells += 1;
        if depth == 0 {
            match iv::certify_cell(&cell, field, delta as f64, CERT_TOL, &u_at, &mut ctx) {
                iv::Verdict::Branch1 => row.b1 += 1,
                iv::Verdict::Branch2 => row.b2 += 1,
                iv::Verdict::Unknown(why) => *row.reasons.entry(why).or_insert(0) += 1,
            }
            continue;
        }
        match iv::certify_cell_adaptive(
            &cell,
            field,
            delta as f64,
            CERT_TOL,
            &u_at,
            depth,
            &mut ctx,
        ) {
            Ok((a, b)) => {
                // Eine Zelle zählt für den Zweig, der die MEISTEN ihrer
                // Teilzellen getragen hat — die Tabelle bleibt so lesbar.
                if a >= b {
                    row.b1 += 1;
                } else {
                    row.b2 += 1;
                }
            }
            Err(why) => *row.reasons.entry(why).or_insert(0) += 1,
        }
    }
    row
}

fn cell_scan(
    name: &str,
    field: &BoundedCsg,
    delta: f32,
    lo: [f64; 3],
    hi: [f64; 3],
    n: usize,
) -> CellRow {
    cell_scan_at(name, field, field, delta, lo, hi, n, 0)
}

/// **§8u-Sonde: die Zertifizierungs-Tabelle über die volle §8p-Matrix.**
///
/// Drei Spaltenblöcke: das AUSGELIEFERTE Feld (das ist die Zusage), dasselbe
/// mit adaptiver Unterteilung (Tiefe 3), und die Vertex-Ablation (der
/// vollständig unterstützte Ausschnitt — er zeigt, was Zweig 2 kann, sobald
/// ihm die Naht-Distanz zur Verfügung steht).
#[test]
#[ignore]
fn probe_384_interval_cell_certification() {
    let delta = 2.0f32;
    println!(
        "| Fixture | Zellen | Z1 | Z2 | Unknown | Hauptgründe | Z1 (adaptiv) | Z2 (adaptiv) | \
         Unknown (adaptiv) | Z2 (Vertex-Ablation) |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (name, field, _exact, lo, hi, _n) in cert_matrix(delta) {
        let r = cell_scan(&name, &field, delta, lo, hi, 12);
        let a = cell_scan_at(&name, &field, &field, delta, lo, hi, 12, 3);
        let cut = strip_channels(&field);
        let v = cell_scan_at(&name, &cut, &field, delta, lo, hi, 12, 0);
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.name,
            r.cells,
            r.b1,
            r.b2,
            r.unknown(),
            r.top_reason(),
            a.b1,
            a.b2,
            a.unknown(),
            v.b2
        );
    }
}

/// **§8u-Sonde: das Term-Inventar der Matrix** — wer hat Kanäle, wer
/// Vertex-Kugeln, woraus besteht der `L`-Block.
#[test]
#[ignore]
fn probe_384_interval_stop_inventory() {
    let delta = 2.0f32;
    println!("| Fixture | Kanäle | Vertex-Kugeln | Tri | Rev |");
    println!("|---|---|---|---|---|");
    for (name, field, _e, _lo, _hi, _n) in cert_matrix(delta) {
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        for st in stops {
            if let BoundedCsg::ArcChannel { channels, verts, lb, .. } = st {
                let (mut tri, mut rev) = (0usize, 0usize);
                if let Some(b) = lb {
                    for p in &b.pieces {
                        match p {
                            crate::offset_lb::LbPiece::Tri { .. } => tri += 1,
                            crate::offset_lb::LbPiece::Rev { .. } => rev += 1,
                        }
                    }
                }
                println!(
                    "| {} | {} | {} | {} | {} |",
                    name,
                    channels.len(),
                    verts.len(),
                    tri,
                    rev
                );
            }
        }
    }
}

/// **§8u-Sonde: welcher Knoten-Typ steht im Baum?** (Für die ehrliche
/// Benennung der `Unknown("Operator")`-Fälle.)
#[test]
#[ignore]
fn probe_384_interval_node_kinds() {
    fn walk(n: &BoundedCsg, out: &mut std::collections::BTreeMap<&'static str, usize>) {
        let k: &'static str = match n {
            BoundedCsg::Empty => "Empty",
            BoundedCsg::Leaf { primitive, .. } => match primitive {
                Primitive::Box { .. } => "Leaf/Box",
                Primitive::Cylinder { .. } => "Leaf/Cylinder",
                Primitive::Sphere { .. } => "Leaf/Sphere",
                _ => "Leaf/ANDERES",
            },
            BoundedCsg::Union { .. } => "Union",
            BoundedCsg::Difference { .. } => "Difference",
            BoundedCsg::Intersection { .. } => "Intersection",
            BoundedCsg::Offset { .. } => "Offset",
            BoundedCsg::InnerWall { .. } => "InnerWall",
            BoundedCsg::ArcChannel { .. } => "ArcChannel",
            BoundedCsg::Extrude { .. } => "Extrude",
            BoundedCsg::Revolve { .. } => "Revolve",
            _ => "SONSTIGES",
        };
        *out.entry(k).or_insert(0) += 1;
        match n {
            BoundedCsg::Union { left, right, .. }
            | BoundedCsg::Difference { left, right, .. }
            | BoundedCsg::Intersection { left, right, .. } => {
                walk(left, out);
                walk(right, out);
            }
            BoundedCsg::Offset { inner, .. }
            | BoundedCsg::InnerWall { inner, .. }
            | BoundedCsg::ArcChannel { inner, .. } => walk(inner, out),
            _ => {}
        }
    }
    for (name, field, _e, _lo, _hi, _n) in cert_matrix(2.0) {
        let mut m = std::collections::BTreeMap::new();
        walk(&field, &mut m);
        println!("{name:<24} {m:?}");
    }
}

/// **§8u-Sonde: wie tief liegt die Kanal-Neutralität?** Für jede Fixture die
/// Trust-Radien und Bounding-Radien der Kanäle — die Zahlen, an denen die
/// `channel_skip`-Neutralität scheitert.
#[test]
#[ignore]
fn probe_384_interval_channel_reach() {
    for (name, field, _e, _lo, _hi, _n) in cert_matrix(2.0) {
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        for st in stops {
            if let BoundedCsg::ArcChannel { channels, .. } = st {
                let mut tr: Vec<f32> = channels.iter().map(|c| c.trust).collect();
                let mut br: Vec<f32> = channels.iter().map(|c| c.bound_r).collect();
                let mut ar: Vec<f32> =
                    channels.iter().flat_map(|c| c.arcs.iter().map(|a| a.r)).collect();
                for v in [&mut tr, &mut br, &mut ar] {
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                }
                let med = |v: &Vec<f32>| if v.is_empty() { f32::NAN } else { v[v.len() / 2] };
                println!(
                    "{name:<24} K med {:.2} max {:.2} | bound_r med {:.2} max {:.2} | \
                     arc.r med {:.2} max {:.2}",
                    med(&tr),
                    tr.last().copied().unwrap_or(f32::NAN),
                    med(&br),
                    br.last().copied().unwrap_or(f32::NAN),
                    med(&ar),
                    ar.last().copied().unwrap_or(f32::NAN),
                );
            }
        }
    }
}

/// Die **Vertex-Ablation**: derselbe Baum, aber ohne Kanal-Terme. Kein
/// ausgeliefertes Feld — ein Schnitt, an dem die vollständig unterstützte
/// Term-Klasse des Pakets 1 (Vertex-Kugeln, `L`-Block, Grenzwert-Klemme) allein
/// steht. Er beantwortet die Frage, die ein leerer Zweig 2 offenlässt: rechnet
/// die Einschliessung überhaupt, oder ist sie tote Ausnahme?
fn strip_channels(n: &BoundedCsg) -> BoundedCsg {
    match n {
        BoundedCsg::ArcChannel { inner, verts, rims, lb, aabb, .. } => BoundedCsg::ArcChannel {
            inner: Box::new(strip_channels(inner)),
            channels: Vec::new(),
            verts: verts.clone(),
            rims: rims.clone(),
            lb: lb.clone(),
            aabb: *aabb,
        },
        BoundedCsg::Union { left, right, aabb } => BoundedCsg::Union {
            left: Box::new(strip_channels(left)),
            right: Box::new(strip_channels(right)),
            aabb: *aabb,
        },
        BoundedCsg::Difference { left, right, aabb } => BoundedCsg::Difference {
            left: Box::new(strip_channels(left)),
            right: Box::new(strip_channels(right)),
            aabb: *aabb,
        },
        BoundedCsg::Intersection { left, right, aabb } => BoundedCsg::Intersection {
            left: Box::new(strip_channels(left)),
            right: Box::new(strip_channels(right)),
            aabb: *aabb,
        },
        BoundedCsg::Offset { inner, delta, aabb } => BoundedCsg::Offset {
            inner: Box::new(strip_channels(inner)),
            delta: *delta,
            aabb: *aabb,
        },
        other => other.clone(),
    }
}

/// **§8u-Pin 1: die Algebra rundet nach aussen, und der f32-Schritt trägt.**
///
/// Drei Behauptungen, alle drei falsifizierbar:
///
/// * `next_up`/`next_down` sind die echten IEEE-Nachbarn (kein Wert liegt
///   dazwischen), und `f32_down`/`f32_up` klammern jedes f64 zwischen zwei
///   benachbarte f32.
/// * **Elementare Inclusion** (Beweispflicht 1): für zufällige f32-Operanden
///   in den Intervallen liegt das PRODUKTIONS-f32-Ergebnis von `+ − × ÷ √`
///   in der Einschliessung.
/// * Der f32-Schritt ist NICHT verzichtbar: es gibt Operanden, bei denen der
///   f32-Produktionswert die reelle obere Schranke ÜBERSTEIGT (Aufrundung).
#[test]
fn interval_arithmetic_is_outward_and_f32_sound_384() {
    use iv::{f32_down, f32_up, next_down, next_up, Ctx, Iv};
    // Nachbarschaft.
    for x in [0.0f64, 1.0, -1.0, 1e-300, 1e300, -3.5, 2.5e-8] {
        assert!(next_up(x) > x && next_down(x) < x, "next_up/down bei {x} nicht strikt");
        assert_eq!(next_down(next_up(x)), x, "next_down∘next_up ist keine Identität bei {x}");
    }
    for x in [0.3f64, -0.3, 1.0 / 3.0, 1e-45, 7.7e30, -2.0f64.powi(-150)] {
        assert!(f32_down(x) <= x && f32_up(x) >= x, "f32-Klammer verletzt bei {x}");
        assert!(
            (f32_down(x) as f32 as f64) == f32_down(x) && (f32_up(x) as f32 as f64) == f32_up(x),
            "f32-Klammer liegt nicht auf dem f32-Gitter bei {x}"
        );
    }
    // Elementare Inclusion gegen die echte f32-Arithmetik.
    let mut s = 0x243f_6a88_85a3_08d3u64;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    };
    let mut c = Ctx::new();
    let (mut checked, mut lifted) = (0usize, 0usize);
    for _ in 0..4000 {
        let scale = 10f64.powi(((rnd() * 6.0) as i32).clamp(-6, 6));
        let a0 = (rnd() * scale) as f32;
        let a1 = a0 + (rnd().abs() * scale) as f32;
        let b0 = (rnd() * scale) as f32;
        let b1 = b0 + (rnd().abs() * scale) as f32;
        let (ia, ib) = (
            Iv { lo: a0 as f64, hi: a1 as f64 },
            Iv { lo: b0 as f64, hi: b1 as f64 },
        );
        let add = c.fadd(ia, ib).unwrap();
        let sub = c.fsub(ia, ib).unwrap();
        let mul = c.fmul(ia, ib).unwrap();
        for t in 0..5 {
            let f = t as f32 / 4.0;
            // Die Interpolation selbst rundet — ohne `clamp` kann die Probe
            // knapp AUSSERHALB des Intervalls landen und einen Scheinfehler
            // melden.
            let x = (a0 + (a1 - a0) * f).clamp(a0, a1);
            let y = (b1 + (b0 - b1) * f).clamp(b0, b1);
            checked += 3;
            assert!(add.contains((x + y) as f64), "f32-Summe {x}+{y} nicht in {add:?}");
            assert!(sub.contains((x - y) as f64), "f32-Differenz {x}-{y} nicht in {sub:?}");
            assert!(mul.contains((x * y) as f64), "f32-Produkt {x}*{y} nicht in {mul:?}");
            // Die Aufrundung: der f32-Wert kann die REELLE Schranke übersteigen —
            // genau dafür steht der f32-Schritt im `seal`.
            if (x * y) as f64 > (x as f64) * (y as f64) {
                lifted += 1;
            }
        }
        if ib.lo > 0.0 {
            let d = c.fdiv(ia, ib).unwrap();
            for t in 0..3 {
                let f = t as f32 / 2.0;
                let x = (a0 + (a1 - a0) * f).clamp(a0, a1);
                let y = (b0 + (b1 - b0) * (1.0 - f)).clamp(b0, b1);
                checked += 1;
                assert!(d.contains((x / y) as f64), "f32-Quotient {x}/{y} nicht in {d:?}");
            }
        }
        if ia.lo >= 0.0 {
            let r = c.fsqrt(ia).unwrap();
            for t in 0..3 {
                let x = (a0 + (a1 - a0) * (t as f32 / 2.0)).clamp(a0, a1);
                checked += 1;
                assert!(r.contains(x.sqrt() as f64), "f32-Wurzel von {x} nicht in {r:?}");
            }
        }
    }
    assert!(checked > 40_000, "nur {checked} Proben — der Pin misst nichts");
    assert!(
        lifted > 0,
        "kein einziges f32-Produkt lag ÜBER dem exakten reellen Wert — dann kann der Pin \
         nicht zeigen, dass der f32-Schritt im `seal` tragend ist"
    );
    // Die sichere Wurzel über einem nullhaltigen Radikanden.
    let z = c.fsqrt(Iv { lo: -1e-9, hi: 4.0 }).unwrap();
    assert!(z.lo == 0.0 && z.hi >= 2.0, "safe_sqrt über nullhaltigem Radikanden falsch: {z:?}");
    assert!(c.fsqrt(Iv { lo: -4.0, hi: -1e-9 }).is_err(), "sqrt(<0) muss fail-closed sein");
    assert!(
        c.fdiv(Iv { lo: 1.0, hi: 2.0 }, Iv { lo: -1.0, hi: 1.0 }).is_err(),
        "Division durch ein nullhaltiges Intervall muss fail-closed sein"
    );
}

/// Punkte einer Zelle: acht Ecken, Mittelpunkt und ein deterministisches
/// Untergitter. Für eine FALSIFIKATION sind die Ecken das Schärfste — dort
/// nimmt jede monotone Komponente ihr Extremum an.
fn cell_samples(cell: &iv::Box3, sub: usize) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    for i in 0..=sub {
        for j in 0..=sub {
            for k in 0..=sub {
                let f = |t: usize, d: usize| {
                    cell.lo[d] + (cell.hi[d] - cell.lo[d]) * t as f64 / sub as f64
                };
                out.push([f(i, 0), f(j, 1), f(k, 2)]);
            }
        }
    }
    out
}

/// Ein von Hand gebautes Intervall auf dem **f32-Gitter** — dieselbe Disziplin
/// wie `Box3::world_f32`: die Einschliessung spricht über f32-Werte, also
/// müssen auch die Eingangs-Intervalle jede f32-Rundung ihrer Grenzen
/// enthalten. (Ohne das meldet der Pin Scheinfehler: `-0.006f64` liegt unter
/// `-0.006f32`.)
fn ivf(lo: f64, hi: f64) -> iv::Iv {
    iv::Iv { lo: iv::f32_down(lo), hi: iv::f32_up(hi) }
}

/// Drei Proben eines Intervalls (Rand, Rand, Mitte).
fn iv_samples(v: iv::Iv) -> [f32; 3] {
    let m = 0.5 * (v.lo + v.hi);
    [v.lo as f32, v.hi as f32, if m.is_finite() { m as f32 } else { v.lo as f32 }]
}

/// Eine synthetische Vertex-Kugel mit orthogonalen Kegel-Facetten.
fn synth_vertex(v: [f32; 3], delta: f32, trust: f32, gate: f32, concave: bool) -> VertexSphere {
    let s = 1.0f32 / 3.0f32.sqrt();
    VertexSphere {
        v,
        gates: [[s, s, s], [-s, s, s], [s, -s, s]],
        delta,
        concave,
        trust,
        gate,
        branch: 0x700,
    }
}

/// Zählt, an wie vielen Proben der PRODUKTIONS-Wert von `vertex_apply`
/// ausserhalb der Einschliessung liegt. Null ist die Zusage.
fn vertex_enclosure_escapes(
    knobs: iv::Knobs,
    vs: &VertexSphere,
    cell: &iv::Box3,
    base: iv::Iv,
    d: iv::Iv,
    extra: iv::Iv,
    sub: usize,
) -> (usize, usize, usize) {
    let mut c = iv::Ctx::with_knobs(knobs);
    // §8u-Korrektur: ein `Unknown` darf nicht als „null Ausreisser" durchgehen —
    // sonst kann die ganze Familie leer-grün sein. Der dritte Rückgabewert
    // zählt die tatsächlich AUSGEWERTETEN Fälle.
    let Ok(enc) = iv::enclose_vertex_apply(&mut c, cell.world_f32(), base, d, vs, extra) else {
        return (0, 0, 0);
    };
    let (mut esc, mut n) = (0usize, 0usize);
    for p in cell_samples(cell, sub) {
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        for b in iv_samples(base) {
            for dd in iv_samples(d) {
                for e in iv_samples(extra) {
                    let got = crate::offset_channel::vertex_apply(b, dd, pf, vs, e);
                    n += 1;
                    if !got.is_finite() || !enc.contains(got as f64) {
                        esc += 1;
                    }
                }
            }
        }
    }
    (esc, n, 1)
}

/// Die adversariale Zellen-/Term-Familie des Pakets: jede Zeile zielt auf EINE
/// Schaltfläche oder EINE Rundungs-Achse.
fn adversarial_vertex_cases() -> Vec<(String, VertexSphere, iv::Box3, iv::Iv, iv::Iv, iv::Iv)> {
    let mut out = Vec::new();
    let full = iv::Iv { lo: -6.0, hi: 3.0 };
    for &(scale, tag) in &[(1.0e-3f64, "µ"), (1.0f64, "1"), (1.0e3f64, "k")] {
        let (delta, trust, gate) = ((2.0 * scale) as f32, (3.91 * scale) as f32, (0.87 * scale) as f32);
        for &(off, otag) in &[([0.0f64; 3], "origin"), ([1.0e5, -3.0e4, 7.0e4], "translatiert")] {
            let v = [off[0] as f32, off[1] as f32, off[2] as f32];
            let vs_cc = synth_vertex(v, delta, trust, gate, true);
            let vs_cx = synth_vertex(v, delta, trust, gate, false);
            let ctr = |r: f64| [off[0] + r, off[1] + r * 0.3, off[2] - r * 0.7];
            let d_full = ivf(full.lo * scale, full.hi * scale);
            // (1) Radikand enthält NULL: die Zelle umschliesst `v`.
            out.push((
                format!("{tag}/{otag} ρ-Null"),
                vs_cc.clone(),
                iv::Box3::cube(off, 0.4 * scale),
                d_full,
                d_full,
                ivf(-1.0e30, 1.0),
            ));
            // (2) beliebig kleiner positiver Unterrand des Radikanden.
            out.push((
                format!("{tag}/{otag} ρ-fast-Null"),
                vs_cc.clone(),
                iv::Box3::new(
                    [off[0] + 1e-7 * scale, off[1] - 0.2 * scale, off[2] - 0.2 * scale],
                    [off[0] + 0.3 * scale, off[1] + 0.2 * scale, off[2] + 0.2 * scale],
                ),
                d_full,
                d_full,
                ivf(-1.0e30, 1.0),
            ));
            // (3) Zelle schneidet die Skip-Fläche ρ = K.
            out.push((
                format!("{tag}/{otag} ρ=K"),
                vs_cc.clone(),
                iv::Box3::cube(ctr(trust as f64 * 0.577), 0.25 * scale),
                d_full,
                d_full,
                ivf(-1.0e30, 1.0),
            ));
            // (4) Zelle schneidet ρ = δ (die `w_geo`-Schaltfläche).
            out.push((
                format!("{tag}/{otag} ρ=δ"),
                vs_cc.clone(),
                iv::Box3::cube(ctr(delta as f64 * 0.577), 0.2 * scale),
                d_full,
                d_full,
                ivf(-1.0e30, 1.0),
            ));
            // (5) `extra` überspannt die LB_AVAIL-Umschaltung EXAKT.
            out.push((
                format!("{tag}/{otag} LB_AVAIL"),
                vs_cc.clone(),
                iv::Box3::cube(ctr(delta as f64 * 0.6), 0.3 * scale),
                d_full,
                d_full,
                ivf(crate::offset_channel::LB_AVAIL as f64 - 1.0e28, 0.5),
            ));
            // (6) SLIVER: eine Achse verschwindend dünn, quer zur Kegel-Facette.
            out.push((
                format!("{tag}/{otag} Sliver"),
                vs_cc.clone(),
                iv::Box3::new(
                    [off[0] - 0.9 * scale, off[1] - 1e-9 * scale, off[2] - 0.9 * scale],
                    [off[0] + 0.9 * scale, off[1] + 1e-9 * scale, off[2] + 0.9 * scale],
                ),
                d_full,
                d_full,
                ivf(-1.0e30, 1.0),
            ));
            // (7) der KONVEXE Zweig (`d.max(w)`) mit derselben Familie.
            out.push((
                format!("{tag}/{otag} konvex"),
                vs_cx,
                iv::Box3::cube(ctr(delta as f64 * 0.577), 0.3 * scale),
                d_full,
                d_full,
                ivf(-1.0e30, 1.0),
            ));
            // (8) `w = 0` exakt: `gate` als Punkt genau auf der Schwelle.
            out.push((
                format!("{tag}/{otag} w=0"),
                vs_cc.clone(),
                iv::Box3::cube(ctr(delta as f64 * 0.58), 0.15 * scale),
                ivf(0.0, 0.0),
                d_full,
                ivf(0.0, 0.0),
            ));
            // (9) **Der Drei-Wege-Schalter, scharf gestellt.** `w_geo > 0` über
            //     der ganzen Zelle, `gate` überspannt die Null: Zweig A
            //     (`min(d,−w)`) und Zweig C (`d`) liegen weit auseinander, und
            //     `d` ist ein PUNKT weit über null — genau die Konstellation,
            //     an der ein willkürlich gewählter Zweig auffliegt.
            let cone = [off[0], off[1], off[2] + 3.0 * scale];
            out.push((
                format!("{tag}/{otag} Kegel w±0"),
                vs_cc.clone(),
                iv::Box3::cube(cone, 0.1 * scale),
                ivf(0.5 * scale, 0.5 * scale),
                ivf(0.5 * scale, 0.5 * scale),
                ivf(-0.05 * scale, 0.05 * scale),
            ));
            // (10) **Die Luftseiten-Klemme auf dem Rand.** `w_geo = ρ − δ`
            //      knapp negativ, `gate` weit offen, `d` gross: das Ergebnis
            //      IST `−w_geo` und liegt damit exakt auf der Intervall-
            //      Schranke — die Stelle, an der ein ulp zu scharf reisst.
            let band = [off[0], off[1], off[2] + 1.75 * scale];
            out.push((
                format!("{tag}/{otag} ρ≲δ-Klemme"),
                vs_cc,
                iv::Box3::cube(band, 0.25 * scale),
                ivf(5.0 * scale, 5.0 * scale),
                ivf(5.0 * scale, 5.0 * scale),
                ivf(10.0 * scale, 10.0 * scale),
            ));
        }
    }
    out
}

/// **§8u-Pin 2: die Einschliessung von `vertex_apply` hält gegen die
/// Produktion.**
///
/// Die Doktrin des Pakets: dichte Proben sind hier **Falsifikatoren der
/// Einschliessung**, nicht Belege der Zertifizierung. Jede Familie zielt auf
/// eine Schaltfläche (Skip, `ρ = δ`, `LB_AVAIL`, `w = 0`), auf die
/// `safe_sqrt`-Entartung (Radikand enthält null / beliebig kleiner positiver
/// Unterrand), auf die Dependency-Breite (Sliver) oder auf die
/// Rundungs-Skalierung (Translation um 10⁵, Massstab 10⁻³ … 10³).
///
/// Dazu die ECHTEN Vertex-Kugeln der Ecken-Schale — die synthetische Familie
/// prüft die Schalter, die echte prüft, dass die Formel dieselbe ist.
#[test]
fn interval_encloses_the_production_vertex_term_384() {
    let (mut cases, mut probes) = (0usize, 0usize);
    for (name, vs, cell, base, d, extra) in adversarial_vertex_cases() {
        let (esc, n, ok) =
            vertex_enclosure_escapes(iv::Knobs::default(), &vs, &cell, base, d, extra, 3);
        assert_eq!(esc, 0, "{name}: {esc} von {n} Produktionswerten liegen AUSSERHALB der Einschliessung");
        cases += ok;
        probes += n;
    }
    // Die echten Kugeln der Ecken-Schale.
    let delta = 2.0f32;
    let (field, _) = compile_for(thin_corner_node(2.5 * delta, 40.0, delta));
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let mut real = 0usize;
    for st in stops {
        let BoundedCsg::ArcChannel { verts, .. } = st else { continue };
        for vs in verts {
            for h in [0.05f64, 0.4, 1.7] {
                let cell = iv::Box3::cube(
                    [vs.v[0] as f64 + 0.11, vs.v[1] as f64 - 0.07, vs.v[2] as f64 + 0.23],
                    h,
                );
                for extra in [ivf(-1.0e30, -1.0e30), ivf(-1.0e30, 0.9), ivf(-0.6, 0.9)] {
                    let dv = ivf(-7.0, 2.0);
                    let (esc, n, ok) =
                        vertex_enclosure_escapes(iv::Knobs::default(), vs, &cell, dv, dv, extra, 2);
                    assert_eq!(esc, 0, "echte Kugel {:?} h={h}: {esc} von {n} Werte entkommen", vs.v);
                    real += ok;
                    probes += n;
                }
            }
        }
    }
    assert!(
        cases >= 40 && real >= 24,
        "nur {cases} synthetische / {real} echte Fälle wurden überhaupt AUSGEWERTET          (der Rest war `Unknown`) — eine leer-grüne Familie belegt nichts"
    );
    assert!(probes > 120_000, "nur {probes} Proben — der Pin misst nichts");
}


/// Zählt Ausreisser einer Feld-Einschliessung über einer Zelle.
fn field_enclosure_escapes(
    node: &BoundedCsg,
    cell: &iv::Box3,
    sub: usize,
) -> (usize, usize, bool, u64) {
    let mut c = iv::Ctx::new();
    let Ok(enc) = iv::enclose_node(&mut c, node, cell.world_f32()) else {
        return (0, 0, false, 0);
    };
    let (mut esc, mut n) = (0usize, 0usize);
    for p in cell_samples(cell, sub) {
        let v = node.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
        n += 1;
        if !enc.contains(v as f64) {
            esc += 1;
        }
    }
    (esc, n, true, c.channel_evals())
}

/// Ein Zellen-Raster über einer Fixture (dieselbe Aufteilung wie [`cell_scan`]).
fn fixture_cells(lo: [f64; 3], hi: [f64; 3], n: usize) -> Vec<iv::Box3> {
    let step = |k: usize, d: usize| lo[d] + (hi[d] - lo[d]) * k as f64 / n as f64;
    let mut out = Vec::new();
    for ix in 0..n {
        for iy in 0..n {
            for iz in 0..n {
                out.push(iv::Box3::new(
                    [step(ix, 0), step(iy, 1), step(iz, 2)],
                    [step(ix + 1, 0), step(iy + 1, 1), step(iz + 1, 2)],
                ));
            }
        }
    }
    out
}

/// **§8u-Pin 3: die Feld-Einschliessung hält gegen den Auswerter.**
///
/// #384 §8ac: trägt irgendein Kanal dieses Feldes einen bewiesenen
/// Laufzeitmodus? Dann verweigert die Intervall-Stufe den Stopp (P5b-1A).
fn fixture_has_proved_mode(field: &BoundedCsg) -> bool {
    let mut stops = Vec::new();
    stop_nodes(field, &mut stops);
    stops.iter().any(|st| match st {
        BoundedCsg::ArcChannel { channels, .. } => {
            channels.iter().any(|c| c.limit_mode().is_proved())
        }
        _ => false,
    })
}

/// Zwei Ebenen, beide über der echten §8p-Matrix:
///
/// * das **naive Vereinigungsfeld** unter dem Stopp (`Union`/`Difference`/
///   `Offset`/`Leaf` — inklusive der AABB-Prunes, die NICHT als korrekt
///   vorausgesetzt, sondern als Fallunterscheidung eingeschlossen werden);
/// * die **Vertex-Ablation** des ausgelieferten Feldes (Vertex-Kugeln,
///   `L`-Block, Grenzwert-Klemme);
/// * seit §8v das **ausgelieferte Feld selbst** — also inklusive der
///   Kanal-Terme mit Nearest-Segment, `nlerp` und den beiden affinen Gates.
///
/// Ein Ausreisser wäre der Beweis, dass die Einschliessung lügt.
#[test]
fn interval_encloses_the_delivered_field_384() {
    let delta = 2.0f32;
    let (mut cells, mut probes, mut evaluated) = (0usize, 0usize, 0usize);
    let mut delivered = 0usize;
    let mut channel_work = 0u64;
    let mut refused = 0usize;
    for (name, field, _exact, lo, hi, _n) in cert_matrix(delta) {
        if !(name.starts_with("Stufen")
            || name == "Ecken-Schale t/δ=2.5"
            || name == "Rippen-Kamm 4×2/1"
            || name == "notch α=90")
        {
            continue;
        }
        let (stop, _blk) = stop_with_lb(&field);
        let cut = strip_channels(&field);
        for cell in fixture_cells(lo, hi, 5) {
            for (tag, node) in
                [("naiv", stop), ("Vertex-Ablation", &cut), ("ausgeliefert", &field)]
            {
                let (esc, n, ok, chev) = field_enclosure_escapes(node, &cell, 2);
                assert_eq!(
                    esc, 0,
                    "{name}/{tag}: {esc} von {n} Feldwerten liegen AUSSERHALB der \
                     Einschliessung, Zelle {cell:?}"
                );
                cells += 1;
                probes += n;
                evaluated += ok as usize;
                if tag == "ausgeliefert" {
                    // #384 §8ac / P5b-1A: trägt der Stopp einen BEWIESENEN
                    // Laufzeitmodus, verweigert die Intervall-Stufe ihn
                    // ausdrücklich (die alte Kette ist nicht mehr die
                    // ausgelieferte). Das ist kein Verlust an Wahrheit,
                    // sondern das Gegenteil: eine Zusage, die nicht mehr gilt,
                    // wird nicht mehr gegeben. P5b-2 holt sie zurück.
                    if fixture_has_proved_mode(&field) {
                        refused += 1;
                    } else if ok {
                        delivered += 1;
                        channel_work += chev;
                    }
                }
            }
        }
    }
    assert!(cells >= 1200, "nur {cells} Zell-Läufe");
    assert!(probes > 25_000, "nur {probes} Proben — der Pin misst nichts");
    assert!(
        refused > 100,
        "nur {refused} Zellen mit bewiesenem Laufzeitmodus — dann prüft der Pin die \
         P5b-1A-Verweigerung nicht"
    );
    assert!(
        delivered >= 60,
        "nur {delivered} Zellen des AUSGELIEFERTEN Feldes (LEGACY-Stopps) liefern überhaupt \
         eine Einschliessung — dann prüft der Pin den §8v-Kanal-Pfad nicht"
    );
    // Eine Einschliessung allein beweist noch nicht, dass ein Kanal GERECHNET
    // wurde — sie könnte auch aus lauter neutralen Kanälen entstehen.
    assert!(
        channel_work >= 100,
        "nur {channel_work} nicht-neutrale Kanal-Auswertungen im ausgelieferten Feld — \
         dann trägt der §8v-Pfad den Pin nicht"
    );
    assert!(
        evaluated * 2 >= cells,
        "nur {evaluated} von {cells} Zellen liefern überhaupt eine Einschliessung — \
         dann prüft der Pin überwiegend `Unknown` und nicht die Arithmetik"
    );
}

/// **§8u-Pin 4: Unterteilung erhält die Einschliessung.**
///
/// Jede Teilzelle muss den Produktionswert an JEDER ihrer Proben weiterhin
/// enthalten — und die Unterteilung muss die Intervalle messbar verengen,
/// sonst ist die adaptive Rekursion ein Placebo.
#[test]
fn interval_subdivision_keeps_every_sample_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(thin_corner_node(2.5 * delta, 40.0, delta));
    let cut = strip_channels(&field);
    let (stop, _b) = stop_with_lb(&field);
    let (mut kids, mut narrowed, mut probes) = (0usize, 0usize, 0usize);
    for cell in fixture_cells([-7.0; 3], [1.0; 3], 4) {
        for node in [stop, &cut, &field] {
            let mut c0 = iv::Ctx::new();
            let Ok(parent) = iv::enclose_node(&mut c0, node, cell.world_f32()) else { continue };
            let Some((a, b)) = cell.split() else { continue };
            for child in [a, b] {
                let (esc, n, ok, _) = field_enclosure_escapes(node, &child, 2);
                assert_eq!(esc, 0, "Kind-Zelle {child:?}: {esc} von {n} Werten entkommen");
                if !ok {
                    continue;
                }
                let mut c1 = iv::Ctx::new();
                let kid = iv::enclose_node(&mut c1, node, child.world_f32()).unwrap();
                assert!(
                    kid.lo >= parent.lo && kid.hi <= parent.hi,
                    "Kind-Einschliessung {kid:?} verlässt die Eltern-Einschliessung {parent:?} — \
                     die Inklusions-Isotonie ist verletzt"
                );
                if kid.width() < parent.width() {
                    narrowed += 1;
                }
                kids += 1;
                probes += n;
            }
        }
    }
    assert!(kids >= 200, "nur {kids} Kind-Zellen geprüft");
    assert!(probes > 5_000, "nur {probes} Proben");
    assert!(
        narrowed * 4 >= kids,
        "nur {narrowed} von {kids} Teilzellen sind ENGER als ihre Elternzelle — \
         dann bringt die Unterteilung nichts"
    );
}

/// **§8u-Pin 5: das Zell-Zertifikat ist unter-claim-frei** (Consumer-Lemma).
///
/// Für JEDE zertifizierte Zelle gilt an dicht gesetzten Proben: der Punkt ist
/// KEIN Unter-Claim — gemessen gegen die EXAKTE Komplement-Referenz der §8k-
/// Fixturen, nicht gegen eine Schranke. Fällt hier auch nur eine Probe, ist
/// einer der beiden Zweige falsch.
#[test]
fn interval_cell_certificate_is_under_claim_free_384() {
    let delta = 2.0f32;
    let (mut certified, mut probes, mut checked_fixtures) = (0usize, 0usize, 0usize);
    for (name, field, exact, lo, hi, _n) in cert_matrix(delta) {
        if !(name.starts_with("Stufen")
            || name == "Ecken-Schale t/δ=2.25"
            || name == "Rippen-Kamm 4×2/1"
            || name == "notch α=90")
        {
            continue;
        }
        let (stop, blk) = stop_with_lb(&field);
        let cap = delta as f64 + CERT_TOL;
        let u_at = |c: [f64; 3]| u_upper(stop, blk, delta, c, 2.0 * cap);
        let mut here = 0usize;
        for cell in fixture_cells(lo, hi, 12) {
            let mut c = iv::Ctx::new();
            let v = iv::certify_cell(&cell, &field, delta as f64, CERT_TOL, &u_at, &mut c);
            if !v.certified() {
                continue;
            }
            certified += 1;
            here += 1;
            for p in cell_samples(&cell, 1) {
                let f = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual) as f64;
                let e = exact(p);
                probes += 1;
                assert!(
                    !(f >= 0.0 && e - delta as f64 > CERT_TOL),
                    "{name}: die als {v:?} zertifizierte Zelle {cell:?} enthält bei {p:?} einen \
                     ECHTEN Unter-Claim (f = {f:.6}, exakt − δ = {:.6})",
                    e - delta as f64
                );
            }
        }
        assert!(here > 20, "{name}: nur {here} zertifizierte Zellen — der Pin misst dort nichts");
        checked_fixtures += 1;
    }
    assert_eq!(checked_fixtures, 4, "nur {checked_fixtures} Fixturen geprüft");
    assert!(certified > 1_000 && probes > 8_000, "{certified} Zellen / {probes} Proben");
}

/// **§8u-Gegenprobe 1: eine um ein ulp ZU SCHARFE Rundung muss reissen.**
///
/// Die blosse ABWESENHEIT der Aussen-Rundung ist nachweislich wirkungslos:
/// nach jedem `seal` liegen beide Schranken auf dem f32-Gitter, und f64
/// rechnet mit solchen Operanden für `+ − ×` exakt. Die Gegenprobe muss die
/// Schranke deshalb — wie schon `emax = −0.05` in §8t — aktiv zu eng stellen.
/// Tut sie es, MUSS die adversariale Familie fallen; sonst trägt Pin 2 nichts.
#[test]
fn interval_counter_probe_inward_rounding_384() {
    let knobs = iv::Knobs { inward_rounding: true, ..Default::default() };
    let (mut broken, mut esc_total) = (0usize, 0usize);
    for (_name, vs, cell, base, d, extra) in adversarial_vertex_cases() {
        let (esc, _n, _ok) = vertex_enclosure_escapes(knobs, &vs, &cell, base, d, extra, 3);
        if esc > 0 {
            broken += 1;
            esc_total += esc;
        }
    }
    assert!(
        broken > 0,
        "die Gegenprobe blieb still: mit einer um ein ulp nach INNEN gedrehten Rundung muss \
         mindestens eine Familie Produktionswerte verlieren ({esc_total} Ausreisser gezählt) — \
         sonst prüft `interval_encloses_the_production_vertex_term_384` die Rundung nicht"
    );
}

/// **§8u-Gegenprobe 2: ein willkürlich gewählter Zweig muss reissen.**
///
/// Ersetzt man die Hülle unentschiedener Schalter durch „nimm den ersten
/// erreichbaren Zweig" — der klassische Fehler —, MUSS die Familie fallen. Das
/// ist der Beweis, dass die Schalter-Pflicht (Beweispflicht 4) in den Tests
/// wirklich trägt und nicht bloss zufällig nie greift.
#[test]
fn interval_counter_probe_first_branch_only_384() {
    let knobs = iv::Knobs { first_branch_only: true, ..Default::default() };
    let (mut broken, mut esc_total) = (0usize, 0usize);
    for (_name, vs, cell, base, d, extra) in adversarial_vertex_cases() {
        let (esc, _n, _ok) = vertex_enclosure_escapes(knobs, &vs, &cell, base, d, extra, 3);
        if esc > 0 {
            broken += 1;
            esc_total += esc;
        }
    }
    assert!(
        broken > 0,
        "die Gegenprobe blieb still: ohne Hülle über unentschiedenen Schaltern muss die \
         Familie Produktionswerte verlieren ({esc_total} Ausreisser) — sonst ist die \
         Schalter-Pflicht in den Tests nicht belegt"
    );
}

/// **§8u-Pin 6: Zweig 2 ist keine tote Ausnahme — am UNVERÄNDERTEN Feld.**
///
/// Auf den Doktrin-Gittern der §8p-Matrix zertifiziert Zweig 2 null Zellen,
/// und das ist kein Zufall: diese Gitter sind um die NAHT gelegt, also genau
/// dorthin, wo die Kanal-Terme leben. Dort scheitert die Neutralität an der
/// Kanal-Bounding-Sphere (`bound_r` 3.3 … 54.5 mm gegen `K = 3.91 mm`), und
/// `channel_limit` bleibt ohne obere Schranke.
///
/// **Naht-FERN sieht es anders aus.** Dieser Pin nimmt die Ecken-Schale
/// `t/δ = 6` — dasselbe ausgelieferte Feld, kein struktureller Eingriff, keine
/// Ablation — und legt Zellen tief in eine Platte, weit weg von der Ecke. Dort
/// sind ALLE 24 Kanäle nachweislich neutral, die Vertex-Kugeln ebenso, und die
/// Intervall-Auswertung trägt: sie zertifiziert, und an jeder zertifizierten
/// Zelle behauptet das Feld an dicht gesetzten Proben tatsächlich Material.
///
/// Zweig 1 ist dabei abgeschaltet — was hier zählt, ist ausschliesslich
/// `sup_C f < 0`.
///
/// Daneben steht dieselbe Messung an der Vertex-Ablation (Kanäle entfernt):
/// sie prüft die Term-Klasse, die dieses Paket vollständig unterstützt, auf
/// dem Doktrin-Gitter selbst. Die Ablation ist **kein ausgeliefertes Feld**
/// und wird nirgends als eines ausgegeben.
#[test]
fn interval_branch_two_is_alive_on_the_delivered_field_384() {
    let delta = 2.0f32;
    let t = 6.0 * delta;
    let (field, _) = compile_for(thin_corner_node(t, 40.0, delta));
    let none = |_c: [f64; 3]| None; // Zweig 1 abgeschaltet — nur Zweig 2 zählt.
    let check = |node: &BoundedCsg, lo: [f64; 3], hi: [f64; 3], n: usize| -> (usize, usize) {
        let (mut b2, mut probes) = (0usize, 0usize);
        for cell in fixture_cells(lo, hi, n) {
            let mut c = iv::Ctx::new();
            match iv::certify_cell(&cell, node, delta as f64, CERT_TOL, &none, &mut c) {
                iv::Verdict::Branch2 => {
                    b2 += 1;
                    for p in cell_samples(&cell, 2) {
                        let f = node.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                        probes += 1;
                        assert!(
                            f < 0.0,
                            "Zweig 2 hat die Zelle {cell:?} zertifiziert, das Feld liefert bei \
                             {p:?} aber {f} ≥ 0 — die obere Schranke lügt"
                        );
                    }
                }
                iv::Verdict::Branch1 => unreachable!("Zweig 1 ist in diesem Pin abgeschaltet"),
                iv::Verdict::Unknown(_) => {}
            }
        }
        (b2, probes)
    };
    // (1) Das AUSGELIEFERTE Feld, tief in einer Platte, naht-fern.
    let (b2, probes) = check(&field, [-36.0, -8.0, -36.0], [-30.0, -5.0, -30.0], 8);
    assert!(
        b2 >= 100,
        "Zweig 2 zertifiziert nur {b2} Zellen des AUSGELIEFERTEN Feldes im naht-fernen \
         Plattenkern — dann ist die Intervall-Auswertung dort nicht belegt, sondern bloss still"
    );
    assert!(probes > 2_000, "nur {probes} Proben am ausgelieferten Feld");
    // (2) Die Vertex-Ablation auf dem Doktrin-Gitter.
    let cut = strip_channels(&field);
    let lo = -(t as f64 + delta as f64 + 1.0);
    let (b2c, pc) = check(&cut, [lo; 3], [1.0; 3], 11);
    assert!(b2c >= 30, "die Vertex-Ablation zertifiziert nur {b2c} Zellen");
    assert!(pc > 800, "nur {pc} Proben an der Ablation");
}

// ══════════ #384 §8u-Korrektur — die vier Familien des vierten Reviews ═══════

/// **Ein Klotz mit kleinem Buckel** — die Fixture, an der Zweig 2 am
/// UNVERÄNDERTEN Feld greift.
///
/// Die Kanal-Neutralität steht auf `dbs = |p − bound_c| − bound_r ≥ K`, und
/// `bound_r` ist die halbe Nahtlänge. An der §8p-Matrix ist die Naht so lang
/// wie das Bauteil (22 … 54 mm gegen `K = 3.91 mm`), also greift sie nie im
/// Material. Hier ist es umgekehrt: die Naht ist der 4×4-Ring um den Buckel
/// (`bound_r ≈ 3 mm`), der Körper aber eine 60×60×20-Platte. Tief in der
/// Platte sind alle Kanäle nachweislich neutral — kein struktureller Eingriff,
/// keine Ablation, das ausgelieferte Feld.
fn bump_on_plate_node(delta: f32) -> CsgNode {
    let plate = box_at(1, [0.0, 0.0, -10.0], [30.0, 30.0, 10.0]);
    let bump = box_at(2, [0.0, 0.0, 2.0], [2.0, 2.0, 2.0]);
    CsgNode::Offset(
        Box::new(CsgNode::Union(Box::new(plate), Box::new(bump))),
        -delta,
    )
}

/// **§8u-K1: die Quaternion-Kette folgt der Produktions-Klammerung.**
///
/// Erster blockierender Befund des vierten Mathe-Reviews. Die Produktion
/// rechnet je Komponente `(((v ⊕ qw⊗t) ⊕ qy⊗t₂) ⊖ qz⊗t₁)`; die erste
/// §8u-Fassung sammelte erst den Rotationsanteil und addierte `v` zum Schluss.
/// Über ℝ dasselbe, über f32 nicht — und bei AUSLÖSCHUNG (grosses `v`, fast
/// kompensierender Rotationsanteil) weicht die Reassoziation um weit mehr ab
/// als eine ULP-Weitung.
///
/// Die Familie stellt genau das her: eine 90°-Drehung um `z`, bei der die
/// Kette auf `((v₀ ⊕ v₁) ⊖ v₀)` hinausläuft, mit `|v₀|` bis 10⁷ und `|v₁|` im
/// Millimeterbereich — in f32 verschluckt die erste Addition `v₁` vollständig.
///
/// Zwei Richtungen: mit Produktions-Reihenfolge KEIN Ausreisser, mit
/// reassoziierter Reihenfolge MUSS die Familie fallen.
#[test]
fn interval_mirrors_the_production_rotation_order_384() {
    let rot = |deg: f64, axis: usize| -> [f32; 4] {
        let h = 0.5 * deg.to_radians();
        let (s, c) = (h.sin() as f32, h.cos() as f32);
        let mut q = [0.0f32, 0.0, 0.0, c];
        q[axis] = s;
        q
    };
    let mut cases: Vec<(BoundedCsg, iv::Box3)> = Vec::new();
    for (deg, axis) in [(90.0f64, 2usize), (45.0, 2), (30.0, 1), (120.0, 0)] {
        let node = BoundedCsg::Leaf {
            id: id(1),
            primitive: Primitive::Box { center: [0.0; 3], half_extents: [3.0, 4.0, 5.0] },
            world_transform: Transform {
                translate: [0.0; 3],
                rotation: rot(deg, axis),
                scale: [1.0; 3],
            },
            aabb: crate::csg::Aabb::EMPTY,
        };
        for big in [1.0e3f64, 1.0e5, 1.0e7] {
            for small in [0.3f64, 1.7, -0.9] {
                cases.push((
                    node.clone(),
                    iv::Box3::new(
                        [big, small, small * 0.5],
                        [big + 0.75, small + 0.75, small * 0.5 + 0.75],
                    ),
                ));
            }
        }
    }
    let run = |knobs: iv::Knobs, cases: &Vec<(BoundedCsg, iv::Box3)>| -> (usize, usize, usize) {
        let (mut esc, mut n, mut ok) = (0usize, 0usize, 0usize);
        for (node, cell) in cases {
            let mut c = iv::Ctx::with_knobs(knobs);
            let Ok(enc) = iv::enclose_node(&mut c, node, cell.world_f32()) else { continue };
            ok += 1;
            for p in cell_samples(cell, 3) {
                let v = node.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                n += 1;
                if !enc.contains(v as f64) {
                    esc += 1;
                }
            }
        }
        (esc, n, ok)
    };
    let (esc, n, ok) = run(iv::Knobs::default(), &cases);
    assert_eq!(
        esc, 0,
        "{esc} von {n} Produktionswerten entkommen der Einschliessung einer GEDREHTEN \
         Leaf — die Op-Reihenfolge stimmt dann nicht mit `quat_inverse_rotate` überein"
    );
    assert!(ok >= 30 && n > 2_000, "nur {ok} Fälle / {n} Proben ausgewertet");

    // **Die Gegenprobe, und ein ehrlicher Negativbefund dazu.**
    //
    // Auf der OPERATIONS-Ebene ist die Reassoziation nachweislich eine echte
    // f32-Abweichung — das wird hier gegen die Produktions-Funktion
    // `world_to_local` selbst gezeigt, nicht gegen einen Nachbau des Modells.
    // Damit ist der Fix von Beweispflicht 5 gefordert und nicht kosmetisch.
    //
    // Auf der INTERVALL-Ebene lässt sie sich dagegen NICHT durch Sampling
    // widerlegen: die letzte Operation berührt in beiden Reihenfolgen `v` in
    // voller Grösse, beide Einschliessungen bekommen deshalb eine Weite in der
    // Grössenordnung `ulp(max |Zwischenwert|)`, und die schluckt den
    // Reihenfolge-Unterschied. Gemessen an 112 000 zufälligen Quaternionen und
    // Beträgen von 10² bis 10³⁰: null Ausreisser in BEIDEN Reihenfolgen. Der
    // Befund steht so im Doc; eine erfundene Gegenprobe wäre schlimmer als
    // keine.
    let mut differs = 0usize;
    for (node, cell) in &cases {
        let BoundedCsg::Leaf { world_transform: t, .. } = node else { continue };
        for p in cell_samples(cell, 3) {
            let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
            let prod = crate::transform::world_to_local(t, pf);
            // Dieselbe Formel, nur der Rotationsanteil zuerst zusammengefasst.
            let v = [pf[0] - t.translate[0], pf[1] - t.translate[1], pf[2] - t.translate[2]];
            let (qx, qy, qz, qw) = (-t.rotation[0], -t.rotation[1], -t.rotation[2], t.rotation[3]);
            let tt = [
                2.0 * (qy * v[2] - qz * v[1]),
                2.0 * (qz * v[0] - qx * v[2]),
                2.0 * (qx * v[1] - qy * v[0]),
            ];
            let re = [
                (v[0] + (qw * tt[0] + qy * tt[2] - qz * tt[1])) / t.scale[0],
                (v[1] + (qw * tt[1] + qz * tt[0] - qx * tt[2])) / t.scale[1],
                (v[2] + (qw * tt[2] + qx * tt[1] - qy * tt[0])) / t.scale[2],
            ];
            if (0..3).any(|k| prod[k].to_bits() != re[k].to_bits()) {
                differs += 1;
            }
        }
    }
    assert!(
        differs > 0,
        "die reassoziierte Quaternion-Kette liefert an KEINER Probe einen anderen f32-Wert \
         als `world_to_local` — dann wäre die Op-Reihenfolge tatsächlich egal und der \
         §8u-Korrektur-Fix unbegründet"
    );
}

/// **§8u-K2a: `|x − c| ≤ ρ` gilt für den TATSÄCHLICHEN Mittelpunkt.**
///
/// Zweiter blockierender Befund, erste Brücke. `ρ` wurde aus den Halbbreiten
/// `(hi−lo)/2` gebildet, `u_at` aber am GERUNDETEN Mittelpunkt ausgewertet.
/// Bei `lo = 10¹⁶`, `hi = lo + 2` fällt der gerundete Mittelpunkt mit `lo`
/// zusammen: Halbbreite 1, echter Maximalabstand 2.
///
/// Der Pin prüft die Zusage direkt an adversarialen Zellen — und rechnet
/// daneben die ALTE Formel mit, die an mindestens einer davon reissen MUSS.
#[test]
fn interval_cell_radius_covers_the_rounded_centre_384() {
    use iv::CellDomain;
    let mut cells: Vec<iv::Box3> = Vec::new();
    for base in [1.0e8f64, 1.0e12, 1.0e16, 4.5e15] {
        for w in [2.0f64, 1.0, 0.5, 1.0e-3] {
            cells.push(iv::Box3::new([base, -base, base * 0.5], [base + w, -base + w, base * 0.5 + w]));
        }
        // Benachbarte f64-Endpunkte: die Zelle ist numerisch nicht mehr teilbar.
        let hi = [
            iv::next_up(base),
            iv::next_up(iv::next_up(-base)),
            iv::next_up(base * 0.5),
        ];
        cells.push(iv::Box3::new([base, -base, base * 0.5], hi));
    }
    cells.push(iv::Box3::new([-1.0e300, 0.0, 0.0], [1.0e300, 1.0, 1.0]));
    let (mut checked, mut old_broken, mut unsplittable) = (0usize, 0usize, 0usize);
    for cell in &cells {
        let c = cell.center();
        let rho = cell.rho();
        // Die alte Formel: Halbbreiten statt tatsächlicher Abstände.
        let mut s = 0.0f64;
        for k in 0..3 {
            let h = iv::next_up(0.5 * iv::next_up(cell.hi[k] - cell.lo[k]));
            s = iv::next_up(s + iv::next_up(h * h));
        }
        let rho_old = iv::next_up(s.sqrt());
        let mut worst = 0.0f64;
        for x in cell_samples(cell, 2) {
            for k in 0..3 {
                assert!(
                    c[k] >= cell.lo[k] && c[k] <= cell.hi[k],
                    "der Mittelpunkt {c:?} liegt nicht in der Zelle {cell:?}"
                );
            }
            let d = dist_up(x, c);
            worst = worst.max(d);
            checked += 1;
            assert!(
                d <= rho,
                "Zelle {cell:?}: Probe {x:?} liegt {d} vom Mittelpunkt {c:?} entfernt, \
                 ρ ist aber nur {rho} — die 1-Lipschitz-Brücke von Zweig 1 bricht"
            );
        }
        if worst > rho_old {
            old_broken += 1;
        }
        if cell.split().is_none() {
            unsplittable += 1;
        }
    }
    assert!(checked > 400, "nur {checked} Proben");
    assert!(
        old_broken > 0,
        "die Gegenprobe blieb still: die ALTE Halbbreiten-Formel muss an mindestens einer \
         dieser Zellen unter dem echten Maximalabstand liegen — sonst belegt der Pin nichts"
    );
    assert!(
        unsplittable > 0,
        "keine einzige Zelle ist numerisch unteilbar — dann prüft der Pin die \
         Terminierungs-Weigerung von `Box3::split` nicht"
    );
}

/// **§8u-K2c: `U` ist eine echte obere Schranke von `dist(c,∁Ω)`.**
///
/// Zweiter blockierender Befund, dritte Brücke: der Adapter bewies die
/// Aussenlage am f32-GERUNDETEN Punkt, mass die Distanz aber zum f64-Punkt —
/// und `d₀ > δ ⇒ Some(0)` war das klarste Beispiel. Das 1-Lipschitz-Lemma von
/// Zweig 1 ist nur BEDINGT korrekt; die Vorbedingung muss der Adapter erfüllen.
///
/// Geprüft wird sie hier gegen die EXAKTE §8k-Komplement-Referenz, an genau
/// den Punkten, an denen Zweig 1 sie später benutzt: den Zellmittelpunkten.
#[test]
fn interval_branch_one_upper_bound_is_sound_384() {
    let delta = 2.0f32;
    let (mut checked, mut zero_branch) = (0usize, 0usize);
    for (name, field, exact, lo, hi, _n) in cert_matrix(delta) {
        if !(name.starts_with("Stufen") || name == "Ecken-Schale t/δ=2.5" || name == "notch α=90") {
            continue;
        }
        let (stop, blk) = stop_with_lb(&field);
        let cap = delta as f64 + CERT_TOL;
        for cell in fixture_cells(lo, hi, 8) {
            use iv::CellDomain;
            let c = cell.center();
            let Some(u) = u_upper(stop, blk, delta, c, 2.0 * cap) else { continue };
            let e = exact(c);
            checked += 1;
            if u < 1e-3 {
                zero_branch += 1;
            }
            assert!(
                e <= u + 1e-9,
                "{name}: exakt dist({c:?},∁Ω) = {e:.9} übersteigt die behauptete obere \
                 Schranke U = {u:.9} — dann zertifiziert Zweig 1 Unter-Claims weg"
            );
        }
    }
    assert!(checked > 500, "nur {checked} Mittelpunkte mit U geprüft");
    assert!(
        zero_branch > 0,
        "kein einziger Mittelpunkt lief über den `d₀ > δ`-Zweig — dann prüft der Pin \
         genau die Brücke nicht, an der die falsche Null stand"
    );
}

/// **§8u-K3: ein nicht-endliches Stück-Minimum ist `LB_NONE`, nicht `∞`.**
///
/// Dritter blockierender Befund. `lb_value` startet mit `best = +∞` und
/// aktualisiert nur bei `d < best`; bleiben alle Stück-Distanzen nicht-endlich
/// (in f32 läuft die Quadratsumme bei Koordinaten nahe `f32::MAX` über),
/// liefert die Produktion `LB_NONE`. Ein modelliertes `∞ − slack` wählt
/// dagegen den `L`-Zweig des Gates, während die Produktion den `G`-Zweig
/// nimmt — die Ergebnismengen enthalten einander dann NICHT.
#[test]
fn interval_lb_non_finite_minimum_is_neutral_384() {
    use crate::offset_lb::{LbBlock, LbPiece};
    let far = 3.0e38f32;
    let blk = LbBlock {
        pieces: vec![
            LbPiece::Tri {
                a: [far, 0.0, 0.0],
                b: [far, 1.0, 0.0],
                c: [far, 0.0, 1.0],
                n: [1.0, 0.0, 0.0],
                emax: f32::INFINITY,
            },
            LbPiece::Tri {
                a: [far, 2.0, 0.0],
                b: [far, 3.0, 0.0],
                c: [far, 2.0, 1.0],
                n: [1.0, 0.0, 0.0],
                emax: f32::INFINITY,
            },
        ],
        delta: 2.0,
        slack: 1.0e-4,
        center: [0.0; 3],
        skip_r: f32::INFINITY,
    };
    let cell = iv::Box3::cube([0.0, 0.0, 0.0], 0.5);
    let base = iv::Iv { lo: -6.0, hi: -5.0 };
    // Die Produktion liefert an jedem Zellpunkt LB_NONE.
    let mut saw_none = 0usize;
    for p in cell_samples(&cell, 2) {
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        let v = crate::offset_lb::lb_value(pf, -5.5, &blk);
        assert_eq!(
            v,
            crate::offset_lb::LB_NONE,
            "die Fixture erzeugt kein nicht-endliches Stück-Minimum (bekam {v}) — \
             dann prüft der Pin den `is_finite`-Schalter gar nicht"
        );
        saw_none += 1;
    }
    assert!(saw_none > 20, "nur {saw_none} Proben");
    // Mit Schalter: die Einschliessung enthält LB_NONE.
    let mut c = iv::Ctx::new();
    let enc = iv::enclose_lb_value(&mut c, cell.world_f32(), base, &blk).unwrap();
    assert!(
        enc.contains(crate::offset_lb::LB_NONE as f64),
        "die Einschliessung {enc:?} enthält LB_NONE nicht"
    );
    // Ohne Schalter (Gegenprobe): sie MUSS ihn verlieren.
    let mut c2 = iv::Ctx::with_knobs(iv::Knobs { skip_lb_finite_gate: true, ..Default::default() });
    let bad = iv::enclose_lb_value(&mut c2, cell.world_f32(), base, &blk).unwrap();
    assert!(
        !bad.contains(crate::offset_lb::LB_NONE as f64),
        "die Gegenprobe blieb still: ohne den `is_finite`-Schalter muss die Einschliessung \
         {bad:?} den Produktionswert LB_NONE verlieren — sonst belegt der Pin ihn nicht"
    );
}



/// **§8u-Sonde: Zweig 2 naht-fern, am unveränderten Feld.**
///
/// Die Doktrin-Gitter der §8p-Matrix liegen um die NAHT — also genau dort, wo
/// die Kanal-Terme leben und die Neutralität an der Bounding-Sphere scheitert.
/// Diese Sonde misst dasselbe ausgelieferte Feld naht-FERN und zeigt die
/// Abhängigkeit vom Abstand zur Ecke.
///
/// Dazu der Nebenbefund „kleiner Buckel auf grosser Platte": die naheliegende
/// Idee, eine KURZE Naht in einem GROSSEN Körper zu bauen, geht nicht auf —
/// die Synthese macht daraus keinen Kanal, sondern einen ∂P-Rand-Fillet
/// (`RimBlock`), und den unterstützt Paket 1 nicht.
#[test]
#[ignore]
fn probe_384_interval_seam_far() {
    let delta = 2.0f32;
    let (bump, _) = compile_for(bump_on_plate_node(delta));
    let mut st = Vec::new();
    stop_nodes(&bump, &mut st);
    for n in &st {
        if let BoundedCsg::ArcChannel { channels, verts, rims, .. } = n {
            println!(
                "Buckel auf Platte: Kanäle {} verts {} rims {} — die kurze Naht wird ein Rim",
                channels.len(),
                verts.len(),
                rims.len()
            );
        }
    }
    let (field, _) = compile_for(thin_corner_node(6.0 * delta, 40.0, delta));
    let none = |_c: [f64; 3]| None;
    println!("| Gebiet | Zellen | Zweig 2 | Hauptgründe |");
    println!("|---|---|---|---|");
    for (tag, lo, hi, n) in [
        ("Doktrin-Gitter (naht-nah)", [-15.0f64; 3], [1.0f64; 3], 12usize),
        ("Plattenkern, mittel", [-38.0, -9.0, -38.0], [-24.0, -3.0, -24.0], 12),
        ("Plattenkern, tief", [-36.0, -8.0, -36.0], [-30.0, -5.0, -30.0], 12),
    ] {
        let (mut b2, mut unk) = (0usize, std::collections::BTreeMap::new());
        for cell in fixture_cells(lo, hi, n) {
            let mut c = iv::Ctx::new();
            match iv::certify_cell(&cell, &field, delta as f64, CERT_TOL, &none, &mut c) {
                iv::Verdict::Branch2 => b2 += 1,
                iv::Verdict::Unknown(w) => *unk.entry(w).or_insert(0usize) += 1,
                iv::Verdict::Branch1 => {}
            }
        }
        let mut v: Vec<_> = unk.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let top: String =
            v.iter().take(2).map(|(k, c)| format!("{k}·{c}")).collect::<Vec<_>>().join(", ");
        println!("| {tag} | {} | {b2} | {top} |", n * n * n);
    }
}

// ══════════════ #384 §8v — Kanal-Terme, gerade Segmente ══════════════

use crate::offset_channel::{ChannelArc, OffsetChannel};

/// Ein Geradensegment-Bogen für die synthetischen §8v-Familien.
#[allow(clippy::too_many_arguments)]
fn seg_arc(
    a: [f32; 3],
    b: [f32; 3],
    m1s: [f32; 3],
    m1e: [f32; 3],
    m2s: [f32; 3],
    m2e: [f32; 3],
) -> ChannelArc {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let u = [d[0] / len, d[1] / len, d[2] / len];
    ChannelArc {
        c: a,
        u,
        v: [0.0; 3],
        r: len,
        sweep: -1.0,
        e: [1.0, 0.0],
        d_e: 0.0,
        bc: [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1]), 0.5 * (a[2] + b[2])],
        br: 0.5 * len,
        m1s,
        m2s,
        m1e,
        m2e,
    }
}

fn synth_channel(arcs: Vec<ChannelArc>, concave: bool) -> OffsetChannel {
    let mut bc = [0.0f32; 3];
    for a in &arcs {
        for k in 0..3 {
            bc[k] += a.bc[k] / arcs.len() as f32;
        }
    }
    let mut br = 0.0f32;
    for a in &arcs {
        let d = ((a.bc[0] - bc[0]).powi(2) + (a.bc[1] - bc[1]).powi(2) + (a.bc[2] - bc[2]).powi(2))
            .sqrt();
        br = br.max(d + a.br);
    }
    OffsetChannel::unproven(
        arcs,
        2.0,
        concave,
        3.91,
        0.87,
        bc,
        br,
        [crate::offset_channel::END_PLANE_NONE; 4],
        [crate::offset_channel::END_PLANE_NONE; 4],
        0x700,
        [0, 0],
    )
}

/// Zählt Ausreisser der `channel_apply`-Einschliessung gegen die Produktion.
fn channel_enclosure_escapes(
    knobs: iv::Knobs,
    ch: &OffsetChannel,
    cell: &iv::Box3,
    base: iv::Iv,
    d: iv::Iv,
    extra: iv::Iv,
    sub: usize,
) -> (usize, usize, usize) {
    let mut c = iv::Ctx::with_knobs(knobs);
    let Ok(enc) = iv::enclose_channel_apply(&mut c, cell.world_f32(), base, d, ch, extra) else {
        return (0, 0, 0);
    };
    let (mut esc, mut n) = (0usize, 0usize);
    for p in cell_samples(cell, sub) {
        let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
        for b in iv_samples(base) {
            for dd in iv_samples(d) {
                for e in iv_samples(extra) {
                    let got = crate::offset_channel::channel_apply(b, dd, pf, ch, e);
                    n += 1;
                    if !got.is_finite() || !enc.contains(got as f64) {
                        esc += 1;
                    }
                }
            }
        }
    }
    (esc, n, 1)
}

/// Die adversariale §8v-Familie: jede Zeile zielt auf eine Schwachstelle des
/// Kanal-Terms.
fn adversarial_channel_cases() -> Vec<(String, OffsetChannel, iv::Box3, iv::Iv, iv::Iv, iv::Iv)> {
    let e = |v: [f32; 3]| {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let mut out = Vec::new();
    let dfull = ivf(-6.0, 3.0);
    let ex = ivf(-1.0e30, 1.0);
    // EIN Segment entlang x, Duale schräg — der Regelfall.
    let one = synth_channel(
        vec![seg_arc(
            [-5.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            e([0.0, 1.0, 1.0]),
            e([0.0, 1.0, 1.2]),
            e([0.0, -1.0, 1.0]),
            e([0.0, -1.2, 1.0]),
        )],
        true,
    );
    // (4) Segment-Slab: Zelle mittig, am Ende, und HALB draussen.
    for (tag, ctr, h) in [
        ("Slab innen", [0.0f64, 1.4, 0.9], 0.4f64),
        ("Slab am Ende", [5.0, 1.4, 0.9], 0.6),
        ("Slab halb draussen", [5.4, 1.0, 0.7], 0.9),
        ("Slab weit draussen", [7.5, 1.0, 0.7], 0.5),
        ("Slab auf der Achse", [0.0, 0.0, 0.0], 0.35),
    ] {
        out.push((format!("1-Seg {tag}"), one.clone(), iv::Box3::cube(ctr, h), dfull, dfull, ex));
    }
    // (2) nlerp mit fast antipodalen Endvektoren: `v` löscht sich bei t≈0.5
    //     aus, die Norm überspannt die 1e-9-Schwelle.
    let anti = synth_channel(
        vec![seg_arc(
            [-5.0, 0.0, 0.0],
            [5.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            e([0.0, 0.0, 1.0]),
            e([1e-9, 0.0, -1.0]),
        )],
        true,
    );
    for (tag, ctr, h) in [
        ("t≈0.5", [0.0f64, 1.3, 0.8], 0.3f64),
        ("t≈0.5 eng", [0.0, 1.3, 0.8], 0.02),
        ("t-Sweep", [0.0, 1.1, 0.6], 2.5),
    ] {
        out.push((
            format!("nlerp antipodal {tag}"),
            anti.clone(),
            iv::Box3::cube(ctr, h),
            dfull,
            dfull,
            ex,
        ));
    }
    // (1)/(3) ZWEI Segmente, deren Nearest-Gewinner in der Zelle wechselt — mit
    //         DEUTLICH verschiedenen Dualvektoren. Spiegelbildliche Duale
    //         genügen nicht: dann ist `min(s₁,s₂)` auf beiden Seiten gleich,
    //         und der Gewinnerwechsel bliebe folgenlos.
    let two = synth_channel(
        vec![
            seg_arc(
                [-6.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                e([0.0, 1.0, 1.0]),
                e([0.0, 1.0, 1.0]),
                e([0.0, -1.0, 1.0]),
                e([0.0, -1.0, 1.0]),
            ),
            seg_arc(
                [1.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ),
        ],
        true,
    );
    // `d` als PUNKT weit über null: dann bestimmt `−w_geo` das Ergebnis und
    // nicht das breite `d`-Intervall — nur so wird ein Gewinnerwechsel im
    // Ergebnis überhaupt sichtbar.
    let dp = ivf(5.0, 5.0);
    let exo = ivf(10.0, 10.0);
    for (tag, ctr, h) in [
        ("Voronoi-Grenze", [0.0f64, 2.6, 0.8], 1.6f64),
        ("Voronoi eng", [0.0, 2.6, 0.8], 0.05),
        ("klar links", [-3.5, 2.6, 0.8], 0.3),
        ("klar rechts", [3.5, 2.6, 0.8], 0.3),
    ] {
        out.push((
            format!("2-Seg {tag}"),
            two.clone(),
            iv::Box3::cube(ctr, h),
            dp,
            dp,
            exo,
        ));
    }
    // Dieselbe Geometrie mit breitem `d` — die Soundness muss auch dort halten.
    out.push((
        "2-Seg Voronoi breites d".into(),
        two,
        iv::Box3::cube([0.0, 2.6, 0.8], 1.6),
        dfull,
        dfull,
        ex,
    ));
    // Der konvexe Zweig.
    let cvx = synth_channel(one.arcs.clone(), false);
    out.push(("1-Seg konvex".into(), cvx, iv::Box3::cube([0.0, 1.4, 0.9], 0.5), dfull, dfull, ex));
    out
}

/// **§8v-Pin 1: die Einschliessung von `channel_apply` hält gegen die
/// Produktion.**
///
/// Dieselbe Doktrin wie §8u: dichte Proben sind Falsifikatoren der
/// Einschliessung. Die Familien zielen auf den Segment-Slab (Sättigung des
/// `clamp`), auf `nlerp` mit fast antipodalen Endvektoren (Norm an der
/// 1e-9-Schwelle) und auf den Argmin-Wechsel zwischen zwei Segmenten.
#[test]
fn interval_encloses_the_production_channel_term_384() {
    let (mut cases, mut probes) = (0usize, 0usize);
    for (name, ch, cell, base, d, extra) in adversarial_channel_cases() {
        let (esc, n, ok) =
            channel_enclosure_escapes(iv::Knobs::default(), &ch, &cell, base, d, extra, 3);
        assert_eq!(
            esc, 0,
            "{name}: {esc} von {n} Produktionswerten liegen AUSSERHALB der Einschliessung"
        );
        cases += ok;
        probes += n;
    }
    // Die ECHTEN Kanäle der Matrix — dieselbe Formel, echte Daten.
    let delta = 2.0f32;
    let (field, _) = compile_for(notch_union_node(90.0, delta as f64));
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let mut real = 0usize;
    for st in stops {
        let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
        for ch in channels {
            for (ctr, h) in [([0.0f64, 0.0, 48.0], 0.7f64), ([1.5, -1.5, 50.0], 1.1)] {
                let cell = iv::Box3::cube(ctr, h);
                let dv = ivf(-7.0, 2.0);
                let (esc, n, ok) = channel_enclosure_escapes(
                    iv::Knobs::default(),
                    ch,
                    &cell,
                    dv,
                    dv,
                    ivf(-1.0e30, 0.9),
                    2,
                );
                assert_eq!(esc, 0, "echter Kanal: {esc} von {n} Werte entkommen");
                real += ok;
                probes += n;
            }
        }
    }
    assert!(
        cases >= 10 && probes > 15_000,
        "nur {cases} synthetische Fälle / {probes} Proben ausgewertet — eine leer-grüne \
         Familie belegt nichts"
    );
    assert!(
        real >= 2,
        "nur {real} ECHTE Kanäle der Matrix wurden ausgewertet — dann prüft der Pin bloss \
         synthetische Daten"
    );
}

/// **§8w-Pin: der Argmin-Hull trägt — und das Streichen eines Kandidaten
/// reisst.**
///
/// §8v verweigerte über einer Argmin-Grenze (`Unknown`). §8w hüllt stattdessen
/// über alle möglichen Gewinner. Das ist die stärkere Aussage, aber auch die
/// gefährlichere: eine Hülle, der ein Kandidat fehlt, ist keine
/// Einschliessung, sondern eine Lüge. Der Pin prüft vier Richtungen:
///
/// * **Über der Grenze wird gehüllt** (Kandidaten-Menge `≥ 2`) und die Hülle
///   enthält JEDEN Produktionswert an dicht gesetzten Proben.
/// * **Daneben bleibt es eindeutig** (Kandidaten-Menge `== 1`) — sonst wäre
///   das Kriterium eine Dauer-Hülle und die Schärfe dahin.
/// * **Ein gestrichener Kandidat reisst** (`Knobs::drop_last_candidate`).
/// * **Blind den ersten nehmen reisst** (`Knobs::first_arc_wins`).
#[test]
fn interval_argmin_hull_is_load_bearing_384() {
    let straddle: Vec<_> = adversarial_channel_cases()
        .into_iter()
        .filter(|(n, ..)| n.starts_with("2-Seg Voronoi"))
        .collect();
    assert!(straddle.len() >= 2, "die Voronoi-Familie fehlt");
    // (a) Über der Grenze: gehüllt, und die Hülle hält.
    let (mut hulled, mut probes) = (0usize, 0usize);
    for (name, ch, cell, base, d, extra) in &straddle {
        let mut c = iv::Ctx::new();
        let enc = iv::enclose_channel_apply(&mut c, cell.world_f32(), *base, *d, ch, *extra)
            .unwrap_or_else(|e| panic!("{name}: der Hull verweigert ({e:?})"));
        assert!(
            c.max_candidates() >= 2,
            "{name}: die Zelle überspannt die Argmin-Grenze, das Selektions-Lemma hat aber \
             nur {} Kandidaten gebildet",
            c.max_candidates()
        );
        for p in cell_samples(cell, 3) {
            let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
            for b in iv_samples(*base) {
                for dd in iv_samples(*d) {
                    for e in iv_samples(*extra) {
                        let got = crate::offset_channel::channel_apply(b, dd, pf, ch, e);
                        probes += 1;
                        assert!(
                            got.is_finite() && enc.contains(got as f64),
                            "{name}: Produktionswert {got} liegt AUSSERHALB des Hulls {enc:?}"
                        );
                    }
                }
            }
        }
        hulled += 1;
    }
    assert_eq!(hulled, straddle.len());
    assert!(probes > 5_000, "nur {probes} Proben");
    // (b) Klar zugeordnet: EIN Kandidat.
    let clear: Vec<_> = adversarial_channel_cases()
        .into_iter()
        .filter(|(n, ..)| n.starts_with("2-Seg klar"))
        .collect();
    assert!(clear.len() >= 2, "die eindeutige Familie fehlt");
    for (name, ch, cell, base, d, extra) in &clear {
        let mut c = iv::Ctx::new();
        iv::enclose_channel_apply(&mut c, cell.world_f32(), *base, *d, ch, *extra)
            .unwrap_or_else(|e| panic!("{name}: eindeutige Zelle verweigert ({e:?})"));
        assert_eq!(
            c.max_candidates(),
            1,
            "{name}: eindeutig zugeordnete Zelle bildet {} Kandidaten — dann ist das \
             Kriterium eine Dauer-Hülle",
            c.max_candidates()
        );
    }
    // (c)/(d) Die beiden Rückdreh-Gegenproben.
    for (tag, knobs) in [
        ("Kandidat gestrichen", iv::Knobs { drop_last_candidate: true, ..Default::default() }),
        ("erster Kandidat blind", iv::Knobs { first_arc_wins: true, ..Default::default() }),
    ] {
        let (mut broken, mut esc_total) = (0usize, 0usize);
        for (_n, ch, cell, base, d, extra) in &straddle {
            let (esc, _n2, _ok) = channel_enclosure_escapes(knobs, ch, cell, *base, *d, *extra, 3);
            if esc > 0 {
                broken += 1;
                esc_total += esc;
            }
        }
        assert!(
            broken > 0,
            "die Gegenprobe {tag} blieb still: über einer Argmin-Grenze MUSS eine \
             unvollständige Hülle Produktionswerte verlieren ({esc_total} Ausreisser) — \
             sonst belegt das Selektions-Lemma nichts"
        );
    }
}

/// **§8v-Pin 3: die Kachelung ist lückenlos.**
///
/// Die Zellen eines Laufs sind eine echte Kachelung des Quaders — sie decken
/// ihn ohne Lücke und ohne Überschneidung. Wenn also im tiefen Plattenkern
/// JEDE Zelle zertifiziert wird, ist der ganze Quader beweisbar
/// unter-claim-frei, und nicht bloss „die meisten Stichproben".
///
/// Genau das nagelt dieser Pin fest: `b2 == n³`, am AUSGELIEFERTEN Feld, mit
/// abgeschaltetem Zweig 1. Bis §8v stand die Zahl nur als Druckausgabe in einer
/// `#[ignore]`-Sonde — eine Aussage, die niemand nachhält, ist keine.
#[test]
fn interval_tiling_of_the_plate_core_is_gapless_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(thin_corner_node(6.0 * delta, 40.0, delta));
    let none = |_c: [f64; 3]| None;
    let (lo, hi, n) = ([-36.0, -8.0, -36.0], [-30.0, -5.0, -30.0], 8usize);
    let cells = fixture_cells(lo, hi, n);
    assert_eq!(cells.len(), n * n * n, "die Kachelung hat nicht n³ Zellen");
    // Lückenlosigkeit der Kachelung selbst: benachbarte Zellen stossen exakt
    // aneinander, die Vereinigung ist der ganze Quader.
    for k in 0..3 {
        let mut edges: Vec<f64> = cells.iter().map(|c| c.lo[k]).collect();
        edges.extend(cells.iter().map(|c| c.hi[k]));
        edges.sort_by(|a, b| a.partial_cmp(b).unwrap());
        edges.dedup();
        assert_eq!(edges.len(), n + 1, "Achse {k}: {} Kanten statt {}", edges.len(), n + 1);
        assert_eq!(edges[0], lo[k]);
        assert_eq!(edges[n], hi[k]);
    }
    // #384 §8ac / P5b-1A — **die Zertifizierung ist hier vorübergehend
    // ZURÜCKGEZOGEN, und das ist Absicht.**
    //
    // Der Plattenkern dieser Fixture liegt unter 24 Kanälen mit BEWIESENEM
    // Laufzeitmodus. Die Intervall-Stufe modelliert `fl(δ − min(de, h))`; die
    // Produktion rechnet seit P5b-1A `fl(δ − min(fl(de+A), fl(h+A_h)))`. Eine
    // Einschliessung der alten Kette wäre eine Zusage über ein Feld, das so
    // nicht mehr ausgeliefert wird — also `Unknown`, bis **P5b-2** die neue
    // Kette op-genau modelliert.
    //
    // Der Pin prüft deshalb jetzt zwei Dinge statt einem, und verliert dabei
    // nichts an Schärfe:
    // 1. die Verweigerung ist **die richtige** (fail-closed mit genau diesem
    //    Grund — ein anderes `Unknown` wäre ein echter Regress);
    // 2. die SUBSTANZ, die der Pin je geschützt hat, steht weiter: das Feld
    //    behauptet über dem ganzen Plattenkern Material (`f < 0`).
    // Die Kachelungs-Geometrie oben ist davon unberührt.
    // Bemerkenswert an der Messung: die Verweigerung ist **chirurgisch**. Wo
    // der Kanal-Term ohnehin neutral ist (Fernfeld-Skip), zertifiziert die
    // Stufe unverändert über Zweig 2; nur die Zellen, in denen die
    // Grenzwert-Klemme wirklich bindet, fallen auf `Unknown`. Der Pin verlangt
    // deshalb BEIDES in nennenswerter Zahl — verschwände eine der beiden
    // Sorten, hätte sich entweder die Verweigerung verselbständigt oder sie
    // greift gar nicht mehr.
    let (mut b2, mut refused, mut probes) = (0usize, 0usize, 0usize);
    for cell in &cells {
        let mut c = iv::Ctx::new();
        match iv::certify_cell(cell, &field, delta as f64, CERT_TOL, &none, &mut c) {
            iv::Verdict::Branch2 => b2 += 1,
            iv::Verdict::Unknown(why) => {
                assert!(
                    why.contains("P5b-1A"),
                    "Zelle {cell:?} ist Unknown({why}) — das ist NICHT die P5b-1A-Verweigerung, \
                     sondern ein echter Verlust an Zertifizierung"
                );
                refused += 1;
            }
            other => panic!("Zelle {cell:?} liefert {other:?}"),
        }
        // Die Substanz, die dieser Pin je geschützt hat: das Feld behauptet
        // über dem ganzen Plattenkern Material — auch dort, wo die
        // Zertifizierung bis P5b-2 aussetzt.
        for p in cell_samples(cell, 1) {
            let f = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
            probes += 1;
            assert!(f < 0.0, "Zelle {cell:?} des Plattenkerns liefert bei {p:?} {f} ≥ 0");
        }
    }
    assert_eq!(b2 + refused, n * n * n, "{b2} + {refused} ≠ {} Zellen", n * n * n);
    assert!(
        b2 > 0 && refused > 0,
        "{b2} zertifiziert / {refused} verweigert — der Pin braucht beide Sorten"
    );
    assert!(probes > 3_000, "nur {probes} Proben");
    println!(
        "§8ac: Plattenkern — {b2} von {} Zellen weiter über Zweig 2 zertifiziert, \
         {refused} bis P5b-2 verweigert",
        n * n * n
    );
}

/// **§8w-Sonde: wie sich die Argmin-Zellen auflösen — und was übrig bleibt.**
///
/// Gefahren wird die ECHTE Konfiguration (Zweig 1 an, wie in `certify_cell`).
/// Der Rest wird dreifach klassifiziert, weil die drei Fälle völlig
/// verschiedene Dinge bedeuten:
///
/// * **Vorzeichenwechsel** — zwei Proben mit verschiedenem Vorzeichen BEZEUGEN,
///   dass die Zelle Material UND Luft enthält. Der direkte Zweig-2-Test ist
///   dort per Konstruktion unerreichbar, der direkte Zweig-1-Test scheitert mit
///   dem heutigen `U` und der heutigen Zellgrösse. Das ist die Decke des
///   DIREKTEN Tests auf diesen Zellen — nicht die der Kriterien schlechthin:
///   die adaptive Unterteilung kann so eine Zelle in Material-Kinder (Zweig 2)
///   und randnahe Kinder (Zweig 1) zerlegen, und tut das auch.
/// * **ganz Luft** / **ganz Material** — reine STICHPROBEN-Klassen ohne
///   Vorzeichen-Beweis: sie sagen nur, welches Vorzeichen die Proben hatten.
///   Zur Orientierung brauchbar, als Aussage über die Zelle nicht.
#[test]
#[ignore]
fn probe_384_interval_argmin_resolution() {
    let delta = 2.0f32;
    println!(
        "| Fixture | Zellen | gehüllt | Z1+Z2 direkt | + per Unterteilung | Rest | \
         Vorz.wechsel | ganz Luft | ganz Material | Tiefen-Hist. | ms |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (name, field, _e, lo, hi, _n) in cert_matrix(delta) {
        if !name.starts_with("Ecken-Schale") && !name.starts_with("notch α=90") {
            continue;
        }
        let (stop, blk) = stop_with_lb(&field);
        let cap = delta as f64 + CERT_TOL;
        let u_at = |c: [f64; 3]| u_upper(stop, blk, delta, c, 2.0 * cap);
        let t0 = std::time::Instant::now();
        let (mut hulled, mut direct, mut viasub, mut rest) = (0, 0, 0, 0);
        let (mut flip, mut air, mut solid) = (0, 0, 0);
        let mut hist = [0usize; 5];
        for cell in fixture_cells(lo, hi, 12) {
            let mut c = iv::Ctx::new();
            let v = iv::certify_cell(&cell, &field, delta as f64, CERT_TOL, &u_at, &mut c);
            if c.max_candidates() >= 2 {
                hulled += 1;
            }
            if v.certified() {
                direct += 1;
                continue;
            }
            let mut done = None;
            for depth in 1..=4u32 {
                let mut c2 = iv::Ctx::new();
                if iv::certify_cell_adaptive(
                    &cell,
                    &field,
                    delta as f64,
                    CERT_TOL,
                    &u_at,
                    depth,
                    &mut c2,
                )
                .is_ok()
                {
                    done = Some(depth);
                    break;
                }
            }
            match done {
                Some(dp) => {
                    viasub += 1;
                    hist[dp as usize] += 1;
                }
                None => {
                    rest += 1;
                    let (mut neg, mut pos) = (false, false);
                    for p in cell_samples(&cell, 2) {
                        let f =
                            field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
                        if f < 0.0 {
                            neg = true;
                        } else {
                            pos = true;
                        }
                    }
                    match (neg, pos) {
                        (true, true) => flip += 1,
                        (false, true) => air += 1,
                        _ => solid += 1,
                    }
                }
            }
        }
        println!(
            "| {name} | 1728 | {hulled} | {direct} | {viasub} | {rest} | {flip} | {air} | \
             {solid} | 1:{} 2:{} 3:{} 4:{} | {} |",
            hist[1],
            hist[2],
            hist[3],
            hist[4],
            t0.elapsed().as_millis()
        );
    }
}

/// **§8w-Pin: der exakte Gleichstand und das Tiefen-Limit.**
///
/// Zwei Pflichten, die leicht durchrutschen:
///
/// * **Exakter Tie.** Auf der Mittelsenkrechten zweier Segmente sind beide
///   Distanzen bit-gleich. Die Produktion aktualisiert `best` nur bei
///   `de < best` — **der FRÜHERE Index gewinnt**. Der Hull muss diesen Zweig
///   enthalten. Geprüft wird mit PUNKT-Zellen exakt auf der Grenze, damit der
///   Tie wirklich eintritt und nicht bloss „fast".
/// * **Das Tiefen-Limit ist kein Erfolg.** Eine Zelle, die das Feld-Vorzeichen
///   wechselt, kann `sup_C f < 0` prinzipiell nie erfüllen; mit abgeschaltetem
///   Zweig 1 MUSS die adaptive Zertifizierung sie auf JEDER Tiefe ablehnen —
///   und nicht etwa beim Erreichen des Limits durchwinken.
#[test]
fn interval_exact_tie_and_depth_limit_384() {
    // (1) Exakter Gleichstand.
    let two: Vec<_> = adversarial_channel_cases()
        .into_iter()
        .filter(|(n, ..)| n.starts_with("2-Seg Voronoi-Grenze"))
        .collect();
    let (_n0, ch, _c0, base, d, extra) = two.into_iter().next().expect("Voronoi-Familie fehlt");
    let mut ties = 0usize;
    for (y, z) in [(2.6f64, 0.8f64), (1.3, 0.4), (3.1, -0.6)] {
        // Punkt-Zelle EXAKT auf der Mittelsenkrechten x = 0.
        let p = [0.0f64, y, z];
        let cell = iv::Box3::new(p, p);
        let pf = [0.0f32, y as f32, z as f32];
        // Der Gleichstand muss echt sein, sonst prüft der Pin nichts.
        let (d0, _, _) = crate::offset_channel::arc_closest(&ch.arcs[0], pf);
        let (d1, _, _) = crate::offset_channel::arc_closest(&ch.arcs[1], pf);
        assert_eq!(
            d0.to_bits(),
            d1.to_bits(),
            "bei {pf:?} sind die Distanzen {d0} / {d1} nicht bit-gleich — kein echter Tie"
        );
        ties += 1;
        let mut c = iv::Ctx::new();
        let enc = iv::enclose_channel_apply(&mut c, cell.world_f32(), base, d, &ch, extra)
            .expect("der Hull verweigert auf der Grenze");
        assert!(
            c.max_candidates() >= 2,
            "auf dem exakten Tie wurden nur {} Kandidaten gebildet — der \
             Erster-gewinnt-Zweig könnte fehlen",
            c.max_candidates()
        );
        for b in iv_samples(base) {
            for dd in iv_samples(d) {
                for e in iv_samples(extra) {
                    let got = crate::offset_channel::channel_apply(b, dd, pf, &ch, e);
                    assert!(
                        enc.contains(got as f64),
                        "Tie bei {pf:?}: Produktionswert {got} fehlt im Hull {enc:?} — der \
                         Erster-gewinnt-Zweig ist nicht enthalten"
                    );
                }
            }
        }
    }
    assert_eq!(ties, 3);

    // (2) Tiefen-Limit: eine Zelle, die das Vorzeichen wechselt, wird nie
    //     zertifiziert — auf keiner Tiefe.
    let delta = 2.0f32;
    let (field, _) = compile_for(thin_corner_node(6.0 * delta, 40.0, delta));
    let none = |_c: [f64; 3]| None;
    let mut checked = 0usize;
    for cell in fixture_cells([-36.0, -12.0, -36.0], [-30.0, -2.0, -30.0], 6) {
        let (mut neg, mut pos) = (false, false);
        for p in cell_samples(&cell, 2) {
            let f = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
            if f < 0.0 {
                neg = true;
            } else {
                pos = true;
            }
        }
        if !(neg && pos) {
            continue;
        }
        checked += 1;
        for depth in [0u32, 2, 5] {
            let mut c = iv::Ctx::new();
            let r = iv::certify_cell_adaptive(
                &cell,
                &field,
                delta as f64,
                CERT_TOL,
                &none,
                depth,
                &mut c,
            );
            assert!(
                r.is_err(),
                "die Vorzeichenwechsel-Zelle {cell:?} wurde auf Tiefe {depth} zertifiziert — \
                 `sup_C f < 0` ist dort aber unmöglich, das Tiefen-Limit darf NIE ein \
                 Erfolg sein"
            );
        }
    }
    assert!(checked >= 5, "nur {checked} Vorzeichenwechsel-Zellen gefunden — der Pin misst nichts");
}

/// **§8w-Korrektur-Pin: der Prune kann auch den ERSTEN Bogen überspringen.**
///
/// Blocker des sechsten Reviews. Die erste §8w-Fassung behauptete „Bogen 0
/// wird nie übersprungen, der Prune vergleicht gegen `best = +∞`". Nach IEEE
/// ist `dbs₀ ≥ best` für `dbs₀ = +∞` und `best = +∞` aber **wahr** — und
/// `dbs` überläuft schon bei einer geometrisch völlig gültigen, nur sehr losen
/// Bogen-Kugel. `de` und `dbs` stammen aus verschiedenen Daten; ein endliches
/// `de` schützt nicht.
///
/// Die Familie des Reviews, direkt übernommen: `p = (0,0,0)`, Segment
/// `(0,0,0) → (1,0,0)` (also `de₀ = 0`), Kugel `bc = (2·10¹⁹, 2·10¹⁹, 0)`,
/// `br = 3·10¹⁹` — die Kugel enthält das Segment, aber `(2·10¹⁹)²` überläuft
/// in f32. Die Produktion überspringt damit den EINZIGEN Bogen,
/// `channel_nearest` liefert `None`, und `channel_apply` gibt `d` zurück.
///
/// Zwei Richtungen: mit dem Fix enthält die Hülle diesen Wert, mit der alten
/// Annahme (`Knobs::assume_first_arc_evaluated`) MUSS sie ihn verlieren.
#[test]
fn interval_prune_can_skip_even_the_first_arc_384() {
    let far = 2.0e19f32;
    let mut arc = seg_arc(
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    );
    arc.bc = [far, far, 0.0];
    arc.br = 3.0e19;
    // Die Kugel enthält das Segment wirklich — sonst wäre die Fixture ungültig.
    for e in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0]] {
        let d = ((e[0] as f64 - far as f64).powi(2)
            + (e[1] as f64 - far as f64).powi(2)
            + (e[2] as f64).powi(2))
        .sqrt();
        assert!(d <= arc.br as f64, "die Bogen-Kugel enthält den Endpunkt {e:?} nicht");
    }
    // Und `dbs` läuft in f32 wirklich über, `de` bleibt endlich.
    let p = [0.0f32, 0.0, 0.0];
    let dbs = {
        let v = [p[0] - arc.bc[0], p[1] - arc.bc[1], p[2] - arc.bc[2]];
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - arc.br
    };
    assert!(dbs.is_infinite(), "dbs ist {dbs}, nicht +∞ — die Fixture trifft den Fall nicht");
    let (de0, _, _) = crate::offset_channel::arc_closest(&arc, p);
    assert!(de0.is_finite(), "de₀ ist {de0}, nicht endlich");

    let mut ch = synth_channel(vec![arc], true);
    // Die KANAL-Kugel muss normal bleiben — sonst greift schon `channel_skip`
    // und der Test prüfte den falschen Ausgang (erste Fassung tat genau das:
    // `synth_channel` erbt `bound_c`/`bound_r` von den Bögen).
    ch.bound_c = [0.0, 0.0, 0.0];
    ch.bound_r = 10.0;
    assert!(
        !crate::offset_channel::channel_skip(5.0, [0.0; 3], &ch),
        "`channel_skip` greift bereits auf Kanal-Ebene — der Pin prüft dann nicht den \
         Bogen-Prune"
    );
    let cell = iv::Box3::new([0.0; 3], [0.0; 3]);
    let (base, d, extra) = (ivf(5.0, 5.0), ivf(5.0, 5.0), ivf(10.0, 10.0));
    // Die Produktion liefert `d` — weil `channel_nearest` `None` ist.
    let got = crate::offset_channel::channel_apply(5.0, 5.0, p, &ch, 10.0);
    assert_eq!(got, 5.0, "die Produktion liefert {got}, erwartet war der unveränderte `d`");
    let lim = crate::offset_channel::channel_limit(p, &ch);
    assert_eq!(lim, -crate::offset_channel::END_PLANE_OFF, "channel_limit ist nicht neutral");

    // (1) Mit dem Fix: die Hülle enthält beides.
    let mut c = iv::Ctx::new();
    let enc = iv::enclose_channel_apply(&mut c, cell.world_f32(), base, d, &ch, extra)
        .expect("die Einschliessung verweigert");
    assert!(
        enc.contains(got as f64),
        "der Produktionswert {got} fehlt in der Hülle {enc:?} — der `None`-Ausgang ist \
         nicht mitgehüllt"
    );
    let mut c2 = iv::Ctx::new();
    let encl = iv::enclose_channel_limit(&mut c2, cell.world_f32(), &ch)
        .expect("channel_limit verweigert");
    assert!(
        encl.contains(lim as f64),
        "der neutrale Grenzwert {lim} fehlt in {encl:?}"
    );

    // (2) Die Rückdreh-Gegenprobe: mit der alten Annahme reisst es.
    let knobs = iv::Knobs { assume_first_arc_evaluated: true, ..Default::default() };
    let mut c3 = iv::Ctx::with_knobs(knobs);
    let bad = iv::enclose_channel_apply(&mut c3, cell.world_f32(), base, d, &ch, extra)
        .expect("die Gegenprobe verweigert statt zu rechnen");
    assert!(
        !bad.contains(got as f64),
        "die Gegenprobe blieb still: mit der Annahme „Bogen 0 wird immer ausgewertet\u{201c} \
         MUSS der Produktionswert {got} aus der Hülle {bad:?} fallen — sonst belegt der \
         Pin den Fix nicht"
    );
}

// ══════════════ #384 §8x / P5a — der Aussenzeugen-Kern ══════════════

use crate::offset_witness as wit;

/// `Ω` und die Kanäle eines Fixture-Feldes.
fn omega_and_channels(field: &BoundedCsg) -> Vec<(&BoundedCsg, &Vec<OffsetChannel>)> {
    let mut stops = Vec::new();
    stop_nodes(field, &mut stops);
    let mut out = Vec::new();
    for st in stops {
        if let BoundedCsg::ArcChannel { inner, channels, .. } = st {
            if let Some(om) = wit::omega_of(inner) {
                out.push((om, channels));
            }
        }
    }
    out
}

/// **§8x-Sonde: die Zeugen-Abdeckung der Nähte.**
#[test]
#[ignore]
fn probe_384_witness_cover() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    println!(
        "| Fixture | Kanäle | Stücke | gedeckt | Naht mm | ungedeckt mm | η_geo mm | Grund |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for (name, field, _e, _lo, _hi, _n) in cert_matrix(delta) {
        let (mut np, mut nc, mut sl, mut ul, mut nch) = (0usize, 0usize, 0.0f64, 0.0f64, 0usize);
        let mut worst: Option<f64> = None;
        let mut any_open = false;
        for (om, chans) in omega_and_channels(&field) {
            for ch in chans {
                let c = wit::cover_channel(om, ch, cfg);
                np += c.pieces;
                nc += c.covered;
                sl += c.seam_len;
                ul += c.uncovered_len;
                nch += 1;
                match c.eta {
                    Some(e) => {
                        let g = e.mm();
                        worst = Some(worst.map_or(g, |w: f64| w.max(g)));
                    }
                    None => any_open = true,
                }
            }
        }
        let eta = match (any_open, worst) {
            (false, Some(w)) => format!("{w:.5}"),
            _ => "— (offen)".into(),
        };
        // Warum bleibt etwas offen? Zwei strukturelle Gründe sind zu trennen.
        let mut curved = 0usize;
        let mut unsupported_leaf = false;
        for (om, chans) in omega_and_channels(&field) {
            for ch in chans {
                curved += ch.arcs.iter().filter(|a| !(a.sweep < 0.0)).count();
            }
            fn walk(n: &BoundedCsg, bad: &mut bool) {
                match n {
                    BoundedCsg::Leaf { primitive, .. } => {
                        if !matches!(
                            primitive,
                            Primitive::Box { .. }
                                | Primitive::Sphere { .. }
                                | Primitive::Cylinder { .. }
                        ) {
                            *bad = true;
                        }
                    }
                    BoundedCsg::Union { left, right, .. }
                    | BoundedCsg::Difference { left, right, .. }
                    | BoundedCsg::Intersection { left, right, .. } => {
                        walk(left, bad);
                        walk(right, bad);
                    }
                    BoundedCsg::Offset { inner, .. } | BoundedCsg::InnerWall { inner, .. } => {
                        walk(inner, bad)
                    }
                    _ => *bad = true,
                }
            }
            walk(om, &mut unsupported_leaf);
        }
        let reason = if !any_open {
            "—".to_string()
        } else if unsupported_leaf {
            "Ω-Blatt nicht unterstützt (Extrude)".into()
        } else if curved > 0 {
            format!("{curved} gekrümmte Bögen (P4)")
        } else {
            "kein Zeuge gefunden".into()
        };
        println!(
            "| {name} | {nch} | {np} | {nc} ({:.1} %) | {sl:.1} | {ul:.2} | {eta} | {reason} |",
            100.0 * nc as f64 / np.max(1) as f64
        );
    }
}

/// Ein L aus zwei Boxen als Ω-Spielwiese.
fn l_omega() -> BoundedCsg {
    let n = CsgNode::Union(
        Box::new(box_at(1, [0.0, -5.0, 0.0], [20.0, 5.0, 20.0])),
        Box::new(box_at(2, [-5.0, 0.0, 0.0], [5.0, 20.0, 20.0])),
    );
    BoundedCsg::from_csg(&n)
}

fn ivp(y: [f64; 3]) -> [iv::Iv; 3] {
    [iv::Iv::point(y[0]), iv::Iv::point(y[1]), iv::Iv::point(y[2])]
}

/// **§8x-Pin 1: die Aussenmitgliedschaft ist geometrisch — und strikt.**
///
/// Beweispflicht 1. Vier Richtungen, alle falsifizierend:
///
/// * klar aussen ⇒ bewiesen;
/// * klar innen ⇒ NICHT bewiesen (das ist der Falsifikator „Zeuge auf der
///   falschen Member-Seite");
/// * **exakt auf der Fläche** ⇒ NICHT bewiesen — eine Berührung ist kein
///   Beweis für „ausserhalb", und genau dafür steht die Marge;
/// * eine Box, die den Rand ÜBERSPANNT ⇒ NICHT bewiesen.
///
/// Dazu die Rückdreh-Probe: mit NEGATIVER Marge fällt die Strenge weg, und der
/// Randpunkt rutscht durch.
#[test]
fn witness_outside_membership_is_geometric_384() {
    let om = l_omega();
    let m = 1.0e-9;
    let (mut outs, mut ins) = (0usize, 0usize);
    // Klar aussen (jenseits beider Boxen).
    for y in [
        [10.0f64, 10.0, 0.0],
        [0.5, 0.5, 0.0],
        [-30.0, 0.0, 0.0],
        [0.0, -30.0, 0.0],
        [0.0, 0.0, 40.0],
    ] {
        assert!(
            wit::box_outside_omega(&om, ivp(y), m),
            "{y:?} liegt ausserhalb, wird aber nicht als aussen bewiesen"
        );
        outs += 1;
    }
    // Klar INNEN — der Falsifikator „falsche Member-Seite".
    for y in [
        [0.0f64, -5.0, 0.0],
        [-5.0, 0.0, 0.0],
        [-2.0, -2.0, 5.0],
        [15.0, -9.0, -15.0],
    ] {
        assert!(
            !wit::box_outside_omega(&om, ivp(y), m),
            "{y:?} liegt IN Ω, wird aber als aussen „bewiesen“ — die Prüfung ist wertlos"
        );
        ins += 1;
    }
    // Exakt auf der Deckfläche der ersten Box (y = 0): Berührung, kein Beweis.
    for y in [[0.0f64, 0.0, 0.0], [10.0, 0.0, 10.0], [-5.0, 20.0, 0.0]] {
        assert!(
            !wit::box_outside_omega(&om, ivp(y), m),
            "der Randpunkt {y:?} wird als aussen bewiesen — die Marge trägt nicht"
        );
    }
    // Eine Box, die den Rand überspannt.
    let straddle = [
        iv::Iv { lo: 8.0, hi: 12.0 },
        iv::Iv { lo: -1.0, hi: 1.0 },
        iv::Iv { lo: 0.0, hi: 1.0 },
    ];
    assert!(
        !wit::box_outside_omega(&om, straddle, m),
        "eine den Rand überspannende Box wird als ganz aussen bewiesen"
    );
    // Die Marge ist FAIL-CLOSED (§8x-Korrektur): eine negative oder nicht
    // endliche Marge hebt die Strenge nicht auf, sie verweigert. Vorher liess
    // sie einen Randpunkt durch — ein real erreichbarer API-Pfad, kein blosser
    // Mutationstest.
    for bad in [-1.0f64, -1e-18, f64::NAN, f64::INFINITY] {
        assert!(
            !wit::box_outside_omega(&om, ivp([10.0, 10.0, 0.0]), bad),
            "Marge {bad} wird akzeptiert — die Konfiguration ist nicht fail-closed"
        );
    }
    // Dass die STRENGE trägt, zeigt der Vergleich Randpunkt gegen einen Punkt
    // ein Stück daneben: beide legal (Marge 0), nur einer bewiesen.
    assert!(!wit::box_outside_omega(&om, ivp([10.0, 0.0, 10.0]), 0.0));
    assert!(wit::box_outside_omega(&om, ivp([10.0, 1e-6, 10.0]), 0.0));
    assert!(outs >= 5 && ins >= 4, "nur {outs}/{ins} Fälle geprüft");
}

/// **§8x-Pin 2: ein kleiner Feldwert ist KEIN Aussenzeuge.**
///
/// Der stehende Vorbehalt aus §8u, jetzt als Pin. Auf der Erosionsfläche ist
/// `|f| ≈ 0` — und trotzdem liegen diese Punkte **tief in `Ω`** (nämlich `δ`
/// vom Rand entfernt). Wer aus `|f| ≤ ε` auf Randzugehörigkeit oder gar auf
/// Aussenlage schlösse, läge um die volle Wandstärke daneben.
#[test]
fn witness_small_field_value_is_not_outside_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(thin_corner_node(6.0 * delta, 40.0, delta));
    let om = omega_and_channels(&field).first().map(|(o, _)| *o).expect("kein Ω");
    let mut near_zero = 0usize;
    for cell in fixture_cells([-15.0; 3], [1.0; 3], 26) {
        use iv::CellDomain;
        let c = cell.center();
        let f = field.sdf([c[0] as f32, c[1] as f32, c[2] as f32], EvalMode::Visual);
        // Nahe der Erosionsfläche: dort ist |f| klein UND der Punkt liegt δ tief
        // in Ω — die schärfste Form des Missverständnisses.
        if f.abs() > 0.2 {
            continue;
        }
        near_zero += 1;
        assert!(
            !wit::box_outside_omega(om, ivp(c), 1.0e-9),
            "der Punkt {c:?} hat |f| = {} ≈ 0 und wird trotzdem als AUSSERHALB von Ω \
             bewiesen — dann verwechselt die Prüfung den Feldwert mit der Geometrie",
            f.abs()
        );
    }
    assert!(
        near_zero >= 20,
        "nur {near_zero} Punkte mit |f| ≈ 0 gefunden — der Pin misst nichts"
    );
}

/// **§8x-Pin 3: die Abdeckung ist eine ABBILDUNG, keine Stichprobe.**
///
/// Beweispflichten 2 und 3. Für dicht gesetzte Naht-Punkte `q` — deutlich
/// mehr als es Stücke gibt — wird der zugehörige Zeuge `y(q) = q + t·d`
/// unabhängig nachgerechnet:
///
/// * `y(q)` liegt **einzeln** bewiesen in `∁Ω`;
/// * `|q − y(q)| ≤ η`;
/// * die Stück-Intervalle **kacheln** `[0,1]` lückenlos.
///
/// Rückdreh-Probe: wird ein Stück aus der Abdeckung gestrichen, MUSS ein
/// dichter Naht-Punkt ohne Zeugen bleiben.
#[test]
fn witness_cover_is_a_map_not_a_sample_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(notch_union_node(90.0, delta as f64));
    let cfg = wit::WitnessCfg::default();
    let (mut checked, mut chans) = (0usize, 0usize);
    for (om, channels) in omega_and_channels(&field) {
        for ch in channels {
            let c = wit::cover_channel(om, ch, cfg);
            let Some(eta) = c.eta else { continue };
            chans += 1;
            let g = eta.mm();
            assert_eq!(eta.cover_mm(), 0.0, "der Abdeckungs-Anteil ist nicht null");
            // Die Kachelung je Bogen ist lückenlos.
            for ai in 0..ch.arcs.len() {
                let mut ts: Vec<(f64, f64)> = c
                    .covered_pieces
                    .iter()
                    .filter(|p| p.arc == ai)
                    .map(|p| (p.t0, p.t1))
                    .collect();
                ts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                assert_eq!(ts.first().map(|x| x.0), Some(0.0), "Bogen {ai}: Kachelung beginnt nicht bei 0");
                assert_eq!(ts.last().map(|x| x.1), Some(1.0), "Bogen {ai}: Kachelung endet nicht bei 1");
                for w in ts.windows(2) {
                    assert_eq!(w[0].1, w[1].0, "Bogen {ai}: Lücke zwischen {:?} und {:?}", w[0], w[1]);
                }
            }
            // Dichte Naht-Punkte: jeder bekommt seinen Zeugen.
            let dense = 24 * cfg.pieces_per_arc;
            for (ai, arc) in ch.arcs.iter().enumerate() {
                for i in 0..=dense {
                    let s = i as f64 / dense as f64;
                    let q = [
                        arc.c[0] as f64 + arc.u[0] as f64 * (s * arc.r as f64),
                        arc.c[1] as f64 + arc.u[1] as f64 * (s * arc.r as f64),
                        arc.c[2] as f64 + arc.u[2] as f64 * (s * arc.r as f64),
                    ];
                    let piece = c
                        .covered_pieces
                        .iter()
                        .find(|p| p.arc == ai && s >= p.t0 && s <= p.t1)
                        .unwrap_or_else(|| panic!("Naht-Punkt s={s} ohne Stück"));
                    // **Dieselben Daten wie im Beweis**: `y(q) = q + shift`.
                    let y = [
                        q[0] + piece.shift[0],
                        q[1] + piece.shift[1],
                        q[2] + piece.shift[2],
                    ];
                    // **Unabhängiges Aussen-Orakel** (§8x-Korrektur): NICHT
                    // wieder `box_outside_omega` — sonst prüfte der Pin
                    // dieselbe Logik, die schon die Stück-Box akzeptiert hat.
                    // `Ω` besteht hier aus konvexen Primitiven mit EXAKTER SDF,
                    // deshalb ist `sdf(y) > 0` ⟺ `y ∉ Ω`. (Das ist eine
                    // Test-Annahme über diese Fixture, kein allgemeiner Satz —
                    // der Beweis selbst benutzt sie nirgends.)
                    let orac = om.sdf([y[0] as f32, y[1] as f32, y[2] as f32], EvalMode::Visual);
                    assert!(
                        orac > 0.0,
                        "der Zeuge {y:?} zu q={q:?} liegt laut unabhängigem Orakel IN Ω \
                         (sdf = {orac})"
                    );
                    let d = ((y[0] - q[0]).powi(2) + (y[1] - q[1]).powi(2) + (y[2] - q[2]).powi(2))
                        .sqrt();
                    assert!(d <= g + 1e-12, "|q − y| = {d} übersteigt η = {g}");
                    checked += 1;
                }
            }
            // Rückdreh 1: ein gestrichenes Stück lässt Naht-Punkte ohne Zeugen.
            let mut gapped = c.covered_pieces.clone();
            let removed = gapped.remove(gapped.len() / 2);
            let mid = 0.5 * (removed.t0 + removed.t1);
            assert!(
                !gapped.iter().any(|p| p.arc == removed.arc && mid >= p.t0 && mid <= p.t1),
                "die Gegenprobe blieb still: nach dem Streichen eines Stücks MUSS der Punkt \
                 s={mid} unbedeckt sein — sonst prüft die Kachelung nichts"
            );
            // Rückdreh 2 (§8x-Korrektur): **den gespeicherten `shift` mutieren.**
            // Ein anderer Verschiebungsvektor ist eine andere Behauptung — wer
            // den Beweis für `t·d` führt und `off·d` ausliefert, hat nichts
            // bewiesen. Der um den Faktor −3 gedrehte Zeuge muss in `Ω` landen.
            let mut broke = 0usize;
            for piece in &c.covered_pieces {
                let arc = &ch.arcs[piece.arc];
                let sm = 0.5 * (piece.t0 + piece.t1);
                let q = [
                    arc.c[0] as f64 + arc.u[0] as f64 * (sm * arc.r as f64),
                    arc.c[1] as f64 + arc.u[1] as f64 * (sm * arc.r as f64),
                    arc.c[2] as f64 + arc.u[2] as f64 * (sm * arc.r as f64),
                ];
                let y = [
                    q[0] - 3.0 * piece.shift[0],
                    q[1] - 3.0 * piece.shift[1],
                    q[2] - 3.0 * piece.shift[2],
                ];
                if om.sdf([y[0] as f32, y[1] as f32, y[2] as f32], EvalMode::Visual) <= 0.0 {
                    broke += 1;
                }
            }
            assert!(
                broke > 0,
                "die Gegenprobe blieb still: ein mutierter Verschiebungsvektor MUSS bei \
                 mindestens einem Stück in Ω landen — sonst belegt der Pin nicht, dass \
                 Beweis und Auslieferung DIESELBEN Daten sind"
            );
        }
    }
    assert!(chans >= 1 && checked > 300, "{chans} Kanäle / {checked} Naht-Punkte");
}

/// **§8x-Pin 4: das Box-Ball-Lemma.**
///
/// Beweispflicht 4 (Baustein für P6a). `|x − y|²` ist über einer AABB
/// separabel, jeder Summand als konvexe Funktion von `x_k` an einem Endpunkt
/// maximal — das Maximum liegt also an einer ECKE. Der Pin prüft die Schranken
/// gegen dichte Proben, an adversarialen Boxen (grosse Beträge, entartete
/// Achsen), und weist nach, dass die obere Schranke wirklich an einer Ecke
/// angenommen wird.
#[test]
fn witness_box_ball_lemma_384() {
    let mut cases = 0usize;
    let boxes: Vec<([f64; 3], [f64; 3], [f64; 3])> = vec![
        ([0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [5.0, -4.0, 0.5]),
        ([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]),
        ([1e8, 1e8, 1e8], [1e8 + 1.0, 1e8 + 2.0, 1e8], [-1e8, 0.0, 1e8]),
        ([3.0, 3.0, 3.0], [3.0, 3.0, 3.0], [0.0, 0.0, 0.0]),
        ([-1e-9, 0.0, 0.0], [1e-9, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    for (lo, hi, y) in boxes {
        let (dmin, dmax) = wit::box_point_dist_bounds(lo, hi, y);
        assert!(dmin <= dmax, "Schranken verdreht: {dmin} > {dmax}");
        let n = 6usize;
        let mut worst = 0.0f64;
        for i in 0..=n {
            for j in 0..=n {
                for k in 0..=n {
                    let f = |a: f64, b: f64, t: usize| a + (b - a) * t as f64 / n as f64;
                    let x = [f(lo[0], hi[0], i), f(lo[1], hi[1], j), f(lo[2], hi[2], k)];
                    let d = ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2))
                        .sqrt();
                    assert!(d <= dmax, "Probe {x:?}: {d} > obere Schranke {dmax}");
                    assert!(d >= dmin, "Probe {x:?}: {d} < untere Schranke {dmin}");
                    worst = worst.max(d);
                }
            }
        }
        // Das Maximum liegt an einer Ecke: das dichte Gitter enthält alle Ecken,
        // also muss es die obere Schranke praktisch erreichen.
        assert!(
            worst >= dmax * (1.0 - 1e-9) - 1e-9,
            "die obere Schranke {dmax} wird von keiner Ecke erreicht (bestes {worst}) — \
             dann ist sie nicht die Ecken-Schranke des Lemmas"
        );
        cases += 1;
    }
    assert_eq!(cases, 5);
}

/// **§8x-Pin 5: die öffentliche Konfiguration ist fail-closed.**
///
/// Blocker 2 des achten Reviews. `WitnessCfg` war ungeprüft — und das war
/// nicht bloss unsauber: bei einem NEGATIVEN `min_offset` konnte die Suche
/// einen echten Zeugen finden, während `worst_offset.max(off)` bei `0` blieb.
/// Ergebnis wäre `η = 0` **trotz positiven Abstands** gewesen: eine Zusage aus
/// einer kaputten Eingabe, und die ist schlimmer als keine.
#[test]
fn witness_config_is_fail_closed_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(notch_union_node(90.0, delta as f64));
    let (om, chans) = {
        let v = omega_and_channels(&field);
        let (o, c) = v.into_iter().next().expect("kein Ω");
        (o, c.clone())
    };
    let ch = chans.first().expect("kein Kanal");
    let good = wit::WitnessCfg::default();
    assert!(good.valid());
    assert!(
        wit::cover_channel(om, ch, good).eta.is_some(),
        "die gültige Konfiguration liefert kein η — dann prüft der Pin nur Ablehnungen"
    );
    let bad: Vec<(&str, wit::WitnessCfg)> = vec![
        ("min_offset negativ", wit::WitnessCfg { min_offset: -1e-4, ..good }),
        ("min_offset null", wit::WitnessCfg { min_offset: 0.0, ..good }),
        ("min > max", wit::WitnessCfg { min_offset: 9.0, max_offset: 1.0, ..good }),
        ("max_offset NaN", wit::WitnessCfg { max_offset: f64::NAN, ..good }),
        ("min_offset inf", wit::WitnessCfg { min_offset: f64::INFINITY, ..good }),
        ("margin negativ", wit::WitnessCfg { margin: -1.0, ..good }),
        ("margin NaN", wit::WitnessCfg { margin: f64::NAN, ..good }),
        ("pieces null", wit::WitnessCfg { pieces_per_arc: 0, ..good }),
        ("steps null", wit::WitnessCfg { steps: 0, ..good }),
    ];
    for (tag, cfg) in bad {
        assert!(!cfg.valid(), "{tag}: `valid()` sagt ja");
        let c = wit::cover_channel(om, ch, cfg);
        assert!(c.eta.is_none(), "{tag}: liefert trotzdem ein η");
        assert_eq!(c.covered, 0, "{tag}: meldet {} gedeckte Stücke", c.covered);
        assert!(c.uncovered_len > 0.0, "{tag}: meldet nichts als ungedeckt");
    }
    // Entartete Bögen werden ebenfalls verweigert, nicht „gedeckt".
    let mut degen = ch.clone();
    degen.arcs[0].r = 0.0;
    assert!(wit::cover_channel(om, &degen, good).eta.is_none(), "entarteter Bogen liefert η");
    let mut nan_arc = ch.clone();
    nan_arc.arcs[0].u[0] = f32::NAN;
    assert!(wit::cover_channel(om, &nan_arc, good).eta.is_none(), "NaN-Bogen liefert η");
}

/// **§8x-Pin 6: die Aussen-Regeln über Primitive und CSG-Verschachtelung.**
///
/// Ergänzung aus dem achten Review: die erste Fassung prüfte nur eine Union
/// achsenparalleler Boxen. Hier kommen die Fälle dazu, in denen die
/// Richtungslogik wirklich kippen könnte — gedrehte und stark skalierte Box,
/// Sphere, Zylinder radial UND axial, `Intersection`, `Difference` im
/// `Difference`, und beide Offset-Vorzeichen.
///
/// Gegenprobe je Fall: ein Punkt INNEN darf nie als aussen durchgehen. Und der
/// `Offset(δ > 0)`-Fall MUSS verweigern, obwohl das Kind aussen ist — dort
/// dreht sich die Schlussrichtung um.
#[test]
fn witness_outside_rules_cover_primitives_and_nesting_384() {
    let m = 1.0e-9;
    let t_rot = |deg: f64, sc: f32| Transform {
        translate: [3.0, -1.0, 2.0],
        rotation: {
            let h = 0.5 * deg.to_radians();
            [0.0, 0.0, h.sin() as f32, h.cos() as f32]
        },
        scale: [sc; 3],
    };
    let leaf = |prim: Primitive, tr: Transform| BoundedCsg::Leaf {
        id: id(1),
        primitive: prim,
        world_transform: tr,
        aabb: crate::csg::Aabb::EMPTY,
    };
    let ident = Transform { translate: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
    // (name, Knoten, aussen-Punkte, innen-Punkte)
    let cases: Vec<(&str, BoundedCsg, Vec<[f64; 3]>, Vec<[f64; 3]>)> = vec![
        (
            "gedrehte + skalierte Box",
            leaf(Primitive::Box { center: [0.0; 3], half_extents: [2.0, 1.0, 3.0] }, t_rot(37.0, 2.5)),
            vec![[3.0, -1.0, 2.0 + 9.0], [3.0 + 12.0, -1.0, 2.0], [3.0, -1.0 - 8.0, 2.0]],
            vec![[3.0, -1.0, 2.0], [3.0, -1.0, 2.0 + 5.0]],
        ),
        (
            "Sphere",
            leaf(Primitive::Sphere { center: [1.0, 2.0, 3.0], radius: 4.0 }, ident),
            vec![[1.0, 2.0, 8.0], [-4.5, 2.0, 3.0]],
            vec![[1.0, 2.0, 3.0], [1.0, 2.0, 6.5]],
        ),
        (
            "Zylinder radial und axial",
            leaf(
                Primitive::Cylinder { center: [0.0; 3], radius: 2.0, half_height: 5.0 },
                ident,
            ),
            vec![[3.0, 0.0, 0.0], [0.0, 6.0, 0.0], [0.0, -6.0, 0.0], [0.0, 0.0, 2.5]],
            vec![[0.0, 0.0, 0.0], [1.5, 4.0, 0.0], [0.0, -4.9, 1.9]],
        ),
    ];
    let mut checked = 0usize;
    for (name, node, outs, ins) in cases {
        for y in &outs {
            assert!(wit::box_outside_omega(&node, ivp(*y), m), "{name}: {y:?} nicht aussen belegt");
            checked += 1;
        }
        for y in &ins {
            assert!(
                !wit::box_outside_omega(&node, ivp(*y), m),
                "{name}: {y:?} liegt INNEN, wird aber als aussen belegt"
            );
            checked += 1;
        }
    }
    // Verschachtelung.
    let b = |c: [f32; 3], h: [f32; 3]| BoundedCsg::from_csg(&box_at(1, c, h));
    let big = b([0.0; 3], [10.0; 3]);
    let small = b([0.0; 3], [3.0; 3]);
    let inter = BoundedCsg::Intersection {
        left: Box::new(big.clone()),
        right: Box::new(small.clone()),
        aabb: crate::csg::Aabb::EMPTY,
    };
    // Aussen vom KLEINEN genügt für den Schnitt.
    assert!(wit::box_outside_omega(&inter, ivp([5.0, 0.0, 0.0]), m));
    assert!(!wit::box_outside_omega(&inter, ivp([1.0, 1.0, 1.0]), m));
    checked += 2;
    // Difference im Difference: links aussen genügt, egal was rechts steht.
    let dd = BoundedCsg::Difference {
        left: Box::new(BoundedCsg::Difference {
            left: Box::new(small.clone()),
            right: Box::new(b([2.0, 0.0, 0.0], [1.0; 3])),
            aabb: crate::csg::Aabb::EMPTY,
        }),
        right: Box::new(b([-2.0, 0.0, 0.0], [1.0; 3])),
        aabb: crate::csg::Aabb::EMPTY,
    };
    assert!(wit::box_outside_omega(&dd, ivp([9.0, 0.0, 0.0]), m));
    assert!(!wit::box_outside_omega(&dd, ivp([0.0, 0.0, 0.0]), m));
    checked += 2;
    // Offset: δ ≤ 0 erbt die Aussenlage, δ > 0 MUSS verweigern.
    let neg = BoundedCsg::Offset {
        inner: Box::new(small.clone()),
        delta: -1.0,
        aabb: crate::csg::Aabb::EMPTY,
    };
    let pos = BoundedCsg::Offset {
        inner: Box::new(small),
        delta: 1.0,
        aabb: crate::csg::Aabb::EMPTY,
    };
    let just_out = ivp([3.5, 0.0, 0.0]);
    assert!(
        wit::box_outside_omega(&neg, just_out, m),
        "Offset(δ<0) erbt die Aussenlage des Kindes nicht"
    );
    assert!(
        !wit::box_outside_omega(&pos, just_out, m),
        "Offset(δ>0) behauptet Aussenlage, obwohl der Körper GRÖSSER ist als sein Kind — \
         dort dreht sich die Schlussrichtung um"
    );
    checked += 2;
    assert!(checked >= 20, "nur {checked} Fälle geprüft");
}

// ══════════════ #384 §8y / P4-A0 — der algebraische Bogen ══════════════

use crate::offset_channel::{arc_data_valid, diamond, diamond_dir};

/// Ein gekrümmter Bogen von Hand: Radius `r` um den Ursprung in der xy-Ebene,
/// Startrichtung `+x`, Öffnungswinkel `alpha`.
fn synth_arc(r: f32, alpha: f64, m1s: [f32; 3], m1e: [f32; 3]) -> ChannelArc {
    let e = [alpha.cos() as f32, alpha.sin() as f32];
    let en = (e[0] * e[0] + e[1] * e[1]).sqrt();
    let e = [e[0] / en, e[1] / en];
    let d_e = diamond(e[0], e[1]);
    let a = [r, 0.0, 0.0];
    let b = [r * e[0], r * e[1], 0.0];
    ChannelArc {
        c: [0.0; 3],
        u: [1.0, 0.0, 0.0],
        v: [0.0, 1.0, 0.0],
        r,
        sweep: alpha as f32,
        e,
        d_e,
        bc: [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1]), 0.0],
        br: 0.5 * ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() + 1e-4,
        m1s,
        m2s: [0.0, 0.0, 1.0],
        m1e,
        m2e: [0.0, 0.0, 1.0],
    }
}

/// **§8y-Pin 1: das Diamant-Lemma.**
///
/// Streng monoton auf `[0,π]`, an der Quadranten-Naht `x = 0` konsistent,
/// Nenner bewiesen nullfrei, und die Inverse liefert DENSELBEN Strahl.
/// Dazu die geschlossene Form `D(φ) = (1 + tan(φ − π/4))/2` auf `[0,π/2]`,
/// die die Analyse analytisch statt abgetastet macht.
#[test]
fn arc_diamond_is_monotone_and_invertible_384() {
    let n = 4000;
    let mut prev = -1.0f64;
    let mut min_den = f64::INFINITY;
    for i in 0..=n {
        let phi = std::f64::consts::PI * i as f64 / n as f64;
        let (x, y) = (phi.cos() as f32, phi.sin() as f32);
        let d = diamond(x, y) as f64;
        assert!(d.is_finite() && (0.0..=2.0).contains(&d), "D({phi}) = {d} ausserhalb [0,2]");
        assert!(d > prev - 1e-6, "D nicht monoton bei φ={phi}: {prev} → {d}");
        prev = d;
        min_den = min_den.min(if x >= 0.0 { (x + y) as f64 } else { (y - x) as f64 });
        // Inverse: derselbe Strahl.
        let w = diamond_dir(d as f32);
        let wn = (w[0] * w[0] + w[1] * w[1]).sqrt();
        let (cx, cy) = (w[0] / wn, w[1] / wn);
        assert!(
            ((cx - x).abs() as f64) < 2e-3 && ((cy - y).abs() as f64) < 2e-3,
            "Inverse bei φ={phi}: ({cx},{cy}) statt ({x},{y})"
        );
    }
    assert!(min_den > 0.9, "kleinster Nenner {min_den} — das Nullfrei-Lemma trägt nicht");
    // Quadranten-Naht: beide Zweige liefern exakt 1.
    assert_eq!(diamond(0.0, 1.0), 1.0);
    assert_eq!(diamond(0.0, 7.5), 1.0);
    assert_eq!(diamond(-0.0, 1.0), 1.0);
    // Endpunkte.
    assert_eq!(diamond(1.0, 0.0), 0.0);
    assert_eq!(diamond(-1.0, 0.0), 2.0);
    // Geschlossene Form auf [0, π/2].
    for i in 0..=200 {
        let phi = std::f64::consts::FRAC_PI_2 * i as f64 / 200.0;
        let cf = (1.0 + (phi - std::f64::consts::FRAC_PI_4).tan()) / 2.0;
        let d = diamond(phi.cos() as f32, phi.sin() as f32) as f64;
        assert!((d - cf).abs() < 1e-5, "geschlossene Form bricht bei φ={phi}: {d} vs {cf}");
    }
}

/// **§8y-Pin 2: das Sektorlemma und der nächste Punkt.**
///
/// `innen ⟺ a = y ≥ 0 ∧ b = x·e₁ − y·e₀ ≥ 0` für Minor-Bögen. Geprüft gegen
/// eine unabhängige Referenz: der wahre nächste Punkt eines Kreisbogens,
/// per dichter Abtastung des Bogens berechnet.
#[test]
fn arc_closest_matches_a_dense_reference_384() {
    let (mut cases, mut worst) = (0usize, 0.0f64);
    for alpha in [0.02f64, 0.4, 1.0, 1.5707963, 2.2, 3.0, 3.14159265] {
        for r in [0.05f32, 1.0, 40.0] {
            let arc = synth_arc(r, alpha, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
            assert!(arc_data_valid(&arc), "α={alpha}, r={r}: Dateninvariante verletzt");
            for i in 0..23 {
                for j in 0..23 {
                    for k in 0..3 {
                        let p = [
                            (i as f32 - 11.0) * 0.37 * r.max(1.0),
                            (j as f32 - 11.0) * 0.41 * r.max(1.0),
                            (k as f32 - 1.0) * 0.9 * r.max(1.0),
                        ];
                        let (de, q, t) = crate::offset_channel::arc_closest(&arc, p);
                        // Unabhängige Referenz: dichte Abtastung des Bogens.
                        let n = 20000;
                        let mut best = f64::INFINITY;
                        for s in 0..=n {
                            let ph = alpha * s as f64 / n as f64;
                            let qq = [
                                r as f64 * ph.cos(),
                                r as f64 * ph.sin(),
                                0.0,
                            ];
                            let d = ((p[0] as f64 - qq[0]).powi(2)
                                + (p[1] as f64 - qq[1]).powi(2)
                                + (p[2] as f64 - qq[2]).powi(2))
                            .sqrt();
                            best = best.min(d);
                        }
                        let err = (de as f64 - best).abs();
                        worst = worst.max(err);
                        assert!(
                            err < 1e-3 * (r as f64).max(1.0),
                            "α={alpha} r={r} p={p:?}: arc_closest {de} vs Referenz {best}"
                        );
                        assert!((0.0..=1.0).contains(&t), "t={t} ausserhalb [0,1]");
                        // Der Fusspunkt liegt auf dem Kreis.
                        let rq = ((q[0] * q[0] + q[1] * q[1]).sqrt() - r).abs();
                        assert!(rq < 1e-3 * r.max(1.0), "Fusspunkt nicht auf dem Kreis: {rq}");
                        cases += 1;
                    }
                }
            }
        }
    }
    assert!(cases > 30_000, "nur {cases} Proben");
    println!("§8y: max |arc_closest − dichte Referenz| = {worst:.3e}");
}

/// **§8y-Pin 3: die Randfälle — Strahlen, ρ-Schwelle, Tie, Massstab.**
#[test]
fn arc_closest_edge_families_384() {
    let eps = 1e-4f32;
    for alpha in [1e-4f64, 0.5, 1.5707963, 3.14159, 3.14159265358979] {
        let arc = synth_arc(2.0, alpha, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(arc_data_valid(&arc));
        // Auf dem Startstrahl und knapp beidseits.
        let on_start = [3.0f32, 0.0, 0.0];
        let (_, _, t0) = crate::offset_channel::arc_closest(&arc, on_start);
        assert_eq!(t0, 0.0, "α={alpha}: Startstrahl liefert t={t0}");
        let just_below = [3.0f32, -eps, 0.0];
        let (_, _, tb) = crate::offset_channel::arc_closest(&arc, just_below);
        assert_eq!(tb, 0.0, "knapp unter dem Startstrahl muss auf den Start klemmen");
        // Auf dem Endstrahl.
        let (ex, ey) = (arc.e[0], arc.e[1]);
        let on_end = [3.0 * ex, 3.0 * ey, 0.0];
        let (_, _, te) = crate::offset_channel::arc_closest(&arc, on_end);
        assert!((te - 1.0).abs() < 2e-3, "α={alpha}: Endstrahl liefert t={te}");
        // ρ unter und über der Schwelle.
        for rho in [0.0f32, 5e-10, 2e-9, 1e-6] {
            let p = [rho, 0.0, 1.0];
            let (de, q, t) = crate::offset_channel::arc_closest(&arc, p);
            assert!(de.is_finite() && q.iter().all(|v| v.is_finite()) && t.is_finite(),
                "ρ={rho}: nicht endlich");
        }
        // Bitgleiche Endpunkt-Distanzen (Tie): der Punkt auf der
        // Winkelhalbierenden JENSEITS des Bogens.
        let mid = [(1.0 + ex) * 0.5, (0.0 + ey) * 0.5, 0.0];
        let ml = (mid[0] * mid[0] + mid[1] * mid[1]).sqrt();
        if ml > 1e-6 && alpha > 0.1 {
            let far = [-mid[0] / ml * 50.0, -mid[1] / ml * 50.0, 0.0];
            let (_, _, tt) = crate::offset_channel::arc_closest(&arc, far);
            assert!(tt == 0.0 || tt == 1.0, "ausserhalb muss ein Endpunkt gewinnen, t={tt}");
        }
    }
    // Grosse Translation, kleiner Radius.
    let mut arc = synth_arc(0.01, 1.0, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
    arc.c = [1.0e6, -3.0e5, 7.0e5];
    let (de, q, t) = crate::offset_channel::arc_closest(&arc, [1.0e6 + 0.02, -3.0e5, 7.0e5]);
    assert!(de.is_finite() && q.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&t));
}

/// **§8y-Pin 4: Major- und Vollbögen werden fail-closed verweigert.**
///
/// Kein toter Beweis-Code. Die Invariante läuft über die DATEN (`e₁ ≥ 0`,
/// `0 < D_e ≤ 2`), nicht über das alte `sweep ≤ π`-Laufzeitkriterium — `sweep`
/// ist keine Autorität mehr.
#[test]
fn arc_major_and_full_are_refused_384() {
    // Major (α > π) ⇒ e₁ < 0 ⇒ Invariante verletzt.
    for alpha in [3.3f64, 4.0, 5.0, 6.0] {
        let arc = synth_arc(1.0, alpha, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(
            !arc_data_valid(&arc),
            "Major-Bogen α={alpha} (e₁={}) wird als gültig akzeptiert",
            arc.e[1]
        );
    }
    // Beschädigte Werte.
    let good = synth_arc(1.0, 1.0, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
    assert!(arc_data_valid(&good));
    for (tag, mut bad) in [
        ("r = 0", { let mut a = good.clone(); a.r = 0.0; a }),
        ("r NaN", { let mut a = good.clone(); a.r = f32::NAN; a }),
        ("e NaN", { let mut a = good.clone(); a.e[0] = f32::NAN; a }),
        ("e nicht normiert", { let mut a = good.clone(); a.e = [0.5, 0.5]; a }),
        ("D_e = 0", { let mut a = good.clone(); a.d_e = 0.0; a }),
        ("D_e > 2", { let mut a = good.clone(); a.d_e = 2.5; a }),
        ("D_e NaN", { let mut a = good.clone(); a.d_e = f32::NAN; a }),
        ("c inf", { let mut a = good.clone(); a.c[1] = f32::INFINITY; a }),
    ] {
        assert!(!arc_data_valid(&bad), "{tag}: wird als gültig akzeptiert");
        bad.sweep = 1.0;
    }
    // Und die ECHTE Synthese emittiert nie einen Major-/Vollbogen.
    let delta = 2.0f32;
    let mut checked = 0usize;
    for node in [stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta)] {
        let (field, _) = compile_for(node);
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        for st in stops {
            let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
            for ch in channels {
                for arc in &ch.arcs {
                    assert!(
                        arc_data_valid(arc),
                        "die Synthese hat einen ungültigen Bogen emittiert: {arc:?}"
                    );
                    if arc.sweep >= 0.0 {
                        assert!(arc.e[1] >= 0.0, "Major-Bogen aus der Synthese");
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked >= 4, "nur {checked} echte gekrümmte Bögen geprüft");
}

/// **§8y-Pin 5: die Mutationen müssen reissen.**
///
/// AND↔OR im Sektortest, fehlender Endpunktzweig, gedrehte Tie-Richtung —
/// jede Mutation muss an der Familie messbar falsch werden. Sonst prüft
/// `arc_closest_matches_a_dense_reference_384` die Struktur nicht.
#[test]
fn arc_mutations_break_the_reference_384() {
    let alpha = 1.0f64;
    let r = 2.0f32;
    let arc = synth_arc(r, alpha, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
    let reference = |p: [f32; 3]| -> f64 {
        let n = 20000;
        let mut best = f64::INFINITY;
        for s in 0..=n {
            let ph = alpha * s as f64 / n as f64;
            let q = [r as f64 * ph.cos(), r as f64 * ph.sin(), 0.0];
            best = best.min(
                ((p[0] as f64 - q[0]).powi(2)
                    + (p[1] as f64 - q[1]).powi(2)
                    + (p[2] as f64 - q[2]).powi(2))
                .sqrt(),
            );
        }
        best
    };
    // Die drei Mutationen, lokal nachgebaut.
    let mutant = |p: [f32; 3], mode: u8| -> f32 {
        let d = [p[0] - arc.c[0], p[1] - arc.c[1], p[2] - arc.c[2]];
        let x = d[0] * arc.u[0] + d[1] * arc.u[1] + d[2] * arc.u[2];
        let y = d[0] * arc.v[0] + d[1] * arc.v[1] + d[2] * arc.v[2];
        let rho = (x * x + y * y).sqrt().max(1e-9);
        let q0 = [arc.c[0] + arc.r * arc.u[0], arc.c[1] + arc.r * arc.u[1], arc.c[2]];
        let q1 = [
            arc.c[0] + arc.r * (arc.u[0] * arc.e[0] + arc.v[0] * arc.e[1]),
            arc.c[1] + arc.r * (arc.u[1] * arc.e[0] + arc.v[1] * arc.e[1]),
            arc.c[2],
        ];
        let nd = |q: [f32; 3]| {
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
        };
        let a = y;
        let b = x * arc.e[1] - y * arc.e[0];
        let inside = match mode {
            0 => a >= 0.0 || b >= 0.0, // Mutation: OR statt AND
            _ => a >= 0.0 && b >= 0.0,
        };
        if inside {
            let q = [
                arc.c[0] + arc.r * (arc.u[0] * (x / rho) + arc.v[0] * (y / rho)),
                arc.c[1] + arc.r * (arc.u[1] * (x / rho) + arc.v[1] * (y / rho)),
                arc.c[2],
            ];
            nd(q)
        } else if mode == 1 {
            nd(q0) // Mutation: Endpunktzweig fehlt, immer der Start
        } else {
            let (d0, d1) = (nd(q0), nd(q1));
            if mode == 2 {
                if d0 < d1 { d1 } else { d0 } // Mutation: falsche Tie-Richtung/Auswahl
            } else if d0 <= d1 {
                d0
            } else {
                d1
            }
        }
    };
    let mut broke = [0usize; 3];
    let mut ok_ref = 0usize;
    for i in 0..40 {
        for j in 0..40 {
            let p = [(i as f32 - 20.0) * 0.31, (j as f32 - 20.0) * 0.29, 0.0];
            let want = reference(p);
            let got = crate::offset_channel::arc_closest(&arc, p).0 as f64;
            assert!((got - want).abs() < 2e-3, "Produktion weicht ab: {got} vs {want}");
            ok_ref += 1;
            for m in 0..3u8 {
                if (mutant(p, m) as f64 - want).abs() > 2e-3 {
                    broke[m as usize] += 1;
                }
            }
        }
    }
    assert!(ok_ref > 1000, "nur {ok_ref} Referenzproben");
    for (m, tag) in ["OR statt AND", "kein Endpunktzweig", "falsche Endpunkt-Auswahl"]
        .iter()
        .enumerate()
        .map(|(i, t)| (i, *t))
    {
        assert!(
            broke[m] > 0,
            "die Mutation „{tag}“ blieb still — dann prüft die Familie die Struktur nicht"
        );
    }
}

/// **§8y-Pin 6: das Abweichungslemma — analytisch, nicht nur Fixture-Delta.**
///
/// Der Parameterwechsel `φ/α → D(φ)/D_e` verschiebt nur den nlerp-Parameter.
/// Die Kette:
///
/// 1. `ε_D(α) = sup_φ |D(φ)/D(α) − φ/α|` — mit der geschlossenen Form
///    `D(φ) = (1+tan(φ−π/4))/2` analytisch fassbar; das Maximum über ALLE
///    `α ∈ (0,π]` liegt bei `α ≈ 49°` und beträgt **0.0911**.
/// 2. `|N(a,b,t₁) − N(a,b,t₂)| ≤ 2·√((1−c)/(1+c))·|t₁−t₂|` mit `c = a·b`
///    (nlerp-Lipschitz; die Wurzel explodiert bei fast antipodalen `a`,`b`).
/// 3. `|Δs_i| ≤ K · 2√((1−c)/(1+c)) · ε_D(α)` im Trust-Radius. `min`/`max`
///    sind nicht-expansiv, also ist das zugleich die **Feldabweichungs-Schranke**
///    des reinen Parameterwechsels.
///
/// Der Pin misst `c` an den ECHTEN Bögen, rechnet die Schranke aus und hält
/// fest, dass sie das gemessene Golden-Delta (0.031 mm an der Steinmetz)
/// dominiert. Ausserdem: die Schranke explodiert nachweislich bei `c → −1`.
#[test]
fn arc_parameter_deviation_bound_384() {
    // (1) ε_D analytisch über α, geschlossene Form.
    let d_of = |phi: f64| -> f64 {
        let (x, y) = (phi.cos(), phi.sin());
        if x >= 0.0 { y / (x + y) } else { 1.0 + (-x) / (y - x) }
    };
    let mut eps_max = 0.0f64;
    let mut at = 0.0f64;
    for i in 1..=2000 {
        let a = std::f64::consts::PI * i as f64 / 2000.0;
        let da = d_of(a);
        let mut m = 0.0f64;
        for j in 0..=400 {
            let p = a * j as f64 / 400.0;
            m = m.max((d_of(p) / da - p / a).abs());
        }
        if m > eps_max {
            eps_max = m;
            at = a;
        }
    }
    assert!(
        (0.085..0.095).contains(&eps_max),
        "ε_D = {eps_max} — erwartet ≈ 0.0911 bei α ≈ 0.859 rad, gemessen bei {at}"
    );
    // (2)/(3) an den ECHTEN gekrümmten Bögen.
    let delta = 2.0f32;
    let (field, _) = compile_for(stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta));
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let (mut arcs, mut worst_bound, mut worst_c) = (0usize, 0.0f64, 1.0f64);
    for st in stops {
        let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
        for ch in channels {
            for a in &ch.arcs {
                if a.sweep < 0.0 {
                    continue;
                }
                arcs += 1;
                for (p, q) in [(a.m1s, a.m1e), (a.m2s, a.m2e)] {
                    let c = (p[0] * q[0] + p[1] * q[1] + p[2] * q[2]) as f64;
                    let c = c.clamp(-1.0, 1.0);
                    worst_c = worst_c.min(c);
                    let lip = 2.0 * ((1.0 - c) / (1.0 + c).max(1e-12)).sqrt();
                    worst_bound = worst_bound.max(ch.trust as f64 * lip * eps_max);
                }
            }
        }
    }
    assert!(arcs >= 4, "nur {arcs} gekrümmte Bögen");
    println!(
        "§8y-Abweichung: ε_D = {eps_max:.4} (α ≈ {at:.3} rad) | schlechtestes c = {worst_c:.4} \
         | Feldschranke = {worst_bound:.4} mm | gemessenes Golden-Delta = 0.0315 mm"
    );
    assert!(
        worst_bound >= 0.0315,
        "die analytische Schranke {worst_bound} unterschreitet das GEMESSENE Golden-Delta \
         0.0315 mm — dann ist die Herleitung falsch"
    );
    // Die Schranke explodiert bei fast antipodalen nlerp-Endvektoren — genau
    // deshalb ist ein Änderungsbudget nötig.
    let blow = |c: f64| 2.0 * ((1.0 - c) / (1.0 + c)).sqrt() * eps_max * 3.91;
    assert!(blow(-0.9) > 2.0 * blow(-0.5), "die Schranke wächst bei c → −1 nicht");
    assert!(blow(-0.999) > 20.0, "die Schranke explodiert bei c → −1 nicht: {}", blow(-0.999));
    println!(
        "§8y-Budget: c = −0.5 ⇒ {:.3} mm, c = −0.9 ⇒ {:.3} mm, c = −0.999 ⇒ {:.1} mm",
        blow(-0.5),
        blow(-0.9),
        blow(-0.999)
    );
    // Das A0-Änderungsbudget sitzt im EXPLOSIONS-Regime, nicht als scharfe
    // Schranke: die echten Bögen liegen bei c = −0.835 (Schranke 2.37 mm), das
    // Gate bei c = −0.95. Ein schärferes Budget würde gesunde Bögen verwerfen,
    // ein weicheres den antipodalen Fall durchlassen.
    assert!(
        worst_c > crate::offset_channel::ARC_NLERP_CONDITION_MIN as f64,
        "die echten Bögen ({worst_c}) unterschreiten das Explosions-Gate ({}) — dann ist \
         das Gate falsch gesetzt",
        crate::offset_channel::ARC_NLERP_CONDITION_MIN
    );
}

/// **§8y-Korrektur-Pin: der fidget-Spiegel folgt dem ρ-Zweig der CPU.**
///
/// Blocker 1 des zehnten Reviews. Die CPU liefert für `ρ < 1e-9` **zwingend**
/// `(q₀, t = 0)`; der fidget-Baum ersetzte nur `ρ` durch `max(ρ, 1e-9)` und
/// projizierte weiter. Auf der Bogenachse (`x = y = 0`) sind dann BEIDE
/// Diamant-Nenner null, `dm` wird `0/0 = NaN` — und **NaN wandert durch
/// `select` hindurch**, ein toter Ast vergiftet also den ganzen Baum. Auch
/// abseits der Achse, bei `0 < ρ < 1e-9`, projizierte fidget auf den Kreis,
/// während die CPU den Startpunkt nimmt.
///
/// `check_interp` deckt fidget nicht ab — dieser Pin ist der einzige Prüfpfad.
#[test]
fn arc_fidget_mirrors_the_rho_branch_384() {
    use fidget::context::Tree;
    let delta = 2.0f32;
    let (field, _) = compile_for(stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta));
    // Die Achsen-Punkte der gekrümmten Bögen einsammeln.
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let mut probes: Vec<[f32; 3]> = Vec::new();
    for st in &stops {
        let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
        for ch in channels {
            for a in &ch.arcs {
                if a.sweep < 0.0 {
                    continue;
                }
                // Exakt auf der Achse und knapp daneben, in mehreren Winkeln,
                // über die ρ-Schwelle hinweg.
                for rho in [
                    0.0f32,
                    0.5e-9,
                    f32::from_bits(1.0e-9f32.to_bits() - 1),
                    1.0e-9,
                    f32::from_bits(1.0e-9f32.to_bits() + 1),
                    1.0e-6,
                ] {
                    for k in 0..6 {
                        let ang = k as f32 * 1.0472;
                        let (sa, ca) = (ang.sin(), ang.cos());
                        for h in [-0.7f32, 0.0, 0.9] {
                            probes.push([
                                a.c[0] + (a.u[0] * ca + a.v[0] * sa) * rho + a.u[0] * 0.0 + h * 0.0,
                                a.c[1] + (a.u[1] * ca + a.v[1] * sa) * rho + h * 0.0,
                                a.c[2] + (a.u[2] * ca + a.v[2] * sa) * rho + h,
                            ]);
                        }
                    }
                }
            }
        }
    }
    assert!(probes.len() >= 100, "nur {} Achsen-Proben", probes.len());
    // fidget-Baum aufbauen und punktweise gegen die CPU prüfen.
    let px = Tree::x();
    let py = Tree::y();
    let pz = Tree::z();
    let tree = crate::fidget_bridge::node_to_tree(&field, &[px, py, pz])
        .expect("fidget-Baum lässt sich nicht bauen");
    let mut ctx = fidget::Context::new();
    let root = ctx.import(&tree);
    let (mut worst, mut checked) = (0.0f64, 0usize);
    for p in &probes {
        let cpu = field.sdf(*p, EvalMode::Visual) as f64;
        let fid = ctx
            .eval_xyz(root, p[0] as f64, p[1] as f64, p[2] as f64)
            .expect("fidget-Auswertung schlägt fehl");
        assert!(
            fid.is_finite(),
            "fidget liefert bei {p:?} {fid} — ein NaN-Ast im Baum (die Diamant-Nenner \
             sind auf der Achse null)"
        );
        assert!(cpu.is_finite(), "CPU liefert bei {p:?} {cpu}");
        worst = worst.max((cpu - fid).abs());
        checked += 1;
    }
    assert!(checked >= 100, "nur {checked} Proben");
    assert!(
        worst < 2.0e-4,
        "CPU und fidget weichen um {worst:.3e} ab — der ρ-Zweig ist nicht gespiegelt"
    );
    println!("§8y fidget-ρ-Parität: {checked} Proben, max |CPU − fidget| = {worst:.3e}");
}

/// **§8y-Korrektur-Pin: die Dateninvariante bindet die Autoritäten zusammen.**
///
/// Blocker 3. Vorher passierten: ein gültiges `e` mit beliebigem anderem `D_e`
/// (derselbe Bogen, anderes `t`, anderes Material-Gate); `sweep = NaN` (weil
/// `NaN < 0` falsch ist und der Bogen-Arm griff); `sweep = 3π` mit
/// `e = (−1,0)`; nicht endliche `m`-Vektoren; und der Segment-Zweig kehrte
/// zurück, bevor irgendetwas davon geprüft war.
#[test]
fn arc_data_invariant_binds_the_authorities_384() {
    let good = synth_arc(2.0, 1.0, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
    assert!(arc_data_valid(&good));
    let cases: Vec<(&str, ChannelArc)> = vec![
        ("D_e mutiert bei unverändertem e", {
            let mut a = good.clone();
            a.d_e = a.d_e * 1.0000001 + 1e-7;
            a
        }),
        ("sweep = NaN", { let mut a = good.clone(); a.sweep = f32::NAN; a }),
        ("sweep = 3π mit e = (−1,0)", {
            let mut a = good.clone();
            a.sweep = 3.0 * std::f32::consts::PI;
            a.e = [-1.0, 0.0];
            a.d_e = diamond(-1.0, 0.0);
            a
        }),
        ("sweep = 0", { let mut a = good.clone(); a.sweep = 0.0; a }),
        ("m1e nicht endlich", { let mut a = good.clone(); a.m1e[2] = f32::NAN; a }),
        ("m2s unendlich", { let mut a = good.clone(); a.m2s[0] = f32::INFINITY; a }),
        ("v nicht endlich", { let mut a = good.clone(); a.v[1] = f32::NAN; a }),
    ];
    for (tag, bad) in &cases {
        assert!(!arc_data_valid(bad), "{tag}: wird als gültig akzeptiert");
    }
    // Auch der SEGMENT-Zweig prüft jetzt die gemeinsamen Felder.
    let mut seg = seg_arc(
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.0],
    );
    assert!(arc_data_valid(&seg), "gültiges Segment wird abgelehnt");
    for (tag, mut bad) in [
        ("Segment mit NaN-m", { let mut a = seg.clone(); a.m1s[0] = f32::NAN; a }),
        ("Segment mit NaN-sweep", { let mut a = seg.clone(); a.sweep = f32::NAN; a }),
        ("Segment mit NaN-v", { let mut a = seg.clone(); a.v[0] = f32::NAN; a }),
    ]
    .into_iter()
    .map(|(t, a)| (t, a))
    {
        assert!(!arc_data_valid(&bad), "{tag}: wird als gültig akzeptiert");
        bad.r = 1.0;
    }
    seg.r = 1.0;
    // Und die ECHTE Synthese erfüllt die vollständige Invariante.
    let delta = 2.0f32;
    let mut n = 0usize;
    for node in [
        stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta),
        thin_corner_node(2.5 * delta, 40.0, delta),
        notch_union_node(90.0, delta as f64),
    ] {
        let (field, _) = compile_for(node);
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        for st in stops {
            let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
            for ch in channels {
                for a in &ch.arcs {
                    assert!(arc_data_valid(a), "Synthese emittiert ungültigen Bogen: {a:?}");
                    n += 1;
                }
            }
        }
    }
    assert!(n >= 20, "nur {n} echte Bögen geprüft");
}

/// **§8y-Korrektur-Pin: die Bogen-Kugel umfasst auch schiefe Rahmen.**
///
/// Blocker 2. Die Sehnen-Kugel brauchte einen orthonormalen Rahmen, den
/// niemand prüfte — mit `u = v` fielen beide Endpunkte zusammen und der Prune
/// durfte einen Bogenpunkt überspringen, der `√2 − 1` entfernt lag. Die
/// strukturelle Schranke `r·√(|u|²+|v|²)` kommt ohne Rahmen-Voraussetzung aus.
#[test]
fn arc_bounding_sphere_covers_skew_frames_384() {
    let mut worst = 0.0f64;
    let mut cases = 0usize;
    for (uu, vv) in [
        ([1.0f32, 0.0, 0.0], [0.0f32, 1.0, 0.0]), // orthonormal
        ([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]),       // entartet: u = v
        ([1.0, 0.0, 0.0], [0.7, 0.7, 0.0]),       // schief
        ([2.0, 0.0, 0.0], [0.0, 0.5, 0.0]),       // nicht normiert
        ([1.0, 0.0, 0.0], [0.0, 0.0, 3.0]),       // stark anisotrop
    ] {
        for (r, alpha) in [(1.0f32, 1.0f64), (40.0, 2.5), (0.01, 0.2)] {
            let mut arc = synth_arc(r, alpha, [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]);
            arc.u = uu;
            arc.v = vv;
            arc.c = [3.0, -2.0, 1.5];
            // Kugel wie die Produktion sie baut.
            let g = ((uu[0] * uu[0] + uu[1] * uu[1] + uu[2] * uu[2])
                + (vv[0] * vv[0] + vv[1] * vv[1] + vv[2] * vv[2]))
                .sqrt();
            arc.bc = arc.c;
            arc.br = arc.r * g * (1.0 + 1.0e-5) + 1.0e-4;
            // JEDER Bogenpunkt muss in der Kugel liegen.
            for i in 0..=400 {
                let phi = alpha * i as f64 / 400.0;
                let (cp, sp) = (phi.cos() as f32, phi.sin() as f32);
                let pt = [
                    arc.c[0] + r * (uu[0] * cp + vv[0] * sp),
                    arc.c[1] + r * (uu[1] * cp + vv[1] * sp),
                    arc.c[2] + r * (uu[2] * cp + vv[2] * sp),
                ];
                let d = (((pt[0] - arc.bc[0]) as f64).powi(2)
                    + ((pt[1] - arc.bc[1]) as f64).powi(2)
                    + ((pt[2] - arc.bc[2]) as f64).powi(2))
                .sqrt();
                assert!(
                    d <= arc.br as f64,
                    "u={uu:?} v={vv:?} r={r} α={alpha}: Bogenpunkt {d} > Kugelradius {}",
                    arc.br
                );
                worst = worst.max(d / arc.br as f64);
            }
            cases += 1;
        }
    }
    assert!(cases >= 15, "nur {cases} Fälle");
    println!("§8y Bogen-Kugel: schlechteste Ausnutzung {worst:.4} (≤ 1 = umfassend)");
}

/// **§8y-Korrektur-2-Pin: `t` und `de` direkt gegen fidget — inklusive der
/// Diagonalen.**
///
/// Zwei Defekte der ersten Korrektur, die der Feld-Pin NICHT gefangen hat:
///
/// * **Der äussere ρ-Select auf `t` war totes Recht** — eine zweite
///   `let t = …` überschattete ihn sofort (rustc meldete es als „unused
///   variable"). Für `0 < ρ < 1e-9` lieferte die CPU `t = 0`, fidget aber
///   `t_in` aus der Projektion. Dass der Feld-Pin nur `6.3·10⁻⁷` mass, heisst
///   bloss, dass die WIRKUNG an den gesampelten Punkten klein war — die
///   `t`-Parität war trotzdem gebrochen. Deshalb vergleicht dieser Pin `t`
///   **direkt**, gegen dieselbe Funktion, die auch die Produktion baut.
/// * **Die „sicheren Nenner" deckten nur die Achse.** `select_lt` ist ein
///   arithmetischer Blend, kein Sprung: ein `±inf` im VERWORFENEN Zweig wird
///   mit `0` multipliziert und ergibt `NaN`. Auf `x = y > 0` (φ = 45°, ein
///   gewöhnlicher In-Sektor-Punkt) ist der Q2-Nenner exakt `+0`, auf
///   `x = −y > 0` (φ = 135°) der Q1-Nenner. Exakte f32-Diagonalen sind auf
///   Gittern und bei symmetrischen Szenen real erreichbar.
#[test]
fn arc_fidget_t_and_diagonals_are_exact_384() {
    use fidget::context::Tree;
    let (mut checked, mut worst_t, mut worst_de) = (0usize, 0.0f64, 0.0f64);
    for alpha in [0.7854f64, 1.5708, 2.3562, 3.1415] {
        for r in [0.5f32, 2.0, 37.0] {
            for shift in [[0.0f64, 0.0, 0.0], [1.0e4, -2.5e3, 7.0e3]] {
                let mut arc = synth_arc(r, alpha, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
                arc.c = [shift[0] as f32, shift[1] as f32, shift[2] as f32];
                let mut probes: Vec<[f32; 3]> = Vec::new();
                // (a) EXAKTE Diagonalen: x = y und x = −y, in-Sektor und
                //     ausserhalb, in mehreren Abständen.
                for m in [0.3f32, 1.0, 2.5, 9.0] {
                    for (sx, sy) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                        probes.push([
                            arc.c[0] + sx * m * r,
                            arc.c[1] + sy * m * r,
                            arc.c[2],
                        ]);
                    }
                }
                // (b) Die ρ-Familie — hier fällt der `t`-Shadowing-Defekt auf.
                for rho in [
                    0.0f32,
                    0.5e-9,
                    f32::from_bits(1.0e-9f32.to_bits() - 1),
                    1.0e-9,
                    f32::from_bits(1.0e-9f32.to_bits() + 1),
                    1.0e-6,
                    1.0e-3,
                ] {
                    for k in 0..5 {
                        let ang = k as f32 * 0.9;
                        probes.push([
                            arc.c[0] + ang.cos() * rho,
                            arc.c[1] + ang.sin() * rho,
                            arc.c[2] + 0.4,
                        ]);
                    }
                }
                // Bäume für `t` und `de` — dieselbe Funktion, die auch
                // `channel_to_tree` benutzt.
                let pv = [Tree::x(), Tree::y(), Tree::z()];
                let tt = crate::fidget_bridge::arc_t_tree(&arc, &pv);
                let td = crate::fidget_bridge::arc_de_tree(&arc, &pv);
                let mut ctx = fidget::Context::new();
                let rt = ctx.import(&tt);
                let rd = ctx.import(&td);
                for p in &probes {
                    let (de_cpu, _, t_cpu) = crate::offset_channel::arc_closest(&arc, *p);
                    let t_fid = ctx
                        .eval_xyz(rt, p[0] as f64, p[1] as f64, p[2] as f64)
                        .expect("fidget-t schlägt fehl");
                    let de_fid = ctx
                        .eval_xyz(rd, p[0] as f64, p[1] as f64, p[2] as f64)
                        .expect("fidget-de schlägt fehl");
                    // **NaN-Freiheit gilt IMMER** — das ist die Invariante des
                    // Baums, ohne Ausnahme und ohne Toleranz.
                    assert!(
                        t_fid.is_finite() && de_fid.is_finite(),
                        "α={alpha} r={r} p={p:?}: fidget liefert t={t_fid}, de={de_fid} — \
                         ein NaN-/inf-Ast im Baum (Diagonal-Nenner!)"
                    );
                    assert!(t_cpu.is_finite() && de_cpu.is_finite());
                    let dt = (t_cpu as f64 - t_fid).abs();
                    let dd = (de_cpu as f64 - de_fid).abs();
                    // **Die Toleranz ist hergeleitet, nicht geraten.** Die CPU
                    // rechnet in f32, fidget in f64 — das ist Absicht (der
                    // Mesher-Arm ist f64). Der dominierende Term ist die
                    // f32-Darstellung von `p − c`: bei Beträgen `|c|` liegt der
                    // ulp bei `|c|·2⁻²³`, und die Kette aus Subtraktion,
                    // Projektion und Norm summiert einige davon. Angesetzt:
                    // `5·|c|·2⁻²³` plus ein radiusbezogener Sockel.
                    let cmag = (arc.c[0].abs()).max(arc.c[1].abs()).max(arc.c[2].abs()) as f64;
                    let tol_de = 5.0 * cmag * (2.0f64).powi(-23) + 1.0e-4 * (r as f64).max(1.0);
                    assert!(
                        dd < tol_de,
                        "α={alpha} r={r} p={p:?}: de CPU={de_cpu} fidget={de_fid} \
                         (Δ={dd:.3e}, Toleranz {tol_de:.3e})"
                    );
                    // Geometrie des Punktes, in f64 nachgerechnet.
                    let dx = (p[0] - arc.c[0]) as f64;
                    let dy = (p[1] - arc.c[1]) as f64;
                    let rho = (dx * dx + dy * dy).sqrt();
                    let d0 = ((p[0] - (arc.c[0] + arc.r * arc.u[0])) as f64).powi(2)
                        + ((p[1] - (arc.c[1] + arc.r * arc.u[1])) as f64).powi(2)
                        + ((p[2] - (arc.c[2] + arc.r * arc.u[2])) as f64).powi(2);
                    let qe = [
                        arc.c[0] + arc.r * (arc.u[0] * arc.e[0] + arc.v[0] * arc.e[1]),
                        arc.c[1] + arc.r * (arc.u[1] * arc.e[0] + arc.v[1] * arc.e[1]),
                        arc.c[2] + arc.r * (arc.u[2] * arc.e[0] + arc.v[2] * arc.e[1]),
                    ];
                    let d1 = ((p[0] - qe[0]) as f64).powi(2)
                        + ((p[1] - qe[1]) as f64).powi(2)
                        + ((p[2] - qe[2]) as f64).powi(2);
                    let tie = (d0.sqrt() - d1.sqrt()).abs() < 1.0e-6 * (r as f64).max(1.0);
                    if rho <= 5.0e-10 {
                        // **Der Achsen-Zweig — hier greift Defekt 1.** Die CPU
                        // erzwingt `t = 0`; fidget muss das EXAKT auch tun.
                        assert_eq!(
                            t_cpu, 0.0,
                            "CPU liefert auf der Achse t={t_cpu} statt 0"
                        );
                        assert!(
                            t_fid.abs() < 1.0e-12,
                            "α={alpha} r={r} p={p:?} (ρ={rho:.3e}): fidget liefert t={t_fid} \
                             statt 0 — der ρ-Select auf `t` greift nicht"
                        );
                        checked += 1;
                    } else if rho < 2.0e-9 || tie {
                        // Genau AN der ρ-Schwelle bzw. bei einem
                        // Endpunkt-Gleichstand ist `t` eine DISKRETE Wahl, und
                        // CPU (f32) und fidget (f64) dürfen dort verschieden
                        // runden. NaN-Freiheit und `de` sind oben trotzdem
                        // geprüft; die `t`-Zusage wäre hier eine Zusage über
                        // Rundung, nicht über Struktur.
                    } else {
                        assert!(
                            dt < 1.0e-6,
                            "α={alpha} r={r} p={p:?}: t CPU={t_cpu} fidget={t_fid} \
                             (Δ={dt:.3e})"
                        );
                        worst_t = worst_t.max(dt);
                        checked += 1;
                    }
                    worst_de = worst_de.max(dd);
                }
            }
        }
    }
    assert!(checked > 900, "nur {checked} strikt geprüfte Proben");
    println!(
        "§8y-K2 fidget-Parität: {checked} Proben, max |Δt| = {worst_t:.3e}, \
         max |Δde| = {worst_de:.3e}"
    );
}

/// **§8z-Sonde: wo trägt die Bogen-Einschliessung?**
#[test]
#[ignore]
fn probe_384_arc_interval_reach() {
    let delta = 2.0f32;
    let (t, r1, r2, zh1, e1, e2) = (3.0f32, 6.0f32, 4.0f32, 0.5f32, 1.2f32, 1.8f32);
    let (field, _) = compile_for(stepped_hub_node(t, r1, r2, zh1, e1, e2, delta));
    let none = |_c: [f64; 3]| None;
    println!("| Gebiet | n³ | Z2 | davon mit Bogen | Hauptgründe |");
    println!("|---|---|---|---|---|");
    for (tag, lo, hi, n) in [
        ("Doktrin-Gitter", [-9.0f64, -9.0, -4.5], [9.0f64, 9.0, 0.5], 12usize),
        ("Doktrin fein", [-9.0, -9.0, -4.5], [9.0, 9.0, 0.5], 24),
        ("Plattenkern", [-8.5, -8.5, -2.6], [-6.5, -6.5, -1.0], 10),
        ("Nabe nah", [-7.0, -7.0, -3.0], [-3.0, -3.0, -0.5], 14),
        // Tief IM oberen Zylinder (r2 = 4, z bis 6), nahe der Kreisnaht
        // r = 4 / z = 0.5 — dort gibt es echtes Material UND der Kanal ist
        // im Trust-Radius.
        ("Zylinder b, nahtnah", [-1.5, -1.5, 1.2], [1.5, 1.5, 3.2], 12),
        ("Zylinder b, tiefer", [-1.2, -1.2, 2.0], [1.2, 1.2, 4.0], 10),
        // Ring knapp innerhalb r = 6 über der Platte (Naht r = 6 / z = 0).
        ("Zylinder a, nahtnah", [2.0, 2.0, -0.9], [3.6, 3.6, 0.3], 10),
    ] {
        let (mut b2, mut b2_arc) = (0usize, 0usize);
        let mut unk = std::collections::BTreeMap::new();
        for cell in fixture_cells(lo, hi, n) {
            let mut c = iv::Ctx::new();
            match iv::certify_cell(&cell, &field, delta as f64, CERT_TOL, &none, &mut c) {
                iv::Verdict::Branch2 => {
                    b2 += 1;
                    if c.curved_arc_evals() > 0 {
                        b2_arc += 1;
                    }
                }
                iv::Verdict::Unknown(w) => *unk.entry(w).or_insert(0usize) += 1,
                _ => {}
            }
        }
        let mut v: Vec<_> = unk.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let top: String =
            v.iter().take(2).map(|(k, c)| format!("{k}·{c}")).collect::<Vec<_>>().join(", ");
        println!("| {tag} | {} | {b2} | {b2_arc} | {top} |", n * n * n);
    }
}

/// **§8z-Pin 1: die Bogen-Alternativen enthalten den Produktionswert —
/// GEMEINSAM.**
///
/// Der Falsifikator von A1 (Beweispflicht 7). Für dicht gesetzte Proben muss es
/// eine Alternative geben, deren `de`, `q` UND `t` den Produktionswert
/// **zusammen** enthalten. Getrennt zu prüfen wäre schwächer: es liesse die
/// Kombination „`q` vom Endpunkt-Ast, `t` vom Fusspunkt-Ast" durchgehen, ein
/// Paar, das `arc_closest` nie erzeugt.
#[test]
fn arc_interval_alternatives_contain_production_384() {
    use iv::CellDomain;
    let (mut cases, mut probes) = (0usize, 0usize);
    for alpha in [0.05f64, 0.6, 1.5707963, 2.4, 3.14159265] {
        for r in [0.3f32, 2.0, 25.0] {
            let arc = synth_arc(r, alpha, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
            // Zellen quer zu jeder Schaltfläche: Achse, ρ-Schwelle,
            // Sektorgrenzen, Diagonalen, Endpunkt-Voronoi.
            let cells: Vec<iv::Box3> = vec![
                iv::Box3::cube([0.0, 0.0, 0.0], (0.3 * r) as f64),
                iv::Box3::cube([0.0, 0.0, 0.0], 5.0e-10),
                iv::Box3::cube([0.0, 0.0, 0.0], 2.0e-9),
                iv::Box3::cube([(0.9 * r) as f64, 0.0, 0.0], (0.2 * r) as f64),
                iv::Box3::cube([(0.7 * r) as f64, (0.7 * r) as f64, 0.0], (0.3 * r) as f64),
                iv::Box3::cube([-(0.7 * r) as f64, (0.7 * r) as f64, 0.0], (0.3 * r) as f64),
                iv::Box3::cube([0.0, (1.4 * r) as f64, (0.5 * r) as f64], (0.4 * r) as f64),
                iv::Box3::new(
                    [-(2.0 * r) as f64, -(2.0 * r) as f64, -(0.1 * r) as f64],
                    [(2.0 * r) as f64, (2.0 * r) as f64, (0.1 * r) as f64],
                ),
            ];
            for cell in cells {
                let mut c = iv::Ctx::new();
                let Ok(alts) = iv::enc_arc_closest(&mut c, cell.world_f32(), &arc) else {
                    continue;
                };
                cases += 1;
                for p in cell_samples(&cell, 3) {
                    let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                    let (de, q, t) = crate::offset_channel::arc_closest(&arc, pf);
                    let hit = alts.iter().any(|a| {
                        a.de.contains(de as f64)
                            && a.t.contains(t as f64)
                            && (0..3).all(|k| a.q[k].contains(q[k] as f64))
                    });
                    assert!(
                        hit,
                        "α={alpha} r={r} p={pf:?}: (de={de}, q={q:?}, t={t}) liegt in KEINER \
                         gemeinsamen Alternative — {} Alternativen: {alts:?}",
                        alts.len()
                    );
                    probes += 1;
                }
            }
        }
    }
    assert!(cases >= 70 && probes > 4_000, "{cases} Zellen / {probes} Proben");
    println!("§8z: {cases} Zellen, {probes} Proben — jede in einer gemeinsamen Alternative");
}

/// **§8z-Pin 2: der A1-Erfolg hängt kausal an der Bogen-Einschliessung.**
///
/// Das Erfolgskriterium: mindestens eine Zelle der Stufen-Nabe wird durch
/// Zweig 2 zertifiziert, mit einem **tatsächlich beteiligten** gekrümmten
/// Kandidaten — und die Ablation (`arc_refuse_curved`) entfernt GENAU diesen
/// Erfolg wieder.
#[test]
fn arc_interval_unlocks_the_hub_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta));
    let none = |_c: [f64; 3]| None;
    // Tief im oberen Zylinder, im Trust-Radius der Kreisnaht r = 4 / z = 0.5.
    let cells = fixture_cells([-1.5, -1.5, 1.2], [1.5, 1.5, 3.2], 8);
    let run = |knobs: iv::Knobs, cells: &Vec<iv::Box3>| -> (usize, usize) {
        let (mut b2, mut with_arc) = (0usize, 0usize);
        for cell in cells {
            let mut c = iv::Ctx::with_knobs(knobs);
            if matches!(
                iv::certify_cell(cell, &field, delta as f64, CERT_TOL, &none, &mut c),
                iv::Verdict::Branch2
            ) {
                b2 += 1;
                // **Der starke Nachweis**: der Bogen hat die §8w-Auswahl
                // überlebt UND sein Downstream ist in die Hülle eingegangen.
                if c.curved_downstream() > 0 {
                    with_arc += 1;
                }
            }
        }
        (b2, with_arc)
    };
    let (b2, with_arc) = run(iv::Knobs::default(), &cells);
    assert!(
        b2 > 0 && with_arc == b2,
        "Zweig 2 zertifiziert {b2} Zellen der Nabe, davon {with_arc} mit gekrümmtem \
         Kandidaten — erwartet: mindestens eine, und ALLE mit Bogen"
    );
    // Und an jeder behauptet das Feld an dichten Proben tatsächlich Material.
    let mut probes = 0usize;
    for cell in &cells {
        let mut c = iv::Ctx::new();
        if !matches!(
            iv::certify_cell(cell, &field, delta as f64, CERT_TOL, &none, &mut c),
            iv::Verdict::Branch2
        ) {
            continue;
        }
        for p in cell_samples(cell, 2) {
            let f = field.sdf([p[0] as f32, p[1] as f32, p[2] as f32], EvalMode::Visual);
            probes += 1;
            assert!(f < 0.0, "zertifizierte Zelle {cell:?} liefert bei {p:?} {f} ≥ 0");
        }
    }
    assert!(probes > 500, "nur {probes} Proben");
    // **Die Ablation entfernt genau diesen Erfolg.**
    let (b2_abl, _) = run(
        iv::Knobs { arc_refuse_curved: true, ..Default::default() },
        &cells,
    );
    assert_eq!(
        b2_abl, 0,
        "mit abgeschalteter Bogen-Einschliessung bleiben {b2_abl} Zertifikate übrig — \
         dann hängt der Erfolg nicht an A1"
    );
    println!("§8z: Nabe {b2} Zellen über Zweig 2 (alle mit Bogen), Ablation ⇒ {b2_abl}");
}

/// **§8z-Pin 3: die Rückdreh-Gegenproben der Bogen-Kette.**
///
/// Fünf Sicherungen, einzeln gezogen. Jede MUSS an der adversarialen Familie
/// reissen (Produktionswert fällt aus allen Alternativen) — ausser der
/// `q`/`t`-Frühhüllung, die sound bleibt und deshalb über ihre BREITE
/// widerlegt wird.
#[test]
fn arc_interval_counter_probes_384() {
    use iv::CellDomain;
    let mut fams: Vec<(ChannelArc, iv::Box3)> = Vec::new();
    for alpha in [0.6f64, 1.5707963, 2.4] {
        for r in [1.0f32, 8.0] {
            let arc = synth_arc(r, alpha, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
            for ctr in [
                [0.0f64, 0.0, 0.0],
                [(0.8 * r) as f64, (0.4 * r) as f64, 0.0],
                [-(0.6 * r) as f64, (0.6 * r) as f64, (0.2 * r) as f64],
                [0.0, (1.5 * r) as f64, 0.0],
            ] {
                fams.push((arc.clone(), iv::Box3::cube(ctr, (0.35 * r) as f64)));
            }
        }
    }
    let escapes = |knobs: iv::Knobs| -> usize {
        let mut esc = 0usize;
        for (arc, cell) in &fams {
            let mut c = iv::Ctx::with_knobs(knobs);
            let Ok(alts) = iv::enc_arc_closest(&mut c, cell.world_f32(), arc) else {
                continue;
            };
            for p in cell_samples(cell, 3) {
                let pf = [p[0] as f32, p[1] as f32, p[2] as f32];
                let (de, q, t) = crate::offset_channel::arc_closest(arc, pf);
                let hit = alts.iter().any(|a| {
                    a.de.contains(de as f64)
                        && a.t.contains(t as f64)
                        && (0..3).all(|k| a.q[k].contains(q[k] as f64))
                });
                if !hit {
                    esc += 1;
                }
            }
        }
        esc
    };
    assert_eq!(escapes(iv::Knobs::default()), 0, "die Produktion entkommt schon ohne Mutation");
    for (tag, knobs) in [
        ("nur erste Alternative", iv::Knobs { arc_first_alt_only: true, ..Default::default() }),
        ("letzte Alternative gestrichen", iv::Knobs { arc_drop_last_alt: true, ..Default::default() }),
        ("Fusspunkt-Ast fehlt", iv::Knobs { arc_drop_foot_alt: true, ..Default::default() }),
        ("Endpunkt-Ast fehlt", iv::Knobs { arc_drop_end_alt: true, ..Default::default() }),
    ] {
        let e = escapes(knobs);
        assert!(e > 0, "die Gegenprobe {tag} blieb still — dann prüft die Familie sie nicht");
    }
    // Die ρ-Intersektion: ohne sie verliert die Kette Zellen an die Division
    // (fail-closed), statt sie auszuwerten.
    // Der ρ-Schnitt hat einen eigenen Pin
    // (`arc_interval_rho_cut_is_sharpness_not_soundness_384`).
}



/// **§8z-Korrektur-Pin: der ρ-Schnitt ist SCHÄRFE, keine Soundness-Pflicht —
/// und die behauptete Dichotomie ist falsch.**
///
/// Die erste A1-Fassung behauptete zweierlei zu stark, beides wird hier
/// richtiggestellt:
///
/// * **„Nullfreie Diamant-Nenner und ein gemischter ρ-Schalter schliessen
///   einander aus."** Das ist im f32-Intervallmodell **kein Satz**. Bei sehr
///   engem `x` um null und `y` wenige ulp oberhalb der Schwelle sind `x + y`
///   und `y − x` klar positiv, während die auswärts versiegelte Kette
///   `sqrt(x² + y²)` mit ihrer UNTEREN Schranke unter die Schwelle rutscht.
///   Der Pin führt genau diese Konfiguration vor.
/// * **„Der Schnitt ist eine Soundness-Pflicht."** Auch falsch: ohne ihn
///   verweigert `fdiv` ein nullhaltiges Roh-Intervall ohnehin fail-closed, und
///   die Division durch ein nullfreies Roh-Intervall ist sound, nur breiter.
///
/// **Was statt der Dichotomie gilt** — und warum die ursprünglich geforderte
/// Familie („ohne Schnitt verloren, mit Schnitt ausgewertet") trotz ehrlichen
/// Versuchs nicht baubar ist:
///
/// > Die Roh-Division verweigert genau dann, wenn `0 ∈ ρ`, also `ρ.lo = 0`.
/// > Mit `fsquare ≥ 0` gilt `ρ.lo = 0` genau dann, wenn `0 ∈ x` UND `0 ∈ y`.
/// > Dann ist aber auch `0 ∈ x + y` (untere Schranken addieren sich, obere
/// > ebenso), und der Q1-Diamant-Zweig wird wegen `0 ∈ x` immer mit
/// > ausgewertet (`decide_ge(x,0) ≠ False`). Sein `fdiv` verweigert also
/// > ebenfalls — mit oder ohne ρ-Schnitt. ∎
///
/// Der Schnitt bleibt richtig und gewollt: er hebt `1/ρ` aus der
/// Schwellen-Nähe und verengt das Ergebnis.
#[test]
fn arc_interval_rho_cut_is_sharpness_not_soundness_384() {
    use iv::CellDomain;
    let arc = synth_arc(2.0, 1.5707963, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let ulp = |v: f32, n: i64| f32::from_bits((v.to_bits() as i64 + n) as u32);
    let thr = 1.0e-9f32 as f64;
    // (1) Ko-Existenz: gemischter ρ-Schalter UND nullfreie Diamant-Nenner.
    let mut co_existence = 0usize;
    for (xlo, xhi, ylo, yhi) in [
        (-1.0e-15f32, 1.0e-15f32, 1.0e-9f32, ulp(1.0e-9, 1)),
        (-1.0e-12, 1.0e-12, 1.0e-9, ulp(1.0e-9, 4)),
    ] {
        let cell = iv::Box3::new([xlo as f64, ylo as f64, 0.0], [xhi as f64, yhi as f64, 0.0]);
        let w = cell.world_f32();
        let mut c = iv::Ctx::new();
        let (x, y) = (w[0], w[1]);
        let xx = c.fsquare(x).unwrap();
        let yy = c.fsquare(y).unwrap();
        let r2 = c.fadd(xx, yy).unwrap();
        let rho = c.fsqrt(r2).unwrap();
        let d1 = c.fadd(x, y).unwrap();
        let d2 = c.fsub(y, x).unwrap();
        let mixed = rho.lo < thr && rho.hi >= thr;
        let nz = !(d1.lo <= 0.0 && d1.hi >= 0.0) && !(d2.lo <= 0.0 && d2.hi >= 0.0);
        if mixed && nz {
            co_existence += 1;
            // Und GENAU hier gibt es keinen Fail-closed-Unterschied: `ρ.lo > 0`.
            assert!(rho.lo > 0.0, "ρ.lo = 0 trotz nullfreier Nenner");
            let ok_cut = iv::enc_arc_closest(&mut iv::Ctx::new(), w, &arc).is_ok();
            let ok_raw = iv::enc_arc_closest(
                &mut iv::Ctx::with_knobs(iv::Knobs {
                    arc_skip_rho_intersect: true,
                    ..Default::default()
                }),
                w,
                &arc,
            )
            .is_ok();
            assert!(
                ok_cut && ok_raw,
                "hier darf der Schnitt keinen Fail-closed-Unterschied machen"
            );
        }
    }
    assert!(
        co_existence > 0,
        "die behauptete Dichotomie liesse sich nicht widerlegen — dann wäre die \
         Richtigstellung unbegründet"
    );
    // (2) Wo die Roh-Division WIRKLICH verweigert, verweigert der Diamant auch.
    let both_zero = iv::Box3::new([-1.0e-9, -1.0e-9, 0.0], [1.0e-9, 1.0e-9, 0.0]);
    let w = both_zero.world_f32();
    let mut c = iv::Ctx::new();
    let sx = c.fsquare(w[0]).unwrap();
    let sy = c.fsquare(w[1]).unwrap();
    let r2 = c.fadd(sx, sy).unwrap();
    let rho = c.fsqrt(r2).unwrap();
    assert_eq!(rho.lo, 0.0, "die Konfiguration trifft den ρ.lo = 0-Fall nicht");
    let den = c.fadd(w[0], w[1]).unwrap();
    assert!(
        den.lo <= 0.0 && den.hi >= 0.0,
        "ρ.lo = 0, aber der Q1-Nenner ist nullfrei — dann bricht das Argument"
    );
    assert!(iv::enc_arc_closest(&mut iv::Ctx::new(), w, &arc).is_err());
    // (3) Der Schnitt VERENGT: das Ergebnis wird schmaler.
    let cell = iv::Box3::new([-1.0e-15, 1.0e-9, 0.0], [1.0e-15, ulp(1.0e-9, 1) as f64, 0.0]);
    let w = cell.world_f32();
    let width = |knobs: iv::Knobs| -> f64 {
        let mut c = iv::Ctx::with_knobs(knobs);
        let alts = iv::enc_arc_closest(&mut c, w, &arc).expect("nicht auswertbar");
        alts.iter().map(|a| a.t.width()).fold(0.0f64, f64::max)
    };
    let with_cut = width(iv::Knobs::default());
    let without = width(iv::Knobs { arc_skip_rho_intersect: true, ..Default::default() });
    // **Ehrlich gemessen:** der Schnitt verschlechtert nichts, aber sein
    // Gewinn liegt hier UNTER der f32-Auflösung — er hebt `ρ.lo` nur um rund
    // `10⁻⁷` relativ, und das Ergebnis rastet auf dasselbe f32-Gitter. Der
    // Schnitt wird deshalb NICHT mit gemessener Schärfe begründet, sondern mit
    // **Modell-Treue**: die Produktion erreicht diesen Ast nur mit `ρ ≥ 1e-9`,
    // und genau das soll im Modell stehen — statt dass die Verweigerung an der
    // Koinzidenz hängt, wo `fdiv` zuerst zuschlägt.
    assert!(
        with_cut <= without,
        "der Schnitt VERBREITERT: {with_cut:.3e} gegen {without:.3e}"
    );
    println!(
        "§8z-Korrektur: Ko-Existenz an {co_existence} Konfigurationen belegt (Dichotomie \
         widerlegt); t-Breite mit Schnitt {with_cut:.3e}, ohne {without:.3e} — die \
         Verengung liegt unter der f32-Auflösung, der Schnitt steht auf Modell-Treue"
    );
}

/// **§8z-Korrektur-Pin: die `L → Entire`-Brücke trägt 480 Zellen.**
///
/// Die Brücke ist sound (beide `lbv`-Konsumenten führen nur in `min`-Ketten,
/// und im konkaven Zweig ist jeder Ausgang `≤ d`), aber ihre Wirkung war nicht
/// separat gepinnt. Mit der Ablation (`lb_unknown_rejects`) fallen die
/// Nabe-Zellen zurück.
#[test]
fn arc_interval_lb_entire_bridge_is_load_bearing_384() {
    let delta = 2.0f32;
    let (field, _) = compile_for(stepped_hub_node(3.0, 6.0, 4.0, 0.5, 1.2, 1.8, delta));
    let none = |_c: [f64; 3]| None;
    let cells = fixture_cells([-1.5, -1.5, 1.2], [1.5, 1.5, 3.2], 8);
    let run = |knobs: iv::Knobs| -> usize {
        cells
            .iter()
            .filter(|cell| {
                let mut c = iv::Ctx::with_knobs(knobs);
                matches!(
                    iv::certify_cell(cell, &field, delta as f64, CERT_TOL, &none, &mut c),
                    iv::Verdict::Branch2
                )
            })
            .count()
    };
    let with_bridge = run(iv::Knobs::default());
    let without = run(iv::Knobs { lb_unknown_rejects: true, ..Default::default() });
    assert!(with_bridge > 0, "die Brücke trägt hier nichts — der Pin misst nichts");
    assert!(
        without < with_bridge,
        "ohne die Brücke bleiben genauso viele Zertifikate ({without} von {with_bridge}) — \
         dann ist sie nicht lasttragend"
    );
    println!("§8z: L→Entire trägt {with_bridge} Zellen, Ablation ⇒ {without}");
}

/// **§8z-Korrektur-Pin: die frühe `q`/`t`-Hüllung kostet Breite.**
///
/// Sie wäre sound — die Gegenprobe ist deshalb keine Soundness-, sondern eine
/// SCHÄRFE-Probe: sie kombiniert Paare, die `arc_closest` nie erzeugt (`q` vom
/// Endpunkt-Ast, `t` vom Fusspunkt-Ast). Messbar wird das nur, wo eine Zelle
/// wirklich MEHRERE Alternativen hat — an den Nabe-Zellen hat jeder Bogen nur
/// eine, dort ist die frühe Hüllung folgerichtig ein No-op.
#[test]
fn arc_interval_early_qt_hull_is_wider_384() {
    use iv::CellDomain;
    let e = |v: [f32; 3]| {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    // Ein gekrümmter Bogen mit STARK verschiedenen Dualvektoren — sonst wirkt
    // ein anderes `t` sich gar nicht aus.
    let mut arc = synth_arc(2.0, 1.5707963, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    arc.m1s = e([0.0, 1.0, 1.0]);
    arc.m1e = e([0.0, -1.0, 1.0]);
    arc.m2s = e([1.0, 0.0, 1.0]);
    arc.m2e = e([-1.0, 0.0, 1.0]);
    let ch = synth_channel(vec![arc], true);
    let dfull = ivf(-6.0, 3.0);
    let ex = ivf(-1.0e30, 1.0);
    // Zellen, die die Sektor- bzw. Endpunkt-Schaltfläche überspannen — dort
    // koexistieren Fusspunkt- und Endpunkt-Alternative.
    let mut cells: Vec<iv::Box3> = Vec::new();
    for i in -6i32..=6 {
        for j in -6i32..=6 {
            for h in [0.35f64, 0.9, 1.8] {
                cells.push(iv::Box3::cube(
                    [i as f64 * 0.8, j as f64 * 0.8, 0.3],
                    h,
                ));
            }
        }
    }
    let (mut multi, mut wider) = (0usize, 0usize);
    let (mut w_t, mut w_e) = (0.0f64, 0.0f64);
    for cell in &cells {
        let mut cc = iv::Ctx::new();
        let alts = iv::enc_arc_closest(&mut cc, cell.world_f32(), &ch.arcs[0]);
        if !matches!(&alts, Ok(a) if a.len() > 1) {
            continue;
        }
        multi += 1;
        let width = |knobs: iv::Knobs| -> Option<f64> {
            let mut c = iv::Ctx::with_knobs(knobs);
            iv::enclose_channel_apply(&mut c, cell.world_f32(), dfull, dfull, &ch, ex)
                .ok()
                .map(|v| v.width())
        };
        let (Some(t), Some(ee)) = (
            width(iv::Knobs::default()),
            width(iv::Knobs { arc_hull_qt_early: true, ..Default::default() }),
        ) else {
            continue;
        };
        assert!(
            ee >= t - 1e-12,
            "die frühe Hüllung ist ENGER ({ee:.6e} < {t:.6e}) — das wäre unmöglich"
        );
        w_t = w_t.max(t);
        w_e = w_e.max(ee);
        if ee > t {
            wider += 1;
        }
    }
    assert!(multi >= 2, "nur {multi} Zellen mit mehreren Alternativen — der Pin misst nichts");
    assert!(
        wider > 0,
        "die frühe q/t-Hüllung kostet an KEINER der {multi} mehrdeutigen Zellen Breite \
         (max {w_e:.6e} gegen {w_t:.6e}) — dann ist die Behauptung im Doc unbegründet \
         und gehört gestrichen"
    );
    println!(
        "§8z: frühe q/t-Hüllung an {wider} von {multi} mehrdeutigen Zellen breiter \
         (max {w_e:.6e} gegen {w_t:.6e})"
    );
}

// ══════════════ #384 §8aa / P5b-0 — die η-Klemme, noch ohne Konsument ══════════════

/// Zertifiziert jeden Kanal eines Fixture-Feldes (auf einer KOPIE — das Feld
/// selbst bleibt unangetastet) und liefert den Shadow-Bericht.
fn clamp_scan(
    field: &BoundedCsg,
    cfg: wit::WitnessCfg,
) -> (wit::SeamProofTally, Vec<(OffsetChannel, wit::SeamProofStatus, Option<f32>)>) {
    let mut tally = wit::SeamProofTally::default();
    let mut out = Vec::new();
    for (om, chans) in omega_and_channels(field) {
        for ch in chans {
            let mut c = ch.clone();
            let ((st, _), hst) = wit::certify_channel_reported(om, &mut c, cfg);
            tally.add(st);
            if let Some(h) = hst {
                tally.add_h(h);
            }
            // Der Addend ist nur GEGEN sein Ω lesbar — hier ist es zur Hand.
            let a = c.seam_addend(om);
            out.push((c, st, a));
        }
    }
    (tally, out)
}

/// **§8aa-Sonde: der Shadow-Bericht der η-Klemme.**
#[test]
#[ignore]
fn probe_384_clamp_shadow() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    println!(
        "| Fixture | Kanäle | proved | curved | no_cover | no_bridge | η_geo mm | ε_q mm | ε_d mm | field_round mm | A mm |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (name, field, _e, _lo, _hi, _n) in cert_matrix(delta) {
        let (t, chans) = clamp_scan(&field, cfg);
        let mut mx = [0.0f64; 4];
        let mut a_max = 0.0f32;
        for (c, st, a) in &chans {
            if *st != wit::SeamProofStatus::Proved {
                continue;
            }
            let k = c.seam_proof().unwrap();
            mx[0] = mx[0].max(k.geometric_mm());
            mx[1] = mx[1].max(k.q_bridge_mm());
            mx[2] = mx[2].max(k.norm_bridge_mm());
            mx[3] = mx[3].max(k.field_round_mm());
            a_max = a_max.max(a.unwrap());
        }
        let cell = |v: f64| if v > 0.0 { format!("{v:.3e}") } else { "—".into() };
        println!(
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            t.total(),
            t.proved_seam,
            t.curved,
            t.no_cover,
            t.no_bridge,
            cell(mx[0]),
            cell(mx[1]),
            cell(mx[2]),
            cell(mx[3]),
            if a_max > 0.0 { format!("{a_max:.3e}") } else { "—".into() },
        );
    }
}

use crate::offset_channel as oc;

/// Ein deterministischer Strom (LCG) — Proben ohne `rand`-Abhängigkeit.
struct Lcg(u64);
impl Lcg {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn sym(&mut self) -> f64 {
        2.0 * self.next_f64() - 1.0
    }
}

/// `n` ulp neben `v` (Vorzeichen-korrekt für positive Werte).
fn ulp_off(v: f32, n: i64) -> f32 {
    f32::from_bits((v.to_bits() as i64 + n).max(0) as u32)
}

/// Adversariale gerade Bögen: Clamp-Ränder, kurze Segmente unter grosser
/// Translation, sehr kleine und sehr grosse `r`, schräge Richtungen.
fn clamp_adversarial_arcs() -> Vec<(&'static str, ChannelArc)> {
    let m = [0.0f32, 0.0, 1.0];
    let mk = |a: [f32; 3], b: [f32; 3]| seg_arc(a, b, m, m, m, m);
    vec![
        ("Einheit-x", mk([0.0, 0.0, 0.0], [1.0, 0.0, 0.0])),
        ("r = 2e-9", mk([0.0, 0.0, 0.0], [2.0e-9, 0.0, 0.0])),
        ("r = 1e-6", mk([0.0, 0.0, 0.0], [1.0e-6, 0.0, 0.0])),
        ("r = 1e+6", mk([0.0, 0.0, 0.0], [1.0e6, 0.0, 0.0])),
        ("Translation 1e5, r = 1/32", mk([1.0e5, -3.0e4, 7.0e4], [1.0e5 + 0.031_25, -3.0e4, 7.0e4])),
        ("Translation 1e5, r = 43", mk([1.0e5, -3.0e4, 7.0e4], [1.0e5 + 40.0, -3.0e4 + 10.0, 7.0e4 - 5.0])),
        ("schräg", mk([-1.5, 2.25, -0.125], [3.75, -4.5, 6.125])),
        ("krumme Koeffizienten", mk([0.137_913_3, -2.918_44, 11.000_003], [-4.318_77, 0.019_2, -3.777_1])),
    ]
}

/// Der reelle Naht-Punkt `c + u·(s·r)` in f64 — die modellierte Menge `Q`,
/// über die [`wit::cover_channel`] den Zeugen führt.
fn seam_point_f64(arc: &ChannelArc, s: f64) -> [f64; 3] {
    let r = arc.r as f64;
    [
        arc.c[0] as f64 + arc.u[0] as f64 * (s * r),
        arc.c[1] as f64 + arc.u[1] as f64 * (s * r),
        arc.c[2] as f64 + arc.u[2] as f64 * (s * r),
    ]
}

/// Der EXAKTE Fussparameter `s ∈ [0,1]` von `p` auf der reellen Segmentmenge
/// `{c + u·(s·r)}` — geschlossene Form in f64. `u` ist NICHT als Einheitsvektor
/// vorausgesetzt (die f32-Normierung liegt relativ `≈ 10⁻⁷` daneben), deshalb
/// steht `|u|²` im Nenner.
fn seam_foot_f64(arc: &ChannelArc, p: [f64; 3]) -> f64 {
    let r = arc.r as f64;
    let u = [arc.u[0] as f64, arc.u[1] as f64, arc.u[2] as f64];
    let uu = u[0] * u[0] + u[1] * u[1] + u[2] * u[2];
    if !(uu > 0.0) || !(r > 0.0) {
        return 0.0;
    }
    let d = [p[0] - arc.c[0] as f64, p[1] - arc.c[1] as f64, p[2] - arc.c[2] as f64];
    let w = (d[0] * u[0] + d[1] * u[1] + d[2] * u[2]) / uu;
    (w / r).clamp(0.0, 1.0)
}

fn dist_f64_up(a: [f64; 3], b: [f64; 3]) -> f64 {
    let mut s = 0.0f64;
    for k in 0..3 {
        let d = crate::offset_interval::next_up((a[k] - b[k]).abs());
        s = crate::offset_interval::next_up(s + crate::offset_interval::next_up(d * d));
    }
    crate::offset_interval::next_up(s.sqrt())
}

fn dist_f64_down(a: [f64; 3], b: [f64; 3]) -> f64 {
    let mut s = 0.0f64;
    for k in 0..3 {
        let d = crate::offset_interval::next_down((a[k] - b[k]).abs());
        s = crate::offset_interval::next_down(s + crate::offset_interval::next_down(d * d));
    }
    crate::offset_interval::next_down(s.max(0.0).sqrt())
}

/// Probe-Punkte um EINEN geraden Bogen: die Clamp-Ränder `t = 0` und `t = 1`
/// exakt, knapp davor/dahinter, sowie Punkte direkt AUF und wenige ulp neben
/// der Segment-Voronoi-Grenze (der Ebene durch den Endpunkt senkrecht zu `u`).
fn clamp_arc_probes(arc: &ChannelArc) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let u = arc.u;
    // Zwei zu `u` senkrechte Richtungen.
    let a = if u[0].abs() < 0.9 { [1.0f32, 0.0, 0.0] } else { [0.0f32, 1.0, 0.0] };
    let n1 = {
        let d = u[0] * a[0] + u[1] * a[1] + u[2] * a[2];
        let v = [a[0] - d * u[0], a[1] - d * u[1], a[2] - d * u[2]];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let n2 = [
        u[1] * n1[2] - u[2] * n1[1],
        u[2] * n1[0] - u[0] * n1[2],
        u[0] * n1[1] - u[1] * n1[0],
    ];
    let r = arc.r;
    let mut g = Lcg(0x5eed_1234_9876_abcd);
    for ts in [-0.75f32, -1.0e-6, 0.0, 1.0e-7, 0.125, 0.5, 0.87, 1.0 - 1.0e-7, 1.0, 1.0 + 1.0e-6, 1.9] {
        for off in [0.0f32, 1.0e-6, 1.0e-3, 0.25, 1.0, 7.5] {
            for sc in [1.0f32, -1.0] {
                let base = [
                    arc.c[0] + u[0] * (ts * r),
                    arc.c[1] + u[1] * (ts * r),
                    arc.c[2] + u[2] * (ts * r),
                ];
                out.push([
                    base[0] + sc * off * n1[0] + 0.5 * sc * off * n2[0],
                    base[1] + sc * off * n1[1] + 0.5 * sc * off * n2[1],
                    base[2] + sc * off * n1[2] + 0.5 * sc * off * n2[2],
                ]);
            }
        }
    }
    // Direkt auf der Voronoi-Grenze (Endpunkt-Ebene) und wenige ulp daneben.
    for e in [0.0f32, 1.0] {
        let base = [arc.c[0] + u[0] * (e * r), arc.c[1] + u[1] * (e * r), arc.c[2] + u[2] * (e * r)];
        for d in [-3i64, -1, 0, 1, 3] {
            for off in [1.0e-4f32, 1.0, 3.0] {
                let sh = ulp_off(off, d);
                out.push([
                    base[0] + sh * n1[0],
                    base[1] + sh * n1[1],
                    base[2] + sh * n1[2],
                ]);
            }
        }
    }
    for _ in 0..260 {
        let t = g.sym() as f32 * 1.5;
        let o = (g.sym() * 4.0) as f32;
        let o2 = (g.sym() * 4.0) as f32;
        out.push([
            arc.c[0] + u[0] * (t * r) + o * n1[0] + o2 * n2[0],
            arc.c[1] + u[1] * (t * r) + o * n1[1] + o2 * n2[1],
            arc.c[2] + u[2] * (t * r) + o * n1[2] + o2 * n2[2],
        ]);
    }
    out.retain(|p| p.iter().all(|v| v.is_finite()));
    out
}

/// **§8aa-Pin 0: die f32-Konstanten sind exakt, und das Rundungsmodell stimmt.**
///
/// `u = 2⁻²⁴` und `2⁻¹⁵⁰` sind die Angelpunkte JEDER Zeile des Budgets. Stünde
/// eine der beiden auch nur ein ulp daneben, wären alle vier Brücken-Posten
/// stillschweigend falsch — und zwar in der Richtung, in der es niemand merkt.
/// Dazu die Modell-Aussage selbst, an dichten Proben: `|fl32(z) − z| ≤ u|z| +
/// 2⁻¹⁵⁰`, und — der Posten, der in Lemma 3 WEGFÄLLT — die Subtraktion zweier
/// f32-Zahlen hat KEINEN Unterlauf-Anteil (Hauser).
#[test]
fn clamp_f32_constants_are_exact_384() {
    assert_eq!(wit::F32_U, 2.0f64.powi(-24), "u ist nicht 2⁻²⁴");
    assert_eq!(wit::F32_SUB, 2.0f64.powi(-150), "der Unterlauf-Posten ist nicht 2⁻¹⁵⁰");
    assert_eq!(wit::F32_SUB, (f32::from_bits(1) as f64) * 0.5, "2⁻¹⁵⁰ ≠ halbe kleinste Subnormale");

    let mut g = Lcg(0xa11ce);
    let (mut checked, mut tight) = (0usize, 0usize);
    for _ in 0..40_000 {
        let e = (g.sym() * 40.0) as i32;
        let z = g.sym() * 2.0f64.powi(e);
        let f = z as f32;
        if !f.is_finite() {
            continue;
        }
        let err = (f as f64 - z).abs();
        let bound = wit::F32_U * z.abs() + wit::F32_SUB;
        assert!(err <= bound, "fl32-Modell verletzt: z={z:e}, Fehler {err:e} > {bound:e}");
        checked += 1;
        if err > 0.4 * bound {
            tight += 1;
        }
    }
    assert!(checked > 30_000 && tight > 1_000, "{checked} Proben, davon {tight} nahe an der Schranke");

    // Hauser: `fl32(a − b)` für f32-Operanden ist im Subnormalbereich EXAKT.
    let mut exact_sub = 0usize;
    for i in 0..2000u32 {
        let a = f32::from_bits(i + 1);
        let b = f32::from_bits(i / 2 + 1);
        let d = a - b;
        assert_eq!(d as f64, a as f64 - b as f64, "subnormale f32-Subtraktion war nicht exakt");
        exact_sub += 1;
    }
    assert!(exact_sub >= 2000);
}

/// **§8aa-Pin 1 (Lemma 2): der Produktions-Fusspunkt liegt höchstens `ε_q`
/// neben einem WIRKLICHEN Naht-Punkt.**
///
/// Geprüft wird nicht, dass `arc_closest` den exakten Nearest-Punkt findet —
/// das behauptet der Beweis nirgends. Geprüft wird die Aussage, die er
/// braucht: der geklemmte Parameter liegt in `[0,1]`, der zugehörige REELLE
/// Punkt `c + u·(t·r)` liegt in `Q`, und `q_f32` liegt höchstens `ε_q` daneben.
/// Zusätzlich gegen eine dichte reelle Referenz der ganzen Segmentmenge.
///
/// **Gegenprobe**: `ε_q = 0` muss reissen — der Pin zählt deshalb die Fälle mit
/// echt positivem Abstand und verlangt eine Mindestzahl.
#[test]
fn clamp_point_bridge_is_a_real_seam_point_384() {
    let (mut probes, mut positive) = (0usize, 0usize);
    let mut worst_ratio = 0.0f64;
    for (name, arc) in clamp_adversarial_arcs() {
        // Der `len == r`-Satz: `arc_closest` rechnet mit `r.max(1e-12)`.
        assert_eq!(
            arc.r.max(1.0e-12).to_bits(),
            arc.r.to_bits(),
            "{name}: `len` ist nicht bitweise `r` — Produktion und Beweis reden über \
             verschiedene Segmente"
        );
        let eps_q = wit::point_bridge_mm(&arc)
            .unwrap_or_else(|| panic!("{name}: keine Punkt-Brücke"));
        assert!(eps_q > 0.0 && eps_q.is_finite(), "{name}: ε_q = {eps_q:e}");
        for p in clamp_arc_probes(&arc) {
            let (de, q, t) = crate::offset_channel::arc_closest(&arc, p);
            if !de.is_finite() || !q.iter().all(|v| v.is_finite()) {
                continue;
            }
            assert!((0.0..=1.0).contains(&t), "{name}: t = {t} ausserhalb [0,1]");
            let qf = [q[0] as f64, q[1] as f64, q[2] as f64];
            // (a) gegen den REELLEN Punkt zu DEMSELBEN t.
            let d_same = dist_f64_up(qf, seam_point_f64(&arc, t as f64));
            assert!(
                d_same <= eps_q,
                "{name}: |q_f32 − q(t)| = {d_same:e} > ε_q = {eps_q:e} bei t = {t}"
            );
            // (b) gegen die EXAKTE reelle Segmentmenge (geschlossene Form in
            // f64, nicht ein Raster — ein Raster misst seine eigene Maschung).
            let best = dist_f64_up(qf, seam_point_f64(&arc, seam_foot_f64(&arc, qf)));
            assert!(best <= eps_q, "{name}: Abstand zur Segmentmenge {best:e} > ε_q = {eps_q:e}");
            if d_same > 0.0 {
                positive += 1;
                worst_ratio = worst_ratio.max(d_same / eps_q);
            }
            probes += 1;
        }
    }
    assert!(probes > 2_000, "nur {probes} Proben — der Pin misst zu wenig");
    assert!(
        positive > 200,
        "nur {positive} Proben mit echt positivem |q_f32 − q(t)| — mit ε_q = 0 würde der Pin \
         nicht reissen, also trägt er nichts"
    );
    println!(
        "§8aa Lemma 2: {probes} Proben, {positive} mit positivem Abstand, \
         schärfste Ausnutzung {:.1} % von ε_q",
        100.0 * worst_ratio
    );
}

/// **§8aa-Pin 2 (Lemma 3): die f32-Norm unterschätzt `|x − q_f32|` höchstens
/// um `ε_d`.**
///
/// Die Proben sitzen bewusst AUF der Segment-Voronoi-Grenze und wenige ulp
/// daneben: dort wechselt der Produktionszweig, und ein Beweis, der den
/// Wechsel nicht aushält, ist keiner.
///
/// **Gegenprobe**: `ε_d = 0` muss reissen — gezählt werden die Fälle, in denen
/// die f32-Norm den reellen Abstand ECHT unterschätzt.
#[test]
fn clamp_norm_bridge_never_underestimates_384() {
    let (mut probes, mut under) = (0usize, 0usize);
    let mut worst_ratio = 0.0f64;
    for (name, arc) in clamp_adversarial_arcs() {
        for p in clamp_arc_probes(&arc) {
            let (de, q, _t) = crate::offset_channel::arc_closest(&arc, p);
            if !de.is_finite() || !q.iter().all(|v| v.is_finite()) || de <= 0.0 {
                continue;
            }
            // `channel_limit` prüft `de < trust`; die Brücke darf also `trust`
            // als obere Schranke von `de` benutzen. Hier: das kleinste f32
            // oberhalb von `de`.
            let trust = ulp_off(de, 1);
            let eps_d = wit::norm_bridge_mm(trust).unwrap();
            let pf = [p[0] as f64, p[1] as f64, p[2] as f64];
            let qf = [q[0] as f64, q[1] as f64, q[2] as f64];
            let hi = dist_f64_up(pf, qf);
            let lo = dist_f64_down(pf, qf);
            assert!(
                hi <= de as f64 + eps_d,
                "{name}: |x − q_f32| = {hi:e} > de + ε_d = {:e}",
                de as f64 + eps_d
            );
            if lo > de as f64 {
                under += 1;
                worst_ratio = worst_ratio.max((lo - de as f64) / eps_d);
            }
            probes += 1;
        }
    }
    assert!(probes > 2_500, "nur {probes} Proben");
    assert!(
        under > 50,
        "nur {under} Proben, in denen die f32-Norm ECHT unterschätzt — mit ε_d = 0 würde der \
         Pin nicht reissen"
    );
    println!(
        "§8aa Lemma 3: {probes} Proben, {under} echte Unterschätzungen, \
         schärfste Ausnutzung {:.1} % von ε_d",
        100.0 * worst_ratio
    );
}

/// **§8aa-Pin 3 (Lemma 4): die Dreiecksungleichung, gegen das exakte Orakel.**
///
/// `dist(x,∁Ω) ≤ |x−q| + dist(q,∁Ω) ≤ (de + ε_d + ε_q) + η_geo`. Geprüft an
/// den echten Fixturen mit der ANALYTISCHEN Distanz zum Komplement — nicht mit
/// einem Feldwert, denn ein Feldwert ist genau das, was hier bewiesen werden
/// soll.
///
/// Der Pin misst zusätzlich, wie oft die NAIVE §8j-Annahme
/// (`dist(x,∁Ω) ≤ de`) an den Proben verletzt ist. Das ist die einzige
/// ehrliche Antwort auf „wozu überhaupt η": entweder es gibt solche Punkte —
/// dann ist die Klemme nachweisbar nötig —, oder es gibt sie an DIESEN Proben
/// nicht, und dann steht das genauso da.
#[test]
fn clamp_total_bridge_is_the_triangle_inequality_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let mut g = Lcg(0x384_5b00_0004);
    let (mut probes, mut inside) = (0usize, 0usize);
    let (mut naive_viol, mut worst_naive) = (0usize, f64::NEG_INFINITY);
    let mut worst_where = String::new();
    let mut tightest = f64::INFINITY;
    let mut fixtures = 0usize;
    for (name, field, exact, _lo, _hi, _n) in cert_matrix(delta) {
        let mut here = 0usize;
        for (om, chans) in omega_and_channels(&field) {
            for ch in chans {
                let mut c = ch.clone();
                if wit::certify_channel(om, &mut c, cfg).0 != wit::SeamProofStatus::Proved {
                    continue;
                }
                let bridge = c.seam_proof().unwrap().bridge_total_mm();
                for ai in 0..c.arcs.len() {
                    for _ in 0..40 {
                        let arc = &c.arcs[ai];
                        let s = g.next_f64() as f32;
                        let rad = (g.next_f64() * c.trust as f64 * 0.95) as f32;
                        let d = [g.sym(), g.sym(), g.sym()];
                        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                        if !(n > 1.0e-9) {
                            continue;
                        }
                        let p = [
                            arc.c[0] + arc.u[0] * (s * arc.r) + (d[0] / n) as f32 * rad,
                            arc.c[1] + arc.u[1] * (s * arc.r) + (d[1] / n) as f32 * rad,
                            arc.c[2] + arc.u[2] * (s * arc.r) + (d[2] / n) as f32 * rad,
                        ];
                        let Some((de, _, _, _)) = oc::channel_nearest(p, &c) else { continue };
                        if !(de.is_finite() && de < c.trust) {
                            continue;
                        }
                        probes += 1;
                        let dist = exact([p[0] as f64, p[1] as f64, p[2] as f64]);
                        if !(dist > 0.0) {
                            continue; // ausserhalb Ω — dort ist nichts zu zeigen
                        }
                        inside += 1;
                        here += 1;
                        assert!(
                            dist <= de as f64 + bridge,
                            "{name}: dist(x,∁Ω) = {dist:e} > de + Δ = {:e} bei x = {p:?}",
                            de as f64 + bridge
                        );
                        tightest = tightest.min(de as f64 + bridge - dist);
                        if dist > de as f64 {
                            naive_viol += 1;
                            if dist - de as f64 > worst_naive {
                                worst_naive = dist - de as f64;
                                worst_where = name.clone();
                            }
                        }
                    }
                }
            }
        }
        if here > 0 {
            fixtures += 1;
        }
    }
    assert!(fixtures >= 8, "nur {fixtures} Fixturen mit Klemme geprüft");
    assert!(inside > 8_000, "nur {inside} Proben INNERHALB Ω ({probes} gesamt)");
    println!(
        "§8aa Lemma 4: {inside}/{probes} Proben in Ω über {fixtures} Fixturen; \
         engste Reserve {tightest:.3e} mm; naive §8j-Annahme (dist ≤ de) an {naive_viol} \
         Proben verletzt, schlimmstenfalls um {:.3e} mm ({worst_where})",
        if naive_viol > 0 { worst_naive } else { 0.0 }
    );
    // Der Befund, der P5b-1 überhaupt begründet: die naive §8j-Annahme ist an
    // diesen Fixturen NICHT bloss unbewiesen, sie ist FALSCH. Bricht dieser
    // Zähler auf null ein, ist entweder die Geometrie eine andere geworden
    // oder die Probe misst die Naht nicht mehr — beides gehört gesehen.
    assert!(
        naive_viol > 500,
        "nur {naive_viol} Proben mit dist > de — dann ist die Notwendigkeit der η-Klemme an \
         dieser Matrix nicht mehr belegt und §8aa muss das sagen"
    );
    assert!(
        worst_naive < 2.3e-3,
        "die naive Annahme ist um {worst_naive:e} mm verletzt — mehr als das grösste η_geo \
         der Matrix; dann deckt die Klemme den Befund nicht mehr ab"
    );
}

/// Die adversariale `de`-Familie für Lemma 5: Bit-Fächer um jede interessante
/// Grössenordnung plus ein deterministischer Streu-Anteil.
fn clamp_de_family(trust: f32) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::with_capacity(140_000);
    let mut push = |v: f32| {
        if v.is_finite() && v >= 0.0 && v < trust {
            out.push(v);
        }
    };
    for base in [
        1.0e-6f32,
        1.0e-4,
        1.0e-2,
        0.125,
        0.5,
        0.999,
        1.0,
        1.5,
        2.0,
        2.5,
        3.0,
        trust * 0.5,
        trust * 0.9,
        trust * 0.999,
    ] {
        for n in -2500i64..=2500 {
            push(ulp_off(base, n));
        }
    }
    let mut g = Lcg(0x0f32_0f32_0f32);
    for _ in 0..30_000 {
        push((g.next_f64() * trust as f64) as f32);
    }
    out
}

/// Die Reserve EINES f32-Falls: wie weit die eingefrorene Kette unter der
/// bewiesenen Untergrenze `δ − de − Δ` bleibt. `< 0` heisst: die Klemme läge
/// ÜBER der gültigen Feld-Untergrenze und könnte Material entfernen.
fn clamp_round_reserve(delta: f32, de: f32, h: f32, a: f32, bridge: f64) -> f64 {
    let nd = crate::offset_interval::next_down;
    let lim = wit::frozen_limit_tail(delta, de, h, a, 0.0) as f64;
    let lb = nd(nd(delta as f64 - de as f64) - bridge);
    lb - lim
}

/// Das Minimum der Reserve über die ganze Familie, für einen gegebenen
/// Addenden.
fn clamp_family_min_reserve(a: f32, cases: &[(f32, f32, f32, f64)]) -> f64 {
    let mut m = f64::INFINITY;
    for &(delta, de, h, bridge) in cases {
        m = m.min(clamp_round_reserve(delta, de, h, a, bridge));
    }
    m
}

/// **§8aa-Pin 4 (Lemma 5): der kodierte Addend absorbiert BEIDE Rundungen der
/// eingefrorenen Feldkette.**
///
/// Ein Fixture-Pin genügt hier nicht (§8t). Der Pin baut eine gezielte
/// f32-Familie — Bit-Fächer von ±2500 ulp um jede relevante Grössenordnung
/// von `de`, dazu 30.000 Streuwerte, über drei `δ` — und fährt sie durch
/// [`wit::frozen_limit_tail`], also durch **genau die drei f32-Ops**, die
/// P5b-1 ins Feld setzt.
///
/// **Zwei Gegenproben, und die zweite ist die eigentliche:**
///
/// 1. `field_round := 0` (Addend nur aus `η_geo + ε_q + ε_d`) MUSS reissen.
/// 2. Der über der Familie **minimale** Addend `A*` wird per Binärsuche über
///    dem f32-Gitter bestimmt (die Kette ist monoton in `A`, s. Pin 6) —
///    und `A* − 1 ulp` MUSS reissen.
///
/// **Warum nicht `A − 1 ulp`** (die wörtliche Forderung des Reviews): weil
/// das nachweislich NICHT reisst, und der Grund ist kein Versehen. `A` kommt
/// aus einer Worst-Case-Komposition (jede Einzelop mit vollem `u·|z|`), die
/// **kein einzelner** f32-Fall gleichzeitig ausreizen kann — die Addition
/// müsste ihren halben ulp am unteren Binadenrand verlieren und die
/// Subtraktion ihren am oberen gewinnen. Der gemessene Abstand `A − A*` ist
/// deshalb rund **vier Zehnerpotenzen** grösser als ein ulp von `A`; ein
/// einzelnes ulp kann diese Lücke nicht schliessen, und ein Pin, der es
/// behauptete, wäre grün ohne Aussage. Gepinnt wird stattdessen der
/// MECHANISMUS: an `A*` ist die Familie ulp-scharf, und das Budget wird als
/// Faktor über `A*` beziffert statt behauptet.
#[test]
fn clamp_rounding_absorbs_both_f32_ops_384() {
    let trust = 3.91f32;
    // Ein realistisches Δ aus der Matrix (Ecken-Schale) plus zwei Extreme.
    for &bridge in &[1.0e-4f64, 2.257e-3, 1.0e-6] {
        let mut cases: Vec<(f32, f32, f32, f64)> = Vec::new();
        for &delta in &[2.0f32, 0.25, 12.5] {
            for de in clamp_de_family(trust) {
                // η-Zweig: `h` neutral (der Sentinel-Wert der Produktion) und
                // ein grosser endlicher Wert.
                for &h in &[1.0e30f32, delta + trust * 4.0] {
                    if wit::frozen_limit_tail(delta, de, h, 0.0, 0.0) != delta - de {
                        continue; // h-Zweig — nicht Gegenstand von Lemma 5
                    }
                    cases.push((delta, de, h, bridge));
                }
            }
        }
        assert!(cases.len() > 200_000, "nur {} Fälle in der Familie", cases.len());

        let (_fr, a) = wit::round_budget(2.0, trust, bridge).unwrap();
        // Der Addend hängt an δ; für die Familie über drei δ nehmen wir das
        // Maximum — die Produktion hat je Kanal genau EIN δ.
        let a = [2.0f32, 0.25, 12.5]
            .iter()
            .map(|&d| wit::round_budget(d, trust, bridge).unwrap().1)
            .fold(a, f32::max);

        let m = clamp_family_min_reserve(a, &cases);
        assert!(m >= 0.0, "Δ = {bridge:e}: die Familie reisst schon mit dem BEWIESENEN Addenden (Reserve {m:e})");

        // Gegenprobe 1: ohne Rundungsbudget.
        let a0 = wit::f32_ceil(bridge).unwrap();
        let m0 = clamp_family_min_reserve(a0, &cases);
        assert!(
            m0 < 0.0,
            "Δ = {bridge:e}: ohne Rundungsbudget (A = {a0:e}) bleibt die Familie still \
             (Reserve {m0:e}) — dann ist das Budget unbegründet"
        );

        // Gegenprobe 2: der über der Familie minimale Addend, ulp-scharf.
        let (mut lo, mut hi) = (0u32, a.to_bits());
        assert!(clamp_family_min_reserve(f32::from_bits(lo), &cases) < 0.0);
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if clamp_family_min_reserve(f32::from_bits(mid), &cases) >= 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let a_star = f32::from_bits(hi);
        assert!(clamp_family_min_reserve(a_star, &cases) >= 0.0);
        let shaved = f32::from_bits(hi - 1);
        let ms = clamp_family_min_reserve(shaved, &cases);
        assert!(
            ms < 0.0,
            "Δ = {bridge:e}: A* − 1 ulp ({shaved:e}) reisst NICHT (Reserve {ms:e}) — dann ist \
             die Familie nicht ulp-scharf und die Gegenprobe wertlos"
        );
        let ulp_a = (ulp_off(a, 1) - a) as f64;
        println!(
            "§8aa Lemma 5: Δ = {bridge:.3e} | A = {a:.6e} | A* = {a_star:.6e} | \
             A/A* = {:.6} | A − A* = {:.3e} = {:.0} ulp(A) | Reserve(A) = {m:.3e}",
            a as f64 / a_star as f64,
            (a - a_star) as f64,
            (a - a_star) as f64 / ulp_a
        );
    }
}

/// **§8aa-Pin 5 (Lemma 6): der Beweis lässt sich nicht verpflanzen — und das
/// ist jetzt eine Eigenschaft des Typsystems, nicht eines Hashes.**
///
/// §8aa-Korrektur (dreizehntes Review, Blocker 2 + 3). Die erste Fassung
/// verglich einen 64-Bit-FNV-Wert und nannte das „Identität"; sie prüfte
/// sieben ausgewählte Ein-ulp-Mutationen und schloss daraus auf Kollisionsfreiheit
/// über alle Kanäle. Das ist keine Beweisführung. Zusätzlich akzeptierte sie
/// einen bit-identischen Kanal unter einem ANDEREN `Ω` — die konkrete
/// Verpflanzungsfamilie des Reviews.
///
/// Jetzt geprüft:
///
/// 1. **Der Schreibweg ist versiegelt.** `OffsetChannel::seam` ist privat, und
///    [`OffsetChannel::seal_seam_proof`] verlangt ein `SeamProofToken`, das nur
///    `offset_witness` prägen kann. Dieser Test kann eine Verpflanzung deshalb
///    nur über [`wit::transplant_proof`] überhaupt VERSUCHEN — den einzigen
///    `pub(crate)`-Weg, und den gibt es ausschliesslich, damit die Ablehnung
///    beobachtbar ist.
/// 2. **Der Vergleich ist bitweise, nicht gehasht.** Über den kanonischen
///    Kanal-Schnappschuss wird JEDE einzelne Byte-Position einzeln mutiert
///    (nicht sieben ausgewählte Felder) und muss bemerkt werden.
/// 3. **`Ω` gehört dazu.** Die Familie „bit-identischer Kanal, anderes `Ω`"
///    wird gebaut und muss abgelehnt werden.
#[test]
fn clamp_proof_cannot_be_transplanted_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    // Ein zertifizierter Kanal je Fixture, mitsamt seinem Ω.
    let fields: Vec<BoundedCsg> =
        cert_matrix(delta).into_iter().map(|(_n, f, _e, _l, _h, _c)| f).collect();
    let mut proved: Vec<(OffsetChannel, &BoundedCsg)> = Vec::new();
    for field in &fields {
        for (om, chans) in omega_and_channels(field) {
            for ch in chans.iter().take(4) {
                let mut c = ch.clone();
                if wit::certify_channel(om, &mut c, cfg).0 == wit::SeamProofStatus::Proved {
                    proved.push((c, om));
                }
            }
        }
    }
    assert!(proved.len() >= 20, "nur {} zertifizierte Kanäle", proved.len());
    let (a, om_a) = (proved[0].0.clone(), proved[0].1);

    // (0) Unveränderte Kopie mit demselben Ω: derselbe Wert.
    let copy = a.clone();
    assert_eq!(
        copy.seam_addend(om_a).map(f32::to_bits),
        a.seam_addend(om_a).map(f32::to_bits),
        "eine unveränderte Kopie hat ihren Beweis verloren"
    );
    assert!(a.seam_addend(om_a).is_some());

    // (1) Verpflanzung auf einen ANDEREN Kanal — über den versiegelten Weg.
    let snap_a = wit::channel_snapshot(&a);
    let mut transplanted = 0usize;
    for i in 1..proved.len() {
        if wit::channel_snapshot(&proved[i].0) == snap_a {
            continue; // bit-identische Kanäle sind derselbe Kanal
        }
        let mut b = proved[i].0.clone();
        wit::transplant_proof(&a, &mut b);
        assert!(
            b.seam_addend(proved[i].1).is_none(),
            "ein fremder Beweis wurde akzeptiert — die Bindung trägt nicht"
        );
        assert!(b.seam_addend(om_a).is_none(), "…auch nicht mit dem fremden Ω");
        transplanted += 1;
    }
    assert!(transplanted >= 10, "nur {transplanted} echte Verpflanzungen geprüft");

    // (2) JEDE Byte-Position des Kanal-Schnappschusses muss zählen: alle
    //     Bytes, jeweils XOR 0x01 und XOR 0x80, durch DIE Prüfung geschickt,
    //     die auch `addend_for` nimmt (Review 14, Sollte-11 — vorher stand
    //     hier nur eine `Vec`-Ungleichheit, die aus der Sprachsemantik folgt
    //     und den Abrufpfad nie berührte).
    let proof_a = a.seam_proof().expect("Kanal a trägt einen Beweis");
    assert!(
        proof_a.channel_bytes_accepted_for_test(&snap_a),
        "die UNMUTIERTE Folge wird abgelehnt — der Untertest misst eine kaputte Prüfung"
    );
    let mut probed_bytes = 0usize;
    for pos in 0..snap_a.len() {
        for xor in [1u8, 0x80] {
            let mut m = snap_a.clone();
            m[pos] ^= xor;
            assert!(
                !proof_a.channel_bytes_accepted_for_test(&m),
                "Byte {pos} (XOR {xor:#04x}): der Abrufpfad akzeptierte die Mutation"
            );
            probed_bytes += 1;
        }
    }
    assert!(probed_bytes > 400, "nur {probed_bytes} Byte-Mutationen");

    // (2b) …und dieselbe Aussage am echten Kanal: alle Felder, die im
    //      Schnappschuss stehen, einzeln um ein ulp verdreht.
    let mut caught = 0usize;
    for mutate in 0..11usize {
        let mut m = a.clone();
        match mutate {
            0 => m.arcs[0].c[1] = ulp_off(m.arcs[0].c[1].abs().max(1.0), 1),
            1 => m.arcs[0].r = ulp_off(m.arcs[0].r, 1),
            2 => m.arcs[0].u[0] = ulp_off(m.arcs[0].u[0].abs().max(1.0), 3),
            3 => m.delta = ulp_off(m.delta, 1),
            4 => m.trust = ulp_off(m.trust, 1),
            5 => m.face_planes[0][3] = 1.25,
            6 => m.arcs[0].m1s[2] = 0.5,
            7 => m.gate = ulp_off(m.gate, 1),
            8 => m.bound_r = ulp_off(m.bound_r, 1),
            9 => m.branch ^= 1,
            _ => m.parents[1] ^= 1,
        }
        assert_ne!(
            wit::channel_snapshot(&m),
            snap_a,
            "Mutation {mutate} bewegt den Schnappschuss nicht — sie steht nicht darin"
        );
        assert!(
            m.seam_addend(om_a).is_none(),
            "Mutation {mutate} blieb unbemerkt"
        );
        caught += 1;
    }
    assert_eq!(caught, 11);

    // (3) **Die Familie des Reviews**: bit-identischer Kanal, anderes Ω.
    //     ch1 wird gegen ein leicht deckbares Ω (Empty) zertifiziert, ch2 ist
    //     bit-identisch, lebt aber unter dem echten Ω. Ohne Ω-Bindung wäre der
    //     Beweis übertragbar — genau der gemeldete Angriff.
    let mut ch1 = a.clone();
    assert_eq!(
        wit::certify_channel(&BoundedCsg::Empty, &mut ch1, cfg).0,
        wit::SeamProofStatus::Proved
    );
    let mut ch2 = a.clone();
    wit::transplant_proof(&ch1, &mut ch2);
    assert_eq!(
        wit::channel_snapshot(&ch1),
        wit::channel_snapshot(&ch2),
        "die Familie ist falsch gebaut — die Kanäle sind nicht bit-identisch"
    );
    assert!(
        ch2.seam_addend(om_a).is_none(),
        "der über Ω = Empty geführte Beweis wurde unter dem ECHTEN Ω akzeptiert — \
         genau die Verpflanzungsfamilie des dreizehnten Reviews"
    );
    // …und unter SEINEM Ω ist er selbstverständlich gültig.
    assert!(ch2.seam_addend(&BoundedCsg::Empty).is_some());

    // (3b) Jede Ω-Mutation, die der Aussen-Beweis liest, ändert den Ω-Schnappschuss.
    let mut om_snaps: Vec<Vec<u8>> = Vec::new();
    for field in fields.iter().take(5) {
        for (om, _c) in omega_and_channels(field) {
            om_snaps.push(wit::omega_snapshot(om));
        }
    }
    let n = om_snaps.len();
    om_snaps.sort();
    om_snaps.dedup();
    assert!(
        n > 1 && om_snaps.len() == n,
        "die Ω-Schnappschüsse sind nicht unterscheidbar ({} von {n})",
        om_snaps.len()
    );
    println!(
        "§8aa Pin 5: {transplanted} Verpflanzungen abgelehnt, {probed_bytes} Byte-Positionen \
         des Kanal-Schnappschusses ({} Bytes) geprüft, Ω-Schnappschuss {} Bytes",
        snap_a.len(),
        wit::omega_snapshot(om_a).len()
    );
}

/// **§8aa-Pin 6 (Lemma 7): kein Cover, kein Brückenbeweis, kein Addend.**
///
/// Alle fünf Ausgänge einzeln, jeder mit `eta_addend() == None`. Der wichtige
/// Punkt ist die TRENNUNG: ein gekrümmter Bogen ist ein struktureller
/// Ausschluss (P5c), kein gescheiterter Zeuge — würden beide als `no_cover`
/// gezählt, verschwände die Stufen-Nabe aus dem Bericht.
#[test]
fn clamp_is_fail_closed_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let (_n, field, _e, _l, _h, _c) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Ecken-Schale t/δ=2.5"))
        .unwrap();
    let (om, chans) = {
        let v = omega_and_channels(&field);
        let (om, chans) = v.into_iter().next().unwrap();
        (om as *const BoundedCsg, chans.clone())
    };
    let om: &BoundedCsg = unsafe { &*om };
    let base = chans[0].clone();
    let mut ok = base.clone();
    assert_eq!(wit::certify_channel(om, &mut ok, cfg).0, wit::SeamProofStatus::Proved);
    assert!(ok.seam_addend(om).is_some());

    // (a) gekrümmter Bogen ⇒ Curved, nicht NoCover.
    let mut curved = base.clone();
    curved.arcs[0].sweep = 1.0;
    assert_eq!(wit::certify_channel(om, &mut curved, cfg).0, wit::SeamProofStatus::Curved);
    assert!(curved.seam_addend(om).is_none());

    // (b) leerer Kanal.
    let mut empty = base.clone();
    empty.arcs.clear();
    assert_eq!(wit::certify_channel(om, &mut empty, cfg).0, wit::SeamProofStatus::NoCover);
    assert!(empty.seam_addend(om).is_none());

    // (c) ungültige Konfiguration.
    for bad in [
        wit::WitnessCfg { min_offset: 0.0, ..cfg },
        wit::WitnessCfg { min_offset: -1.0e-4, ..cfg },
        wit::WitnessCfg { steps: 0, ..cfg },
        wit::WitnessCfg { pieces_per_arc: 0, ..cfg },
        wit::WitnessCfg { margin: -1.0, ..cfg },
        wit::WitnessCfg { max_offset: f64::NAN, ..cfg },
    ] {
        let mut c = base.clone();
        assert_eq!(wit::certify_channel(om, &mut c, bad).0, wit::SeamProofStatus::BadConfig);
        assert!(c.seam_addend(om).is_none());
    }

    // (d) entarteter Bogen (`r` unter der `arc_usable`-Schwelle).
    let mut tiny = base.clone();
    tiny.arcs[0].r = 5.0e-10;
    assert_eq!(wit::certify_channel(om, &mut tiny, cfg).0, wit::SeamProofStatus::NoCover);
    assert!(tiny.seam_addend(om).is_none());

    // (e) `Ω` mit einem nicht unterstützten Blatt: der Rippen-Kamm 4×1.5/0.8.
    let (_n2, field2, _e2, _l2, _h2, _c2) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Rippen-Kamm 4×1.5"))
        .unwrap();
    let mut unsupported = 0usize;
    for (om2, chans2) in omega_and_channels(&field2) {
        for ch in chans2 {
            let mut c = ch.clone();
            assert_eq!(wit::certify_channel(om2, &mut c, cfg).0, wit::SeamProofStatus::NoCover);
            assert!(c.seam_addend(om).is_none());
            unsupported += 1;
        }
    }
    assert!(unsupported >= 10, "nur {unsupported} Kanäle über nicht unterstütztem Ω");

    // (f) Ω = Empty ist der Gegen-Extremfall: ALLES ist aussen, der Zeuge
    //     findet sofort einen — der Addend entsteht, und er ist korrekt.
    let mut over = base.clone();
    assert_eq!(
        wit::certify_channel(&BoundedCsg::Empty, &mut over, cfg).0,
        wit::SeamProofStatus::Proved
    );

    // (g) Ablationen: eine geschwächte Brücke ändert den Addenden — sonst
    //     wäre der jeweilige Posten wirkungslos.
    let full = ok.seam_addend(om).unwrap();
    for knobs in [
        wit::SeamProofKnobs { drop_q_bridge: true, ..Default::default() },
        wit::SeamProofKnobs { drop_norm_bridge: true, ..Default::default() },
        wit::SeamProofKnobs { drop_field_round: true, ..Default::default() },
    ] {
        let mut c = base.clone();
        assert_eq!(
            wit::certify_channel_with(om, &mut c, cfg, knobs).0 .0,
            wit::SeamProofStatus::Proved
        );
        let weak = c.seam_addend(om).unwrap();
        assert!(
            weak < full,
            "die Ablation {knobs:?} ändert den Addenden nicht ({weak:e} statt {full:e})"
        );
    }
}

/// **§8aa-Pin 7: die eingefrorene Form ist bei `A = 0` BITWEISE das heutige
/// Feld — und in `A` monoton.**
///
/// Das ist die Feldneutralitäts-Zusage, die P5b-1 einlösen muss: ein Kanal
/// ohne Addenden darf sich nicht bewegen. Und die Monotonie ist die Voraussetzung
/// der Binärsuche in Pin 4 (und, später, des Monotonie-Pins von P5b-1).
#[test]
fn clamp_frozen_form_is_neutral_at_zero_and_monotone_384() {
    let delta = 2.0f32;
    let mut g = Lcg(0x7e57_0007);
    let (mut probes, mut strict) = (0usize, 0usize);
    let mut strict_h = 0usize;
    let (mut proved_probes, mut legacy_probes) = (0usize, 0usize);
    for (name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        for (_om, chans) in omega_and_channels(&field) {
            for ch in chans {
                for _ in 0..200 {
                    let p = [
                        (ch.bound_c[0] as f64 + g.sym() * (ch.bound_r + ch.trust) as f64) as f32,
                        (ch.bound_c[1] as f64 + g.sym() * (ch.bound_r + ch.trust) as f64) as f32,
                        (ch.bound_c[2] as f64 + g.sym() * (ch.bound_r + ch.trust) as f64) as f32,
                    ];
                    // #384 §8ac / P5b-1A — **die Produktion IST jetzt die
                    // eingefrorene Kette**, mit den Addenden des aufgelösten
                    // Modus. Das ist die schärfere Fassung dieses Pins: vorher
                    // stand hier „Produktion == eingefrorene Form bei A = 0",
                    // weil es noch keinen Konsumenten gab.
                    let (a_s, a_hh) = ch.limit_mode().addends();
                    let today = oc::channel_limit(p, ch);
                    let frozen = wit::frozen_channel_limit(p, ch, a_s, a_hh);
                    assert_eq!(
                        today.to_bits(),
                        frozen.to_bits(),
                        "{name}: die Produktion weicht von der eingefrorenen Kette ab \
                         ({today} gegen {frozen}) bei {p:?}, Modus {:?}",
                        ch.limit_mode()
                    );
                    // Und die Legacy-Zusage, unverändert: ein Kanal OHNE Beweis
                    // liefert bitgenau die alte Form.
                    if !ch.limit_mode().is_proved() {
                        assert_eq!(
                            (a_s.to_bits(), a_hh.to_bits()),
                            (0f32.to_bits(), 0f32.to_bits()),
                            "{name}: ein LegacyUnproven-Kanal trägt Addenden"
                        );
                        legacy_probes += 1;
                    } else {
                        proved_probes += 1;
                    }
                    // Die Monotonie-Leiter startet ausdrücklich bei
                    // `A = A_h = 0` — unabhängig vom aufgelösten Modus des
                    // Kanals, sonst vergliche sie gegen einen bereits
                    // abgesenkten Wert.
                    let base0 = wit::frozen_channel_limit(p, ch, 0.0, 0.0);
                    let mut prev = base0;
                    for a in [1.0e-6f32, 1.0e-4, 1.0e-3, 1.0e-2, 0.5] {
                        let v = wit::frozen_channel_limit(p, ch, a, 0.0);
                        assert!(
                            v <= prev,
                            "{name}: die Klemme ist in A nicht monoton ({v} > {prev} bei A = {a})"
                        );
                        if v < prev {
                            strict += 1;
                        }
                        prev = v;
                    }
                    // #384 §8ab — dasselbe für den h-Addenden, und für BEIDE
                    // gemeinsam: die vollständige Kette darf nur fallen.
                    let mut prev_h = base0;
                    for ah in [1.0e-6f32, 1.0e-4, 1.0e-3, 1.0e-2, 0.5] {
                        let v = wit::frozen_channel_limit(p, ch, 0.0, ah);
                        assert!(
                            v <= prev_h,
                            "{name}: die Klemme ist in A_h nicht monoton ({v} > {prev_h})"
                        );
                        if v < prev_h {
                            strict_h += 1;
                        }
                        prev_h = v;
                        let both = wit::frozen_channel_limit(p, ch, ah, ah);
                        assert!(
                            both <= v && both <= wit::frozen_channel_limit(p, ch, ah, 0.0),
                            "{name}: die volle Kette liegt über einem ihrer Einzel-Äste"
                        );
                    }
                    probes += 1;
                }
            }
        }
    }
    assert!(probes > 10_000, "nur {probes} Proben");
    assert!(
        strict > 1_000,
        "nur {strict} strikte Absenkungen — dann ist der Addend an diesen Proben ein No-op \
         und der Monotonie-Pin misst nichts"
    );
    assert!(
        strict_h > 500,
        "nur {strict_h} strikte Absenkungen über A_h — dann ist der h-Addend an diesen Proben \
         ein No-op und der Monotonie-Pin misst dort nichts"
    );
    assert!(
        proved_probes > 1_000 && legacy_probes > 100,
        "{proved_probes} Proben an bewiesenen und {legacy_probes} an Legacy-Kanälen — der Pin \
         muss BEIDE Zweige sehen"
    );
    println!(
        "§8aa/§8ab/§8ac Pin 7: {probes} Proben ({proved_probes} bewiesen / {legacy_probes} \
         legacy), {strict} strikte Absenkungen über A, {strict_h} über A_h"
    );
}

/// **§8aa-Pin 8: die GEOMETRIE-Unterkette von `channel_limit` konsumiert
/// ausschliesslich `de` — bei FESTEM (Legacy-)Modus.**
///
/// §8ac-Korrektur (achtzehntes Review, Punkt 3): der Satz gilt weiter, aber
/// nur über die Unterkette, über die er je gemeint war. Seit P5b-1A hängt am
/// `Proved`-Pfad zusätzlich die Schnappschuss-Prüfung, und die liest `m̂`, `e`
/// und `d_e` sehr wohl — eine `m̂`-Mutation entzieht dem Kanal (richtigerweise)
/// seinen Laufzeitmodus. Der Pin fährt deshalb beide Seiten auf Legacy und
/// misst damit genau das, worüber die ε_D-Freiheit des Budgets spricht: die
/// Distanz-Rechnung selbst. Die Modus-Entziehung ist Gegenstand von
/// `activation_mutation_invalidates_the_runtime_mode_384`.
///
/// Der Satz, auf dem die ε_D-Freiheit des ganzen Budgets steht: die
/// Diamant-Zuordnung und die nlerp-Interpolation der Gate-Vektoren
/// beeinflussen den Grenzwert-Beitrag NICHT. Er wird hier nicht zitiert,
/// sondern am Code gemessen — alle vier Dual-Vektoren und beide
/// Bogen-Parameter (`e`, `d_e`) werden wild verändert, und das Feld muss
/// **bitweise** stehen bleiben. Zum Gegenbeweis der Wirksamkeit: dieselbe
/// Mutation MUSS `channel_nearest`s `m̂`-Ausgaben verändern.
#[test]
fn clamp_geometry_subchain_consumes_only_de_384() {
    let delta = 2.0f32;
    let mut g = Lcg(0x00de_00de);
    let (mut probes, mut m_differs, mut chans_seen) = (0usize, 0usize, 0usize);
    for (name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        for (_om, chans) in omega_and_channels(&field) {
            for ch in chans {
                if ch.arcs.iter().any(|a| !(a.sweep < 0.0)) {
                    continue; // gerade Kanäle — Bögen sind P5c
                }
                // §8ac-Korrektur: die Mutation verändert den Kanal-Digest und
                // entzieht ihm damit (richtigerweise) seinen Laufzeitmodus.
                // Der Pflichtsatz spricht aber über die GEOMETRIE-Kette, nicht
                // über die Addenden — beide Seiten laufen deshalb auf Legacy,
                // und der Vergleich misst wieder genau das, was er soll.
                // Dass die Mutation den Modus entzieht, ist Gegenstand von
                // `activation_mutation_invalidates_the_runtime_mode_384`.
                let ch = &{
                    let mut c = ch.clone();
                    wit::force_legacy_final_channel(
                        &mut c,
                        crate::offset_channel::LegacyReason::NotCertified,
                    );
                    c
                };
                let mut m = ch.clone();
                wit::force_legacy_final_channel(
                    &mut m,
                    crate::offset_channel::LegacyReason::NotCertified,
                );
                for a in &mut m.arcs {
                    a.m1s = [0.301_1, -0.903_3, 0.307_7];
                    a.m1e = [-0.717_3, 0.104_9, 0.688_7];
                    a.m2s = [0.911_3, 0.207_1, -0.355_9];
                    a.m2e = [0.051_7, -0.991_3, 0.121_3];
                    a.d_e = 1.703_1;
                    a.e = [-0.317_9, 0.948_1];
                }
                chans_seen += 1;
                for _ in 0..150 {
                    let p = [
                        (ch.bound_c[0] as f64 + g.sym() * (ch.bound_r + ch.trust) as f64) as f32,
                        (ch.bound_c[1] as f64 + g.sym() * (ch.bound_r + ch.trust) as f64) as f32,
                        (ch.bound_c[2] as f64 + g.sym() * (ch.bound_r + ch.trust) as f64) as f32,
                    ];
                    assert_eq!(
                        oc::channel_limit(p, ch).to_bits(),
                        oc::channel_limit(p, &m).to_bits(),
                        "{name}: channel_limit hat sich durch eine reine m̂/D-Mutation bewegt — \
                         dann trägt ε_D doch ins P5b-Budget ein"
                    );
                    if let (Some(a), Some(b)) =
                        (oc::channel_nearest(p, ch), oc::channel_nearest(p, &m))
                    {
                        assert_eq!(a.0.to_bits(), b.0.to_bits(), "{name}: de hat sich bewegt");
                        if a.2 != b.2 || a.3 != b.3 {
                            m_differs += 1;
                        }
                    }
                    probes += 1;
                }
            }
        }
    }
    assert!(chans_seen > 100 && probes > 15_000, "{chans_seen} Kanäle / {probes} Proben");
    assert!(
        m_differs > 5_000,
        "nur {m_differs} Proben, an denen sich m̂ überhaupt bewegt — dann ist die Mutation \
         wirkungslos und der Pin leer-grün"
    );
    println!("§8aa Pin 8: {probes} Proben über {chans_seen} gerade Kanäle, m̂ an {m_differs} verschieden");
}

/// **§8aa-Pin 9: der Shadow-Bericht — die Zahlen im Doc sind gemessen.**
///
/// §8aa-Korrektur (Sollte-15): gezählt werden **bewiesene Naht-Addenden**, nicht
/// „bewiesene Kanäle". Die ausgelieferte `channel_limit`-Kette ist damit NICHT
/// bewiesen — ihr `h`-Ast hat kein Budget (R1).
///
/// P5b-0 hat keinen Konsumenten; sein einziges Ergebnis IST diese Zählung.
/// Steht sie nicht im Pin, ist die Tabelle in §8aa eine Behauptung.
#[test]
fn clamp_shadow_report_matches_the_matrix_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let mut total = wit::SeamProofTally::default();
    let mut worst = [0.0f64; 4];
    let mut a_max = 0.0f32;
    for (name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        let (t, chans) = clamp_scan(&field, cfg);
        // Jeder Kanal landet in genau EINEM Topf.
        assert_eq!(t.total(), chans.len());
        assert_eq!(t.bad_config, 0, "{name}: BadConfig mit der Default-Konfiguration");
        assert_eq!(t.no_bridge, 0, "{name}: ein Cover ohne Brücke — das wäre neu");
        for (c, st, a) in &chans {
            assert_eq!(
                c.seam_proof().is_some(),
                *st == wit::SeamProofStatus::Proved,
                "{name}: Status und Einbettung sind auseinander"
            );
            assert_eq!(
                a.is_some(),
                *st == wit::SeamProofStatus::Proved,
                "{name}: der Addend ist gegen sein eigenes Ω nicht lesbar"
            );
            if *st != wit::SeamProofStatus::Proved {
                continue;
            }
            let k = c.seam_proof().unwrap();
            assert!(a.unwrap() as f64 >= k.bridge_total_mm());
            worst[0] = worst[0].max(k.geometric_mm());
            worst[1] = worst[1].max(k.q_bridge_mm());
            worst[2] = worst[2].max(k.norm_bridge_mm());
            worst[3] = worst[3].max(k.field_round_mm());
            a_max = a_max.max(a.unwrap());
        }
        total.proved_seam += t.proved_seam;
        total.curved += t.curved;
        total.no_cover += t.no_cover;
        total.no_bridge += t.no_bridge;
        total.bad_config += t.bad_config;
        total.not_concave += t.not_concave;
        total.h_proved += t.h_proved;
        total.h_vacuous += t.h_vacuous;
        total.h_no_cover += t.h_no_cover;
        total.h_not_supporting += t.h_not_supporting;
        total.h_degenerate += t.h_degenerate;
        total.h_no_budget += t.h_no_budget;
    }
    println!(
        "§8aa Shadow: {} Kanäle — {} Naht-Addenden bewiesen / {} curved / {} no_cover / {} no_bridge; \
         η_geo ≤ {:.3e}, ε_q ≤ {:.3e}, ε_d ≤ {:.3e}, field_round ≤ {:.3e}, A ≤ {:.3e} mm; \
         h-Budget: {} proved / {} vacuous / {} no_cover / {} not_supporting",
        total.total(),
        total.proved_seam,
        total.curved,
        total.no_cover,
        total.no_bridge,
        worst[0],
        worst[1],
        worst[2],
        worst[3],
        a_max,
        total.h_proved,
        total.h_vacuous,
        total.h_no_cover,
        total.h_not_supporting
    );
    assert_eq!(
        (
            total.proved_seam,
            total.curved,
            total.no_cover,
            total.no_bridge,
            total.bad_config,
            total.not_concave
        ),
        (217, 2, 19, 0, 0, 0),
        "der Shadow-Bericht hat sich bewegt — §8aa nachziehen, nicht den Pin"
    );
    // #384 §8ab — das h-Budget, gezählt unter den naht-zertifizierten Kanälen.
    assert_eq!(
        (
            total.h_proved,
            total.h_vacuous,
            total.h_no_cover,
            total.h_not_supporting,
            total.h_degenerate,
            total.h_no_budget
        ),
        (217, 0, 0, 0, 0, 0),
        "der h-Bericht hat sich bewegt — §8ab nachziehen, nicht den Pin"
    );
    assert_eq!(total.h_total(), total.proved_seam, "h-Zählung und Naht-Zählung sind auseinander");
    assert!(worst[0] <= 2.3e-3 && worst[1] <= 2.2e-5 && worst[2] <= 1.1e-6 && worst[3] <= 7.3e-7);
    assert!(a_max <= 2.27e-3, "A_max = {a_max:e}");
}

/// **§8aa-Pin 10 (R1): der `h`-Ast ist erreichbar — gemessen, nicht vermutet.**
///
/// §8aa-Korrektur (dreizehntes Review, Blocker 1). Der Review nennt die
/// Angriffsfamilie `de < h ≤ fl(de+A)`: dort wählte das heutige Feld `de`, die
/// eingefrorene Kette wählt neu `h`, und für `h` gibt es weder ein
/// Rundungs- noch ein geometrisches Budget — die Stützebenen-Synthese lässt
/// sogar `h_min ≥ −8·gate_eps` zu (`offset_channel.rs`, `set_face_plane_gates`).
///
/// Bevor eine Korrektur auf einer Behauptung aufbaut, wird sie **gemessen**:
/// Wie oft fällt das `min` überhaupt auf `h`? Und wie oft liegt ein Punkt in
/// genau dem Band, das P5b-1 vom `de`-Ast in den `h`-Ast verschieben würde?
///
/// Der Pin ist bewusst KEINE Korrektheitsaussage über den `h`-Ast — die gibt
/// es nicht, und genau das ist Restposten R1. Er hält die Erreichbarkeit fest,
/// damit niemand R1 später als „theoretisch" abtut.
#[test]
fn clamp_h_branch_is_reachable_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let mut g = Lcg(0x11_5b1_0001);
    let (mut probes, mut h_wins_today, mut band) = (0usize, 0usize, 0usize);
    let mut worst_band = 0.0f64;
    let mut fixtures_with_band = 0usize;
    for (_name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        let mut here = 0usize;
        for (om, chans) in omega_and_channels(&field) {
            for ch in chans {
                let mut c = ch.clone();
                if wit::certify_channel(om, &mut c, cfg).0 != wit::SeamProofStatus::Proved {
                    continue;
                }
                let a = c.seam_addend(om).unwrap();
                // Am Naht-Schlauch entlang proben, nicht im umschreibenden
                // Würfel: dort liegt fast alles ausserhalb von `trust` und der
                // Pin misst am Ende ein paar Dutzend Punkte.
                for i in 0..400 {
                    let arc = &c.arcs[i % c.arcs.len()];
                    let sp = g.next_f64() as f32;
                    let rad = (g.next_f64() * c.trust as f64 * 0.98) as f32;
                    let d = [g.sym(), g.sym(), g.sym()];
                    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if !(n > 1.0e-9) {
                        continue;
                    }
                    let p = [
                        arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n) as f32 * rad,
                        arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n) as f32 * rad,
                        arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n) as f32 * rad,
                    ];
                    let Some((de, _, _, _)) = oc::channel_nearest(p, &c) else { continue };
                    if !(de.is_finite() && de < c.trust) {
                        continue;
                    }
                    let h = oc::face_plane_gate(p, &c) + c.delta;
                    if !h.is_finite() {
                        continue;
                    }
                    probes += 1;
                    if h <= de {
                        h_wins_today += 1;
                    } else if h <= de + a {
                        // Genau das Band, das P5b-1 umhängt.
                        band += 1;
                        here += 1;
                        worst_band = worst_band.max((h - de) as f64);
                    }
                }
            }
        }
        if here > 0 {
            fixtures_with_band += 1;
        }
    }
    assert!(probes > 50_000, "nur {probes} Proben");
    assert!(
        h_wins_today > 0,
        "der h-Ast wird an KEINER Probe genommen — dann wäre R1 an dieser Matrix nicht \
         erreichbar, und §8aa müsste das sagen statt eine Auflage zu formulieren"
    );

    // **Das Umhäng-Band gezielt suchen.** Gleichverteilte Proben treffen ein
    // `A`-breites Band (≈ 10⁻⁴ mm) praktisch nie — das ist eine Aussage über
    // die Auflösung der Probe, KEINE über Erreichbarkeit. Deshalb hier eine
    // Bisektion auf den Vorzeichenwechsel von `h − de` entlang eines Strahls:
    // dort, wo das `min` heute kippt, liegt das Band unmittelbar daneben.
    let mut g2 = Lcg(0x11_5b1_0002);
    let (mut rays, mut hits) = (0usize, 0usize);
    let mut tightest = f64::INFINITY;
    'outer: for (_name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        for (om, chans) in omega_and_channels(&field) {
            for ch in chans {
                let mut c = ch.clone();
                if wit::certify_channel(om, &mut c, cfg).0 != wit::SeamProofStatus::Proved {
                    continue;
                }
                let a = c.seam_addend(om).unwrap();
                let arc = &c.arcs[0].clone();
                for _ in 0..60 {
                    let sp = g2.next_f64() as f32;
                    let d = [g2.sym(), g2.sym(), g2.sym()];
                    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if !(n > 1.0e-9) {
                        continue;
                    }
                    let at = |t: f64| -> Option<(f32, f32)> {
                        let p = [
                            arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n * t) as f32,
                            arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n * t) as f32,
                            arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n * t) as f32,
                        ];
                        let (de, _, _, _) = oc::channel_nearest(p, &c)?;
                        let h = oc::face_plane_gate(p, &c) + c.delta;
                        (de.is_finite() && de < c.trust && h.is_finite()).then_some((de, h))
                    };
                    // Vorzeichenwechsel von `h − de` suchen.
                    let (mut lo, mut hi) = (0.0f64, 0.0f64);
                    let mut found = false;
                    let mut prev: Option<(f64, f32)> = None;
                    for k in 1..=64 {
                        let t = k as f64 / 64.0 * c.trust as f64 * 0.98;
                        let Some((de, h)) = at(t) else { continue };
                        let s = h - de;
                        if let Some((pt, ps)) = prev {
                            if (ps > 0.0) != (s > 0.0) {
                                lo = pt;
                                hi = t;
                                found = true;
                                break;
                            }
                        }
                        prev = Some((t, s));
                    }
                    if !found {
                        continue;
                    }
                    rays += 1;
                    // 80 Halbierungen — danach ist das Intervall unter jeder
                    // f32-Auflösung, und `h − de` liegt beliebig nah an null.
                    for _ in 0..80 {
                        let mid = 0.5 * (lo + hi);
                        let Some((de, h)) = at(mid) else { break };
                        let s = h - de;
                        if let Some((_de0, h0)) = at(lo) {
                            let s0 = h0 - _de0;
                            if (s0 > 0.0) == (s > 0.0) {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        } else {
                            break;
                        }
                        if s > 0.0 && s <= a {
                            hits += 1;
                            tightest = tightest.min(s as f64);
                            break;
                        }
                    }
                    if hits >= 200 {
                        break 'outer;
                    }
                }
            }
        }
    }
    println!(
        "§8aa R1: {probes} gleichverteilte Proben — h gewinnt schon heute an {h_wins_today} \
         ({:.1} %); das Umhäng-Band de < h ≤ de+A trifft dort {band} Proben \
         ({fixtures_with_band} Fixturen, breitester Fall {worst_band:.3e} mm). \
         GEZIELT: {hits} Treffer aus {rays} Strahlen mit Vorzeichenwechsel, \
         engster Fall h − de = {tightest:.3e} mm",
        100.0 * h_wins_today as f64 / probes as f64
    );
    assert!(
        rays > 50,
        "nur {rays} Strahlen mit einem h/de-Vorzeichenwechsel — dann misst die gezielte \
         Suche nichts"
    );
    assert!(
        hits > 0,
        "die gezielte Suche findet das Umhäng-Band an KEINEM der {rays} Strahlen. Dann wäre \
         R1 zwar erreichbar (h gewinnt an {h_wins_today} Proben), aber das UMHÄNGEN durch \
         P5b-1 nicht — und §8aa müsste genau das sagen"
    );
}

// ══════════════ #384 §8ab / P5b-1H — das h-Budget ══════════════

/// **§8ab-Sonde: das h-Budget je Fixture.**
#[test]
#[ignore]
fn probe_384_h_budget() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    println!(
        "| Fixture | Naht | h_proved | h_vacuous | h_no_cover | h_not_sup | h_degen | h_no_budget | τ mm | norm mm | reach mm | round mm | A_h mm |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (name, field, _e, _lo, _hi, _n) in cert_matrix(delta) {
        let (t, chans) = clamp_scan(&field, cfg);
        let mut mx = [0.0f64; 4];
        let mut a_max = 0.0f32;
        for (c, _st, _a) in &chans {
            let Some(k) = c.seam_proof().and_then(|p| p.h_budget()) else { continue };
            mx[0] = mx[0].max(k.tau_mm());
            mx[1] = mx[1].max(k.norm_mm());
            mx[2] = mx[2].max(k.reach_mm());
            mx[3] = mx[3].max(k.round_mm());
        }
        for (om, cs) in omega_and_channels(&field) {
            for ch in cs {
                let mut c = ch.clone();
                let _ = wit::certify_channel(om, &mut c, cfg);
                if let Some(v) = c.h_addend(om) {
                    a_max = a_max.max(v);
                }
            }
        }
        let cell = |v: f64| if v > 0.0 { format!("{v:.3e}") } else { "—".into() };
        println!(
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            t.proved_seam,
            t.h_proved,
            t.h_vacuous,
            t.h_no_cover,
            t.h_not_supporting,
            t.h_degenerate,
            t.h_no_budget,
            cell(mx[0]),
            cell(mx[1]),
            cell(mx[2]),
            cell(mx[3]),
            if a_max > 0.0 { format!("{a_max:.3e}") } else { "—".into() },
        );
    }
}

/// **§8ab-Pin 1: die Box-Überdeckung enthält `Ω` wirklich.**
///
/// Das Halbraum-Lemma steht und fällt mit `Ω ⊆ ⋃ Boxen`. Geprüft wird das
/// gegen ein UNABHÄNGIGES Orakel: jeder Punkt, den die analytische
/// Komplement-Distanz als INNEN ausweist (`dist(x,∁Ω) > 0`), muss in einer
/// Box liegen. Rückdreh: streicht man eine Box, MUSS ein Innenpunkt
/// unbedeckt bleiben.
#[test]
fn h_cover_contains_omega_384() {
    let delta = 2.0f32;
    let mut g = Lcg(0x8ab_c07e2);
    let (mut inside, mut fixtures) = (0usize, 0usize);
    for (name, field, exact, lo, hi, _n) in cert_matrix(delta) {
        let mut covers: Vec<Vec<wit::WorldBox>> = Vec::new();
        for (om, _c) in omega_and_channels(&field) {
            if let Some(c) = wit::omega_cover(om) {
                covers.push(c);
            }
        }
        if covers.is_empty() {
            continue;
        }
        fixtures += 1;
        let mut here = 0usize;
        for _ in 0..4000 {
            let p = [
                lo[0] + g.next_f64() * (hi[0] - lo[0]),
                lo[1] + g.next_f64() * (hi[1] - lo[1]),
                lo[2] + g.next_f64() * (hi[2] - lo[2]),
            ];
            if !(exact(p) > 0.0) {
                continue; // ausserhalb Ω — die Überdeckung sagt darüber nichts
            }
            inside += 1;
            here += 1;
            for cov in &covers {
                let ok = cov.iter().any(|(bl, bh)| {
                    (0..3).all(|k| p[k] >= bl[k] && p[k] <= bh[k])
                });
                assert!(ok, "{name}: Innenpunkt {p:?} liegt in KEINER Überdeckungs-Box");
            }
        }
        assert!(here > 50, "{name}: nur {here} Innenpunkte — der Pin misst dort nichts");
        // Rückdreh: eine gestrichene Box muss eine Lücke hinterlassen.
        let cov = &covers[0];
        if cov.len() > 1 {
            let mut miss = 0usize;
            for drop in 0..cov.len() {
                let thin: Vec<wit::WorldBox> =
                    cov.iter().enumerate().filter(|(i, _)| *i != drop).map(|(_, b)| *b).collect();
                let mut g2 = Lcg(0xdead_0f0f + drop as u64);
                for _ in 0..4000 {
                    let p = [
                        lo[0] + g2.next_f64() * (hi[0] - lo[0]),
                        lo[1] + g2.next_f64() * (hi[1] - lo[1]),
                        lo[2] + g2.next_f64() * (hi[2] - lo[2]),
                    ];
                    if !(exact(p) > 0.0) {
                        continue;
                    }
                    if !thin.iter().any(|(bl, bh)| (0..3).all(|k| p[k] >= bl[k] && p[k] <= bh[k])) {
                        miss += 1;
                        break;
                    }
                }
            }
            assert!(
                miss > 0,
                "{name}: KEINE der {} Boxen ist tragend — dann ist die Überdeckung nicht \
                 minimal genug, um etwas zu beweisen",
                cov.len()
            );
        }
    }
    assert!(fixtures >= 8 && inside > 5_000, "{fixtures} Fixturen / {inside} Innenpunkte");
    println!("§8ab Pin 1: {inside} Innenpunkte über {fixtures} Fixturen, alle überdeckt");
}

/// **§8ab-Pin 2: das Halbraum-Lemma, gegen das exakte Orakel.**
///
/// `−SDF_Ω(x) ≤ (Ĥ(x) + τ)/ν ≤ Ĥ(x) + A^geom ≤ h_safe`. Geprüft an Punkten
/// INNERHALB `Ω` (dort ist `−SDF_Ω = dist(x,∁Ω)`, und genau das liefert die
/// analytische Referenz) über die ganze Matrix — und ausdrücklich AUCH im
/// Umhäng-Band `de < h ≤ de+A`, das §8aa als R1 offengelassen hatte.
#[test]
fn h_budget_bounds_the_true_distance_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let mut g = Lcg(0x8ab_11feed);
    let (mut probes, mut inband, mut hneg) = (0usize, 0usize, 0usize);
    let mut outside_checked = 0usize;
    let mut tightest = f64::INFINITY;
    let mut fixtures = 0usize;
    for (name, field, exact, _l, _h, _c) in cert_matrix(delta) {
        let mut here = 0usize;
        for (om, chans) in omega_and_channels(&field) {
            let cover = wit::omega_cover(om);
            for ch in chans {
                let mut c = ch.clone();
                if wit::certify_channel(om, &mut c, cfg).0 != wit::SeamProofStatus::Proved {
                    continue;
                }
                let (Some(a), Some(ah)) = (c.seam_addend(om), c.h_addend(om)) else { continue };
                for i in 0..300 {
                    let arc = &c.arcs[i % c.arcs.len()];
                    let sp = g.next_f64() as f32;
                    let rad = (g.next_f64() * c.trust as f64 * 0.98) as f32;
                    let d = [g.sym(), g.sym(), g.sym()];
                    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if !(n > 1.0e-9) {
                        continue;
                    }
                    let p = [
                        arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n) as f32 * rad,
                        arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n) as f32 * rad,
                        arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n) as f32 * rad,
                    ];
                    let Some((de, _, _, _)) = oc::channel_nearest(p, &c) else { continue };
                    if !(de.is_finite() && de < c.trust) {
                        continue;
                    }
                    let h = oc::face_plane_gate(p, &c) + c.delta;
                    if !h.is_finite() || h >= 1.0e29 {
                        continue; // alle Slots Sentinel — der h-Ast bindet nie
                    }
                    if h < 0.0 {
                        hneg += 1;
                        // **Signiertes Aussen-Orakel** (Sollte-Punkt des
                        // fünfzehnten Reviews). Für x ∉ Ω ist
                        // `−SDF_Ω(x) = −dist(x,Ω)`, und das exakte
                        // Komplement-Orakel liefert dort nur „0". Geprüft wird
                        // deshalb gegen `dist(x, ⋃Boxen) ≤ dist(x,Ω)`: gilt
                        // schon `dist(x,⋃Boxen) ≥ −h_safe`, ist die Zusage
                        // erst recht erfüllt. Die Überdeckung selbst hängt an
                        // `h_cover_contains_omega_384`.
                        let h_safe = (h + ah) as f64;
                        if h_safe < 0.0 {
                            if let Some(cov) = &cover {
                                let mut best = f64::INFINITY;
                                for (bl, bh) in cov {
                                    let mut q = 0.0f64;
                                    for k in 0..3 {
                                        let v = (bl[k] - p[k] as f64)
                                            .max(p[k] as f64 - bh[k])
                                            .max(0.0);
                                        q += v * v;
                                    }
                                    best = best.min(q.sqrt());
                                }
                                assert!(
                                    best >= -h_safe - 1.0e-9,
                                    "{name}: h < 0, aber dist(x,Ω) ≥ {best:e} < −h_safe = \
                                     {:e} — die Aussen-Seite des Halbraum-Lemmas hält nicht",
                                    -h_safe
                                );
                                outside_checked += 1;
                            }
                        }
                    }
                    if h > de && h <= de + a {
                        inband += 1;
                    }
                    let dist = exact([p[0] as f64, p[1] as f64, p[2] as f64]);
                    if !(dist > 0.0) {
                        continue; // ausserhalb Ω: −SDF_Ω < 0, das Orakel sagt nur „0"
                    }
                    probes += 1;
                    here += 1;
                    let h_safe = (h + ah) as f64;
                    assert!(
                        dist <= h_safe,
                        "{name}: dist(x,∁Ω) = {dist:e} > h_safe = {h_safe:e} bei {p:?}"
                    );
                    tightest = tightest.min(h_safe - dist);
                }
            }
        }
        if here > 0 {
            fixtures += 1;
        }
    }
    assert!(fixtures >= 8, "nur {fixtures} Fixturen");
    assert!(probes > 5_000, "nur {probes} Proben in Ω");

    // **Das Umhäng-Band gezielt.** Gleichverteiltes Proben trifft ein
    // `A`-breites Band nicht (§8aa-Befund); die Bisektion auf den
    // Vorzeichenwechsel von `h − de` setzt genau dort auf.
    let mut g2 = Lcg(0x8ab_ba9d);
    let (mut rays, mut hits) = (0usize, 0usize);
    let mut band_tight = f64::INFINITY;
    'outer: for (name, field, exact, _l, _h, _c) in cert_matrix(delta) {
        for (om, chans) in omega_and_channels(&field) {
            for ch in chans {
                let mut c = ch.clone();
                if wit::certify_channel(om, &mut c, cfg).0 != wit::SeamProofStatus::Proved {
                    continue;
                }
                let (Some(a), Some(ah)) = (c.seam_addend(om), c.h_addend(om)) else { continue };
                let arc = c.arcs[0].clone();
                for _ in 0..60 {
                    let sp = g2.next_f64() as f32;
                    let d = [g2.sym(), g2.sym(), g2.sym()];
                    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if !(n > 1.0e-9) {
                        continue;
                    }
                    let at = |t: f64| -> Option<([f32; 3], f32, f32)> {
                        let p = [
                            arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n * t) as f32,
                            arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n * t) as f32,
                            arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n * t) as f32,
                        ];
                        let (de, _, _, _) = oc::channel_nearest(p, &c)?;
                        let h = oc::face_plane_gate(p, &c) + c.delta;
                        (de.is_finite() && de < c.trust && h.is_finite() && h < 1.0e29)
                            .then_some((p, de, h))
                    };
                    let (mut lo, mut hi) = (0.0f64, 0.0f64);
                    let mut prev: Option<(f64, f32)> = None;
                    let mut found = false;
                    for k in 1..=64 {
                        let t = k as f64 / 64.0 * c.trust as f64 * 0.98;
                        let Some((_p, de, h)) = at(t) else { continue };
                        let sgn = h - de;
                        if let Some((pt0, s0)) = prev {
                            if (s0 > 0.0) != (sgn > 0.0) {
                                lo = pt0;
                                hi = t;
                                found = true;
                                break;
                            }
                        }
                        prev = Some((t, sgn));
                    }
                    if !found {
                        continue;
                    }
                    rays += 1;
                    for _ in 0..80 {
                        let mid = 0.5 * (lo + hi);
                        let (Some((p, de, h)), Some((_p0, de0, h0))) = (at(mid), at(lo)) else {
                            break;
                        };
                        if ((h0 - de0) > 0.0) == ((h - de) > 0.0) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                        if h > de && h <= de + a {
                            let dist = exact([p[0] as f64, p[1] as f64, p[2] as f64]);
                            if dist > 0.0 {
                                let h_safe = (h + ah) as f64;
                                assert!(
                                    dist <= h_safe,
                                    "{name}: IM UMHÄNG-BAND ist dist(x,∁Ω) = {dist:e} > \
                                     h_safe = {h_safe:e} bei {p:?}"
                                );
                                band_tight = band_tight.min(h_safe - dist);
                                hits += 1;
                            }
                            break;
                        }
                    }
                    if hits >= 150 {
                        break 'outer;
                    }
                }
            }
        }
    }
    assert!(rays > 50, "nur {rays} Strahlen mit h/de-Vorzeichenwechsel");
    assert!(
        hits > 0,
        "das Umhäng-Band de < h ≤ de+A wurde auch GEZIELT an keiner der {rays} Strahlen \
         innerhalb Ω getroffen — dann prüft der Pin genau den Fall nicht, für den P5b-1H \
         gebaut ist"
    );
    println!(
        "§8ab Pin 2: {probes} Proben in Ω über {fixtures} Fixturen; engste Reserve \
         {tightest:.3e} mm; Umhäng-Band gleichverteilt an {inband}, gezielt {hits} Treffer \
         aus {rays} Strahlen (engste Reserve dort {band_tight:.3e} mm); h < 0 an {hneg}, \
         davon {outside_checked} gegen das signierte Aussen-Orakel geprüft"
    );
}

/// **§8ab-Pin 3: `τ` und der Normfehler sind hergeleitet, nicht geraten.**
///
/// Der Review verlangt den geometrischen Aufschlag für die von der Synthese
/// zugelassene Stützseiten-Verletzung (`h_min ≥ −8·gate_eps`). `gate_eps` ist
/// im Kanal nicht gespeichert; bezahlt wird hier deshalb nicht ein
/// unsichtbarer Parameter, sondern das SELBST hergeleitete `τ`. Der Pin zeigt,
/// dass diese Herleitung wirklich misst:
///
/// * eine Ebene, die um `ε` IN den Körper geschoben wird, hebt `τ` um `≈ ε`
///   und den Addenden mit;
/// * jenseits des Deckels `H_TAU_LIMIT_MM` gibt es gar kein Budget mehr;
/// * eine Ebene mit `|n| = 1 + 10⁻³` hebt den Normfehler-Anteil um `≈ R·10⁻³`;
/// * die Ablationen `τ := 0` und `Normfehler := 0` senken den Addenden — ein
///   Posten, der den Wert nicht bewegt, ist keiner.
#[test]
fn h_budget_tau_and_norm_are_derived_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let (_n, field, _e, _l, _h, _c) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Ecken-Schale t/δ=2.5"))
        .unwrap();
    let v = omega_and_channels(&field);
    let (om, chans) = v.into_iter().next().unwrap();
    let cover = wit::omega_cover(om).expect("Überdeckung");
    let base = chans[0].clone();
    let k0 = wit::h_budget(&base, &cover, Default::default()).expect("h-Budget");
    assert!(k0.slots() > 0, "die Fixture hat keine belegte Ebene — der Pin misst nichts");
    let a0 = k0.addend() as f64;

    // (a) Ebene um ε in den Körper schieben ⇒ τ steigt um ≈ ε.
    let mut seen = 0usize;
    for eps in [1.0e-4f32, 1.0e-3, 1.0e-2] {
        let mut m = base.clone();
        // `Ĥ = ⟨x,n⟩ − C + δ`: C ERHÖHEN senkt Ĥ, schiebt die Ebene also in
        // den Körper hinein.
        m.face_planes[0][3] += eps;
        let k = wit::h_budget(&m, &cover, Default::default()).expect("h-Budget");
        let got = k.tau_mm();
        assert!(
            got >= 0.9 * eps as f64 && got <= 1.1 * eps as f64 + 1.0e-9,
            "τ = {got:e} statt ≈ {eps:e} — die Herleitung misst die Verletzung nicht"
        );
        assert!(k.addend() as f64 > a0 + 0.5 * eps as f64, "der Addend folgt τ nicht");
        seen += 1;
    }
    assert_eq!(seen, 3);

    // (b) jenseits des Deckels: gar kein Budget.
    let mut far = base.clone();
    far.face_planes[0][3] += 5.0;
    assert_eq!(
        wit::h_budget(&far, &cover, Default::default()).unwrap_err(),
        wit::HStatus::NotSupporting
    );

    // (c) Nicht-Einheitsnorm ⇒ der Normfehler-Anteil steigt mit R·|1−ν|.
    let mut scaled = base.clone();
    let f = 1.0f32 + 1.0e-3;
    for k in 0..3 {
        scaled.face_planes[0][k] *= f;
    }
    scaled.face_planes[0][3] *= f;
    let ks = wit::h_budget(&scaled, &cover, Default::default()).expect("h-Budget");
    assert!(
        ks.norm_mm() > 0.5e-3 * k0.reach_mm() && ks.norm_mm() < 2.0e-3 * ks.reach_mm(),
        "Normfehler {:e} passt nicht zu R = {:e} bei |n| = 1+1e-3",
        ks.norm_mm(),
        ks.reach_mm()
    );
    assert!(ks.addend() > k0.addend(), "der Addend folgt dem Normfehler nicht");

    // (d) Ablationen: jeder Posten muss den Wert bewegen.
    for (name, knobs) in [
        ("τ", wit::SeamProofKnobs { h_drop_tau: true, ..Default::default() }),
        ("Norm", wit::SeamProofKnobs { h_drop_norm: true, ..Default::default() }),
        ("Rundung", wit::SeamProofKnobs { h_drop_round: true, ..Default::default() }),
    ] {
        // An der SKALIERTEN Ebene sind alle drei Posten von null verschieden.
        let mut m = scaled.clone();
        m.face_planes[1][3] += 1.0e-3; // auch τ auf einem zweiten Slot
        let full = wit::h_budget(&m, &cover, Default::default()).unwrap();
        let weak = wit::h_budget(&m, &cover, knobs).unwrap();
        assert!(
            weak.addend() < full.addend(),
            "die Ablation {name} ändert den h-Addenden nicht ({} statt {})",
            weak.addend(),
            full.addend()
        );
    }
    println!(
        "§8ab Pin 3: τ₀ = {:.3e}, Norm₀ = {:.3e}, R = {:.3e}, A_h = {:.3e} mm",
        k0.tau_mm(),
        k0.norm_mm(),
        k0.reach_mm(),
        k0.addend()
    );
}

/// **§8ab-Pin 4: das Rundungsbudget des h-Astes, ulp-scharf.**
///
/// Dieselbe Bauart wie Lemma 5 des Naht-Astes (§8t/§8aa): eine gezielte
/// f32-Familie über den arithmetischen Schwanz — Bit-Fächer um jede
/// Grössenordnung von `h`, dazu Streuwerte, über drei `δ`, gefahren durch
/// **genau die Ops**, die P5b-1A ins Feld setzt.
///
/// Verlangt ist im h-Ast `fl(δ − fl(h + A_h)) ≤ δ − h − A^geom`.
///
/// Zwei Gegenproben: ohne Rundungsbudget MUSS die Familie reissen, und der
/// über der Familie minimale Addend `A*` MUSS bei `−1 ulp` reissen. Wie in
/// §8aa gilt die Rollenverteilung: den ausgelieferten Addenden trägt der
/// algebraische Ausdruck, `A*` zeigt die Schärfe DIESER Familie.
#[test]
fn h_budget_rounding_absorbs_both_f32_ops_384() {
    let trust = 3.91f32;
    for &geom in &[1.0e-13f64, 1.0e-6, 1.0e-3] {
        let mut cases: Vec<(f32, f32, f64)> = Vec::new();
        for &d in &[2.0f32, 0.25, 12.5] {
            for h in clamp_de_family(trust) {
                for sgn in [1.0f32, -1.0] {
                    cases.push((d, sgn * h, geom));
                }
            }
        }
        assert!(cases.len() > 200_000, "nur {} Fälle", cases.len());
        let need = |a: f32| -> f64 {
            let nd = crate::offset_interval::next_down;
            let mut m = f64::INFINITY;
            for &(d, h, gm) in &cases {
                let lim = wit::frozen_limit_tail(d, f32::INFINITY, h, 0.0, a) as f64;
                let lb = nd(nd(nd(d as f64 - h as f64) - gm));
                m = m.min(lb - lim);
            }
            m
        };
        // Der bewiesene Addend: derselbe Ausdruck wie in `h_budget`, hier über
        // der Reichweite der Familie.
        let reach = trust as f64;
        let u = wit::F32_U;
        let num = crate::offset_interval::next_up(
            geom + u * (2.0 * reach + 12.5) + 3.0 * wit::F32_SUB + u * (reach + 12.5) * 2.0,
        );
        let a = wit::f32_ceil(num / (1.0 - 3.0 * u)).unwrap();
        let m = need(a);
        assert!(m >= 0.0, "geom = {geom:e}: die Familie reisst mit dem bewiesenen A_h ({m:e})");

        // Gegenprobe 1: ohne Rundungsbudget.
        let a0 = wit::f32_ceil(geom).unwrap();
        assert!(
            need(a0) < 0.0,
            "geom = {geom:e}: ohne Rundungsbudget (A_h = {a0:e}) bleibt die Familie still"
        );

        // Gegenprobe 2: der familienminimale Addend, ulp-scharf.
        let (mut lo, mut hi) = (0u32, a.to_bits());
        assert!(need(f32::from_bits(lo)) < 0.0);
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if need(f32::from_bits(mid)) >= 0.0 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let a_star = f32::from_bits(hi);
        assert!(need(a_star) >= 0.0);
        assert!(
            need(f32::from_bits(hi - 1)) < 0.0,
            "geom = {geom:e}: A* − 1 ulp reisst NICHT — die Familie ist nicht ulp-scharf"
        );
        println!(
            "§8ab Pin 4: geom = {geom:.3e} | A_h = {a:.6e} | A* = {a_star:.6e} | \
             A/A* = {:.4} | Reserve = {m:.3e}",
            a as f64 / a_star as f64
        );
    }
}

/// **§8ab-Pin 5: die Scope-Sperre — ein KONVEXER Kanal bekommt keinen Beweis.**
///
/// `CertifiedSeamAddend` beweist die konkave Erosions-Aussage über `Ω` = dem
/// Körper VOR der Erosion. Am Dilations-Stopp ist die Dualität eine andere,
/// und einen konvexen Dualsatz gibt es nicht. Vor P5b-1H prägte
/// `certify_channel_with` trotzdem — der Wert wäre auf einem konvexen Kanal
/// unbelegt gewesen. Jetzt: fail-closed beim Prägen, und der Tally weist es
/// ehrlich aus.
#[test]
fn seam_proof_refuses_convex_channels_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let (_n, field, _e, _l, _h, _c) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Ecken-Schale t/δ=2.5"))
        .unwrap();
    let v = omega_and_channels(&field);
    let (om, chans) = v.into_iter().next().unwrap();
    let base = chans[0].clone();
    assert!(base.concave, "die Fixture liefert keinen konkaven Kanal");

    let mut ok = base.clone();
    assert_eq!(wit::certify_channel(om, &mut ok, cfg).0, wit::SeamProofStatus::Proved);
    assert!(ok.seam_addend(om).is_some() && ok.h_addend(om).is_some());

    let mut convex = base.clone();
    convex.concave = false;
    let (st, _) = wit::certify_channel(om, &mut convex, cfg);
    assert_eq!(st, wit::SeamProofStatus::NotConcave, "ein konvexer Kanal wurde zertifiziert");
    assert!(convex.seam_addend(om).is_none(), "…und trug trotzdem einen Naht-Addenden");
    assert!(convex.h_addend(om).is_none(), "…und trotzdem ein h-Budget");

    let mut t = wit::SeamProofTally::default();
    t.add(st);
    assert_eq!(t.not_concave, 1);
    assert_eq!(t.proved_seam, 0);
    assert_eq!(t.total(), 1, "der Tally verliert den konvexen Kanal");
}

/// **§8ab-Pin 6: das h-Budget ist fail-closed.**
///
/// Kein `Ω`-Cover, entartete Ebene, alle Slots Sentinel — jeder Ausgang
/// einzeln, und keiner endet in einer Zusage.
#[test]
fn h_budget_is_fail_closed_384() {
    let delta = 2.0f32;
    let cfg = wit::WitnessCfg::default();
    let (_n, field, _e, _l, _h, _c) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Ecken-Schale t/δ=2.5"))
        .unwrap();
    let v = omega_and_channels(&field);
    let (om, chans) = v.into_iter().next().unwrap();
    let cover = wit::omega_cover(om).unwrap();
    let base = chans[0].clone();

    // (a) nicht unterstütztes Ω-Blatt ⇒ keine Überdeckung.
    let (_n2, field2, _e2, _l2, _h2, _c2) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Rippen-Kamm 4×1.5"))
        .unwrap();
    let mut nocover = 0usize;
    for (om2, _c) in omega_and_channels(&field2) {
        assert!(wit::omega_cover(om2).is_none(), "ein Extrude-Ω lieferte eine Überdeckung");
        nocover += 1;
    }
    assert!(nocover > 0);

    // (b) entartete Ebene: n = 0, aber KEIN Sentinel.
    let mut degen = base.clone();
    degen.face_planes[0] = [0.0, 0.0, 0.0, 1.0];
    assert_eq!(
        wit::h_budget(&degen, &cover, Default::default()).unwrap_err(),
        wit::HStatus::Degenerate
    );

    // (c) alle Slots Sentinel ⇒ gegenstandslos wahr, Addend 0, h-Ast unerreichbar.
    let mut sentinel = base.clone();
    sentinel.face_planes = [crate::offset_channel::END_PLANE_NONE; 4];
    let k = wit::h_budget(&sentinel, &cover, Default::default()).unwrap();
    assert_eq!(k.slots(), 0);
    assert_eq!(k.addend(), 0.0);
    let g = oc::face_plane_gate([1.0, 2.0, 3.0], &sentinel);
    assert!(g >= 1.0e30, "der Sentinel-Gate-Wert ist {g}, nicht 1e30 — der h-Ast wäre erreichbar");

    // (d) ein Sentinel-Slot NEBEN belegten: er zählt nicht mit und kann den
    //     min nicht gewinnen.
    let mut mixed = base.clone();
    mixed.face_planes[3] = crate::offset_channel::END_PLANE_NONE;
    let km = wit::h_budget(&mixed, &cover, Default::default()).unwrap();
    assert!(km.slots() >= 1 && km.slots() <= 3);
    let mut gsum = 0usize;
    let mut g2 = Lcg(0x5e7);
    for _ in 0..2000 {
        let p = [
            (base.bound_c[0] as f64 + g2.sym() * 8.0) as f32,
            (base.bound_c[1] as f64 + g2.sym() * 8.0) as f32,
            (base.bound_c[2] as f64 + g2.sym() * 8.0) as f32,
        ];
        let v = oc::face_plane_gate(p, &mixed);
        assert!(v < 1.0e29, "der Sentinel gewann den min bei {p:?}");
        gsum += 1;
    }
    assert!(gsum > 1000);

    // (e) Ω = Empty: jede Ebene stützt, τ = 0.
    let empty = wit::omega_cover(&BoundedCsg::Empty).unwrap();
    let ke = wit::h_budget(&base, &empty, Default::default()).unwrap();
    assert_eq!(ke.tau_mm(), 0.0);
}

/// Die Welt-AABB einer lokalen Box, aus einer **unabhängigen** Inversen:
/// die 3×3-Matrix wird aus `world_to_local` abgetastet und per
/// Gauss-Elimination invertiert — nicht per Adjunkte wie im Beweiscode.
/// Nur ein Probengebiet für die Pins, kein Beweis.
fn independent_world_aabb(t: &fielddraft_protocol::Transform, he: [f32; 3]) -> Option<([f64; 3], [f64; 3])> {
    let o = crate::transform::world_to_local(t, [0.0; 3]);
    let mut a = [[0.0f64; 3]; 3];
    for c in 0..3 {
        let mut e = [0.0f32; 3];
        e[c] = 1.0;
        let v = crate::transform::world_to_local(t, e);
        for r in 0..3 {
            a[r][c] = v[r] as f64 - o[r] as f64;
        }
    }
    // Gauss-Jordan auf [A | I].
    let mut m = [[0.0f64; 6]; 3];
    for r in 0..3 {
        for c in 0..3 {
            m[r][c] = a[r][c];
        }
        m[r][3 + r] = 1.0;
    }
    for col in 0..3 {
        let piv = (col..3).max_by(|&x, &y| {
            m[x][col].abs().partial_cmp(&m[y][col].abs()).unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if m[piv][col].abs() < 1.0e-12 {
            return None;
        }
        m.swap(col, piv);
        let d = m[col][col];
        for c in 0..6 {
            m[col][c] /= d;
        }
        for r in 0..3 {
            if r == col {
                continue;
            }
            let f = m[r][col];
            for c in 0..6 {
                m[r][c] -= f * m[col][c];
            }
        }
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for cx in 0..8 {
        let l = [
            if cx & 1 == 0 { -he[0] as f64 } else { he[0] as f64 },
            if cx & 2 == 0 { -he[1] as f64 } else { he[1] as f64 },
            if cx & 4 == 0 { -he[2] as f64 } else { he[2] as f64 },
        ];
        for r in 0..3 {
            let mut v = t.translate[r] as f64;
            for c in 0..3 {
                v += m[r][3 + c] * l[c];
            }
            lo[r] = lo[r].min(v);
            hi[r] = hi[r].max(v);
        }
    }
    // Grosszügig aufweiten: das Gebiet muss Ω sicher enthalten UND Luft
    // daneben lassen, damit eine ZU KLEINE Beweisbox auffliegt.
    for r in 0..3 {
        let w = (hi[r] - lo[r]).max(1.0e-3);
        lo[r] -= 0.6 * w;
        hi[r] += 0.6 * w;
        if !lo[r].is_finite() || !hi[r].is_finite() {
            return None;
        }
    }
    Some((lo, hi))
}

/// **§8ab-Pin 7 (Blocker 1): die Überdeckung hält auch ohne Einheits-Quaternion.**
///
/// `local_box_to_world` baute die Vorwärtsrichtung früher aus
/// `quat_rotate(q,·)` und behandelte sie als Umkehrung von
/// `quat_rotate(conj q,·)` — nur bei `|q| = 1` sind das inverse Matrizen.
/// Nichts erzwingt `|q| = 1`: `Transform` hat öffentliche Felder, die
/// Deserialisierung prüft nichts, und f32-Quaternion-Komposition liefert
/// ohnehin keine exakt normierte Viererzahl.
///
/// Der Pin fährt drei Familien und verlangt jeweils **entweder echte
/// Überdeckung oder `None`** — nie eine zu kleine Box:
///
/// 1. die Familie des Reviews (`q = (0,0,0.5,0)`) und weitere grob
///    nichtunitäre Quaternionen;
/// 2. eine realistische f32-Kompositionsfamilie knapp neben `|q| = 1`;
/// 3. entartete Fälle (`q = 0`, `scale = 0`) ⇒ `None`.
#[test]
fn h_cover_survives_non_unit_quaternions_384() {
    use fielddraft_protocol::{Primitive, Transform};
    let he = [1.0f32, 1.0, 1.0];
    let leaf = |t: Transform| {
        crate::bvh::BoundedCsg::Leaf {
            id: fielddraft_protocol::FeatureId::from_u32(1),
            primitive: Primitive::Box { center: [0.0; 3], half_extents: he },
            world_transform: t,
            aabb: crate::csg::Aabb { min: [-1.0e6; 3], max: [1.0e6; 3] },
        }
    };
    // Eine realistische f32-Komposition: mehrfach multiplizierte Rotationen.
    let mut q_comp = [0.0f32, 0.0, 0.0, 1.0];
    for i in 0..40 {
        let a = 0.13f32 * (i as f32 + 1.0);
        let (sn, cs) = (a.sin(), a.cos());
        let inv = 1.0 / 3.0f32.sqrt();
        q_comp = crate::transform::quat_multiply(q_comp, [sn * inv, sn * inv, sn * inv, cs]);
    }
    let ncomp = (q_comp[0] * q_comp[0] + q_comp[1] * q_comp[1] + q_comp[2] * q_comp[2]
        + q_comp[3] * q_comp[3])
        .sqrt();

    let cases: Vec<(&str, [f32; 4], [f32; 3])> = vec![
        ("Review q=(0,0,0.5,0)", [0.0, 0.0, 0.5, 0.0], [1.0, 1.0, 1.0]),
        ("q=(0,0,2,0)", [0.0, 0.0, 2.0, 0.0], [1.0, 1.0, 1.0]),
        ("q=(0.3,0.4,0.5,0.6) unnormiert", [0.3, 0.4, 0.5, 0.6], [1.0, 1.0, 1.0]),
        ("q halbiert", [0.15, 0.2, 0.25, 0.3], [1.0, 1.0, 1.0]),
        ("f32-Komposition", q_comp, [1.0, 1.0, 1.0]),
        ("f32-Komposition + Skalierung", q_comp, [0.25, 3.5, 1.0]),
        ("Einheit + Translation", [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0]),
        // `q = 0`: die Rückwärts-Abbildung entartet zu „untranslate + unscale"
        // — immer noch invertierbar, die Überdeckung ist also BEWEISBAR und wird
        // nicht verweigert. Genau das prüft die Schleife mit.
        ("q = 0 (Identität/Skalierung)", [0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
        ("entartet scale=0", [0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 1.0]),
        ("entartet scale=0 unter Rotation", q_comp, [1.0, 0.0, 1.0]),
    ];
    let mut g = Lcg(0x8ab_9a47);
    let (mut checked, mut refused, mut nonunit) = (0usize, 0usize, 0usize);
    for (name, q, sc) in cases {
        let t = Transform { translate: [3.0, -2.0, 1.5], rotation: q, scale: sc };
        let node = leaf(t.clone());
        let Some(cover) = wit::omega_cover(&node) else {
            refused += 1;
            continue;
        };
        assert_eq!(cover.len(), 1, "{name}: {} Boxen statt einer", cover.len());
        let (bl, bh) = cover[0];
        let qn = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if (qn - 1.0).abs() > 1.0e-6 {
            nonunit += 1;
        }
        // Das Probengebiet kommt aus einer UNABHÄNGIGEN Inversen (Gauss statt
        // Adjunkte): sonst würde der Pin sein Gebiet aus genau dem Code
        // beziehen, den er prüfen soll.
        let Some((slo, shi)) = independent_world_aabb(&t, he) else {
            refused += 1;
            continue;
        };
        let mut inside_seen = 0usize;
        for _ in 0..60_000 {
            let p = [
                (slo[0] + g.next_f64() * (shi[0] - slo[0])) as f32,
                (slo[1] + g.next_f64() * (shi[1] - slo[1])) as f32,
                (slo[2] + g.next_f64() * (shi[2] - slo[2])) as f32,
            ];
            // Mitgliedschaft nach DERSELBEN Semantik, die `box_outside_omega`
            // liest: `world_to_local`, dann die lokalen Halbachsen.
            let l = crate::transform::world_to_local(&t, p);
            if !(0..3).all(|k| l[k].abs() <= he[k]) {
                continue;
            }
            inside_seen += 1;
            assert!(
                (0..3).all(|k| (p[k] as f64) >= bl[k] && (p[k] as f64) <= bh[k]),
                "{name} (|q| = {qn}): Ω-Punkt {p:?} liegt AUSSERHALB der Überdeckungsbox \
                 {bl:?}..{bh:?} — die Überdeckung überdeckt nicht"
            );
        }
        assert!(
            inside_seen > 100,
            "{name}: nur {inside_seen} Innenpunkte getroffen — der Pin misst dort nichts"
        );
        checked += 1;
    }
    // **Die Gegenprobe**: die ALTE Formel (`quat_rotate(q,·)` als Umkehrung von
    // `quat_rotate(conj q,·)`) MUSS an mindestens einer nichtunitären Familie
    // eine zu kleine Box liefern — sonst hätte der Blocker nie bestanden und
    // der Pin bewachte nichts.
    let old_box = |t: &Transform, he: [f32; 3]| -> ([f64; 3], [f64; 3]) {
        let q = [
            t.rotation[0] as f64,
            t.rotation[1] as f64,
            t.rotation[2] as f64,
            t.rotation[3] as f64,
        ];
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for cx in 0..8 {
            let l = [
                if cx & 1 == 0 { -he[0] as f64 } else { he[0] as f64 },
                if cx & 2 == 0 { -he[1] as f64 } else { he[1] as f64 },
                if cx & 4 == 0 { -he[2] as f64 } else { he[2] as f64 },
            ];
            let v = [l[0] * t.scale[0] as f64, l[1] * t.scale[1] as f64, l[2] * t.scale[2] as f64];
            let tv = [
                2.0 * (q[1] * v[2] - q[2] * v[1]),
                2.0 * (q[2] * v[0] - q[0] * v[2]),
                2.0 * (q[0] * v[1] - q[1] * v[0]),
            ];
            let w = [
                v[0] + q[3] * tv[0] + q[1] * tv[2] - q[2] * tv[1] + t.translate[0] as f64,
                v[1] + q[3] * tv[1] + q[2] * tv[0] - q[0] * tv[2] + t.translate[1] as f64,
                v[2] + q[3] * tv[2] + q[0] * tv[1] - q[1] * tv[0] + t.translate[2] as f64,
            ];
            for k in 0..3 {
                lo[k] = lo[k].min(w[k]);
                hi[k] = hi[k].max(w[k]);
            }
        }
        (lo, hi)
    };
    let t_rev = Transform {
        translate: [3.0, -2.0, 1.5],
        rotation: [0.0, 0.0, 0.5, 0.0],
        scale: [1.0, 1.0, 1.0],
    };
    let (olo, ohi) = old_box(&t_rev, he);
    let mut g3 = Lcg(0x0117_0117);
    let (slo, shi) = independent_world_aabb(&t_rev, he).unwrap();
    let mut escaped = 0usize;
    for _ in 0..60_000 {
        let p = [
            (slo[0] + g3.next_f64() * (shi[0] - slo[0])) as f32,
            (slo[1] + g3.next_f64() * (shi[1] - slo[1])) as f32,
            (slo[2] + g3.next_f64() * (shi[2] - slo[2])) as f32,
        ];
        let l = crate::transform::world_to_local(&t_rev, p);
        if !(0..3).all(|k| l[k].abs() <= he[k]) {
            continue;
        }
        if !(0..3).all(|k| (p[k] as f64) >= olo[k] && (p[k] as f64) <= ohi[k]) {
            escaped += 1;
        }
    }
    assert!(
        escaped > 100,
        "die ALTE Quaternion-Formel überdeckt die Review-Familie q=(0,0,0.5,0) auch — dann \
         war Blocker 1 keiner und dieser Pin bewacht nichts ({escaped} Ausreisser)"
    );

    assert!(checked >= 6, "nur {checked} Familien geprüft");
    assert!(nonunit >= 4, "nur {nonunit} echt nichtunitäre Familien");
    assert!(
        refused >= 2,
        "die entarteten Fälle wurden NICHT verweigert ({refused}) — die Determinanten-Sperre \
         greift dann nie"
    );
    println!(
        "§8ab Pin 7: {checked} Familien überdeckt ({nonunit} nichtunitär), {refused} verweigert; \
         |q_komposition| − 1 = {:.3e}; die alte Formel liess {escaped} Ω-Punkte entkommen",
        ncomp as f64 - 1.0
    );
}

/// **§8ab-Pin 8 (Blocker 2): die aktive Domäne folgt aus dem f32-Ausstieg.**
///
/// `channel_limit` prüft `fl(norm_f32(fl(p − bound_c)) − bound_r) < trust`.
/// Daraus die REELLE Ungleichung `|p − bound_c| < bound_r + trust` zu folgern,
/// war der zweite Blocker: jede Op der Kette kann unterschätzen.
///
/// Geprüft an Punkten unmittelbar beiderseits des Ausstiegs (per Bisektion
/// aufgesucht), bei grosser Translation und bei auseinanderliegenden
/// Exponenten von `p` und `bound_c`. **Die Mutation ist die naive Schranke**
/// `bound_r + trust`: sie MUSS an mindestens einem Punkt der aktiven Domäne
/// reissen, sonst trägt die Brücke nichts.
#[test]
fn h_active_span_bridges_the_f32_early_out_384() {
    let m = [0.0f32, 0.0, 1.0];
    let mut cases: Vec<(&str, OffsetChannel)> = Vec::new();
    for (name, a, b, bc, br, tr) in [
        ("nah am Ursprung", [0.0f32, 0.0, 0.0], [1.0f32, 0.0, 0.0], [0.0f32; 3], 1.0f32, 3.91f32),
        (
            "grosse Translation",
            [1.0e4, -3.0e3, 7.0e3],
            [1.0e4 + 40.0, -3.0e3, 7.0e3],
            [1.0e4, -3.0e3, 7.0e3],
            60.0,
            120.0,
        ),
        (
            "gemischte Exponenten",
            [1.0e5, 1.0e-3, -2.0e4],
            [1.0e5 + 5.0, 1.0e-3, -2.0e4],
            [1.0e5, 0.0, -2.0e4],
            250.0,
            400.0,
        ),
    ] {
        let mut ch = synth_channel(vec![seg_arc(a, b, m, m, m, m)], true);
        ch.bound_c = bc;
        ch.bound_r = br;
        ch.trust = tr;
        cases.push((name, ch));
    }
    let (mut probes, mut naive_broken) = (0usize, 0usize);
    let mut worst_naive = f64::NEG_INFINITY;
    let mut worst_use = 0.0f64;
    let mut g = Lcg(0x8ab_5ba7);
    for (name, ch) in &cases {
        let span = wit::active_span(ch).expect("active_span");
        let naive = ch.bound_r as f64 + ch.trust as f64;
        assert!(span >= naive, "{name}: die Brücke ist ENGER als die naive Schranke");
        // Der f32-Ausstieg, nachgebaut wie in `channel_limit`.
        let active = |p: [f32; 3]| -> bool {
            let d = [p[0] - ch.bound_c[0], p[1] - ch.bound_c[1], p[2] - ch.bound_c[2]];
            let nb = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            nb - ch.bound_r < ch.trust
        };
        let real = |p: [f32; 3]| -> f64 {
            let mut s = 0.0f64;
            for k in 0..3 {
                let d = p[k] as f64 - ch.bound_c[k] as f64;
                s = crate::offset_interval::next_up(s + crate::offset_interval::next_up(d * d));
            }
            crate::offset_interval::next_up(s.sqrt())
        };
        for _ in 0..400 {
            let d = [g.sym(), g.sym(), g.sym()];
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if !(n > 1.0e-9) {
                continue;
            }
            let dir = [d[0] / n, d[1] / n, d[2] / n];
            let at = |t: f64| -> [f32; 3] {
                [
                    (ch.bound_c[0] as f64 + dir[0] * t) as f32,
                    (ch.bound_c[1] as f64 + dir[1] * t) as f32,
                    (ch.bound_c[2] as f64 + dir[2] * t) as f32,
                ]
            };
            // Bisektion auf den Ausstieg: aktiv ⟶ inaktiv.
            let (mut lo, mut hi) = (0.0f64, naive * 2.0 + 10.0);
            if !active(at(lo)) || active(at(hi)) {
                continue;
            }
            for _ in 0..200 {
                let mid = 0.5 * (lo + hi);
                if active(at(mid)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            // Beiderseits der Grenze prüfen — nur die AKTIVE Seite ist Gegenstand.
            for t in [lo, hi, 0.5 * (lo + hi), lo * (1.0 - 1.0e-9), lo * (1.0 + 1.0e-9)] {
                let p = at(t);
                if !active(p) {
                    continue;
                }
                let r = real(p);
                assert!(
                    r <= span,
                    "{name}: |p − bound_c| = {r:.9} > span = {span:.9} bei einem AKTIVEN Punkt"
                );
                if r > naive {
                    naive_broken += 1;
                    worst_naive = worst_naive.max(r - naive);
                }
                worst_use = worst_use.max(r / span);
                probes += 1;
            }
        }
    }
    assert!(probes > 1_000, "nur {probes} Proben an der Ausstiegsgrenze");
    assert!(
        naive_broken > 0,
        "die naive Schranke `bound_r + trust` wurde an KEINEM der {probes} aktiven Punkte \
         verletzt — dann trägt die Rundungsbrücke der aktiven Domäne nichts und der Pin \
         behauptet nur"
    );
    println!(
        "§8ab Pin 8: {probes} Punkte an der f32-Ausstiegsgrenze; die naive Schranke reisst an \
         {naive_broken} (max {worst_naive:.3e} mm darüber), die Brücke hält überall \
         (schärfste Ausnutzung {:.6})",
        worst_use
    );
}

/// **§8ab-Pin 9 (Blocker 3): ein belegter Slot darf den Sentinel nie erreichen.**
///
/// Das Argument „ein Sentinel kann den `min` nicht gewinnen" ruhte auf der
/// ungeprüften Aussage „jeder belegte Slot bleibt unter `1e30`". Bei grossen,
/// aber völlig gültigen Koordinaten stimmt das nicht — dann gewinnt der
/// unbelegte Sentinel, und für DEN gibt es keinen Halbraumsatz.
///
/// Der Pin fährt genau diesen Fall: gemischter Sentinel neben einem belegten
/// Slot, bei wachsendem `trust`/`bound_c`. Unterhalb der Schwelle muss ein
/// Budget entstehen, oberhalb MUSS es verweigert werden.
#[test]
fn h_budget_refuses_slots_that_reach_the_sentinel_384() {
    let m = [0.0f32, 0.0, 1.0];
    let cover = wit::omega_cover(&BoundedCsg::Empty).unwrap();
    let mk = |scale: f32| -> OffsetChannel {
        let mut ch = synth_channel(
            vec![seg_arc([0.0, 0.0, 0.0], [scale.max(1.0), 0.0, 0.0], m, m, m, m)],
            true,
        );
        ch.bound_c = [scale, scale, scale];
        ch.bound_r = scale;
        ch.trust = scale;
        ch.face_planes = [crate::offset_channel::END_PLANE_NONE; 4];
        ch.face_planes[0] = [1.0, 0.0, 0.0, -scale];
        ch
    };
    let (mut ok, mut refused) = (0usize, 0usize);
    let mut boundary: Option<f32> = None;
    for e in (0..126u32).step_by(3) {
        let scale = (2.0f32).powi(e as i32);
        if !scale.is_finite() {
            break;
        }
        let ch = mk(scale);
        match wit::h_budget(&ch, &cover, Default::default()) {
            Ok(b) => {
                assert_eq!(b.slots(), 1, "der Sentinel wurde mitgezählt");
                // Wo ein Budget entsteht, MUSS der belegte Slot den min auch
                // wirklich gewinnen können.
                let p = [scale, scale, scale];
                let g = oc::face_plane_gate(p, &ch);
                assert!(
                    g < 1.0e30,
                    "scale = {scale:e}: Budget geprägt, aber der Sentinel gewinnt den min ({g})"
                );
                ok += 1;
            }
            Err(wit::HStatus::NoBudget) | Err(wit::HStatus::Degenerate) => {
                if boundary.is_none() {
                    boundary = Some(scale);
                }
                refused += 1;
            }
            Err(other) => panic!("scale = {scale:e}: unerwarteter Ausgang {other:?}"),
        }
    }
    assert!(ok > 5, "nur {ok} Grössen mit Budget");
    assert!(
        refused > 0,
        "bei KEINER der geprüften Grössen wurde verweigert — dann greift die \
         Sentinel-Schranke nie und der Pin misst nichts"
    );
    println!(
        "§8ab Pin 9: {ok} Grössen mit Budget, ab scale = {:?} verweigert ({refused} Fälle)",
        boundary
    );
}

// ══════════════ #384 §8ac / P5b-1A — die Produktions-Aktivierung ══════════════

/// Zählt die tatsächlich AUFGELÖSTEN Laufzeitmodi eines synthetisierten Feldes.
fn mode_tally(field: &BoundedCsg) -> (usize, usize, std::collections::BTreeMap<String, usize>) {
    let mut stops = Vec::new();
    stop_nodes(field, &mut stops);
    let (mut proved, mut legacy) = (0usize, 0usize);
    let mut why: std::collections::BTreeMap<String, usize> = Default::default();
    for st in stops {
        if let BoundedCsg::ArcChannel { channels, .. } = st {
            for ch in channels {
                match ch.limit_mode() {
                    crate::offset_channel::ChannelLimitMode::Proved { .. } => proved += 1,
                    crate::offset_channel::ChannelLimitMode::LegacyUnproven(r) => {
                        legacy += 1;
                        *why.entry(format!("{r:?}")).or_default() += 1;
                    }
                }
            }
        }
    }
    (proved, legacy, why)
}

/// **§8ac-Sonde: welcher Modus kommt in der ECHTEN Synthese heraus?**
#[test]
#[ignore]
fn probe_384_limit_modes() {
    let delta = 2.0f32;
    for (name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        let (p, l, why) = mode_tally(&field);
        println!("| {name} | proved {p} | legacy {l} | {why:?} |");
    }
}

/// Dasselbe Feld mit ALLEN Kanälen auf `LegacyUnproven` — das ALTE Feld,
/// punktgleich vergleichbar. Nur die Modi werden zurückgedreht; jedes andere
/// Bit bleibt, damit der Vergleich wirklich die Feldänderung misst und nicht
/// eine zweite Synthese.
fn field_with_legacy_modes(field: &BoundedCsg) -> BoundedCsg {
    fn go(n: &BoundedCsg) -> BoundedCsg {
        match n {
            BoundedCsg::ArcChannel { inner, channels, verts, rims, lb, aabb } => {
                let mut cs = channels.clone();
                for c in cs.iter_mut() {
                    wit::force_legacy_final_channel(
                        c,
                        crate::offset_channel::LegacyReason::NotCertified,
                    );
                }
                BoundedCsg::ArcChannel {
                    inner: Box::new(go(inner)),
                    channels: cs,
                    verts: verts.clone(),
                    rims: rims.clone(),
                    lb: lb.clone(),
                    aabb: *aabb,
                }
            }
            BoundedCsg::Union { left, right, aabb } => BoundedCsg::Union {
                left: Box::new(go(left)),
                right: Box::new(go(right)),
                aabb: *aabb,
            },
            BoundedCsg::Difference { left, right, aabb } => BoundedCsg::Difference {
                left: Box::new(go(left)),
                right: Box::new(go(right)),
                aabb: *aabb,
            },
            BoundedCsg::Intersection { left, right, aabb } => BoundedCsg::Intersection {
                left: Box::new(go(left)),
                right: Box::new(go(right)),
                aabb: *aabb,
            },
            BoundedCsg::Offset { inner, delta, aabb } => BoundedCsg::Offset {
                inner: Box::new(go(inner)),
                delta: *delta,
                aabb: *aabb,
            },
            other => other.clone(),
        }
    }
    go(field)
}

/// **§8ac-Pin 1 (Richtung): die Aktivierung nimmt kein Material weg.**
///
/// Für konkave Kanäle gilt `lim_neu ≤ lim_alt` — beide Addenden sind `≥ 0`,
/// `min` und die Subtraktion sind monoton. Das ist kein Argument, sondern eine
/// Messung: geprüft wird der Kanal-Beitrag selbst, dann der Stopp-Wert HINTER
/// `OP_STOP_LIMIT` und schliesslich das ausgelieferte Feld — **inklusive** der
/// nachfolgenden Rim-Terme, deren Monotonie damit gezeigt und nicht
/// vorausgesetzt ist.
#[test]
fn activation_only_adds_material_384() {
    let delta = 2.0f32;
    let mut g = Lcg(0x8ac_d132);
    let (mut lim_probes, mut lim_lower) = (0usize, 0usize);
    let (mut field_probes, mut field_lower) = (0usize, 0usize);
    let mut worst_drop = 0.0f64;
    let mut fixtures = 0usize;
    for (name, field, _e, lo, hi, _c) in cert_matrix(delta) {
        let old = field_with_legacy_modes(&field);
        // (a) der Kanal-Beitrag selbst.
        let mut here = 0usize;
        let mut news = Vec::new();
        stop_nodes(&field, &mut news);
        let mut olds = Vec::new();
        stop_nodes(&old, &mut olds);
        assert_eq!(news.len(), olds.len());
        for (sn, so) in news.iter().zip(olds.iter()) {
            let (BoundedCsg::ArcChannel { channels: cn, .. }, BoundedCsg::ArcChannel { channels: co, .. }) =
                (sn, so)
            else {
                continue;
            };
            for (a, b) in cn.iter().zip(co.iter()) {
                assert!(!b.limit_mode().is_proved(), "die Alt-Kopie trägt noch einen Beweis");
                for _ in 0..250 {
                    let p = [
                        (a.bound_c[0] as f64 + g.sym() * (a.bound_r + a.trust) as f64) as f32,
                        (a.bound_c[1] as f64 + g.sym() * (a.bound_r + a.trust) as f64) as f32,
                        (a.bound_c[2] as f64 + g.sym() * (a.bound_r + a.trust) as f64) as f32,
                    ];
                    let ln = oc::channel_limit(p, a);
                    let l0 = oc::channel_limit(p, b);
                    assert!(
                        ln <= l0,
                        "{name}: lim_neu = {ln} > lim_alt = {l0} — die Aktivierung NIMMT \
                         Material weg"
                    );
                    if ln < l0 {
                        lim_lower += 1;
                    }
                    if !a.limit_mode().is_proved() {
                        assert_eq!(
                            ln.to_bits(),
                            l0.to_bits(),
                            "{name}: ein LegacyUnproven-Kanal ist NICHT bitidentisch"
                        );
                    }
                    lim_probes += 1;
                    here += 1;
                }
            }
        }
        // (b) das ausgelieferte Feld — hinter OP_STOP_LIMIT und hinter den Rims.
        for _ in 0..4000 {
            let p = [
                (lo[0] + g.next_f64() * (hi[0] - lo[0])) as f32,
                (lo[1] + g.next_f64() * (hi[1] - lo[1])) as f32,
                (lo[2] + g.next_f64() * (hi[2] - lo[2])) as f32,
            ];
            let fnew = field.sdf(p, EvalMode::Visual);
            let fold = old.sdf(p, EvalMode::Visual);
            if !(fnew.is_finite() && fold.is_finite()) {
                continue;
            }
            assert!(
                fnew <= fold,
                "{name}: f_neu = {fnew} > f_alt = {fold} bei {p:?} — die Aktivierung nimmt \
                 hinter OP_STOP_LIMIT oder in den Rim-Termen Material weg"
            );
            if fnew < fold {
                field_lower += 1;
                worst_drop = worst_drop.max((fold - fnew) as f64);
            }
            field_probes += 1;
        }
        if here > 0 {
            fixtures += 1;
        }
    }
    assert!(fixtures >= 10, "nur {fixtures} Fixturen");
    assert!(lim_probes > 40_000 && field_probes > 40_000, "{lim_probes}/{field_probes} Proben");
    assert!(
        lim_lower > 1_000 && field_lower > 100,
        "nur {lim_lower} abgesenkte Kanal-Beiträge und {field_lower} abgesenkte Feldwerte — \
         dann ist die Aktivierung an diesen Proben ein No-op und der Richtungs-Pin misst nichts"
    );
    println!(
        "§8ac Pin 1: {lim_probes} Kanal-Proben ({lim_lower} abgesenkt), {field_probes} \
         Feld-Proben ({field_lower} abgesenkt), grösste Absenkung {worst_drop:.3e} mm"
    );
}

/// **§8ac-Pin 2 (Attribution): wer sich bewegt, und um wie viel.**
///
/// Alt/Neu auf IDENTISCHEN Punkten, aufgeschlüsselt nach dem Gewinner der
/// ALTEN Form — `de`, `h`, oder der neutrale Trust-/Skip-Pfad. Ein Punkt, an
/// dem der neutrale Pfad gewann, darf sich NICHT bewegen; an einem
/// `h`-Gewinner darf sich nur `A_h` auswirken, an einem `de`-Gewinner nur
/// `A_seam`. Vorzeichenwechsel werden getrennt ausgewiesen — sie sind die
/// einzige Sorte Änderung, die eine Nullmenge verschiebt.
#[test]
fn activation_attribution_harness_384() {
    let delta = 2.0f32;
    let cfg_names = ["de", "h", "neutral"];
    let mut g = Lcg(0x8ac_a771);
    let mut total = [0usize; 3];
    let mut moved = [0usize; 3];
    let mut worst = [0.0f64; 3];
    let (mut sign_flips, mut probes) = (0usize, 0usize);
    for (name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        let old = field_with_legacy_modes(&field);
        let (mut ns, mut os) = (Vec::new(), Vec::new());
        stop_nodes(&field, &mut ns);
        stop_nodes(&old, &mut os);
        for (sn, so) in ns.iter().zip(os.iter()) {
            let (BoundedCsg::ArcChannel { channels: cn, .. }, BoundedCsg::ArcChannel { channels: co, .. }) =
                (sn, so)
            else {
                continue;
            };
            for (a, b) in cn.iter().zip(co.iter()) {
                for i in 0..600 {
                    let arc = &a.arcs[i % a.arcs.len()];
                    let sp = g.next_f64() as f32;
                    let rad = (g.next_f64() * a.trust as f64 * 1.4) as f32;
                    let d = [g.sym(), g.sym(), g.sym()];
                    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if !(n > 1.0e-9) {
                        continue;
                    }
                    let p = [
                        arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n) as f32 * rad,
                        arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n) as f32 * rad,
                        arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n) as f32 * rad,
                    ];
                    let l0 = oc::channel_limit(p, b);
                    let ln = oc::channel_limit(p, a);
                    // Attribution nach dem ALTEN Gewinner.
                    let bucket = match oc::channel_nearest(p, b) {
                        Some((de, _, _, _)) if de.is_finite() && de < b.trust => {
                            let h = oc::face_plane_gate(p, b) + b.delta;
                            if de <= h {
                                0
                            } else {
                                1
                            }
                        }
                        _ => 2,
                    };
                    total[bucket] += 1;
                    probes += 1;
                    if ln != l0 {
                        moved[bucket] += 1;
                        worst[bucket] = worst[bucket].max((l0 - ln) as f64);
                        assert_ne!(bucket, 2, "{name}: ein NEUTRALER Punkt hat sich bewegt");
                        assert!(a.limit_mode().is_proved(), "ein Legacy-Kanal hat sich bewegt");
                    }
                    if (l0 > 0.0) != (ln > 0.0) {
                        sign_flips += 1;
                    }
                }
            }
        }
    }
    assert!(probes > 60_000, "nur {probes} Proben");
    assert!(total[0] > 1_000 && total[1] > 1_000, "die Attribution trifft nicht beide Gewinner");
    assert!(moved[0] > 0 && moved[1] > 0, "einer der beiden Äste bewegt sich nie");
    for k in 0..3 {
        println!(
            "§8ac Pin 2: Alt-Gewinner {:8} — {} Proben, {} bewegt, grösste Absenkung {:.3e} mm",
            cfg_names[k], total[k], moved[k], worst[k]
        );
    }
    println!("§8ac Pin 2: {sign_flips} Vorzeichenwechsel des Kanal-Beitrags");
}

/// **§8ac-Pin 3: der Tape-Kopf ist in allen Lesern gleich lang.**
///
/// Die drei neuen Worte (Modus, `A_seam`, `A_h`) stehen in vier
/// Implementierungen: Emitter und Interpreter in `sdf_compiler.rs`,
/// `payload_len` in `cgal-mesher/src/tape_spec.h` und der TSL-Leser in
/// `web/src/sdfVmInterpreter.ts`. Driftet eine, liest sie die Bogenliste um
/// drei Worte verschoben — und das ist kein Absturz, sondern ein stilles
/// falsches Feld.
///
/// Der Pin misst die EMITTIERTE Länge gegen `CHANNEL_HEADER` und verankert
/// damit die Konstante, die die anderen drei spiegeln.
#[test]
fn channel_tape_header_stride_384() {
    use crate::sdf_compiler::{compile_body, ARC_STRIDE, CHANNEL_HEADER, OP_ARC_CHANNEL};
    let delta = 2.0f32;
    let mut seen = 0usize;
    for (name, field, _e, _l, _h, _c) in cert_matrix(delta) {
        let t = compile_body(&field);
        let ops = &t.opcodes;
        let mut i = 0usize;
        while i < ops.len() {
            let tag = ops[i].to_bits() & 0xffff_ffff;
            if tag == OP_ARC_CHANNEL {
                let n = ops[i + 1] as usize;
                // Modus + Addenden sitzen an den letzten drei Kopf-Worten.
                let mode = ops[i + 1 + CHANNEL_HEADER - 3];
                let a_seam = ops[i + 1 + CHANNEL_HEADER - 2];
                let a_h = ops[i + 1 + CHANNEL_HEADER - 1];
                assert!(
                    mode == 0.0 || mode == 1.0,
                    "{name}: Modus-Wort {mode} — der Kopf ist verschoben"
                );
                assert!(
                    a_seam >= 0.0 && a_h >= 0.0 && a_seam < 1.0 && a_h < 1.0,
                    "{name}: Addenden {a_seam}/{a_h} an der Kopf-Position — verschoben?"
                );
                if mode == 0.0 {
                    assert_eq!(
                        (a_seam.to_bits(), a_h.to_bits()),
                        (0f32.to_bits(), 0f32.to_bits()),
                        "{name}: LegacyUnproven mit Addenden im Tape"
                    );
                } else {
                    assert!(a_seam > 0.0, "{name}: Proved mit Naht-Addend null");
                }
                // Das nächste Wort nach Kopf + Bögen muss wieder ein Opcode
                // sein — die schärfste Längenprobe, die von hier aus geht.
                let next = i + 1 + CHANNEL_HEADER + n * ARC_STRIDE;
                assert!(next <= ops.len(), "{name}: Kanal-Record läuft über das Tape hinaus");
                seen += 1;
                i = next;
                continue;
            }
            i += 1;
        }
    }
    assert!(seen >= 200, "nur {seen} Kanal-Records im Tape geprüft");
    println!("§8ac Pin 3: {seen} Kanal-Records, Kopf {CHANNEL_HEADER} + {ARC_STRIDE}·N");
}

/// **§8ac-Pin 4 (Sollte-14): die adversarialen h-Familien am ECHTEN Feld.**
///
/// §8ab konnte den `h`-Ast nur rechnerisch prüfen — es gab keinen Konsumenten.
/// Jetzt gibt es einen, und damit ist die Forderung aus Review 13 einlösbar:
/// die Familien am ausgelieferten Feld, **inklusive des Umhäng-Bands**
/// `de < h ≤ de + A`, in dem P5b-1A Punkte vom `de`- in den `h`-Ast schiebt.
///
/// Verlangt ist an jedem Punkt INNERHALB `Ω`, dass der ausgelieferte
/// Kanal-Beitrag die gültige Untergrenze nicht überschreitet:
/// `lim ≤ δ − dist(x,∁Ω)`, gemessen gegen das analytische Orakel.
#[test]
fn activation_h_families_hold_on_the_real_field_384() {
    let delta = 2.0f32;
    let mut g = Lcg(0x8ac_47f5);
    let (mut probes, mut band, mut hwin) = (0usize, 0usize, 0usize);
    let mut tightest = f64::INFINITY;
    for (name, field, exact, _l, _h, _c) in cert_matrix(delta) {
        let mut stops = Vec::new();
        stop_nodes(&field, &mut stops);
        for st in stops {
            let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
            for ch in channels {
                if !ch.limit_mode().is_proved() {
                    continue;
                }
                let (a_seam, _a_h) = ch.limit_mode().addends();
                let arc = ch.arcs[0].clone();
                // Gezielt ins Umhäng-Band: Bisektion auf `h − de = 0`.
                for _ in 0..80 {
                    let sp = g.next_f64() as f32;
                    let d = [g.sym(), g.sym(), g.sym()];
                    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    if !(n > 1.0e-9) {
                        continue;
                    }
                    let at = |t: f64| -> Option<([f32; 3], f32, f32)> {
                        let p = [
                            arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n * t) as f32,
                            arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n * t) as f32,
                            arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n * t) as f32,
                        ];
                        let (de, _, _, _) = oc::channel_nearest(p, ch)?;
                        let h = oc::face_plane_gate(p, ch) + ch.delta;
                        (de.is_finite() && de < ch.trust && h.is_finite() && h < 1.0e29)
                            .then_some((p, de, h))
                    };
                    let (mut lo, mut hi) = (0.0f64, 0.0f64);
                    let mut prev: Option<(f64, f32)> = None;
                    let mut found = false;
                    for k in 1..=64 {
                        let t = k as f64 / 64.0 * ch.trust as f64 * 0.98;
                        let Some((_p, de, h)) = at(t) else { continue };
                        let sg = h - de;
                        if let Some((t0, s0)) = prev {
                            if (s0 > 0.0) != (sg > 0.0) {
                                lo = t0;
                                hi = t;
                                found = true;
                                break;
                            }
                        }
                        prev = Some((t, sg));
                    }
                    if !found {
                        continue;
                    }
                    for _ in 0..80 {
                        let mid = 0.5 * (lo + hi);
                        let (Some((p, de, h)), Some((_q, de0, h0))) = (at(mid), at(lo)) else {
                            break;
                        };
                        if ((h0 - de0) > 0.0) == ((h - de) > 0.0) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                        let dist = exact([p[0] as f64, p[1] as f64, p[2] as f64]);
                        if dist > 0.0 {
                            let lim = oc::channel_limit(p, ch) as f64;
                            let valid = ch.delta as f64 - dist;
                            assert!(
                                lim <= valid + 1.0e-12,
                                "{name}: der AUSGELIEFERTE Kanal-Beitrag {lim:e} liegt über der \
                                 gültigen Untergrenze {valid:e} bei {p:?}"
                            );
                            tightest = tightest.min(valid - lim);
                            probes += 1;
                            if h > de && h <= de + a_seam {
                                band += 1;
                            }
                            if h <= de {
                                hwin += 1;
                            }
                        }
                        if h > de && h <= de + a_seam {
                            break;
                        }
                    }
                }
            }
        }
    }
    assert!(probes > 2_000, "nur {probes} Proben in Ω");
    assert!(band > 0, "das Umhäng-Band wurde am ECHTEN Feld nie getroffen");
    assert!(hwin > 0, "der h-Ast gewinnt am echten Feld nie");
    println!(
        "§8ac Pin 4: {probes} Proben in Ω am ausgelieferten Feld; {band} im Umhäng-Band, \
         {hwin} mit h-Gewinn; engste Reserve {tightest:.3e} mm"
    );
}

/// Ersetzt die Kanalliste des ersten `ArcChannel`-Knotens durch genau einen
/// Kanal — damit der Tape-Emitter wirklich DIESEN Kanal schreibt.
fn field_with_single_channel(field: &BoundedCsg, ch: OffsetChannel) -> BoundedCsg {
    fn go(n: &BoundedCsg, ch: &OffsetChannel, done: &mut bool) -> BoundedCsg {
        match n {
            BoundedCsg::ArcChannel { inner, channels, verts, rims, lb, aabb } if !*done => {
                *done = true;
                let _ = channels;
                BoundedCsg::ArcChannel {
                    inner: inner.clone(),
                    channels: vec![ch.clone()],
                    verts: verts.clone(),
                    rims: rims.clone(),
                    lb: lb.clone(),
                    aabb: *aabb,
                }
            }
            BoundedCsg::Union { left, right, aabb } => BoundedCsg::Union {
                left: Box::new(go(left, ch, done)),
                right: Box::new(go(right, ch, done)),
                aabb: *aabb,
            },
            BoundedCsg::Difference { left, right, aabb } => BoundedCsg::Difference {
                left: Box::new(go(left, ch, done)),
                right: Box::new(go(right, ch, done)),
                aabb: *aabb,
            },
            BoundedCsg::Intersection { left, right, aabb } => BoundedCsg::Intersection {
                left: Box::new(go(left, ch, done)),
                right: Box::new(go(right, ch, done)),
                aabb: *aabb,
            },
            BoundedCsg::Offset { inner, delta, aabb } => BoundedCsg::Offset {
                inner: Box::new(go(inner, ch, done)),
                delta: *delta,
                aabb: *aabb,
            },
            other => other.clone(),
        }
    }
    let mut done = false;
    let out = go(field, &ch, &mut done);
    assert!(done, "das Feld hat keinen ArcChannel-Knoten");
    out
}

/// **§8ac-Pin 5 (Blocker des siebzehnten Reviews): ein mutierter Kanal
/// verliert seinen Modus — in der Produktion UND im Tape.**
///
/// Der gespeicherte Modus allein war nicht sicher: die beweisrelevanten Felder
/// sind öffentlich mutierbar, und `limit_mode()` gab den einmal aufgelösten
/// Modus unverändert zurück. Wer `trust` vergrösserte, erweiterte den
/// Gültigkeitsbereich über das bewiesene Reichweiten- und Rundungsbudget
/// hinaus — und bekam die alten, jetzt zu kleinen Addenden von allen sechs
/// Spiegeln geliefert.
///
/// Geprüft wird die volle Kette, nicht nur der Getter: Laufzeitwert
/// (`channel_limit`), Tape-Wort (Emitter) und Bericht müssen gemeinsam auf
/// Legacy fallen. Ausdrücklich dabei: eine `trust`-Mutation und eine BELEGTE
/// `face_plane` — nicht nur Metadaten.
#[test]
fn activation_mutation_invalidates_the_runtime_mode_384() {
    use crate::sdf_compiler::{compile_body, CHANNEL_HEADER, OP_ARC_CHANNEL};
    let delta = 2.0f32;
    let (_n, field, _e, _l, _h, _c) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Ecken-Schale t/δ=2.5"))
        .unwrap();
    // Ein Kanal mit bewiesenem Modus und einer belegten Fremdebene.
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let mut base: Option<OffsetChannel> = None;
    for st in &stops {
        if let BoundedCsg::ArcChannel { channels, .. } = st {
            for c in channels {
                if c.limit_mode().is_proved()
                    && c.face_planes.iter().any(|g| g[3] != -crate::offset_channel::END_PLANE_OFF)
                {
                    base = Some(c.clone());
                    break;
                }
            }
        }
    }
    let base = base.expect("kein bewiesener Kanal mit belegter Fremdebene");
    let slot = base
        .face_planes
        .iter()
        .position(|g| g[3] != -crate::offset_channel::END_PLANE_OFF)
        .unwrap();
    let (a0, h0) = base.limit_mode().addends();
    assert!(a0 > 0.0 && h0 > 0.0, "die Ausgangs-Addenden sind null — der Pin misst nichts");

    // Probe-Punkte, an denen der Term wirklich bindet.
    let mut g = Lcg(0x8ac_9117);
    let mut pts = Vec::new();
    while pts.len() < 400 {
        let arc = &base.arcs[pts.len() % base.arcs.len()];
        let sp = g.next_f64() as f32;
        let rad = (g.next_f64() * base.trust as f64 * 0.9) as f32;
        let d = [g.sym(), g.sym(), g.sym()];
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if !(n > 1.0e-9) {
            continue;
        }
        let p = [
            arc.c[0] + arc.u[0] * (sp * arc.r) + (d[0] / n) as f32 * rad,
            arc.c[1] + arc.u[1] * (sp * arc.r) + (d[1] / n) as f32 * rad,
            arc.c[2] + arc.u[2] * (sp * arc.r) + (d[2] / n) as f32 * rad,
        ];
        if oc::channel_limit(p, &base) > -1.0e29 {
            pts.push(p);
        }
    }

    // §8ac-Korrektur (achtzehntes Review, Punkt 5): der Tape-Teil hat vorher
    // KEIN Tape gemessen — er kompilierte das UNMUTIERTE Feld, warf das
    // Ergebnis weg und rekonstruierte Modus/Addenden wieder aus
    // `ch.limit_mode()`. Jetzt wird der mutierte Kanal wirklich in den
    // `ArcChannel`-Knoten eingesetzt, kompiliert, und die drei Kopf-Worte
    // werden aus dem emittierten Tape GELESEN.
    let tape_mode = |ch: &OffsetChannel| -> (f32, f32, f32) {
        let f = field_with_single_channel(&field, ch.clone());
        let t = compile_body(&f);
        let ops = &t.opcodes;
        let mut i = 0usize;
        while i < ops.len() {
            if ops[i].to_bits() & 0xffff_ffff == OP_ARC_CHANNEL {
                let n = ops[i + 1] as usize;
                let b = i + 1 + CHANNEL_HEADER - 3;
                return (ops[b], ops[b + 1], ops[b + 2]);
            }
            i += 1;
        }
        panic!("kein OP_ARC_CHANNEL im Tape — der Pin misst kein Tape");
    };

    let mut cases = 0usize;
    for (why, mutate) in [
        ("trust vergrössert", 0usize),
        ("trust verkleinert", 1),
        ("belegte face_plane verschoben", 2),
        ("belegte face_plane-Normale gekippt", 3),
        ("delta geändert", 4),
        ("Bogen-Radius geändert", 5),
        ("bound_r geändert", 6),
    ] {
        let mut m = base.clone();
        match mutate {
            0 => m.trust = ulp_off(m.trust, 4096),
            1 => m.trust = ulp_off(m.trust, -4096),
            2 => m.face_planes[slot][3] += 1.0e-3,
            3 => m.face_planes[slot][0] = ulp_off(m.face_planes[slot][0].abs().max(0.5), 64),
            4 => m.delta = ulp_off(m.delta, 8),
            5 => m.arcs[0].r = ulp_off(m.arcs[0].r, 8),
            _ => m.bound_r = ulp_off(m.bound_r, 64),
        }
        // (1) der gespeicherte Modus ist noch Proved — genau das war die Lücke.
        assert!(
            m.cached_limit_mode().is_proved(),
            "{why}: der Pin misst nichts, wenn schon das Feld den Modus löscht"
        );
        // (2) der NACHGEPRÜFTE Modus ist Legacy(ProofInvalidated).
        assert_eq!(
            m.limit_mode(),
            crate::offset_channel::ChannelLimitMode::LegacyUnproven(
                crate::offset_channel::LegacyReason::ProofInvalidated
            ),
            "{why}: der mutierte Kanal gilt weiter als bewiesen"
        );
        // (3) Produktion: die Addenden sind weg.
        let (s, hh) = m.limit_mode().addends();
        assert_eq!((s, hh), (0.0, 0.0), "{why}: die Produktion liest weiter Addenden");
        // (4) Tape: Modus-Wort 0, Addenden 0.
        assert_eq!(tape_mode(&m), (0.0, 0.0, 0.0), "{why}: das Tape trägt weiter den Beweis");
        // (5) und der Feldwert ist der Legacy-Wert.
        for p in pts.iter().take(60) {
            let mut legacy = m.clone();
            wit::force_legacy_final_channel(
                &mut legacy,
                crate::offset_channel::LegacyReason::NotCertified,
            );
            assert_eq!(
                oc::channel_limit(*p, &m).to_bits(),
                oc::channel_limit(*p, &legacy).to_bits(),
                "{why}: der Feldwert ist nicht der Legacy-Wert"
            );
        }
        cases += 1;
    }
    assert_eq!(cases, 7);

    // (6) Eine ERFOLGLOSE Rezertifizierung muss den Modus ebenfalls löschen.
    let om = {
        let v = omega_and_channels(&field);
        v.into_iter().next().unwrap().0 as *const BoundedCsg
    };
    let om: &BoundedCsg = unsafe { &*om };
    let mut recert = base.clone();
    assert!(recert.cached_limit_mode().is_proved());
    // Gekrümmt ⇒ `certify_channel` scheitert und löscht den Beweis.
    recert.arcs[0].sweep = 1.0;
    let (st, _) = wit::certify_channel(om, &mut recert, wit::WitnessCfg::default());
    assert_eq!(st, wit::SeamProofStatus::Curved);
    assert!(
        !recert.cached_limit_mode().is_proved(),
        "eine erfolglose Rezertifizierung liess den GESPEICHERTEN Modus stehen"
    );
    assert!(!recert.limit_mode().is_proved());

    // (7) Der unveränderte Kanal behält seinen Modus — sonst wäre der Pin
    //     grün, weil er ALLES invalidiert.
    let intact = base.clone();
    assert!(intact.limit_mode().is_proved(), "der unveränderte Kanal verlor seinen Modus");
    assert_eq!(intact.limit_mode().addends(), (a0, h0));
    println!(
        "§8ac Pin 5: 7 Mutationen + erfolglose Rezertifizierung entziehen den Modus; \
         unveränderter Kanal behält A_seam = {a0:.3e}, A_h = {h0:.3e}"
    );
}

/// Die Fixture des GPU-Paritäts-Specs, in Rust: zwei überlappende Boxen,
/// Union, dann `−2 mm` Erosion. Die konkave Naht, an der der Turm auf die
/// Platte trifft, synthetisiert genau die GERADEN Kanäle, die P5b
/// zertifiziert.
fn eroded_union_node(delta: f32) -> CsgNode {
    CsgNode::Offset(
        Box::new(CsgNode::Union(
            Box::new(box_at(1, [0.0, 0.0, 0.0], [20.0, 20.0, 10.0])),
            Box::new(box_at(2, [0.0, 0.0, 15.0], [10.0, 10.0, 20.0])),
        )),
        -delta,
    )
}

/// **§8ac-Pin 6 (Sollte 3b/3d): die Aktivierung ist auf DIESER Fixture
/// messbar — und fidget zieht mit.**
///
/// Der GPU-Spec konnte bisher bestehen, ohne die Feldänderung zu berühren:
/// er verlangte nur `channels > 0` und tolerierte `1e-2` — mehr als die ganze
/// P5b-Änderung (`2.267·10⁻³`). Ein TSL-Spiegel, der beide Addenden ignoriert,
/// wäre durchgekommen.
///
/// Dieser Pin liefert die fehlende Substanz:
///
/// 1. die Fixture trägt wirklich `Proved`-Kanäle mit Addenden `> 0`;
/// 2. es gibt benannte Punkte, an denen Neu und eine Legacy-Ablation
///    **messbar** auseinanderliegen — genau die Punkte, die der Spec dann
///    GPU-gegen-CPU prüft;
/// 3. der fidget-Baum spiegelt die neue Kette an denselben Punkten (die
///    bestehenden fidget-Pins fahren eine BOGEN-Fixture und belegen P5b
///    nicht — Bögen bekommen gar keinen Beweis).
#[test]
fn activation_is_measurable_on_the_gpu_fixture_384() {
    use fidget::context::Tree;
    let delta = 2.0f32;
    let (field, _) = compile_for(eroded_union_node(delta));
    let (proved, legacy, why) = mode_tally(&field);
    assert!(
        proved > 0,
        "die GPU-Fixture synthetisiert KEINEN bewiesenen Kanal ({proved}/{legacy}, {why:?}) — \
         dann belegt der Spec die Feldänderung nicht"
    );
    let mut amax = (0.0f32, 0.0f32);
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    for st in &stops {
        let BoundedCsg::ArcChannel { channels, .. } = st else { continue };
        for c in channels {
            let (s, h) = c.limit_mode().addends();
            amax = (amax.0.max(s), amax.1.max(h));
        }
    }
    // `A_seam > 0` ist die tragende Forderung. `A_h` ist auf DIESER Fixture
    // null, und das ist kein Mangel: ihre Kanäle haben keine belegte
    // Fremdebene, das h-Budget ist also `Vacuous` (der `h`-Ast kann gar nicht
    // binden, `face_plane_gate` liefert den Sentinel). Ehrlich ausgewiesen
    // statt weggerundet — wer den `h`-Ast am echten Feld sehen will, findet
    // ihn in `activation_h_families_hold_on_the_real_field_384`.
    assert!(amax.0 > 0.0, "die Fixture trägt keinen positiven Naht-Addenden ({amax:?})");
    let h_note = if amax.1 > 0.0 { "belegt" } else { "vacuous (keine Fremdebene)" };

    // Punkte, an denen Neu und Legacy messbar auseinanderliegen.
    let old = field_with_legacy_modes(&field);
    let mut g = Lcg(0x8ac_67a0);
    let mut hits: Vec<([f32; 3], f64)> = Vec::new();
    for _ in 0..400_000 {
        let p = [
            (g.sym() * 22.0) as f32,
            (g.sym() * 22.0) as f32,
            (g.sym() * 36.0) as f32,
        ];
        let a = field.sdf(p, EvalMode::Visual) as f64;
        let b = old.sdf(p, EvalMode::Visual) as f64;
        if !(a.is_finite() && b.is_finite()) {
            continue;
        }
        let d = b - a;
        if d > 1.0e-4 {
            hits.push((p, d));
        }
        if hits.len() >= 24 {
            break;
        }
    }
    assert!(
        hits.len() >= 8,
        "nur {} Punkte mit messbarer Differenz (> 1e-4) — dann kann der GPU-Spec die \
         Feldänderung nicht sehen",
        hits.len()
    );

    // fidget an denselben Punkten.
    let mut ctx = fidget::context::Context::new();
    let tree = crate::fidget_bridge::node_to_tree(&field, &[Tree::x(), Tree::y(), Tree::z()])
        .expect("fidget-Baum der GPU-Fixture");
    let root = ctx.import(&tree);
    let mut checked = 0usize;
    let mut worst = 0.0f64;
    for (p, _) in &hits {
        let cpu = field.sdf(*p, EvalMode::Visual) as f64;
        let f = ctx
            .eval_xyz(root, p[0] as f64, p[1] as f64, p[2] as f64)
            .expect("fidget-Auswertung");
        assert!(
            (f - cpu).abs() < 1.0e-4,
            "fidget {f} gegen CPU {cpu} bei {p:?} — der symbolische Spiegel hat die Addenden \
             nicht übernommen"
        );
        worst = worst.max((f - cpu).abs());
        checked += 1;
    }
    assert!(checked >= 8);
    println!(
        "§8ac Pin 6: GPU-Fixture — {proved} Proved / {legacy} Legacy, A_seam ≤ {:.3e}, \
         A_h ≤ {:.3e} ({h_note}); {} Punkte mit Δ > 1e-4 (grösste {:.3e} mm); \
         fidget-Parität {worst:.2e}",
        amax.0,
        amax.1,
        hits.len(),
        hits.iter().map(|h| h.1).fold(0.0, f64::max)
    );
    for (p, d) in hits.iter().take(8) {
        println!("§8ac Pin 6 Punkt: [{}, {}, {}] Δ={:.6e}", p[0], p[1], p[2], d);
    }
}

/// **§8ac-Pin 7 (Blocker des achtzehnten Reviews): ein GEFÄLSCHTER Digest
/// rettet den Modus nicht.**
///
/// Die erste Reparatur des Freeze-Problems verglich einen FNV-64-Digest über
/// die beweisrelevanten Kanaldaten. Der ist **deterministisch** umgehbar —
/// kein Zufallstreffer nötig:
///
/// ```text
/// D = (q ⊕ parents[1]) · P  mod 2⁶⁴,   P = 0x100000001b3 (ungerade)
/// ```
///
/// `P` ist ungerade, also mod 2⁶⁴ invertierbar, und `parents[1]` ist ein
/// ÖFFENTLICHES `u64`, das als LETZTES Wort gehasht wird. Nach einer beliebigen
/// beweisrelevanten Mutation kann man
/// `parents[1]' = q' ⊕ (D_alt · P⁻¹ mod 2⁶⁴)` setzen und bekommt exakt
/// `D_neu = D_alt` — der Modus bliebe `Proved`, und die alten, jetzt zu kleinen
/// Addenden flössen wieder in Produktion und Tape.
///
/// Dieser Pin BAUT diesen Angriff (inklusive der FNV-Inversen via
/// Newton-Iteration) und verlangt, dass der Kanal trotzdem auf Legacy fällt —
/// weil seit v0.65.2 der **exakte** Schnappschuss verglichen wird und nicht
/// mehr ein Hash. Er steht für immer als Begründung dafür, warum hier kein
/// Hash reicht.
#[test]
fn activation_forged_digest_still_falls_back_384() {
    use crate::sdf_compiler::{compile_body, CHANNEL_HEADER, OP_ARC_CHANNEL};
    const P: u64 = 0x0000_0100_0000_01b3;
    const SEED: u64 = 0xcbf2_9ce4_8422_2325;

    // P⁻¹ mod 2⁶⁴ per Newton: x ← x·(2 − P·x). Startwert P selbst ist auf drei
    // Bits korrekt, sechs Schritte reichen für 64.
    let p_inv = {
        let mut x: u64 = P;
        for _ in 0..6 {
            x = x.wrapping_mul(2u64.wrapping_sub(P.wrapping_mul(x)));
        }
        x
    };
    assert_eq!(P.wrapping_mul(p_inv), 1, "die FNV-Inverse stimmt nicht");

    // Der Digest der ALTEN Fassung, exakt nachgebaut — inklusive der Reihenfolge
    // und des `arcs.len() as f32`, das die alte Fassung (verlustbehaftet) hashte.
    fn old_digest(ch: &OffsetChannel, upto_parents1: bool) -> u64 {
        let mut h: u64 = SEED;
        let mut w = |v: u64, h: &mut u64| {
            *h ^= v;
            *h = h.wrapping_mul(P);
        };
        let mut f = |v: f32, h: &mut u64| w(v.to_bits() as u64, h);
        f(ch.arcs.len() as f32, &mut h);
        for a in &ch.arcs {
            for x in a.c.iter().chain(a.u.iter()).chain(a.v.iter()) {
                f(*x, &mut h);
            }
            f(a.r, &mut h);
            f(a.sweep, &mut h);
            f(a.e[0], &mut h);
            f(a.e[1], &mut h);
            f(a.d_e, &mut h);
            for x in a.bc.iter() {
                f(*x, &mut h);
            }
            f(a.br, &mut h);
            for x in a.m1s.iter().chain(a.m2s.iter()).chain(a.m1e.iter()).chain(a.m2e.iter()) {
                f(*x, &mut h);
            }
        }
        f(ch.delta, &mut h);
        f(if ch.concave { 1.0 } else { 0.0 }, &mut h);
        f(ch.trust, &mut h);
        f(ch.gate, &mut h);
        for x in ch.bound_c.iter() {
            f(*x, &mut h);
        }
        f(ch.bound_r, &mut h);
        for g in ch.end_planes.iter().chain(ch.face_planes.iter()) {
            for x in g {
                f(*x, &mut h);
            }
        }
        w(ch.branch, &mut h);
        w(ch.parents[0], &mut h);
        if !upto_parents1 {
            w(ch.parents[1], &mut h);
        }
        h
    }

    let delta = 2.0f32;
    let (_n, field, _e, _l, _h, _c) = cert_matrix(delta)
        .into_iter()
        .find(|(n, ..)| n.starts_with("Ecken-Schale t/δ=2.5"))
        .unwrap();
    let mut stops = Vec::new();
    stop_nodes(&field, &mut stops);
    let mut base: Option<OffsetChannel> = None;
    for st in &stops {
        if let BoundedCsg::ArcChannel { channels, .. } = st {
            for c in channels {
                if c.limit_mode().is_proved()
                    && c.face_planes.iter().any(|g| g[3] != -crate::offset_channel::END_PLANE_OFF)
                {
                    base = Some(c.clone());
                    break;
                }
            }
        }
    }
    let base = base.expect("kein bewiesener Kanal mit belegter Fremdebene");
    let slot = base
        .face_planes
        .iter()
        .position(|g| g[3] != -crate::offset_channel::END_PLANE_OFF)
        .unwrap();
    let d_alt = old_digest(&base, false);

    let mut forged = 0usize;
    for (why, which) in [("trust vergrössert", 0usize), ("belegte face_plane verschoben", 1)] {
        let mut m = base.clone();
        match which {
            0 => m.trust = ulp_off(m.trust, 4096),
            _ => m.face_planes[slot][3] += 1.0e-3,
        }
        // Der Angriff: `parents[1]` so wählen, dass der ALTE Digest wieder passt.
        let q = old_digest(&m, true);
        m.parents[1] = q ^ d_alt.wrapping_mul(p_inv);
        assert_eq!(
            old_digest(&m, false),
            d_alt,
            "{why}: die Digest-Fälschung ist misslungen — dann prüft der Pin nichts"
        );
        // Der gespeicherte Modus ist noch Proved …
        assert!(m.cached_limit_mode().is_proved(), "{why}: der Pin misst nichts");
        // … und der EXAKTE Vergleich fängt es trotzdem.
        assert_eq!(
            m.limit_mode(),
            crate::offset_channel::ChannelLimitMode::LegacyUnproven(
                crate::offset_channel::LegacyReason::ProofInvalidated
            ),
            "{why}: der gefälschte Digest hat den Modus gerettet — ein Hash reicht hier nicht"
        );
        assert_eq!(m.limit_mode().addends(), (0.0, 0.0));
        // Auch im Tape.
        let f = field_with_single_channel(&field, m.clone());
        let t = compile_body(&f);
        let ops = &t.opcodes;
        let mut i = 0usize;
        let mut seen = false;
        while i < ops.len() {
            if ops[i].to_bits() & 0xffff_ffff == OP_ARC_CHANNEL {
                let b = i + 1 + CHANNEL_HEADER - 3;
                assert_eq!(
                    (ops[b], ops[b + 1], ops[b + 2]),
                    (0.0, 0.0, 0.0),
                    "{why}: das Tape trägt den gefälschten Beweis"
                );
                seen = true;
                break;
            }
            i += 1;
        }
        assert!(seen, "{why}: kein OP_ARC_CHANNEL im Tape");
        forged += 1;
    }
    assert_eq!(forged, 2);

    // Und: `seam = None` bei gespeichertem Proved ⇒ ebenfalls Legacy — die
    // Beweis-Löschung ist strukturell abgedeckt, nicht per Caller-Disziplin.
    let mut wiped = base.clone();
    wit::force_legacy_final_channel(
        &mut wiped,
        crate::offset_channel::LegacyReason::NotCertified,
    );
    assert!(!wiped.limit_mode().is_proved());
    println!(
        "§8ac Pin 7: P⁻¹ = {p_inv:#x}; zwei Digest-Fälschungen rekonstruieren D_alt exakt und \
         fallen trotzdem auf Legacy"
    );
}
