//! Export meshing session (Meshing Phase 0).
//!
//! The dedicated, tolerance-driven mesh pass for STL/3MF export —
//! decoupled from the viewport LOD (docs/MESHING-EXPORT-ANALYSIS.md §5,
//! Phase 0). The viewport meshes at a fixed budget (res 64 active /
//! 32 context) tuned for frame rate; export instead derives each body's
//! grid from a requested **cell size in mm**, runs the same Manifold-DC
//! pipeline properly dimensioned, and validates the result
//! (watertight/manifold + measured deviation against the exact SDF)
//! so the export dialog can show an honest report.
//!
//! Session protocol (drives worker-side progress reporting):
//! `export_mesh_begin(cell_mm)` → N× `export_mesh_body(i)` →
//! `export_mesh_stl()` / `export_mesh_3mf()` + `export_mesh_report()`
//! → `export_mesh_end()`.

use crate::dc::Mesh;
use crate::mesh_validate::{self, MeshValidation};
use fielddraft_protocol::Primitive;

/// Never mesh coarser than this many cells along the longest axis —
/// even a tiny body should not degenerate to a handful of voxels.
pub const EXPORT_MIN_RESOLUTION: u32 = 16;

/// Memory guard: cap on grid *corners* per body. The MDC pass allocates
/// one f32 per corner (plus per-cell structures on the surface), so
/// 24M corners ≈ 96 MB — safely inside the WASM heap of the mesher
/// worker. When the requested cell size would exceed this, the cell is
/// coarsened and the report flags it.
pub const EXPORT_MAX_CORNERS: f64 = 24_000_000.0;

/// Sample budget for the deviation measurement per body.
pub const DEVIATION_SAMPLE_BUDGET: usize = 120_000;

/// Per-body slice of the export report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BodyExportReport {
    /// Stable seed-feature id of the body (`Body::id`) — the UI resolves
    /// the display name from its feature list.
    pub body_id: u32,
    pub requested_cell_mm: f32,
    /// Cell size actually used (== requested unless the memory guard
    /// coarsened the grid).
    pub actual_cell_mm: f32,
    /// Cells along the body's longest AABB axis.
    pub resolution: u32,
    /// True when [`EXPORT_MAX_CORNERS`] forced a coarser grid than the
    /// requested cell size.
    pub cell_limited: bool,
    /// True for single-`ImportedMesh` bodies: the source triangles are
    /// exported verbatim (no re-mesh, no deviation sampling — the mesh
    /// IS the ground truth).
    pub passthrough: bool,
    /// Triangles whose winding the orientation-repair pass flipped.
    pub orientation_repaired: usize,
    /// True when the planar/developable simplification made the cut;
    /// false when the raw (dense) mesh shipped instead.
    pub simplified: bool,
    /// True when simplification was rejected because it broke a
    /// watertight raw mesh.
    pub simplification_rejected: bool,
    #[serde(flatten)]
    pub validation: MeshValidation,
}

/// Aggregate export report — one JSON blob for the export dialog.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExportReport {
    pub cell_size_mm: f32,
    pub total_triangles: usize,
    pub all_watertight: bool,
    pub max_deviation_mm: f32,
    pub bodies: Vec<BodyExportReport>,
}

/// One in-flight export pass: the freshly-built export meshes plus their
/// reports, parallel to the kernel's body list at `begin` time. Any
/// document mutation invalidates the session (the kernel drops it on
/// every rebuild).
pub struct ExportSession {
    pub cell_size_mm: f32,
    pub meshes: Vec<Mesh>,
    pub reports: Vec<BodyExportReport>,
}

impl ExportSession {
    pub fn report(&self) -> ExportReport {
        ExportReport {
            cell_size_mm: self.cell_size_mm,
            total_triangles: self.meshes.iter().map(|m| m.triangle_count()).sum(),
            all_watertight: !self.reports.is_empty()
                && self.reports.iter().all(|r| r.validation.watertight),
            max_deviation_mm: self
                .reports
                .iter()
                .map(|r| r.validation.max_deviation_mm)
                .fold(0.0, f32::max),
            bodies: self.reports.clone(),
        }
    }
}

