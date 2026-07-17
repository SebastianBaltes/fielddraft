//! Pflicht-Validierung der Ground Truth gegen analytisch exakte Fälle.
//!
//! Vertrag: |GT-Distanz − analytische Distanz| < 1e-3 · Modellgröße auf
//! allen Testpunkten (uniform + oberflächennah) — sonst ist die
//! Messbank wertlos. Die Fälle decken gekrümmte Flächen (Kugel),
//! scharfe konvexe Kanten/Ecken (Box), Rotation (rotierte Box) und den
//! Offset-Operator (exakte Offset-Fläche einer Kugel) ab.
//!
//! Die Tests laufen bewusst mit moderatem Gitter (64): die
//! Nahfeld-Genauigkeit muss aus der Richtungs-Raffinierung kommen,
//! nicht aus der Punktdichte.

use fielddraft_kernel::csg::CsgNode;
use fielddraft_protocol::{FeatureId, Primitive};
use metricity_lab::field::CsgField;
use metricity_lab::ground_truth::{GroundTruth, GtOptions};
use metricity_lab::metrics::{sample_near_surface, sample_uniform};
use metricity_lab::scenes;

const REL_TOL: f64 = 1e-3;

fn leaf(primitive: Primitive) -> CsgNode {
    CsgNode::Leaf { id: FeatureId::from_u32(1), primitive }
}

/// GT gegen analytische Distanz prüfen; gibt den max. beobachteten
/// relativen Fehler zurück.
fn validate(node: &CsgNode, analytic: impl Fn([f64; 3]) -> f64, label: &str) -> f64 {
    let field = CsgField::visual(node);
    let aabb = node.aabb();
    let lo = [aabb.min[0] as f64, aabb.min[1] as f64, aabb.min[2] as f64];
    let hi = [aabb.max[0] as f64, aabb.max[1] as f64, aabb.max[2] as f64];
    let opts = GtOptions { grid: 64, ..GtOptions::default() };
    let gt = GroundTruth::build(&field, lo, hi, &opts);
    let size = gt.model_size;

    let margin = size * 0.2;
    let mut pts = sample_uniform(
        [lo[0] - margin, lo[1] - margin, lo[2] - margin],
        [hi[0] + margin, hi[1] + margin, hi[2] + margin],
        600,
        7,
    );
    pts.extend(sample_near_surface(&gt, 600, 0.05, 11));
    // Extrem nahe Punkte (Nahfeld-Härtetest, |d| bis hinunter zu ~1e-4·Größe).
    pts.extend(sample_near_surface(&gt, 300, 0.001, 13));

    let mut max_rel = 0.0f64;
    for &p in &pts {
        let d_gt = gt.distance(p);
        let d_an = analytic(p);
        let rel = (d_gt - d_an).abs() / size;
        if rel > max_rel {
            max_rel = rel;
        }
        assert!(
            rel < REL_TOL,
            "{label}: GT error {rel:.2e} rel (gt={d_gt:.5}, analytic={d_an:.5}) at {p:?}"
        );
    }
    eprintln!("[gt-validation] {label}: max rel error {max_rel:.2e} (n={})", pts.len());
    max_rel
}

fn box_sdf(p: [f64; 3], half: [f64; 3]) -> f64 {
    let q = [
        p[0].abs() - half[0],
        p[1].abs() - half[1],
        p[2].abs() - half[2],
    ];
    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
    let inside = q[0].max(q[1]).max(q[2]).min(0.0);
    outside + inside
}

#[test]
fn gt_matches_analytic_sphere() {
    let node = leaf(Primitive::Sphere { center: [0.0; 3], radius: 40.0 });
    let max_rel = validate(
        &node,
        |p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 40.0,
        "sphere r=40",
    );
    assert!(max_rel < REL_TOL);
}

#[test]
fn gt_matches_analytic_box() {
    let half = [40.0, 25.0, 15.0];
    let node = leaf(Primitive::Box {
        center: [0.0; 3],
        half_extents: [half[0] as f32, half[1] as f32, half[2] as f32],
    });
    validate(&node, |p| box_sdf(p, half), "box 80x50x30");
}

#[test]
fn gt_matches_analytic_rotated_box() {
    // Muss dieselbe Rotation wie `scenes::rotated_box_node` verwenden.
    let node = scenes::rotated_box_node();
    let axis_raw = [1.0f64, 1.0, 0.3];
    let len = (axis_raw[0].powi(2) + axis_raw[1].powi(2) + axis_raw[2].powi(2)).sqrt();
    let axis = [axis_raw[0] / len, axis_raw[1] / len, axis_raw[2] / len];
    let angle: f64 = 0.7;
    let (s, c) = (angle * 0.5).sin_cos();
    // Konjugiertes Quaternion (Welt → lokal).
    let q = [-axis[0] * s, -axis[1] * s, -axis[2] * s, c];
    let rotate = move |p: [f64; 3]| -> [f64; 3] {
        // v' = v + 2·qv×(qv×v + w·v)
        let qv = [q[0], q[1], q[2]];
        let t = [
            2.0 * (qv[1] * p[2] - qv[2] * p[1]),
            2.0 * (qv[2] * p[0] - qv[0] * p[2]),
            2.0 * (qv[0] * p[1] - qv[1] * p[0]),
        ];
        [
            p[0] + q[3] * t[0] + qv[1] * t[2] - qv[2] * t[1],
            p[1] + q[3] * t[1] + qv[2] * t[0] - qv[0] * t[2],
            p[2] + q[3] * t[2] + qv[0] * t[1] - qv[1] * t[0],
        ]
    };
    validate(
        &node,
        move |p| box_sdf(rotate(p), [40.0, 25.0, 15.0]),
        "rotated box",
    );
}

#[test]
fn gt_matches_analytic_offset_sphere() {
    // Exakte Offset-Fläche: Kugel r=30, Offset +10 → Kugel r=40.
    let node = CsgNode::Offset(
        Box::new(leaf(Primitive::Sphere { center: [0.0; 3], radius: 30.0 })),
        10.0,
    );
    validate(
        &node,
        |p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 40.0,
        "offset sphere 30+10",
    );
}
