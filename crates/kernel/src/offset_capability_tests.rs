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
