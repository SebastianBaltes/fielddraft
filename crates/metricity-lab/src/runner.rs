//! Kompletter Messlauf einer Szene: Ground Truth bauen, Punktmengen
//! ziehen, Distanz-/Gradienten-/Kosten-Metriken rechnen, Tape-Parität
//! prüfen.

use std::time::Instant;

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::sdf_compiler::compile_body;

use crate::field::{CsgField, Field, TapeField};
use crate::ground_truth::{GroundTruth, GtOptions};
use crate::metrics::{
    cost_report, distance_errors, gradient_report, sample_near_surface, sample_uniform,
};
use crate::report::{GtInfo, PointsInfo, SceneResult};
use crate::scenes::Scene;

#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Ground-Truth-Gitter (0 = Szenen-Default bzw. 96).
    pub grid: usize,
    pub n_uniform: usize,
    pub n_near: usize,
    pub seed: u64,
    /// Evals für den Kosten-Benchmark (0 = kein Benchmark).
    pub bench_evals: usize,
    /// Richtungs-Raffinierung der Ground Truth (nur für Schnellläufe
    /// abschalten — kostet sonst Nahfeld-Genauigkeit!).
    pub refine: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            grid: 0,
            n_uniform: 6000,
            n_near: 6000,
            seed: 4242,
            bench_evals: 1_000_000,
            refine: true,
        }
    }
}

/// Toleranz, ab der das kompilierte Tape als vom `CsgNode`-Visual-Feld
/// divergent gilt (relativ zur Modellgröße) und einen eigenen
/// Distanz-Report gegen die Tape-eigene Ground Truth bekommt.
const TAPE_DIVERGENCE_REL: f64 = 1e-3;

pub fn run_scene(scene: &Scene, cfg: &RunConfig) -> SceneResult {
    let node = &scene.node;
    let aabb = node.aabb();
    assert!(!aabb.is_empty(), "scene {}: empty AABB", scene.name);
    let lo = [aabb.min[0] as f64, aabb.min[1] as f64, aabb.min[2] as f64];
    let hi = [aabb.max[0] as f64, aabb.max[1] as f64, aabb.max[2] as f64];

    let csg = CsgField::visual(node);
    let grid = if cfg.grid > 0 {
        cfg.grid
    } else if scene.gt_grid > 0 {
        scene.gt_grid
    } else {
        96
    };
    let gt_opts = GtOptions { grid, refine: cfg.refine, ..GtOptions::default() };

    let t0 = Instant::now();
    let gt = GroundTruth::build(&csg, lo, hi, &gt_opts);
    let build_ms = t0.elapsed().as_secs_f64() * 1e3;

    // Messpunkte: uniform über den um 20 % Modellgröße erweiterten AABB
    // (Fernfeld) + oberflächennah (±5 % Modellgröße).
    let margin = gt.model_size * 0.2;
    let ulo = [lo[0] - margin, lo[1] - margin, lo[2] - margin];
    let uhi = [hi[0] + margin, hi[1] + margin, hi[2] + margin];
    let mut pts = sample_uniform(ulo, uhi, cfg.n_uniform, cfg.seed);
    let near = sample_near_surface(&gt, cfg.n_near, 0.05, cfg.seed ^ 0xA5A5);
    pts.extend_from_slice(&near);

    let (distance, _rows) = distance_errors(&gt, &csg, &pts);
    let gradient = gradient_report(&csg, gt.model_size, &pts, &near, cfg.seed);

    // Tape: Parität + ggf. eigener Report.
    let compiled = compile_body(&BoundedCsg::from_csg(node));
    let tape = TapeField { compiled: &compiled };
    let tape_max_abs_diff = pts
        .iter()
        .take(4000)
        .map(|&p| (csg.eval(p) - tape.eval(p)).abs())
        .fold(0.0f64, f64::max);
    let tape_distance = if tape_max_abs_diff > TAPE_DIVERGENCE_REL * gt.model_size {
        let tape_gt = GroundTruth::build(&tape, lo, hi, &gt_opts);
        let (rep, _) = distance_errors(&tape_gt, &tape, &pts);
        Some(rep)
    } else {
        None
    };

    let cost = if cfg.bench_evals > 0 {
        Some(cost_report(&csg, &tape, &pts, cfg.bench_evals))
    } else {
        None
    };

    SceneResult {
        name: scene.name.clone(),
        description: scene.description.clone(),
        model_size: gt.model_size,
        gt: GtInfo { grid, n_surface_points: gt.n_surface_points(), build_ms },
        points: PointsInfo {
            n_uniform: cfg.n_uniform,
            n_near_surface: cfg.n_near,
            seed: cfg.seed,
        },
        distance,
        gradient,
        cost,
        tape_max_abs_diff,
        tape_distance,
    }
}