/// Derive the export grid for a body AABB: cells along the longest axis
/// from the requested cell size, coarsened if the total corner count
/// would blow the memory guard. Returns `(resolution, actual_cell_mm,
/// cell_limited)`.
pub fn derive_export_resolution(size: [f32; 3], requested_cell_mm: f32) -> (u32, f32, bool) {
    let max_dim = size[0].max(size[1]).max(size[2]).max(1e-3);
    let mut cell = requested_cell_mm.max(1e-3);

    // Corner-count estimate mirroring dc.rs: the grid is the AABB
    // expanded by DC_MARGIN_CELLS cells on each side.
    let corners = |cell: f32| -> f64 {
        let m = 2.0 * crate::dc::DC_MARGIN_CELLS as f64 + 2.0;
        let nx = (size[0] as f64 / cell as f64).ceil() + m;
        let ny = (size[1] as f64 / cell as f64).ceil() + m;
        let nz = (size[2] as f64 / cell as f64).ceil() + m;
        nx * ny * nz
    };

    let mut limited = false;
    if corners(cell) > EXPORT_MAX_CORNERS {
        // Scale the cell up by the cube root of the overshoot; iterate
        // because the +margin terms make the relation slightly
        // non-cubic.
        while corners(cell) > EXPORT_MAX_CORNERS {
            cell *= (corners(cell) / EXPORT_MAX_CORNERS).powf(1.0 / 3.0).max(1.01) as f32;
            limited = true;
        }
    }

    let resolution = ((max_dim / cell).ceil() as u32).max(EXPORT_MIN_RESOLUTION);
    // Resolution is what dual_contour actually consumes (it re-derives
    // the cell as max_dim/resolution) — report that exact value.
    let actual_cell = max_dim / resolution as f32;
    (resolution, actual_cell, limited)
}

impl crate::Kernel {
    /// Start an export pass at the given cell size. Clears any previous
    /// session and returns the number of bodies to mesh (the caller
    /// loops `export_mesh_body_internal(0..n)` and reports progress).
    pub(crate) fn export_mesh_begin_internal(&mut self, cell_size_mm: f32) -> usize {
        let n = self.bodies.len();
        self.export_session = Some(ExportSession {
            cell_size_mm,
            meshes: Vec::with_capacity(n),
            reports: Vec::with_capacity(n),
        });
        n
    }

    /// Mesh + validate body `i` at the session's cell size. Bodies must
    /// be processed in order (0..n); each call appends to the session.
    /// Returns the per-body report, or `None` without a session.
    pub(crate) fn export_mesh_body_internal(&mut self, i: usize) -> Option<BodyExportReport> {
        let cell_mm = self.export_session.as_ref()?.cell_size_mm;
        let body = self.bodies.get(i)?;
        let body_id = body.id.as_u32_or_root().unwrap_or(0);
        let feature_indices = body.feature_indices.clone();
        let size = body.aabb.size();

        // ImportedMesh passthrough — identical to the viewport rule in
        // `build_mesh_for`: the source triangles ARE the body.
        let passthrough_mesh = if feature_indices.len() == 1 {
            if let crate::csg::CsgNode::Leaf {
                primitive: Primitive::ImportedMesh { mesh },
                ..
            } = &self.decoded[feature_indices[0]].node
            {
                Some(Self::mesh_from_imported(mesh, body_id))
            } else {
                None
            }
        } else {
            None
        };

        let (mesh, report) = if let Some(mesh) = passthrough_mesh {
            let validation = mesh_validate::validate_topology(&mesh);
            let report = BodyExportReport {
                body_id,
                requested_cell_mm: cell_mm,
                actual_cell_mm: 0.0,
                resolution: 0,
                cell_limited: false,
                passthrough: true,
                orientation_repaired: 0,
                simplified: false,
                simplification_rejected: false,
                validation,
            };
            (mesh, report)
        } else {
            let (resolution, actual_cell, cell_limited) =
                derive_export_resolution(size, cell_mm);
            let (bounded, prims) =
                crate::body::build_bounded_with_primitives(&self.decoded, &feature_indices);
            let prim_refs: std::collections::HashMap<u32, &Primitive> =
                prims.iter().map(|(k, v)| (*k, v)).collect();
            let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);

            // Raw pass: Manifold-DC + Newton projection, then winding
            // repair (MDC emits inconsistently-wound triangles along
            // sharp feature edges — invisible in the DoubleSide
            // viewport, fatal in an STL).
            let mut raw = crate::dc::dual_contour_unsimplified(&bounded, resolution);
            let raw_flips = mesh_validate::fix_orientation(&mut raw, &sdf);

            // Simplification candidate: the planar/developable pass cuts
            // the triangle count dramatically (box: ~40×), but its
            // re-triangulation can break topology — so it must EARN the
            // export by staying watertight when the raw mesh was.
            let mut candidate = raw.clone();
            crate::dc::simplify_dc_mesh(&mut candidate, &bounded, &prim_refs);
            let candidate_flips = mesh_validate::fix_orientation(&mut candidate, &sdf);

            let v_raw = mesh_validate::validate_topology(&raw);
            let v_candidate = mesh_validate::validate_topology(&candidate);
            let reject_simplified = v_raw.watertight && !v_candidate.watertight;
            let (mut mesh, mut validation, simplified, orientation_repaired) =
                if reject_simplified {
                    (raw, v_raw, false, raw_flips)
                } else {
                    (candidate, v_candidate, true, raw_flips + candidate_flips)
                };

            let (max_dev, p95_dev, samples) =
                mesh_validate::measure_deviation(&mesh, DEVIATION_SAMPLE_BUDGET, &sdf);
            validation.max_deviation_mm = max_dev;
            validation.p95_deviation_mm = p95_dev;
            validation.deviation_samples = samples;
            // Drop the viewport-only sidecar channels; the exporters
            // only read positions/indices.
            mesh.face_index = Vec::new();

            let report = BodyExportReport {
                body_id,
                requested_cell_mm: cell_mm,
                actual_cell_mm: actual_cell,
                resolution,
                cell_limited,
                passthrough: false,
                orientation_repaired,
                simplified,
                simplification_rejected: reject_simplified,
                validation,
            };
            (mesh, report)
        };

