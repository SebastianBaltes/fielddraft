//! Wiederverwendbare Metriken der Messbank.
//!
//! Alle Fehlerangaben sind in Weltmaß (mm); `model_size` steht in jedem
//! Report, um relativ zu normieren. Bänder relativ zur Modellgröße:
//! Nahfeld |d| < 2 %, Mittelfeld 2–10 %, Fernfeld > 10 %; jeweils nach
//! innen (d < 0) und außen (d ≥ 0) getrennt.

use rayon::prelude::*;
use serde::Serialize;

use crate::field::Field;
use crate::ground_truth::{central_gradient, GroundTruth};
use crate::util::{add, norm, scale, sub, SplitMix64};

pub const NEAR_BAND_REL: f64 = 0.02;
pub const MID_BAND_REL: f64 = 0.10;

// ── Statistik-Bausteine ────────────────────────────────────────────

/// Statistik über eine Fehlermenge. `mean/p95/max` über |e|,
/// `mean_signed` über e (Vorzeichen: f − d_wahr > 0 = Feld
/// überschätzt die Distanz).
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub count: usize,
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
    pub mean_signed: f64,
}

impl Stats {
    pub fn empty() -> Self {
        Self { count: 0, mean: 0.0, p95: 0.0, max: 0.0, mean_signed: 0.0 }
    }

    pub fn from_signed(mut errs: Vec<f64>) -> Self {
        if errs.is_empty() {
            return Self::empty();
        }
        let count = errs.len();
        let mean_signed = errs.iter().sum::<f64>() / count as f64;
        for e in errs.iter_mut() {
            *e = e.abs();
        }
        errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = errs.iter().sum::<f64>() / count as f64;
        let p95 = errs[((count as f64 * 0.95) as usize).min(count - 1)];
        let max = *errs.last().unwrap();
        Self { count, mean, p95, max, mean_signed }
    }
}

// ── (a) Distanzfehler nach Bändern ─────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct DistanceErrorReport {
    pub model_size: f64,
    pub near_out: Stats,
    pub near_in: Stats,
    pub mid_out: Stats,
    pub mid_in: Stats,
    pub far_out: Stats,
    pub far_in: Stats,
    pub overall: Stats,
}

/// Distanzfehler `f(p) − d_wahr(p)` von `meas` gegen die Ground Truth
/// über die Punktmenge `pts`. Rückgabe: Report + pro Punkt
/// `(d_wahr, f − d_wahr)` für eigene Auswertungen.
pub fn distance_errors(
    gt: &GroundTruth<'_>,
    meas: &dyn Field,
    pts: &[[f64; 3]],
) -> (DistanceErrorReport, Vec<(f64, f64)>) {
    let rows: Vec<(f64, f64)> = pts
        .par_iter()
        .map(|&p| {
            let d = gt.distance(p);
            let f = meas.eval(p);
            (d, f - d)
        })
        .filter(|(d, e)| d.is_finite() && e.is_finite())
        .collect();

    let size = gt.model_size;
    let band = |lo: f64, hi: f64, inside: bool| -> Stats {
        Stats::from_signed(
            rows.iter()
                .filter(|(d, _)| {
                    let a = d.abs();
                    a >= lo && a < hi && ((*d < 0.0) == inside)
                })
                .map(|(_, e)| *e)
                .collect(),
        )
    };
    let near = NEAR_BAND_REL * size;
    let mid = MID_BAND_REL * size;
    let report = DistanceErrorReport {
        model_size: size,
        near_out: band(0.0, near, false),
        near_in: band(0.0, near, true),
        mid_out: band(near, mid, false),
        mid_in: band(near, mid, true),
        far_out: band(mid, f64::INFINITY, false),
        far_in: band(mid, f64::INFINITY, true),
        overall: Stats::from_signed(rows.iter().map(|(_, e)| *e).collect()),
    };
    (report, rows)
}

// ── (b) Gradientennorm & Naht-Sprünge ──────────────────────────────

pub const GRAD_HIST_BINS: usize = 25;
pub const GRAD_HIST_MAX: f64 = 2.5;

#[derive(Debug, Clone, Serialize)]
pub struct GradientReport {
    /// |‖∇f‖ − 1| über alle Messpunkte (signed: ‖∇f‖ − 1).
    pub dev: Stats,
    /// Histogramm von ‖∇f‖: [`GRAD_HIST_BINS`] Bins über
    /// [0, [`GRAD_HIST_MAX`]], letzter Bin sammelt Überläufer.
    pub hist: Vec<u32>,
    pub hist_bin_width: f64,
    /// Gradienten-Sprünge an Nahtstellen: ‖∇f(p) − ∇f(p + δ)‖ über
    /// Nahfeld-Paare mit |δ| = 2e-3·Modellgröße (zufällige Richtung).
    /// Ein glattes 1-Lipschitz-Feld hält das nahe 0; harte min/max-
    /// Nähte springen um O(1).
    pub seam_jump: Stats,
    /// Schrittweite der zentralen Differenzen (absolut, mm).
    pub grad_h: f64,
}

