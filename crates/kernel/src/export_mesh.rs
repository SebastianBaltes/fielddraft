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
    /// True when this body shipped the parametric per-face B-Rep
    /// tessellation (Meshing Phase 2, Option B) — watertight by
    /// construction, exact edges, minimal triangles. False = generic
    /// MDC pass.
    pub parametric: bool,
    /// Why the parametric route was not taken (empty when it was).
    pub parametric_skip_reason: String,
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
                parametric: false,
                parametric_skip_reason: String::new(),
                orientation_repaired: 0,
                edges_pinned: 0,
                simplified: false,
                simplification_rejected: false,
                validation,
            };
            (mesh, report)
        } else {
            // ── Parametric route first (Meshing Phase 2, Option B) ──
            // Per-face B-Rep tessellation: watertight by construction,
            // exact edges, minimal triangles. It must EARN the export —
            // the same validator gates it; any failure falls back to
            // the generic MDC pass below.
            match self.try_parametric_export(i, cell_mm) {
                Ok((mesh, validation)) => {
                    let report = BodyExportReport {
                        body_id,
                        requested_cell_mm: cell_mm,
                        actual_cell_mm: 0.0,
                        resolution: 0,
                        cell_limited: false,
                        passthrough: false,
                        parametric: true,
                        parametric_skip_reason: String::new(),
                        orientation_repaired: 0,
                        edges_pinned: 0,
                        simplified: false,
                        simplification_rejected: false,
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

        // Raw pass: Manifold-DC + Newton projection, then winding
        // repair (belt and braces — the geometric quad winding should
        // already be consistent since Phase 1a).
        let mut raw = crate::dc::dual_contour_unsimplified(&bounded, resolution);
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
            self.pin_mesh_to_brep_edges(body_index, &mut raw, grid_min, actual_cell);

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
            orientation_repaired,
            edges_pinned,
            simplified,
            simplification_rejected: reject_simplified,
            validation,
        };
        (mesh, report)
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
        let corners: Vec<[f32; 3]> = brep.vertices.iter().map(|v| place(v.position)).collect();

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
        let n = k.export_mesh_begin(cell);
        assert!(n >= 1, "expected at least one body");
        for i in 0..n {
            k.export_mesh_body(i);
        }
        let report: serde_json::Value =
            serde_json::from_str(&k.export_mesh_report()).unwrap();
        report
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
        assert!(exact_on_rim > 30, "exact rim vertices: {exact_on_rim}");
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

