/**
 * Export meshing (Meshing Phase 0) — shared wire types.
 *
 * Mirrors the kernel's `export_mesh::{BodyExportReport, ExportReport}`
 * JSON (serde snake_case, validation fields flattened into the body
 * report) plus the mesher-worker export protocol.
 */

export type ExportFormat = "stl" | "3mf";

/** Generic mesher selected in the export dialog.
 *  - `adaptive` — parametric-first + adaptive Manifold-DC (default).
 *  - `surface-nets` — dense Surface Nets + QEM decimation via libraries
 *    (fast-surface-nets + baby_shark); robust for non-1-Lipschitz fields
 *    like gyroid/TPMS/helix.
 *  - `surface-nets-builtin` — the same pipeline with our own from-scratch
 *    implementations (dependency-free alternative).
 *  - `interval-dc` — interval-arithmetic octree + Manifold Dual Contouring
 *    (fidget). Topology-safe subdivision (no Lipschitz assumption) and
 *    QEF sharp-edge preservation for closed-form solids; watertight on
 *    smooth solids, a few non-manifold edges at sharp/dense feature
 *    edges. */
export type MeshMethod =
  | "adaptive"
  | "surface-nets"
  | "surface-nets-builtin"
  | "interval-dc"
  | "uniform-dc"
  | "marching-cubes"
  | "marching-cubes-33"
  | "dual-marching-cubes"
  | "extended-marching-cubes"
  | "macet"
  | "manifold-dc"
  | "rfta";

/** Per-body slice of the export report (kernel: `BodyExportReport`,
 *  with the `MeshValidation` fields flattened in). */
export interface ExportBodyReport {
  body_id: number;
  requested_cell_mm: number;
  actual_cell_mm: number;
  resolution: number;
  cell_limited: boolean;
  passthrough: boolean;
  /** Meshing Phase 2: true = parametric per-face B-Rep tessellation
   *  (exact edges, minimal triangles); false = generic MDC pass. */
  parametric: boolean;
  parametric_skip_reason: string;
  /** Phase 1c: true = adaptive Manifold-DC (Schaefer clustering on a
   *  narrow-band octree); false = uniform-grid fallback. */
  adaptive: boolean;
  adaptive_skip_reason: string;
  orientation_repaired: number;
  /** Phase 1b: vertices pinned onto exact B-Rep edge curves (MDC route). */
  edges_pinned: number;
  simplified: boolean;
  simplification_rejected: boolean;
  /** Phase 1d: band cells whose sub-cell feature (thin wall/slit)
   *  slipped between the sampling planes — shrink the cell size. */
  thin_cells: number;
  /** Proper triangle-triangle self-intersections (measured, not gated). */
  self_intersections: number;
  self_intersections_complete: boolean;
  /** True when this body shipped the dense Surface Nets + QEM
   *  decimation route. */
  surface_nets: boolean;
  /** True when this body shipped the interval-octree Manifold-DC route
   *  (fidget). */
  interval_dc: boolean;
  /** Short human label of the mesher route (e.g. "uniform DC 0.5 mm",
   *  "marching cubes 0.5 mm"); empty falls back to the flags. */
  route: string;
  /** Triangle count of the dense Surface Nets mesh before decimation
   *  (0 off that route) — the report shows the reduction. */
  dense_triangles: number;
  vertices: number;
  triangles: number;
  degenerate_triangles: number;
  boundary_edges: number;
  non_manifold_edges: number;
  misoriented_edges: number;
  components: number;
  euler_characteristic: number;
  watertight: boolean;
  max_deviation_mm: number;
  p95_deviation_mm: number;
  deviation_samples: number;
}

/** Per-body slice of the pre-mesh estimate (kernel: `BodyEstimate`). */
export interface ExportBodyEstimate {
  body_id: number;
  /** Finest cell the memory guard allows for this body. */
  min_cell_mm: number;
  /** Cell actually used at the requested size (clamped to the guard). */
  actual_cell_mm: number;
  cell_limited: boolean;
  /** Rough raw-grid (pre-decimation) triangle count at the requested cell. */
  est_triangles: number;
}

/** Pre-mesh estimate for the export dialog (kernel: `ExportEstimate`).
 *  Only meaningful for dense-grid methods (`dense: true`); the octree /
 *  parametric / RFTA routes return `dense: false` and the dialog shows
 *  nothing. */
export interface ExportEstimate {
  dense: boolean;
  /** Coarsest (binding) finest-cell across bodies — the lower bound shown. */
  min_cell_mm: number;
  total_est_triangles: number;
  /** True when the requested cell is finer than some body's achievable min. */
  cell_limited: boolean;
  bodies: ExportBodyEstimate[];
}

/** Aggregate export report (kernel: `ExportReport`). */
export interface ExportReport {
  cell_size_mm: number;
  total_triangles: number;
  all_watertight: boolean;
  max_deviation_mm: number;
  bodies: ExportBodyReport[];
}

/** Merged export-mesh triangles for the in-app solid preview
 *  (xyz-interleaved positions + rebased triangle indices). */
export interface ExportPreviewMesh {
  positions: Float32Array;
  indices: Uint32Array;
}

/** The finished export: encoded bytes, the validation report, and the
 *  raw merged triangles for the solid preview. */
export interface ExportOutcome {
  bytes: Uint8Array;
  report: ExportReport;
  preview: ExportPreviewMesh;
}

export interface ExportProgress {
  done: number;
  total: number;
}

// ── mesher-worker wire messages ───────────────────────────────────

export interface MesherExportRequest {
  type: "export";
  requestId: number;
  documentJson: string;
  cellSizeMm: number;
  method: MeshMethod;
  format: ExportFormat;
}

export interface MesherExportProgressMsg {
  type: "export_progress";
  requestId: number;
  done: number;
  total: number;
}

export type MesherExportResponse =
  | {
      type: "export_result";
      requestId: number;
      ok: true;
      bytes: Uint8Array;
      report: ExportReport;
      previewPositions: Float32Array;
      previewIndices: Uint32Array;
    }
  | { type: "export_result"; requestId: number; ok: false; error: string };