        let session = self.export_session.as_mut()?;
        session.meshes.push(mesh);
        session.reports.push(report.clone());
        Some(report)
    }

    pub(crate) fn export_session_meshes(&self) -> Vec<&Mesh> {
        self.export_session
            .as_ref()
            .map(|s| s.meshes.iter().collect())
            .unwrap_or_default()
    }
}

// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{BoolOp, Primitive};

    #[test]
    fn session_end_to_end_box_is_watertight_and_accurate() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            20.0,
            20.0,
            20.0,
        )
        .unwrap();

        let n = k.export_mesh_begin(0.5);
        assert_eq!(n, 1);
        let body_json = k.export_mesh_body(0);
        assert!(body_json.contains("\"watertight\":true"), "{body_json}");

        let report: serde_json::Value =
            serde_json::from_str(&k.export_mesh_report()).unwrap();
        assert_eq!(report["all_watertight"], true);
        assert!(report["total_triangles"].as_u64().unwrap() > 4);
        // A box is piecewise planar — the export mesh must sit on the
        // exact surface well inside the 0.1 mm target.
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.1,
            "max deviation {}",
            report["max_deviation_mm"]
        );
        // Cell size 0.5 mm on a 40 mm box (half extents 20) → res 80,
        // far beyond the viewport's 64.
        assert_eq!(report["bodies"][0]["resolution"], 80);

        let stl = k.export_mesh_stl();
        let tri_count =
            u32::from_le_bytes([stl[80], stl[81], stl[82], stl[83]]) as usize;
        assert_eq!(stl.len(), 84 + tri_count * 50);
        assert_eq!(
            tri_count,
            report["total_triangles"].as_u64().unwrap() as usize
        );

        let threemf = k.export_mesh_3mf();
        assert!(threemf.len() > 500);

        k.export_mesh_end();
        assert!(k.export_mesh_report().is_empty());
    }

    #[test]
    fn mutation_invalidates_the_session() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            5.0,
            5.0,
            5.0,
        )
        .unwrap();
        k.export_mesh_begin(0.5);
        k.export_mesh_body(0);
        // Any document mutation re-meshes and must drop the session.
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            20.0,
            0.0,
            0.0,
            3.0,
            0.0,
            0.0,
        )
        .unwrap();
        assert!(k.export_mesh_report().is_empty());
        assert!(k.export_mesh_body(0).is_empty());
    }

#[test]
    fn resolution_derives_from_cell_size() {
        let (res, cell, limited) = derive_export_resolution([100.0, 50.0, 20.0], 0.5);
        assert_eq!(res, 200);
        assert!((cell - 0.5).abs() < 1e-4);
        assert!(!limited);
    }

    #[test]
    fn tiny_body_gets_minimum_resolution() {
        let (res, _, limited) = derive_export_resolution([1.0, 1.0, 1.0], 0.5);
        assert_eq!(res, EXPORT_MIN_RESOLUTION);
        assert!(!limited);
    }

    #[test]
    fn memory_guard_coarsens_huge_grids() {
        // 600 mm cube at 0.1 mm cells would be 6000³ = 2.2e11 corners.
        let (res, cell, limited) = derive_export_resolution([600.0, 600.0, 600.0], 0.1);
        assert!(limited);
        assert!(cell > 0.1);
        let m = 2.0 * crate::dc::DC_MARGIN_CELLS as f64 + 2.0;
        let n = res as f64 + m;
        assert!(n * n * n <= EXPORT_MAX_CORNERS * 1.05, "res {res} cell {cell}");
    }
}

