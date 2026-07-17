//! Funktions-Smoke der Messbank: Metriken erkennen bekannte Defekte,
//! Korpus-Szenen sind konstruierbar und liefern plausible Felder.

use metricity_lab::field::{CsgField, Field, FnField};
use metricity_lab::ground_truth::{GroundTruth, GtOptions};
use metricity_lab::metrics::{distance_errors, gradient_report, sample_near_surface, sample_uniform};
use metricity_lab::scenes;

/// Ein absichtlich skaliertes Feld (0.5·Kugel-SDF) hat exakt dieselbe
/// Nullmenge, aber halbierte Distanzwerte — die Messbank muss das als
/// ~50 % Distanzfehler und ‖∇f‖ ≈ 0.5 ausweisen.
#[test]
fn metrics_flag_a_deliberately_scaled_field() {
    let sphere = |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() - 40.0;
    let half = FnField(move |p| 0.5 * sphere(p));
    let gt = GroundTruth::build(
        &half,
        [-40.0; 3],
        [40.0; 3],
        &GtOptions { grid: 48, ..GtOptions::default() },
    );

    let mut pts = sample_uniform([-55.0; 3], [55.0; 3], 400, 3);
    let near = sample_near_surface(&gt, 400, 0.05, 5);
    pts.extend_from_slice(&near);

    let (rep, _) = distance_errors(&gt, &half, &pts);
    // Fernfeld außen: |0.5·d − d| = 0.5·d, bei d ≥ 8 mm also ≥ 4 mm.
    assert!(
        rep.far_out.mean > 2.0,
        "scaled field must show large far-field error, got {:?}",
        rep.far_out
    );
    // Bias: Feld unterschätzt (f − d < 0).
    assert!(rep.far_out.mean_signed < -2.0);

    let grad = gradient_report(&half, gt.model_size, &pts, &near, 9);
    assert!(
        (grad.dev.mean_signed + 0.5).abs() < 0.05,
        "‖∇f‖ − 1 must be ≈ −0.5, got {}",
        grad.dev.mean_signed
    );
}

/// Alle Korpus-Szenen bauen, haben endliche AABBs und ein Feld mit
/// Vorzeichenwechsel (innen wie außen belegt).
#[test]
fn corpus_scenes_are_well_formed() {
    for scene in scenes::corpus() {
        let aabb = scene.node.aabb();
        assert!(!aabb.is_empty(), "{}: empty aabb", scene.name);
        let field = CsgField::visual(&scene.node);
        let c = [
            ((aabb.min[0] + aabb.max[0]) * 0.5) as f64,
            ((aabb.min[1] + aabb.max[1]) * 0.5) as f64,
            ((aabb.min[2] + aabb.max[2]) * 0.5) as f64,
        ];
        let size = (aabb.max[0] - aabb.min[0])
            .max(aabb.max[1] - aabb.min[1])
            .max(aabb.max[2] - aabb.min[2]) as f64;
        let far = field.eval([c[0], c[1], c[2] + 2.0 * size]);
        assert!(far > 0.0, "{}: outside point not positive ({far})", scene.name);
        // Entlang der drei Achsen-Linien durch das AABB-Zentrum muss
        // fein gerastert (fängt auch 0.5-mm-Wände) ein Innen-Punkt liegen.
        let mut has_inside = false;
        'outer: for axis in 0..3 {
            for k in 0..8192 {
                let mut p = c;
                p[axis] = aabb.min[axis] as f64
                    + (k as f64 + 0.5) / 8192.0 * (aabb.max[axis] - aabb.min[axis]) as f64;
                if field.eval(p) < 0.0 {
                    has_inside = true;
                    break 'outer;
                }
            }
        }
        assert!(has_inside, "{}: no inside point found", scene.name);
    }
}

/// Parametrische Familien sind über `scene_by_name` erreichbar.
#[test]
fn scene_registry_resolves_families() {
    assert!(scenes::scene_by_name("deep_csg_64").is_some());
    assert!(scenes::scene_by_name("smin_chain_16").is_some());
    assert!(scenes::scene_by_name("thin_wall_00p5pct").is_some());
    assert!(scenes::scene_by_name("nope").is_none());
}
