//! Ergebnis-Strukturen + CSV/JSON-Serialisierung.

use std::io::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::metrics::{CostReport, DistanceErrorReport, GradientReport, Stats};

/// Komplettes Messergebnis einer Szene (JSON-serialisiert unter
/// `scenes/<name>.json`).
#[derive(Debug, Serialize)]
pub struct SceneResult {
    pub name: String,
    pub description: String,
    pub model_size: f64,
    pub gt: GtInfo,
    pub points: PointsInfo,
    /// Messung des IST-Feldes (`CsgNode::sdf`, `EvalMode::Visual`).
    pub distance: DistanceErrorReport,
    pub gradient: GradientReport,
    pub cost: Option<CostReport>,
    /// Max. |csg_visual − tape| über die Messpunkte. Divergiert das
    /// Tape (MaskedBlend-Render-Fade), steht hier zusätzlich ein
    /// vollständiger Report gegen die Tape-eigene Ground Truth.
    pub tape_max_abs_diff: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tape_distance: Option<DistanceErrorReport>,
}

#[derive(Debug, Serialize)]
pub struct GtInfo {
    pub grid: usize,
    pub n_surface_points: usize,
    pub build_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct PointsInfo {
    pub n_uniform: usize,
    pub n_near_surface: usize,
    pub seed: u64,
}

pub fn write_json(path: &Path, result: &SceneResult) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::File::create(path)?;
    let s = serde_json::to_string_pretty(result)?;
    f.write_all(s.as_bytes())?;
    f.write_all(b"\n")
}

fn stats_cells(s: &Stats) -> String {
    format!("{:.6},{:.6},{:.6},{:.6}", s.mean, s.p95, s.max, s.mean_signed)
}

pub const SUMMARY_HEADER: &str = "scene,model_size,gt_points,\
near_out_mean,near_out_p95,near_out_max,near_out_bias,\
near_in_mean,near_in_p95,near_in_max,near_in_bias,\
mid_out_mean,mid_out_p95,mid_out_max,mid_out_bias,\
mid_in_mean,mid_in_p95,mid_in_max,mid_in_bias,\
far_out_mean,far_out_p95,far_out_max,far_out_bias,\
far_in_mean,far_in_p95,far_in_max,far_in_bias,\
grad_dev_mean,grad_dev_p95,grad_dev_max,\
seam_jump_p95,seam_jump_max,\
csg_ns_per_eval,tape_ns_per_eval,tape_max_abs_diff";

/// Eine Summary-CSV-Zeile (Spalten siehe [`SUMMARY_HEADER`]; Fehler in
/// mm, `bias` = mittlerer signierter Fehler f − d_wahr).
pub fn summary_row(r: &SceneResult) -> String {
    let d = &r.distance;
    let g = &r.gradient;
    let (csg_ns, tape_ns) = r
        .cost
        .as_ref()
        .map(|c| (c.csg_ns_per_eval, c.tape_ns_per_eval))
        .unwrap_or((f64::NAN, f64::NAN));
    format!(
        "{},{:.3},{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.2},{:.2},{:.6}",
        r.name,
        r.model_size,
        r.gt.n_surface_points,
        stats_cells(&d.near_out),
        stats_cells(&d.near_in),
        stats_cells(&d.mid_out),
        stats_cells(&d.mid_in),
        stats_cells(&d.far_out),
        stats_cells(&d.far_in),
        g.dev.mean,
        g.dev.p95,
        g.dev.max,
        g.seam_jump.p95,
        g.seam_jump.max,
        csg_ns,
        tape_ns,
        r.tape_max_abs_diff,
    )
}

pub fn write_csv(path: &Path, header: &str, rows: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = String::with_capacity(rows.len() * 256 + header.len() + 2);
    out.push_str(header);
    out.push('\n');
    for r in rows {
        out.push_str(r);
        out.push('\n');
    }
    std::fs::write(path, out)
}

pub const SWEEP_HEADER: &str = "depth,scene,model_size,\
near_out_mean,near_out_p95,near_out_max,near_out_bias,\
near_in_mean,near_in_p95,near_in_max,near_in_bias,\
grad_dev_mean,grad_dev_p95,grad_dev_max,seam_jump_p95,\
csg_ns_per_eval,tape_ns_per_eval";

/// Sweep-Zeile Fehler-über-CSG-Tiefe.
pub fn sweep_row(depth: usize, r: &SceneResult) -> String {
    let d = &r.distance;
    let g = &r.gradient;
    let (csg_ns, tape_ns) = r
        .cost
        .as_ref()
        .map(|c| (c.csg_ns_per_eval, c.tape_ns_per_eval))
        .unwrap_or((f64::NAN, f64::NAN));
    format!(
        "{},{},{:.3},{},{},{:.6},{:.6},{:.6},{:.6},{:.2},{:.2}",
        depth,
        r.name,
        r.model_size,
        stats_cells(&d.near_out),
        stats_cells(&d.near_in),
        g.dev.mean,
        g.dev.p95,
        g.dev.max,
        g.seam_jump.p95,
        csg_ns,
        tape_ns,
    )
}
