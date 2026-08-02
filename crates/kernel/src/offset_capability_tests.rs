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
