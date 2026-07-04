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

/// Pair budget for the self-intersection scan per body.
pub const SELF_INTERSECT_PAIR_BUDGET: usize = 2_000_000;

/// Which generic mesher the export pass uses when the parametric route
/// doesn't apply. Selected in the export dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshMethod {
    /// Parametric-first, then adaptive Manifold-DC on a narrow-band
    /// octree (default). Optimal triangle counts; the octree's Lipschitz
    /// distance bound assumes a roughly 1-Lipschitz field.
    Adaptive,
    /// Dense Surface Nets + QEM decimation via battle-tested libraries
    /// (`fast-surface-nets` mesher + `baby_shark` decimator). Every cell
    /// is evaluated (no Lipschitz pruning), so the topology of
    /// non-1-Lipschitz fields — gyroid / TPMS shells, dense helix
    /// windings — is captured reliably; decimation then brings the
    /// triangle count back down. Skips the parametric route.
    SurfaceNets,
    /// Same pipeline as [`MeshMethod::SurfaceNets`] but our own
    /// from-scratch implementations (`surface_nets_builtin` +
    /// `decimate_builtin`) — dependency-free alternative, selectable to
    /// diff against the library path.
    SurfaceNetsBuiltin,
    /// Interval-arithmetic octree + Manifold Dual Contouring via the
    /// `fidget` crate (`fidget_bridge`). Certifies octree subdivision by
    /// interval bounds on the actual expression (no Lipschitz
    /// assumption → topology-safe for non-1-Lipschitz fields) and
    /// recovers sharp feature edges from the field (QEF) without a
    /// B-Rep. Only closed-form bodies qualify; sketch/data carriers fall
    /// back.
    IntervalDc,
    /// Built-in uniform Dual Contouring with QEF vertex placement
    /// (`uniform_contour`). Dense grid (non-1-Lipschitz-safe), sharp
    /// feature edges preserved via QEF, then QEM decimation.
    UniformDc,
    /// Marching Tetrahedra (`marching_tets`) — the robust MC-family
    /// mesher: unambiguous tetrahedral cases → watertight + 2-manifold by
    /// construction for any field. Rounds edges; then decimation.
    MarchingCubes,
    /// The viewport's uniform Manifold Dual Contouring (`dc.rs`) with its
    /// planar/developable simplification — now exposed for export too.
    ManifoldDc,
    /// Reach-for-the-Arcs (`rfta`, Sellán et al. 2024): spheres →
    /// tangent points → screened Poisson reconstruction. A sampled-grid
    /// reconstruction method (capped internal resolution) — previously
    /// only available in the mesh viewer.
    Rfta,
}

