/**
 * Export meshing (Meshing Phase 0) — shared wire types.
 *
 * Mirrors the kernel's `export_mesh::{BodyExportReport, ExportReport}`
 * JSON (serde snake_case, validation fields flattened into the body
 * report) plus the mesher-worker export protocol.
 */

export type ExportFormat = "stl" | "3mf";

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
  orientation_repaired: number;
  /** Phase 1b: vertices pinned onto exact B-Rep edge curves (MDC route). */
  edges_pinned: number;
  simplified: boolean;
  simplification_rejected: boolean;
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

/** Aggregate export report (kernel: `ExportReport`). */
export interface ExportReport {
  cell_size_mm: number;
  total_triangles: number;
  all_watertight: boolean;
  max_deviation_mm: number;
  bodies: ExportBodyReport[];
}

/** The finished export: encoded bytes plus the validation report. */
export interface ExportOutcome {
  bytes: Uint8Array;
  report: ExportReport;
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
    }
  | { type: "export_result"; requestId: number; ok: false; error: string };