/// Gradientenmetriken von `meas`: Norm-Abweichung über `pts`,
/// Naht-Sprünge über `near_pts` (Punkte nahe der Oberfläche).
pub fn gradient_report(
    meas: &dyn Field,
    model_size: f64,
    pts: &[[f64; 3]],
    near_pts: &[[f64; 3]],
    seed: u64,
) -> GradientReport {
    let h = model_size * 1e-3;
    let norms: Vec<f64> = pts
        .par_iter()
        .map(|&p| norm(central_gradient(meas, p, h)))
        .filter(|n| n.is_finite())
        .collect();

    let mut hist = vec![0u32; GRAD_HIST_BINS];
    let bin_w = GRAD_HIST_MAX / GRAD_HIST_BINS as f64;
    for &n in &norms {
        let b = ((n / bin_w) as usize).min(GRAD_HIST_BINS - 1);
        hist[b] += 1;
    }
    let dev = Stats::from_signed(norms.iter().map(|n| n - 1.0).collect());

    // Naht-Sprünge: deterministische Offset-Richtungen je Punktindex.
    let delta = model_size * 2e-3;
    let offsets: Vec<[f64; 3]> = {
        let mut rng = SplitMix64::new(seed ^ 0x5EA6_00D5_EED0_0001);
        (0..near_pts.len()).map(|_| rng.unit_dir()).collect()
    };
    let jumps: Vec<f64> = near_pts
        .par_iter()
        .zip(offsets.par_iter())
        .map(|(&p, &u)| {
            let g1 = central_gradient(meas, p, h);
            let g2 = central_gradient(meas, add(p, scale(u, delta)), h);
            norm(sub(g1, g2))
        })
        .filter(|j| j.is_finite())
        .collect();

    GradientReport {
        dev,
        hist,
        hist_bin_width: bin_w,
        seam_jump: Stats::from_signed(jumps),
        grad_h: h,
    }
}

// ── Punktmengen-Generatoren ────────────────────────────────────────

/// Uniform verteilte Punkte im Quader `[lo, hi]` (deterministisch).
pub fn sample_uniform(lo: [f64; 3], hi: [f64; 3], n: usize, seed: u64) -> Vec<[f64; 3]> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| {
            [
                rng.range(lo[0], hi[0]),
                rng.range(lo[1], hi[1]),
                rng.range(lo[2], hi[2]),
            ]
        })
        .collect()
}

/// Punkte nahe der Oberfläche: zufälliger Oberflächen-Samplepunkt der
/// Ground Truth plus Offset in zufälliger Richtung, Länge uniform in
/// `[-max_off_rel, max_off_rel]·Modellgröße`.
pub fn sample_near_surface(
    gt: &GroundTruth<'_>,
    n: usize,
    max_off_rel: f64,
    seed: u64,
) -> Vec<[f64; 3]> {
    let mut rng = SplitMix64::new(seed);
    let pts = gt.surface_points();
    let max_off = max_off_rel * gt.model_size;
    (0..n)
        .map(|_| {
            let q = pts[(rng.next_u64() as usize) % pts.len()];
            let u = rng.unit_dir();
            add(q, scale(u, rng.range(-max_off, max_off)))
        })
        .collect()
}

// ── (d) Kosten ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CostReport {
    /// Anzahl gemessener Evaluierungen je Feld.
    pub evals: usize,
    /// Direkter CSG-Eval (`CsgNode::sdf`, Visual), ns pro Eval.
    pub csg_ns_per_eval: f64,
    /// Kompiliertes Tape (`eval_opcodes`), ns pro Eval.
    pub tape_ns_per_eval: f64,
    /// Sekunden pro 1M Evals, direkt abgeleitet.
    pub csg_s_per_1m: f64,
    pub tape_s_per_1m: f64,
}

/// Misst `n` Evaluierungen beider Felder über eine zyklisch
/// durchlaufene Punktmenge. Die Summe wird via `black_box` am
/// Wegoptimieren gehindert.
pub fn cost_report(csg: &dyn Field, tape: &dyn Field, pts: &[[f64; 3]], n: usize) -> CostReport {
    let bench = |f: &dyn Field| -> f64 {
        // Warmup.
        let mut acc = 0.0f64;
        for &p in pts.iter().take(1000) {
            acc += f.eval(p);
        }
        let start = std::time::Instant::now();
        for i in 0..n {
            acc += f.eval(pts[i % pts.len()]);
        }
        let dt = start.elapsed().as_secs_f64();
        std::hint::black_box(acc);
        dt / n as f64 * 1e9
    };
    let csg_ns = bench(csg);
    let tape_ns = bench(tape);
    CostReport {
        evals: n,
        csg_ns_per_eval: csg_ns,
        tape_ns_per_eval: tape_ns,
        csg_s_per_1m: csg_ns * 1e-3,
        tape_s_per_1m: tape_ns * 1e-3,
    }
}