impl MeshMethod {
    /// Parse the wire string from the export dialog.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "surface-nets" => MeshMethod::SurfaceNets,
            "surface-nets-builtin" => MeshMethod::SurfaceNetsBuiltin,
            "interval-dc" => MeshMethod::IntervalDc,
            "uniform-dc" => MeshMethod::UniformDc,
            "marching-cubes" => MeshMethod::MarchingCubes,
            "manifold-dc" => MeshMethod::ManifoldDc,
            "rfta" => MeshMethod::Rfta,
            _ => MeshMethod::Adaptive,
        }
    }
}

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
    /// True when this body shipped the parametric per-face B-Rep
    /// tessellation (Meshing Phase 2, Option B) — watertight by
    /// construction, exact edges, minimal triangles. False = generic
    /// MDC pass.
    pub parametric: bool,
    /// Why the parametric route was not taken (empty when it was).
    pub parametric_skip_reason: String,
    /// True when the adaptive Manifold-DC pass (Phase 1c, Schaefer
    /// clustering on a narrow-band octree) produced this mesh; false =
    /// uniform-grid fallback.
    pub adaptive: bool,
    /// Why the adaptive pass was skipped (empty when it ran).
    pub adaptive_skip_reason: String,
    /// Triangles whose winding the orientation-repair pass flipped.
    pub orientation_repaired: usize,
    /// Edge/corner-classified vertices pinned onto the exact woven
    /// B-Rep edge curves (Phase 1b; 0 on the parametric route, whose
    /// edges are exact by construction).
    pub edges_pinned: usize,
    /// True when the planar/developable simplification made the cut;
    /// false when the raw (dense) mesh shipped instead.
    pub simplified: bool,
    /// True when simplification was rejected because it broke a
    /// watertight raw mesh.
    pub simplification_rejected: bool,
    /// Finest cells with >1 contour sheet (adaptive route): walls
    /// thinner than one cell live there and may be under-resolved —
    /// shrink the cell size to resolve them.
    pub thin_cells: usize,
    /// Proper triangle-triangle self-intersections (Manifold DC is
    /// topologically 2-manifold but not intersection-free — paper
    /// limitation). Measured, not gated.
    pub self_intersections: usize,
    /// False when the pair budget truncated the self-intersection scan.
    pub self_intersections_complete: bool,
    /// True when this body shipped the dense Surface Nets + QEM
    /// decimation route (robust for non-1-Lipschitz fields).
    pub surface_nets: bool,
    /// True when this body shipped the interval-octree Manifold-DC route
    /// (fidget) — feature-preserving, interval-safe topology.
    pub interval_dc: bool,
    /// Short human label of the mesher route for the report row (e.g.
    /// "uniform DC", "marching cubes"); empty falls back to the flags.
    pub route: String,
    /// Triangle count of the dense Surface Nets mesh BEFORE decimation
    /// (0 off the Surface Nets route) — the report shows the reduction.
    pub dense_triangles: usize,
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
    pub method: MeshMethod,
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
    pub(crate) fn export_mesh_begin_internal(
        &mut self,
        cell_size_mm: f32,
        method: MeshMethod,
    ) -> usize {
        let n = self.bodies.len();
        self.export_session = Some(ExportSession {
            cell_size_mm,
            method,
            meshes: Vec::with_capacity(n),
            reports: Vec::with_capacity(n),
        });
        n
    }

    /// Mesh + validate body `i` at the session's cell size. Bodies must
    /// be processed in order (0..n); each call appends to the session.
    /// Returns the per-body report, or `None` without a session.
    pub(crate) fn export_mesh_body_internal(&mut self, i: usize) -> Option<BodyExportReport> {
        let session = self.export_session.as_ref()?;
        let cell_mm = session.cell_size_mm;
        let method = session.method;
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
                parametric: false,
                parametric_skip_reason: String::new(),
                adaptive: false,
                adaptive_skip_reason: String::new(),
                orientation_repaired: 0,
                edges_pinned: 0,
                simplified: false,
                simplification_rejected: false,
                thin_cells: 0,
                self_intersections: 0,
                self_intersections_complete: true,
                surface_nets: false,
                interval_dc: false,
                route: String::new(),
                dense_triangles: 0,
                validation,
            };
            (mesh, report)
        } else if matches!(
            method,
            MeshMethod::SurfaceNets | MeshMethod::SurfaceNetsBuiltin
        ) {
            // ── Surface Nets + decimation (robust generic route) ──
            // The user picked the dense mesher for non-1-Lipschitz
            // geometry; skip the parametric/adaptive routes entirely.
            // `builtin` selects our own implementations over the library
            // ones (fast-surface-nets / baby_shark).
            let builtin = method == MeshMethod::SurfaceNetsBuiltin;
            self.surface_nets_export_body(i, body_id, &feature_indices, size, cell_mm, builtin)
        } else if method == MeshMethod::IntervalDc {
            // ── Interval-octree Manifold DC (fidget) ── topology-safe for
            // non-1-Lipschitz fields, feature-preserving, no B-Rep needed.
            self.interval_dc_export_body(body_id, &feature_indices, size, cell_mm)
        } else if matches!(
            method,
            MeshMethod::UniformDc
                | MeshMethod::MarchingCubes
                | MeshMethod::ManifoldDc
                | MeshMethod::Rfta
        ) {
            // ── Generic dense meshers: uniform DC (QEF), Marching
            // Tetrahedra, the viewport Manifold-DC, Reach-for-the-Arcs.
            self.generic_dense_export_body(body_id, &feature_indices, size, cell_mm, method)
        } else {
            // ── Parametric route first (Meshing Phase 2, Option B) ──
            // Per-face B-Rep tessellation: watertight by construction,
            // exact edges, minimal triangles. It must EARN the export —
            // the same validator gates it; any failure falls back to
            // the generic MDC pass below.
            match self.try_parametric_export(i, cell_mm) {
                Ok((mesh, validation)) => {
                    let (si, si_complete) =
                        mesh_validate::count_self_intersections(&mesh, SELF_INTERSECT_PAIR_BUDGET);
                    let report = BodyExportReport {
                        body_id,
                        requested_cell_mm: cell_mm,
                        actual_cell_mm: 0.0,
                        resolution: 0,
                        cell_limited: false,
                        passthrough: false,
                        parametric: true,
                        parametric_skip_reason: String::new(),
                        adaptive: false,
                        adaptive_skip_reason: String::new(),
                        orientation_repaired: 0,
                        edges_pinned: 0,
                        simplified: false,
                        simplification_rejected: false,
                        thin_cells: 0,
                        self_intersections: si,
                        self_intersections_complete: si_complete,
                        surface_nets: false,
                        interval_dc: false,
                        route: String::new(),
                        dense_triangles: 0,
                        validation,
                    };
                    (mesh, report)
                }
                Err(skip_reason) => {
                    let (mesh, report) = self.mdc_export_body(
                        i,
                        body_id,
                        &feature_indices,
                        size,
                        cell_mm,
                        skip_reason,
                    );
                    (mesh, report)
                }
            }
        };

        let session = self.export_session.as_mut()?;
        session.meshes.push(mesh);
        session.reports.push(report.clone());
        Some(report)
    }

    /// The generic MDC export pass for one body (the Phase-0/1 route):
    /// raw Manifold-DC + Newton projection at cell-derived resolution,
    /// B-Rep edge pinning, simplification-as-candidate, deviation
    /// sampling. `body_index` selects the body for the edge-pinning
    /// weave lookup.
    #[allow(clippy::too_many_arguments)]
    fn mdc_export_body(
        &mut self,
        body_index: usize,
        body_id: u32,
        feature_indices: &[usize],
        size: [f32; 3],
        cell_mm: f32,
        parametric_skip_reason: String,
    ) -> (Mesh, BodyExportReport) {
        let (resolution, actual_cell, cell_limited) = derive_export_resolution(size, cell_mm);
        let (bounded, prims) =
            crate::body::build_bounded_with_primitives(&self.decoded, feature_indices);
        let prim_refs: std::collections::HashMap<u32, &Primitive> =
            prims.iter().map(|(k, v)| (*k, v)).collect();
        let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);

        // ── Phase 1c: adaptive Manifold-DC first ───────────────────
        // Narrow-band octree + Schaefer vertex clustering: the full
        // requested cell size everywhere on the surface, simplified by
        // the manifold-preserving cluster pass instead of the planar
        // heuristics. Must EARN the export via the same validator; any
        // failure falls through to the uniform pass below.
        let tol = (cell_mm / 4.0).clamp(0.01, 0.1);
        let pin_geo = self.build_pin_geometry(body_index, &sdf);
        // Wider band for bodies whose field may be far from 1-Lipschitz:
        // exactly the bodies the analytic gate rejects (blends, warps,
        // helix/sweep, non-Lipschitz leaves). The wider band costs a few
        // extra evals; a too-narrow band would cost holes.
        let guard = if self.check_analytically_exact(&feature_indices).is_ok() {
            crate::mdc_adaptive::LIPSCHITZ_GUARD_DEFAULT
        } else {
            crate::mdc_adaptive::LIPSCHITZ_GUARD_NON_LIPSCHITZ
        };
        let adaptive_attempt =
            crate::mdc_adaptive::adaptive_mdc(&bounded, cell_mm, tol, guard, &pin_geo).and_then(
                |mut result| {
                    let flips = mesh_validate::fix_orientation(&mut result.mesh, &sdf);
                    let mut validation = mesh_validate::validate_topology(&result.mesh);
                    if !validation.watertight {
                        return Err(format!(
                            "adaptive mesh not watertight ({} boundary, {} non-manifold, {} misoriented)",
                            validation.boundary_edges,
                            validation.non_manifold_edges,
                            validation.misoriented_edges
                        ));
                    }
                    let (max_dev, p95_dev, samples) = mesh_validate::measure_deviation(
                        &result.mesh,
                        DEVIATION_SAMPLE_BUDGET,
                        &sdf,
                    );
                    validation.max_deviation_mm = max_dev;
                    validation.p95_deviation_mm = p95_dev;
                    validation.deviation_samples = samples;
                    Ok((result, validation, flips))
                },
            );
        match adaptive_attempt {
            Ok((result, validation, flips)) => {
                let (si, si_complete) = mesh_validate::count_self_intersections(
                    &result.mesh,
                    SELF_INTERSECT_PAIR_BUDGET,
                );
                let report = BodyExportReport {
                    body_id,
                    requested_cell_mm: cell_mm,
                    actual_cell_mm: result.actual_cell_mm,
                    resolution: result.resolution,
                    cell_limited: false,
                    passthrough: false,
                    parametric: false,
                    parametric_skip_reason,
                    adaptive: true,
                    adaptive_skip_reason: String::new(),
                    orientation_repaired: flips,
                    edges_pinned: result.edges_pinned,
                    simplified: true,
                    simplification_rejected: false,
                    thin_cells: result.thin_cells,
                    self_intersections: si,
                    self_intersections_complete: si_complete,
                    surface_nets: false,
                    interval_dc: false,
                    route: String::new(),
                    dense_triangles: 0,
                    validation,
                };
                return (result.mesh, report);
            }
            Err(reason) => {
                return self.mdc_uniform_export_body(
                    body_index,
                    body_id,
                    &bounded,
                    &prim_refs,
                    resolution,
                    actual_cell,
                    cell_limited,
                    cell_mm,
                    parametric_skip_reason,
                    reason,
                );
            }
        }
    }

    /// The uniform-grid MDC pass (Phase 0/1a/1b) — the fallback when
    /// the adaptive pass fails its gates.
    #[allow(clippy::too_many_arguments)]
    fn mdc_uniform_export_body(
        &mut self,
        body_index: usize,
        body_id: u32,
        bounded: &crate::bvh::BoundedCsg,
        prim_refs: &std::collections::HashMap<u32, &Primitive>,
        resolution: u32,
        actual_cell: f32,
        cell_limited: bool,
        cell_mm: f32,
        parametric_skip_reason: String,
        adaptive_skip_reason: String,
    ) -> (Mesh, BodyExportReport) {
        let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);

        // Raw pass: Manifold-DC + Newton projection, then winding
        // repair (belt and braces — the geometric quad winding should
        // already be consistent since Phase 1a).
        let mut raw = crate::dc::dual_contour_unsimplified(bounded, resolution);
        let raw_flips = mesh_validate::fix_orientation(&mut raw, &sdf);

        // Phase 1b — edge pinning: MDC classifies each QEF vertex as
        // face/edge/corner from its contributor set, and the Newton
        // pass deliberately projects only FACE vertices onto the SDF.
        // Here the edge/corner vertices get their exact positions: the
        // nearest point on the woven B-Rep edge curves (µm-accurate)
        // resp. the nearest topological corner. Faces interpolate
        // between exact boundary and Newton-projected interior — sharp
        // edges land on the true curves instead of wiggling cell-sized.
        let grid_min = {
            let aabb = bounded.aabb();
            let g = aabb.expanded(actual_cell * crate::dc::DC_MARGIN_CELLS);
            g.min
        };
        let edges_pinned =
            self.pin_mesh_to_brep_edges(body_index, &mut raw, grid_min, actual_cell, &sdf);

        // Simplification candidate: the planar/developable pass cuts
        // the triangle count dramatically (box: ~40×), but its
        // re-triangulation can break topology — so it must EARN the
        // export by staying watertight when the raw mesh was.
        let mut candidate = raw.clone();
        crate::dc::simplify_dc_mesh(&mut candidate, bounded, prim_refs);
        let candidate_flips = mesh_validate::fix_orientation(&mut candidate, &sdf);

        let v_raw = mesh_validate::validate_topology(&raw);
        let v_candidate = mesh_validate::validate_topology(&candidate);
        let reject_simplified = v_raw.watertight && !v_candidate.watertight;
        let (mut mesh, mut validation, simplified, orientation_repaired) = if reject_simplified {
            (raw, v_raw, false, raw_flips)
        } else {
            (candidate, v_candidate, true, raw_flips + candidate_flips)
        };

        let (max_dev, p95_dev, samples) =
            mesh_validate::measure_deviation(&mesh, DEVIATION_SAMPLE_BUDGET, &sdf);
        validation.max_deviation_mm = max_dev;
        validation.p95_deviation_mm = p95_dev;
        validation.deviation_samples = samples;
        let (si, si_complete) =
            mesh_validate::count_self_intersections(&mesh, SELF_INTERSECT_PAIR_BUDGET);
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
            parametric: false,
            parametric_skip_reason,
            adaptive: false,
            adaptive_skip_reason,
            orientation_repaired,
            edges_pinned,
            simplified,
            simplification_rejected: reject_simplified,
            thin_cells: 0,
            self_intersections: si,
            self_intersections_complete: si_complete,
            surface_nets: false,
            interval_dc: false,
            route: String::new(),
            dense_triangles: 0,
            validation,
        };
        (mesh, report)
    }

    /// The Surface Nets + QEM decimation route (user-selected). A dense
    /// uniform Surface Nets pass captures the topology of a
    /// non-1-Lipschitz field reliably (every cell evaluated, no octree
    /// pruning); QEM edge collapse then decimates it back down under a
    /// deviation-derived error budget while staying watertight.
    #[allow(clippy::too_many_arguments)]
    fn surface_nets_export_body(
        &mut self,
        body_index: usize,
        body_id: u32,
        feature_indices: &[usize],
        size: [f32; 3],
        cell_mm: f32,
        builtin: bool,
    ) -> (Mesh, BodyExportReport) {
        let (resolution, actual_cell, cell_limited) = derive_export_resolution(size, cell_mm);
        let (bounded, _prims) =
            crate::body::build_bounded_with_primitives(&self.decoded, feature_indices);
        let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);
        let _ = body_index;

        // Library path (fast-surface-nets + baby_shark) vs our own
        // from-scratch implementations, per the dropdown selection.
        let mut mesh = if builtin {
            crate::uniform_contour::surface_nets_builtin(&bounded, resolution)
        } else {
            crate::surface_nets::surface_nets(&bounded, resolution)
        };
        let dense_triangles = mesh.triangle_count();

        // Error budget: the QEM cost is the summed squared distance to
        // the incident planes ≈ (chordal deviation)². Bound it by the
        // same tol the adaptive path targets.
        let tol = (cell_mm / 4.0).clamp(0.01, 0.1);
        let stats = if builtin {
            crate::decimate_builtin::simplify_qem_builtin(&mut mesh, tol * tol)
        } else {
            crate::decimate::simplify_qem(&mut mesh, tol * tol)
        };

        let flips = mesh_validate::fix_orientation(&mut mesh, &sdf);
        let mut validation = mesh_validate::validate_topology(&mesh);
        let (max_dev, p95_dev, samples) =
            mesh_validate::measure_deviation(&mesh, DEVIATION_SAMPLE_BUDGET, &sdf);
        validation.max_deviation_mm = max_dev;
        validation.p95_deviation_mm = p95_dev;
        validation.deviation_samples = samples;
        let (si, si_complete) =
            mesh_validate::count_self_intersections(&mesh, SELF_INTERSECT_PAIR_BUDGET);

        let report = BodyExportReport {
            body_id,
            requested_cell_mm: cell_mm,
            actual_cell_mm: actual_cell,
            resolution,
            cell_limited,
            passthrough: false,
            parametric: false,
            parametric_skip_reason: if builtin {
                "surface nets (built-in) selected".into()
            } else {
                "surface nets (library) selected".into()
            },
            adaptive: false,
            adaptive_skip_reason: "surface nets selected".into(),
            orientation_repaired: flips,
            edges_pinned: 0,
            simplified: stats.collapses > 0,
            simplification_rejected: false,
            thin_cells: 0,
            self_intersections: si,
            self_intersections_complete: si_complete,
            surface_nets: true,
            interval_dc: false,
            route: String::new(),
            dense_triangles,
            validation,
        };
        (mesh, report)
    }

    /// The interval-octree Manifold-DC route (fidget). Translates the
    /// body's field to a fidget `Tree` and meshes it with interval
    /// arithmetic + DC. Only closed-form bodies qualify; a body with a
    /// sketch/data carrier can't be expressed symbolically and falls back
    /// to the (robust) library Surface Nets path.
    fn interval_dc_export_body(
        &mut self,
        body_id: u32,
        feature_indices: &[usize],
        size: [f32; 3],
        cell_mm: f32,
    ) -> (Mesh, BodyExportReport) {
        let (bounded, _prims) =
            crate::body::build_bounded_with_primitives(&self.decoded, feature_indices);
        let aabb = bounded.aabb();
        let (resolution, actual_cell, cell_limited) = derive_export_resolution(size, cell_mm);

        let mut mesh = match crate::fidget_bridge::mesh_body_fidget(
            &bounded, aabb.min, aabb.max, cell_mm,
        ) {
            Ok(m) => m,
            Err(reason) => {
                // Not closed-form — fall back to the robust dense mesher so
                // the export still produces a solid, and record why.
                let (mesh, mut report) =
                    self.surface_nets_export_body(0, body_id, feature_indices, size, cell_mm, false);
                report.adaptive_skip_reason = format!("interval-DC fell back: {reason}");
                return (mesh, report);
            }
        };

        let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);
        let flips = mesh_validate::fix_orientation(&mut mesh, &sdf);
        let mut validation = mesh_validate::validate_topology(&mesh);
        let (max_dev, p95_dev, samples) =
            mesh_validate::measure_deviation(&mesh, DEVIATION_SAMPLE_BUDGET, &sdf);
        validation.max_deviation_mm = max_dev;
        validation.p95_deviation_mm = p95_dev;
        validation.deviation_samples = samples;
        let (si, si_complete) =
            mesh_validate::count_self_intersections(&mesh, SELF_INTERSECT_PAIR_BUDGET);

        let report = BodyExportReport {
            body_id,
            requested_cell_mm: cell_mm,
            actual_cell_mm: actual_cell,
            resolution,
            cell_limited,
            passthrough: false,
            parametric: false,
            parametric_skip_reason: "interval-DC selected".into(),
            adaptive: false,
            adaptive_skip_reason: "interval-DC selected".into(),
            orientation_repaired: flips,
            edges_pinned: 0,
            simplified: false,
            simplification_rejected: false,
            thin_cells: 0,
            self_intersections: si,
            self_intersections_complete: si_complete,
            surface_nets: false,
            interval_dc: true,
            route: String::new(),
            dense_triangles: 0,
            validation,
        };
        (mesh, report)
    }

    /// The generic dense-mesh routes selected in the dialog: built-in
    /// uniform DC (QEF), Marching Tetrahedra, the viewport's uniform
    /// Manifold-DC, and Reach-for-the-Arcs. Each produces a dense mesh
    /// (non-1-Lipschitz-safe), optionally QEM-decimates it, then
    /// validates/measures like the other routes.
    fn generic_dense_export_body(
        &mut self,
        body_id: u32,
        feature_indices: &[usize],
        size: [f32; 3],
        cell_mm: f32,
        method: MeshMethod,
    ) -> (Mesh, BodyExportReport) {
        let (resolution, actual_cell, cell_limited) = derive_export_resolution(size, cell_mm);
        let (bounded, prims) =
            crate::body::build_bounded_with_primitives(&self.decoded, feature_indices);
        let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);

        // `decimate` = follow the dense mesh with QEM. RFTA and the
        // viewport Manifold-DC already emit reconstructed / simplified
        // meshes, so they skip it.
        let (mut mesh, label, decimate) = match method {
            MeshMethod::UniformDc => (
                crate::uniform_contour::dual_contour_uniform(&bounded, resolution),
                "uniform DC",
                true,
            ),
            MeshMethod::ManifoldDc => {
                let prim_refs: std::collections::HashMap<u32, &Primitive> =
                    prims.iter().map(|(k, v)| (*k, v)).collect();
                (
                    crate::dc::dual_contour_with_primitives(&bounded, resolution, &prim_refs),
                    "manifold DC",
                    false,
                )
            }
            MeshMethod::Rfta => (
                crate::rfta::reach_for_the_arcs(&bounded, resolution),
                "reach-for-the-arcs",
                false,
            ),
            _ => (
                crate::marching_tets::marching_tets(&bounded, resolution),
                "marching cubes",
                true,
            ),
        };
        let dense_triangles = mesh.triangle_count();
        let route = format!("{label} {actual_cell:.3} mm");

        // Our own QEM decimator (not baby_shark): it tolerates the
        // occasional non-manifold edge these dense grids can carry —
        // baby_shark's corner-table build panics on such input — and its
        // sliver guard keeps the result degenerate-free.
        let tol = (cell_mm / 4.0).clamp(0.01, 0.1);
        let stats = if decimate {
            crate::decimate_builtin::simplify_qem_builtin(&mut mesh, tol * tol)
        } else {
            crate::decimate::DecimateStats {
                collapses: 0,
                triangles_before: dense_triangles,
                triangles_after: dense_triangles,
            }
        };
        // Only report a dense→decimated reduction when we actually decimated.
        let dense_reported = if decimate { dense_triangles } else { 0 };

        let flips = mesh_validate::fix_orientation(&mut mesh, &sdf);
        let mut validation = mesh_validate::validate_topology(&mesh);
        let (max_dev, p95_dev, samples) =
            mesh_validate::measure_deviation(&mesh, DEVIATION_SAMPLE_BUDGET, &sdf);
        validation.max_deviation_mm = max_dev;
        validation.p95_deviation_mm = p95_dev;
        validation.deviation_samples = samples;
        let (si, si_complete) =
            mesh_validate::count_self_intersections(&mesh, SELF_INTERSECT_PAIR_BUDGET);

        let report = BodyExportReport {
            body_id,
            requested_cell_mm: cell_mm,
            actual_cell_mm: actual_cell,
            resolution,
            cell_limited,
            passthrough: false,
            parametric: false,
            parametric_skip_reason: route.clone(),
            adaptive: false,
            adaptive_skip_reason: route.clone(),
            orientation_repaired: flips,
            edges_pinned: 0,
            simplified: stats.collapses > 0,
            simplification_rejected: false,
            thin_cells: 0,
            self_intersections: si,
            self_intersections_complete: si_complete,
            surface_nets: false,
            interval_dc: false,
            route,
            dense_triangles: dense_reported,
            validation,
        };
        (mesh, report)
    }

    /// World-space pin geometry (B-Rep edge polylines + corners) for
    /// the adaptive pass. Empty when the body has no woven B-Rep.
    ///
    /// Every curve is VALIDATED against the exact SDF before it may
    /// pin: the weave describes the hard-boolean skeleton, so bodies
    /// with blends (or fillets) carry ghost edges where the blended
    /// field has no sharp feature at all — the real surface can sit a
    /// blend radius away. Pinning onto such a curve would drag mesh
    /// vertices INTO the material (measured: 0.5 mm on a blended
    /// box∪sphere). A curve pins only if it lies on the field surface.
    fn build_pin_geometry(
        &mut self,
        body_index: usize,
        sdf: &dyn Fn([f32; 3]) -> f32,
    ) -> crate::mdc_adaptive::PinGeometry {
        let parametric = self.parametric_weave;
        self.bodies[body_index].ensure_brep(&self.decoded, parametric);
        let placement = self.bodies[body_index].placement;
        let Some(brep) = self.bodies[body_index].brep_cached() else {
            return crate::mdc_adaptive::PinGeometry::empty();
        };
        let place = |p: [f64; 3]| -> [f32; 3] {
            let scaled = [
                p[0] * placement.scale[0] as f64,
                p[1] * placement.scale[1] as f64,
                p[2] * placement.scale[2] as f64,
            ];
            let r = crate::quat_rotate(
                placement.rotation,
                [scaled[0] as f32, scaled[1] as f32, scaled[2] as f32],
            );
            [
                r[0] + placement.translate[0],
                r[1] + placement.translate[1],
                r[2] + placement.translate[2],
            ]
        };
        // A woven curve is trusted to ~µm; anything beyond this off the
        // field surface is a ghost edge (blend/fillet zone) and must
        // not attract vertices.
        const ON_SURFACE_TOL: f32 = 0.02;
        let on_surface = |pts: &[[f32; 3]]| -> bool {
            let stride = (pts.len() / 16).max(1);
            pts.iter()
                .step_by(stride)
                .chain(pts.last())
                .all(|&p| sdf(p).abs() <= ON_SURFACE_TOL)
        };
        let polylines: Vec<Vec<[f32; 3]>> = brep
            .edges
            .iter()
            .map(|e| {
                crate::brep::edge_sampling::sample_edge_dense(&e.geometry, 32)
                    .into_iter()
                    .map(place)
                    .collect::<Vec<[f32; 3]>>()
            })
            .filter(|p: &Vec<[f32; 3]>| p.len() >= 2 && on_surface(p))
            .collect();
        let corners: Vec<[f32; 3]> = brep
            .vertices
            .iter()
            .map(|v| place(v.position))
            .filter(|&p| sdf(p).abs() <= ON_SURFACE_TOL)
            .collect();
        crate::mdc_adaptive::PinGeometry { polylines, corners }
    }

    /// Phase 1b: snap MDC vertices onto the exact woven B-Rep edge
    /// curves / topological corners, using the classic DC pinning
    /// criterion — a vertex is pinned iff its OWN grid cell is crossed
    /// by an edge curve (the nearest curve point lies inside that
    /// cell). One vertex per crossed cell forms an exact polyline
    /// chain along every sharp edge; neighbouring face vertices whose
    /// cells do not touch the curve stay untouched, so faces cannot
    /// collapse onto the edge. (The mesh CLASSIFICATION channel cannot
    /// drive this: it works at feature granularity, so a primitive's
    /// intrinsic edges — all six box edge loops — are face-classified.)
    ///
    /// Returns the number of pinned vertices. No-op when the body has
    /// no woven B-Rep (blend/warp bodies — their surfaces have no
    /// sharp edges to pin) or no edges.
    fn pin_mesh_to_brep_edges(
        &mut self,
        body_index: usize,
        mesh: &mut Mesh,
        grid_min: [f32; 3],
        cell: f32,
        sdf: &dyn Fn([f32; 3]) -> f32,
    ) -> usize {
        let parametric = self.parametric_weave;
        self.bodies[body_index].ensure_brep(&self.decoded, parametric);
        let placement = self.bodies[body_index].placement;
        let Some(brep) = self.bodies[body_index].brep_cached() else {
            return 0;
        };
        if brep.edges.is_empty() {
            return 0;
        }

        let place = |p: [f64; 3]| -> [f32; 3] {
            let scaled = [
                p[0] * placement.scale[0] as f64,
                p[1] * placement.scale[1] as f64,
                p[2] * placement.scale[2] as f64,
            ];
            let r = crate::quat_rotate(
                placement.rotation,
                [scaled[0] as f32, scaled[1] as f32, scaled[2] as f32],
            );
            [
                r[0] + placement.translate[0],
                r[1] + placement.translate[1],
                r[2] + placement.translate[2],
            ]
        };

        // World-space edge polylines + per-edge AABB for cheap pruning.
        struct EdgePoly {
            pts: Vec<[f32; 3]>,
            min: [f32; 3],
            max: [f32; 3],
        }
        // Ghost-edge validation (see build_pin_geometry): blend/fillet
        // bodies carry hard-boolean weave edges the field has rounded
        // away — those must not attract vertices into the material.
        const ON_SURFACE_TOL: f32 = 0.02;
        let mut polys: Vec<EdgePoly> = Vec::with_capacity(brep.edges.len());
        for edge in &brep.edges {
            let pts: Vec<[f32; 3]> =
                crate::brep::edge_sampling::sample_edge_dense(&edge.geometry, 32)
                    .into_iter()
                    .map(place)
                    .collect();
            if pts.len() < 2 {
                continue;
            }
            let stride = (pts.len() / 16).max(1);
            if !pts
                .iter()
                .step_by(stride)
                .chain(pts.last())
                .all(|&p| sdf(p).abs() <= ON_SURFACE_TOL)
            {
                continue;
            }
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for p in &pts {
                for k in 0..3 {
                    min[k] = min[k].min(p[k]);
                    max[k] = max[k].max(p[k]);
                }
            }
            polys.push(EdgePoly { pts, min, max });
        }
        let corners: Vec<[f32; 3]> = brep
            .vertices
            .iter()
            .map(|v| place(v.position))
            .filter(|&p| sdf(p).abs() <= ON_SURFACE_TOL)
            .collect();

        // A cell can only be crossed by a curve whose distance to the
        // cell-clamped vertex is under one cell diagonal.
        let tau = cell * 1.75;
        let tau2 = tau * tau;
        // Cell membership slack: the curve counts as crossing the cell
        // when its nearest point lies inside the cell expanded by this.
        let slack = cell * 0.05;

        // The vertex's own cell bounds (vertices are clamped into their
        // cell by the MDC pass, so this reconstruction is exact).
        let cell_bounds = |p: [f32; 3]| -> ([f32; 3], [f32; 3]) {
            let mut lo = [0.0f32; 3];
            let mut hi = [0.0f32; 3];
            for k in 0..3 {
                let i = ((p[k] - grid_min[k]) / cell).floor();
                lo[k] = grid_min[k] + i * cell - slack;
                hi[k] = grid_min[k] + (i + 1.0) * cell + slack;
            }
            (lo, hi)
        };
        let inside = |q: [f32; 3], lo: [f32; 3], hi: [f32; 3]| -> bool {
            (0..3).all(|k| q[k] >= lo[k] && q[k] <= hi[k])
        };

        // Nearest curve point to `p` that lies INSIDE the given cell.
        let nearest_in_cell = |p: [f32; 3], lo: [f32; 3], hi: [f32; 3]| -> Option<([f32; 3], f32)> {
            let mut best: Option<([f32; 3], f32)> = None;
            for poly in &polys {
                let mut d2 = 0.0f32;
                for k in 0..3 {
                    let d = (poly.min[k] - tau - p[k]).max(p[k] - poly.max[k] - tau).max(0.0);
                    d2 += d * d;
                }
                if d2 > tau2 {
                    continue;
                }
                for w in poly.pts.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
                    let t = if len2 > 1e-20 {
                        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1] + (p[2] - a[2]) * ab[2])
                            / len2)
                            .clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
                    if !inside(q, lo, hi) {
                        continue;
                    }
                    let dq = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                    let d2 = dq[0] * dq[0] + dq[1] * dq[1] + dq[2] * dq[2];
                    if best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
                        best = Some((q, d2));
                    }
                }
            }
            best
        };

        // Occupancy guard: two neighbouring cell vertices snapping onto
        // the SAME point (both clamped to a corner) would create a
        // degenerate/welded pair — first pin wins, later ones skip.
        let mut occupied: std::collections::HashSet<[u32; 3]> = std::collections::HashSet::new();
        let quantize = |p: [f32; 3]| -> [u32; 3] {
            [
                ((p[0] * 8192.0).round() as i32) as u32,
                ((p[1] * 8192.0).round() as i32) as u32,
                ((p[2] * 8192.0).round() as i32) as u32,
            ]
        };

        let mut pinned = 0usize;
        for v in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let (lo, hi) = cell_bounds(p);
            // A topological corner inside the cell takes priority over
            // the edge curves running through it.
            let corner = corners
                .iter()
                .filter(|&&q| inside(q, lo, hi))
                .map(|&q| {
                    let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                    (q, d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let target = corner.or_else(|| nearest_in_cell(p, lo, hi));
            let Some((q, _)) = target else { continue };
            if !occupied.insert(quantize(q)) {
                continue;
            }
            mesh.positions[v * 3] = q[0];
            mesh.positions[v * 3 + 1] = q[1];
            mesh.positions[v * 3 + 2] = q[2];
            pinned += 1;
        }
        pinned
    }

    /// Attempt the parametric per-face B-Rep tessellation for body `i`.
    /// `Err(reason)` sends the body down the generic MDC route.
    ///
    /// Three gates, in order:
    /// 1. **CSG scan** — the body's tree must be analytically exact
    ///    (no blends/warps/fillets whose geometry the woven B-Rep does
    ///    not carry; a sparse deviation sample could miss a small
    ///    fillet, so this is checked structurally).
    /// 2. **Tessellation** — every face must sit on a supported
    ///    analytic carrier and every loop must chain (see
    ///    [`crate::brep::tessellate`]).
    /// 3. **Validation** — the mesh must measure watertight and inside
    ///    tolerance against the exact SDF.
    fn try_parametric_export(
        &mut self,
        i: usize,
        cell_mm: f32,
    ) -> Result<(Mesh, MeshValidation), String> {
        let feature_indices = self.bodies[i].feature_indices.clone();
        self.check_analytically_exact(&feature_indices)?;

        // Chord tolerance for edges + face sag, derived from the dialog
        // cell size (a 0.4 mm grid targets ~0.1 mm chords).
        let tol = (cell_mm as f64 / 4.0).clamp(0.01, 0.1);

        let parametric = self.parametric_weave;
        self.bodies[i].ensure_brep(&self.decoded, parametric);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i]
            .brep_cached()
            .ok_or_else(|| "no woven B-Rep".to_string())?;
        let mut mesh = crate::brep::tessellate::tessellate_brep(brep, &placement, tol)?;

        // Validation gate — identical instruments as the MDC route.
        let bounded = crate::body::build_bounded_for_body(&self.decoded, &feature_indices);
        let sdf = |p: [f32; 3]| bounded.sdf(p, crate::csg::EvalMode::Visual);
        let flips = mesh_validate::fix_orientation(&mut mesh, &sdf);
        let mut validation = mesh_validate::validate_topology(&mesh);
        if !validation.watertight {
            return Err(format!(
                "parametric mesh not watertight ({} boundary, {} non-manifold, {} misoriented)",
                validation.boundary_edges,
                validation.non_manifold_edges,
                validation.misoriented_edges
            ));
        }
        let (max_dev, p95_dev, samples) =
            mesh_validate::measure_deviation(&mesh, DEVIATION_SAMPLE_BUDGET, &sdf);
        // The B-Rep and the SDF describe the same analytic solid; the
        // measured deviation is chord sag only. Anything beyond a small
        // multiple of the tolerance means the B-Rep disagrees with the
        // field — do not ship it.
        let dev_gate = (tol * 4.0).max(0.05) as f32;
        if max_dev > dev_gate {
            return Err(format!(
                "parametric mesh deviates {max_dev:.4} mm from the SDF (gate {dev_gate:.4})"
            ));
        }
        let _ = flips;
        validation.max_deviation_mm = max_dev;
        validation.p95_deviation_mm = p95_dev;
        validation.deviation_samples = samples;
        Ok((mesh, validation))
    }

    /// Gate 1 of the parametric route: every CSG node in the body must
    /// be exactly representable by its woven B-Rep. Blends, warps and
    /// edge fillets shape the SDF but not the woven faces — a parametric
    /// mesh would silently miss them.
    fn check_analytically_exact(&self, feature_indices: &[usize]) -> Result<(), String> {
        if !self.edge_fillets.is_empty() {
            return Err("document has edge fillets (SDF-only geometry)".into());
        }
        for &fi in feature_indices {
            check_node_analytic(&self.decoded[fi].node)?;
        }
        Ok(())
    }

    pub(crate) fn export_session_meshes(&self) -> Vec<&Mesh> {
        self.export_session
            .as_ref()
            .map(|s| s.meshes.iter().collect())
            .unwrap_or_default()
    }
}

/// Walk a body's CSG tree and reject any node whose geometry the woven
/// B-Rep does not carry exactly. Hard booleans, transforms and the
/// analytic leaves pass; blend/warp operators and non-Lipschitz or
/// mesh-import leaves do not. Shell/Offset/Extrude/Revolve weave
/// analytically (their carriers fold the operation in) and pass — a
/// carrier the weave could NOT lift (spline extrude wall) shows up as a
/// tessellated face and is rejected by gate 2 instead.
fn check_node_analytic(node: &crate::csg::CsgNode) -> Result<(), String> {
    use crate::csg::CsgNode as N;
    match node {
        N::Leaf { primitive, .. } => {
            if primitive.is_non_lipschitz() {
                return Err(format!(
                    "non-Lipschitz primitive {}",
                    primitive.default_name()
                ));
            }
            if matches!(primitive, Primitive::ImportedMesh { .. }) {
                return Err("imported-mesh leaf inside a multi-feature body".into());
            }
            Ok(())
        }
        N::Transform(_, a) | N::Shell(a, _, _) | N::Offset(a, _) => check_node_analytic(a),
        N::Union(a, b) | N::Difference(a, b) | N::Intersection(a, b) => {
            check_node_analytic(a)?;
            check_node_analytic(b)
        }
        N::Empty | N::Extrude { .. } | N::Revolve { .. } => Ok(()),
        N::SmoothUnion(..) | N::SmoothDifference(..) => {
            Err("smooth boolean (blend zone is SDF-only)".into())
        }
        N::MaskedBlend { .. } => Err("masked blend (SDF-only)".into()),
        N::Twist(..) | N::Bend(..) | N::Displace(..) | N::Morph(..) => {
            Err("warp/morph operator (SDF-only)".into())
        }
        N::Helix { .. } | N::Sweep { .. } => {
            Err("helix/sweep (weave-excluded geometry)".into())
        }
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

        let n = k.export_mesh_begin(0.5, "adaptive".into());
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
        // Phase 2: a plain box takes the parametric per-face route —
        // 6 planar faces tessellate to exactly 12 triangles.
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert_eq!(report["total_triangles"].as_u64().unwrap(), 12);

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

    /// Run one body through the session and return its report JSON.
    fn export_single(k: &mut crate::Kernel, cell: f32) -> serde_json::Value {
        export_single_method(k, cell, "adaptive")
    }

    fn export_single_method(
        k: &mut crate::Kernel,
        cell: f32,
        method: &str,
    ) -> serde_json::Value {
        let n = k.export_mesh_begin(cell, method.into());
        assert!(n >= 1, "expected at least one body");
        for i in 0..n {
            k.export_mesh_body(i);
        }
        let report: serde_json::Value =
            serde_json::from_str(&k.export_mesh_report()).unwrap();
        report
    }

    /// Reproduce ticket 9ocx: sphere r50 ∩ gyroid(cell 10, thick 1) − cyl r30.
    fn ticket_9ocx_kernel() -> crate::Kernel {
        let mut k = crate::Kernel::new();
        let sphere = k
            .add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 50.0, 0.0, 0.0)
            .unwrap();
        k.add_feature_call(
            "gyroid",
            &format!(
                r#"{{ "cell_size": {{ "value": 10.0, "unit": "mm" }},
                      "thickness": {{ "value": 1.0, "unit": "mm" }},
                      "op": "intersect", "targets": "qCreatedBy({sphere})" }}"#
            ),
        )
        .unwrap();
        k.add_feature_call(
            "cylinder",
            &format!(
                r#"{{ "radius": 30.0, "half_height": 60.0,
                      "op": "remove", "targets": "qCreatedBy({sphere})" }}"#
            ),
        )
        .unwrap();
        assert!(k.feature_errors.is_empty(), "feature errors {:?}", k.feature_errors);
        k
    }

    /// A gyroid shell ∩ box: same non-1-Lipschitz field as the ticket, made
    /// finite by the box. Shared by the marching-tets regression test.
    fn gyroid_in_box() -> crate::Kernel {
        let mut k = crate::Kernel::new();
        let b = k
            .add_primitive(Primitive::KIND_BOX, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 40.0, 40.0, 40.0)
            .unwrap();
        k.add_feature_call(
            "gyroid",
            &format!(
                r#"{{ "cell_size": {{ "value": 10.0, "unit": "mm" }},
                      "thickness": {{ "value": 1.0, "unit": "mm" }},
                      "op": "intersect", "targets": "qCreatedBy({b})" }}"#
            ),
        )
        .unwrap();
        k
    }

    /// Ticket 9ocx: Marching Tetrahedra is watertight + 2-manifold **by
    /// construction** — every zero-crossing shared between two cells must
    /// weld. It didn't: the marching cases interpolate a crossing from the
    /// "singleton" corner, whose local identity flips between the two cells
    /// sharing a face diagonal, so one computed `edge(a,b)` and the other
    /// `edge(b,a)`; since `t = va/(va−vb)` isn't symmetric in floating point
    /// the two points differed in the last bit and never welded — a plain
    /// sphere leaked 14 672 open edges. Interpolating from the
    /// position-canonical lower end fixes it field-agnostically.
    #[test]
    fn marching_tets_is_watertight_by_construction() {
        // Smooth solid — must be fully watertight AND 2-manifold.
        let mut sphere = crate::Kernel::new();
        sphere
            .add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 50.0, 0.0, 0.0)
            .unwrap();
        let r = export_single_method(&mut sphere, 2.5, "marching-cubes");
        let b = &r["bodies"][0];
        assert_eq!(b["boundary_edges"], 0, "sphere leaked open edges: {r}");
        assert_eq!(b["non_manifold_edges"], 0, "sphere non-manifold: {r}");
        assert_eq!(b["watertight"], true, "sphere not watertight: {r}");

        // The gyroid shell the ticket is about — the fix is field-agnostic, so
        // even this strongly non-1-Lipschitz field meshes watertight.
        let mut gb = gyroid_in_box();
        let r = export_single_method(&mut gb, 2.0, "marching-cubes");
        let b = &r["bodies"][0];
        assert_eq!(b["boundary_edges"], 0, "gyroid∩box leaked open edges: {r}");
        assert_eq!(b["watertight"], true, "gyroid∩box not watertight: {r}");

        // The full ticket model (sphere ∩ gyroid − cylinder). CSG trim seams
        // still leave a few non-manifold weld junctions (distinct surface
        // sheets rounding to identical bits), but there must be NO holes — the
        // ticket reported 27 494 open edges here.
        let mut full = ticket_9ocx_kernel();
        let r = export_single_method(&mut full, 2.0, "marching-cubes");
        assert_eq!(
            r["bodies"][0]["boundary_edges"], 0,
            "full ticket model still has holes: {r}"
        );
    }

    #[test]
    fn surface_nets_meshes_a_gyroid_watertight() {
        // The motivating case: a TPMS gyroid is a strongly non-1-Lipschitz
        // field the adaptive octree's distance bound can misjudge. Dense
        // Surface Nets evaluates every cell, so a gyroid lattice clipped to
        // a box must mesh watertight. (Intersecting with the box makes the
        // otherwise-infinite lattice a finite solid.)
        let mut k = crate::Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                12.0,
                12.0,
                12.0,
            )
            .unwrap();
        k.add_feature_call(
            "gyroid",
            &format!(
                r#"{{
                    "cell_size": {{ "value": 6.0, "unit": "mm" }},
                    "thickness": {{ "value": 1.5, "unit": "mm" }},
                    "op": "intersect",
                    "targets": "qCreatedBy({box_id})"
                }}"#
            ),
        )
        .unwrap();
        assert!(
            k.feature_errors.is_empty(),
            "feature errors {:?}",
            k.feature_errors
        );
        let report = export_single_method(&mut k, 0.5, "surface-nets");
        let body = &report["bodies"][0];
        assert_eq!(body["surface_nets"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        // A gyroid lattice has lots of surface — many triangles even after
        // decimation, and the dense mesh was much larger still.
        let dense = body["dense_triangles"].as_u64().unwrap();
        let tris = report["total_triangles"].as_u64().unwrap();
        assert!(tris > 500, "gyroid too sparse: {tris}");
        assert!(dense >= tris, "dense {dense} < decimated {tris}");
    }

    /// RFTA (screened Poisson) is slow in a debug build (super-linear
    /// solver); run in release: `cargo test --release rfta_route -- --ignored`.
    #[test]
    #[ignore]
    fn rfta_route_produces_solid() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report = export_single_method(&mut k, 0.5, "rfta");
        let body = &report["bodies"][0];
        assert!(
            body["route"].as_str().unwrap().starts_with("reach-for-the-arcs"),
            "{report}"
        );
        assert!(report["total_triangles"].as_u64().unwrap() > 50, "{report}");
    }

    #[test]
    fn manifold_dc_sphere_is_watertight() {
        // The viewport's uniform Manifold-DC, now exported, stays
        // watertight on a plain sphere (raw MDC winding is consistent).
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report = export_single_method(&mut k, 0.5, "manifold-dc");
        assert_eq!(report["all_watertight"], true, "{report}");
    }

    #[test]
    fn uniform_dc_sharp_edged_solid_is_watertight() {
        // Built-in uniform Dual Contouring on a box with a cylindrical
        // hole: the uniform grid keeps it watertight (unlike fidget's
        // adaptive DC at sharp edges) and QEF places vertices accurately.
        let mut k = box_with_hole();
        let report = export_single_method(&mut k, 0.5, "uniform-dc");
        let body = &report["bodies"][0];
        assert_eq!(report["all_watertight"], true, "{report}");
        assert_eq!(body["route"], "uniform DC 0.500 mm", "{report}");
        assert!(body["p95_deviation_mm"].as_f64().unwrap() < 0.05, "{report}");
    }

    #[test]
    fn marching_cubes_meshes_a_gyroid_watertight() {
        // Marching Tetrahedra is watertight/manifold by construction for
        // ANY field — including the non-1-Lipschitz TPMS gyroid∩box that
        // fidget's DC leaves a few non-manifold edges on.
        let mut k = crate::Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                12.0,
                12.0,
                12.0,
            )
            .unwrap();
        k.add_feature_call(
            "gyroid",
            &format!(
                r#"{{
                    "cell_size": {{ "value": 6.0, "unit": "mm" }},
                    "thickness": {{ "value": 1.5, "unit": "mm" }},
                    "op": "intersect",
                    "targets": "qCreatedBy({box_id})"
                }}"#
            ),
        )
        .unwrap();
        assert!(k.feature_errors.is_empty(), "{:?}", k.feature_errors);
        let report = export_single_method(&mut k, 0.5, "marching-cubes");
        let body = &report["bodies"][0];
        assert_eq!(report["all_watertight"], true, "{report}");
        assert_eq!(body["route"], "marching cubes 0.500 mm", "{report}");
        assert!(
            report["total_triangles"].as_u64().unwrap() > 500,
            "gyroid too sparse: {report}"
        );
    }

    #[test]
    fn interval_dc_sharp_edged_solid_is_accurate() {
        // A closed-form solid with sharp edges (box with a cylindrical
        // through-hole). fidget's QEF preserves the sharp rim edges and the
        // interval octree keeps the faces on-surface (p95 well under the
        // tolerance). CAVEAT: adaptive DC leaves a few non-manifold edges
        // where the sharp rim crosses cell boundaries — for guaranteed
        // watertight use Surface Nets; here we assert accuracy + that the
        // defects are a tiny fraction.
        let mut k = box_with_hole();
        let report = export_single_method(&mut k, 0.5, "interval-dc");
        let body = &report["bodies"][0];
        assert!(
            body["p95_deviation_mm"].as_f64().unwrap() < 0.05,
            "p95 deviation {}",
            body["p95_deviation_mm"]
        );
        let tris = body["triangles"].as_u64().unwrap();
        let defects = body["boundary_edges"].as_u64().unwrap()
            + body["non_manifold_edges"].as_u64().unwrap();
        assert!(defects * 500 < tris, "defects {defects} for {tris} tris");
    }

    #[test]
    fn interval_dc_meshes_a_gyroid() {
        // The interval octree meshes the non-1-Lipschitz TPMS gyroid∩box
        // feature-preservingly. NOTE: where the thin sheets meet the sharp
        // box clip, adaptive DC leaves a handful of boundary/non-manifold
        // edges (the classic DC-vs-manifold tension) — for guaranteed-
        // watertight TPMS use Surface Nets. Here we assert it produces a
        // substantial solid and that the defects stay a tiny fraction.
        let mut k = crate::Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                12.0,
                12.0,
                12.0,
            )
            .unwrap();
        k.add_feature_call(
            "gyroid",
            &format!(
                r#"{{
                    "cell_size": {{ "value": 6.0, "unit": "mm" }},
                    "thickness": {{ "value": 1.5, "unit": "mm" }},
                    "op": "intersect",
                    "targets": "qCreatedBy({box_id})"
                }}"#
            ),
        )
        .unwrap();
        assert!(k.feature_errors.is_empty(), "{:?}", k.feature_errors);
        let report = export_single_method(&mut k, 0.5, "interval-dc");
        let body = &report["bodies"][0];
        let tris = body["triangles"].as_u64().unwrap();
        assert!(tris > 5000, "gyroid too sparse via interval-DC: {tris}");
        let defects = body["boundary_edges"].as_u64().unwrap()
            + body["non_manifold_edges"].as_u64().unwrap();
        assert!(
            defects * 1000 < tris,
            "interval-DC gyroid defects {defects} too high for {tris} tris"
        );
    }

    #[test]
    fn interval_dc_meshes_a_sphere_watertight_and_accurate() {
        // A closed-form sphere via interval-DC: watertight and — thanks to
        // interval sampling of the exact expression — well under the export
        // tolerance envelope.
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report = export_single_method(&mut k, 0.4, "interval-dc");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.4,
            "deviation {}",
            report["max_deviation_mm"]
        );
    }

    #[test]
    fn surface_nets_builtin_route_is_watertight_and_deterministic() {
        // The from-scratch alternative (own Surface Nets + own QEM) must
        // also produce a watertight, decimated solid — and be
        // deterministic (identical triangle count on a re-run), which the
        // export requires.
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report = export_single_method(&mut k, 0.5, "surface-nets-builtin");
        let body = &report["bodies"][0];
        assert_eq!(body["surface_nets"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        let dense = body["dense_triangles"].as_u64().unwrap();
        let tris = report["total_triangles"].as_u64().unwrap();
        assert!(dense > 0 && tris < dense, "decimation reduced: {report}");
        assert!(tris > 100, "too few triangles: {tris}");

        // Re-run: identical triangle count ⇒ deterministic pipeline.
        let mut k2 = crate::Kernel::new();
        k2.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report2 = export_single_method(&mut k2, 0.5, "surface-nets-builtin");
        assert_eq!(
            report2["total_triangles"], report["total_triangles"],
            "built-in pipeline must be deterministic"
        );
    }

    #[test]
    fn surface_nets_route_is_watertight_and_decimates() {
        // A sphere would normally take the exact parametric route; forcing
        // the Surface Nets method exercises the dense-mesh + QEM path and
        // must still produce a watertight solid under the deviation bound,
        // with decimation cutting the dense triangle count substantially.
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report = export_single_method(&mut k, 0.5, "surface-nets");
        let body = &report["bodies"][0];
        assert_eq!(body["surface_nets"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        // Dense Surface Nets + decimation is looser than the parametric
        // route but must still respect the export tolerance envelope.
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.4,
            "max deviation {}",
            report["max_deviation_mm"]
        );
        let dense = body["dense_triangles"].as_u64().unwrap();
        let tris = report["total_triangles"].as_u64().unwrap();
        assert!(dense > 0, "dense triangle count reported: {report}");
        assert!(
            tris < dense,
            "decimation should reduce triangles: {tris} vs dense {dense}"
        );
        // Watertight ⇒ a genuinely closed solid, not a fragment.
        assert!(tris > 100, "too few triangles for a sphere: {tris}");
    }

    #[test]
    fn cylinder_exports_parametric_watertight() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            8.0,
            15.0,
            0.0,
        )
        .unwrap();
        let report = export_single(&mut k, 0.4);
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.1,
            "{report}"
        );
        // Sag-driven barrel + two earcut caps: far below any voxel mesh.
        let tris = report["total_triangles"].as_u64().unwrap();
        assert!(tris < 2000, "triangles {tris}");
    }

    #[test]
    fn sphere_exports_parametric_watertight() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_SPHERE,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            0.0,
            0.0,
        )
        .unwrap();
        let report = export_single(&mut k, 0.4);
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.1,
            "{report}"
        );
    }

    #[test]
    fn box_minus_cylinder_hole_exports_parametric_watertight() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            15.0,
            10.0,
            15.0,
        )
        .unwrap();
        // Cylinder axis is local Y — a through-hole in the box top face.
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_REMOVE,
            0.0,
            0.0,
            0.0,
            4.0,
            25.0,
            0.0,
        )
        .unwrap();
        let report = export_single(&mut k, 0.4);
        // Planes with an inner circular loop (hole bridging) + a full
        // cylinder band between the two rim circles.
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.1,
            "{report}"
        );
    }

    #[test]
    fn union_box_cylinder_partial_patch_exports_watertight() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_BOX,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            10.0,
            10.0,
            10.0,
        )
        .unwrap();
        // Overlapping cylinder sticking out of the box side: the barrel
        // is a boolean-trimmed partial patch (winding 0 → earcut +
        // refinement route).
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_ADD,
            8.0,
            0.0,
            0.0,
            5.0,
            8.0,
            0.0,
        )
        .unwrap();
        let report = export_single(&mut k, 0.4);
        // Watertight is the hard requirement; the route may fall back
        // to MDC if the weave leaves this configuration incomplete.
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.15,
            "{report}"
        );
    }

    #[test]
    fn analytic_gate_rejects_sdf_only_operators() {
        use crate::csg::{BlendMode, CsgNode};
        let smooth = CsgNode::SmoothUnion(
            Box::new(CsgNode::Empty),
            Box::new(CsgNode::Empty),
            2.0,
            BlendMode::Fillet,
        );
        assert!(check_node_analytic(&smooth)
            .unwrap_err()
            .contains("smooth"));
        let twist = CsgNode::Twist(Box::new(CsgNode::Empty), 0.5);
        assert!(check_node_analytic(&twist).unwrap_err().contains("warp"));
        // Hard booleans over Empty pass the structural gate.
        let hard = CsgNode::Union(Box::new(CsgNode::Empty), Box::new(CsgNode::Empty));
        assert!(check_node_analytic(&hard).is_ok());
    }

    /// The canonical vase (same profile as the UI vase exercise and
    /// `features_tests::vase_sketch_and_revolve_calls`): a spline-walled
    /// revolve about the sketch's x=0 axis line. The spline wall weaves
    /// as a `RevolvedProfile` carrier — the Phase-2b route.
    fn vase_document() -> String {
        serde_json::json!({
            "schema_version": 2,
            "name": "vase",
            "next_feature_id": 3,
            "feature_calls": [
                {
                    "type": "sketch",
                    "id": "1",
                    "params": {
                        "plane": "qCreatedBy(origin_plane_xy)",
                        "sub_calls": [
                            { "type": "sketch_point", "id": "p1", "params": { "at": [0.0, -70.0] } },
                            { "type": "sketch_point", "id": "p2", "params": { "at": [0.0, 68.0] } },
                            { "type": "sketch_point", "id": "p3", "params": { "at": [45.0, 68.0] } },
                            { "type": "sketch_point", "id": "p4", "params": { "at": [30.0, -62.0] } },
                            { "type": "sketch_point", "id": "p5", "params": { "at": [26.0, -70.0] } },
                            { "type": "sketch_line", "id": "l1", "params": { "start": [0.0, -70.0], "end": [0.0, 68.0] } },
                            { "type": "sketch_line", "id": "l2", "params": { "start": [0.0, 68.0], "end": [45.0, 68.0] } },
                            { "type": "sketch_line", "id": "l3", "params": { "start": [30.0, -62.0], "end": [26.0, -70.0] } },
                            { "type": "sketch_line", "id": "l4", "params": { "start": [26.0, -70.0], "end": [0.0, -70.0] } },
                            { "type": "sketch_spline", "id": "s1", "params": {
                                "points": [45.0, 68.0, 33.044, 50.471, 26.707, 42.324, 38.476, 20.596, 50.245, -9.732, 44.360, -45.039, 30.0, -62.0],
                                "closed": false, "mode": "interpolate" } }
                        ]
                    }
                },
                {
                    "type": "revolve",
                    "id": "2",
                    "params": {
                        "profiles": "qCreatedBy(1)",
                        "axis": "qSketchLine(1, 8)"
                    }
                }
            ]
        })
        .to_string()
    }

    #[test]
    fn spline_revolve_vase_exports_parametric_watertight() {
        let mut k = crate::Kernel::new();
        // Native tests must avoid the wasm-bindgen `load_json` wrapper
        // (JsError::new panics off-wasm) — drive the same internals.
        k.document = serde_json::from_str(&vase_document()).unwrap();
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();
        let report = export_single(&mut k, 0.4);
        assert_eq!(report["bodies"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.15,
            "{report}"
        );
        eprintln!("VASE REPORT: {report}");
    }

    /// A dome: spline profile touching the axis at the top, revolved —
    /// the wall weaves as a `RevolvedProfile` whose face closes onto
    /// the axis (pole-disk topology, one once-winding rim, no inner
    /// loops).
    #[test]
    fn spline_dome_with_pole_exports_parametric_watertight() {
        let mut k = crate::Kernel::new();
        let doc = serde_json::json!({
            "schema_version": 2,
            "name": "dome",
            "next_feature_id": 3,
            "feature_calls": [
                {
                    "type": "sketch",
                    "id": "1",
                    "params": {
                        "plane": "qCreatedBy(origin_plane_xy)",
                        "sub_calls": [
                            { "type": "sketch_line", "id": "l1", "params": { "start": [0.0, 0.0], "end": [0.0, 20.0] } },
                            { "type": "sketch_arc", "id": "a1", "params": {
                                "center": [0.0, 12.0], "radius": 8.0,
                                "start_angle": 1.5707963, "sweep_angle": -1.5707963 } },
                            { "type": "sketch_line", "id": "l2", "params": { "start": [8.0, 12.0], "end": [8.0, 0.0] } },
                            { "type": "sketch_line", "id": "l3", "params": { "start": [8.0, 0.0], "end": [0.0, 0.0] } }
                        ]
                    }
                },
                {
                    "type": "revolve",
                    "id": "2",
                    "params": { "profiles": "qCreatedBy(1)", "axis": "qSketchLine(1, 3)" }
                }
            ]
        })
        .to_string();
        k.document = serde_json::from_str(&doc).unwrap();
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();

        let report = export_single(&mut k, 0.4);
        eprintln!("DOME REPORT: {report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.15,
            "{report}"
        );
    }

    /// The HOLLOW vase: 2 mm shell opened on the planar top rim. The
    /// inner wall weaves as an offset `RevolvedProfile` (the stadium
    /// end-cap path). Watertight is the hard requirement; whether the
    /// parametric route wins or the MDC fallback ships is reported.
    ///
    /// `#[ignore]`: the shelled-vase weave takes minutes in a debug
    /// build (same reason `vase_weave_timing_probe` is ignored) — run
    /// explicitly via `cargo test --release -- --ignored shelled_vase`.
    #[test]
    #[ignore]
    fn shelled_vase_exports_watertight() {
        let mut k = crate::Kernel::new();
        k.document = serde_json::from_str(&vase_document()).unwrap();
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();

        // Find the planar top rim face's stable hash (same probe as
        // features_tests::regen_shelled_vase) and append the shell.
        let parametric = k.parametric_weave;
        let decoded = k.decoded.clone();
        k.bodies[0].ensure_brep(&decoded, parametric);
        let brep = k.bodies[0].brep_cached().unwrap();
        let mut top_hash = None;
        for i in 0..brep.faces.len() {
            let fid = crate::brep::FaceId(i as u32);
            if let crate::brep::SurfaceGeometry::Analytic { surface, transform } =
                &brep.faces[i].surface
            {
                if let fielddraft_protocol::HalfSpace::Plane { normal, .. } = surface {
                    if normal[1] > 0.99 && (transform.translate[1] - 68.0).abs() < 1e-3 {
                        top_hash =
                            Some(crate::brep::naming::TopoRef::from_face(brep, fid).id_hash);
                    }
                }
            }
        }
        let top_hash = top_hash.expect("weave exposes the planar top rim face");
        k.document.feature_calls.push(fielddraft_protocol::FeatureCall::new(
            "shell",
            fielddraft_protocol::FeatureId::from_u32(3),
            serde_json::json!({
                "open_faces": format!("qFace({top_hash})"),
                "thickness": { "value": 2.0, "unit": "mm" },
            }),
        ));
        k.bootstrap_features_from_calls().unwrap();
        k.rebuild_packed();

        let report = export_single(&mut k, 0.4);
        eprintln!("SHELLED VASE REPORT: {report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.2,
            "{report}"
        );
    }

    /// A 30×20×30 box with a real through-hole: the cylinder REMOVE
    /// carries an explicit merge scope (`targets`) — touching solids
    /// never boolean implicitly.
    fn box_with_hole() -> crate::Kernel {
        let mut k = crate::Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                15.0,
                10.0,
                15.0,
            )
            .unwrap();
        k.add_feature_call(
            "cylinder",
            &format!(
                r#"{{
                    "radius": 4.0,
                    "half_height": 25.0,
                    "op": "remove",
                    "targets": "qCreatedBy({box_id})"
                }}"#
            ),
        )
        .unwrap();
        assert_eq!(k.bodies.len(), 1, "remove must fuse into one body");
        assert!(
            k.feature_errors.is_empty() && k.feature_warnings.is_empty(),
            "feature errors {:?} warnings {:?}",
            k.feature_errors,
            k.feature_warnings
        );
        k
    }

    /// Phase 1b edge pinning: on the generic MDC route, vertices whose
    /// grid cell is crossed by a B-Rep edge curve must land ON the
    /// exact curve. A box with a cylindrical through-hole has two
    /// circular rim edges (r = 4 at y = ±10) — before pinning the QEF
    /// vertices wiggled cell-sized around them.
    #[test]
    fn mdc_edge_pinning_snaps_rim_vertices_onto_the_circle() {
        let mut k = box_with_hole();
        let idx = k.bodies[0].feature_indices.clone();
        let size = k.bodies[0].aabb.size();
        // Force the generic MDC route.
        let (mesh, report) = k.mdc_export_body(0, 1, &idx, size, 0.6, String::new());
        assert!(report.edges_pinned > 50, "pinned {}", report.edges_pinned);
        assert!(report.validation.watertight, "{:?}", report.validation);

        // An exact chain of vertices must now sit on the top rim circle
        // (|sqrt(x²+z²) − 4| ≈ 0 at y ≈ 10) — roughly one per crossed
        // cell (circumference ≈ 25 mm / 0.44 mm cells ≈ 57).
        let mut exact_on_rim = 0;
        for v in 0..mesh.vertex_count() {
            let p = [
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            ];
            let radial = ((p[0] * p[0] + p[2] * p[2]).sqrt() - 4.0).abs();
            if (p[1] - 10.0).abs() < 2e-3 && radial < 2e-3 {
                exact_on_rim += 1;
            }
        }
        // The adaptive cluster pass thins the chain along the edge (the
        // QEF collapses collinear runs up to the chord tolerance) — the
        // remaining support vertices must still sit EXACTLY on the curve.
        assert!(exact_on_rim > 12, "exact rim vertices: {exact_on_rim}");
    }

    /// Phase 1d: a feature thinner than the sampling lattice must be
    /// flagged. A 0.28 mm SLIT inside a solid box (cells 0.6 mm): the
    /// box AABB pins the grid, the slit sits between two corner
    /// planes — every corner samples inside material, the MC case is
    /// empty, yet the band centre proves the surface passes through.
    #[test]
    fn sub_cell_slit_is_flagged_as_thin_cells() {
        let mut k = crate::Kernel::new();
        let box_id = k
            .add_primitive(
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
        k.add_feature_call(
            "box",
            &format!(
                r#"{{
                    "half_extents": [6.0, 0.14, 6.0],
                    "center": [0.0, 0.3, 0.0],
                    "op": "remove",
                    "targets": "qCreatedBy({box_id})"
                }}"#
            ),
        )
        .unwrap();
        let idx = k.bodies[0].feature_indices.clone();
        let size = k.bodies[0].aabb.size();
        // Force the generic MDC route (the parametric route would
        // resolve the slit exactly and needs no flag).
        let (_, report) = k.mdc_export_body(0, 1, &idx, size, 0.6, String::new());
        assert!(report.adaptive, "{report:?}");
        assert!(report.thin_cells > 50, "thin_cells {}", report.thin_cells);
    }

    /// Phase 2c: a hard-cut sphere — the spherical patch is trimmed by
    /// a plane (one once-winding rim, closes onto the pole) and must
    /// tessellate parametrically via the synthetic semicircle profile.
    #[test]
    fn sphere_cut_by_box_exports_parametric_watertight() {
        let mut k = crate::Kernel::new();
        let sphere_id = k
            .add_primitive(
                Primitive::KIND_SPHERE,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                10.0,
                0.0,
                0.0,
            )
            .unwrap();
        // Cut away everything above y = 4 (box remove with scope).
        k.add_feature_call(
            "box",
            &format!(
                r#"{{
                    "half_extents": [20.0, 8.0, 20.0],
                    "center": [0.0, 12.0, 0.0],
                    "op": "remove",
                    "targets": "qCreatedBy({sphere_id})"
                }}"#
            ),
        )
        .unwrap();
        assert_eq!(k.bodies.len(), 1);
        let report = export_single(&mut k, 0.4);
        eprintln!("SPHERE-CUT REPORT: {report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert_eq!(report["bodies"][0]["parametric"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.12,
            "{report}"
        );
    }

    /// Phase 1c on SDF-only geometry: a smooth-blended union has no
    /// parametric route and no woven B-Rep edges — the adaptive pass
    /// alone must deliver watertight + in-tolerance + economical.
    #[test]
    fn smooth_blend_exports_adaptive_watertight() {
        let mut k = crate::Kernel::new();
        let box_id = k
            .add_primitive(
                Primitive::KIND_BOX,
                BoolOp::TAG_NEW,
                0.0,
                0.0,
                0.0,
                10.0,
                10.0,
                10.0,
            )
            .unwrap();
        k.add_feature_call(
            "sphere",
            &format!(
                r#"{{
                    "radius": 7.0,
                    "center": [10.0, 8.0, 0.0],
                    "op": "add",
                    "targets": "qCreatedBy({box_id})",
                    "blend_radius": 3.0
                }}"#
            ),
        )
        .unwrap();
        assert_eq!(k.bodies.len(), 1);
        let report = export_single(&mut k, 0.5);
        eprintln!("BLEND REPORT: {report}");
        assert_eq!(report["bodies"][0]["parametric"], false, "{report}");
        assert_eq!(report["bodies"][0]["adaptive"], true, "{report}");
        assert_eq!(report["all_watertight"], true, "{report}");
        assert!(
            report["max_deviation_mm"].as_f64().unwrap() < 0.12,
            "{report}"
        );
        // Economy: flat box faces must collapse to coarse clusters.
        let tris = report["total_triangles"].as_u64().unwrap();
        assert!(tris < 25_000, "triangles {tris}");
    }

    /// Phase 1a determinism: identical input must produce a bit-identical
    /// simplified export mesh across repeated fresh runs — the planar
    /// simplification used to leak HashMap iteration order into region
    /// processing (and its slit bridging could emit crossing bridges),
    /// making watertightness flip between runs of the SAME document.
    #[test]
    fn mdc_export_is_deterministic_and_watertight_across_runs() {
        let build = || {
            // Real through-hole → planar regions WITH holes (bridging).
            let mut k = box_with_hole();
            // Force the generic MDC route (bypass the parametric path) by
            // running the internal MDC body export directly.
            let idx = k.bodies[0].feature_indices.clone();
            let size = k.bodies[0].aabb.size();
            let (mesh, report) = k.mdc_export_body(0, 1, &idx, size, 0.6, String::new());
            (mesh, report)
        };
        let (m1, r1) = build();
        let (m2, r2) = build();
        assert!(r1.validation.watertight, "run 1 not watertight: {r1:?}");
        assert!(r2.validation.watertight, "run 2 not watertight");
        assert!(r1.simplified, "simplification must have been accepted");
        assert_eq!(m1.indices, m2.indices, "indices differ between runs");
        assert_eq!(m1.positions, m2.positions, "positions differ between runs");
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
        k.export_mesh_begin(0.5, "adaptive".into());
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

