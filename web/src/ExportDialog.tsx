/**
 * Export dialog (Meshing Phase 0) — the STL/3MF export front-end.
 *
 * Opening the dialog immediately runs the tolerance-driven export pass
 * in the mesher worker (per-body progress), then shows the validation
 * report the kernel measured: watertight verdict, measured max
 * deviation against the exact SDF, triangle counts, per-body detail.
 * Save only hands the already-encoded bytes to the browser download.
 * Changing the cell size re-runs the pass.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { KernelStore } from "./store";
import type {
  ExportFormat,
  ExportProgress,
  ExportReport,
} from "./exportTypes";

const CELL_SIZE_STORAGE_KEY = "fielddraft.export.cellSizeMm";
const DEFAULT_CELL_MM = 0.4;

function initialCellSize(): number {
  try {
    const stored = Number(localStorage.getItem(CELL_SIZE_STORAGE_KEY));
    if (Number.isFinite(stored) && stored >= 0.05 && stored <= 5) {
      return stored;
    }
  } catch {
    // no localStorage (tests) — use the default
  }
  return DEFAULT_CELL_MM;
}

interface ExportDialogProps {
  store: KernelStore;
  format: ExportFormat;
  onClose: () => void;
}

type Phase =
  | { kind: "running"; progress: ExportProgress | null }
  | { kind: "done"; report: ExportReport; bytes: Uint8Array }
  | { kind: "error"; message: string };

export function ExportDialog({ store, format, onClose }: ExportDialogProps) {
  const [cellSize, setCellSize] = useState(initialCellSize);
  const [cellInput, setCellInput] = useState(String(initialCellSize()));
  const [phase, setPhase] = useState<Phase>({
    kind: "running",
    progress: null,
  });
  // Monotonic run id — a re-run (cell-size change) must not let a
  // stale response overwrite the fresh one.
  const runIdRef = useRef(0);

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", handleKey, true);
    return () => window.removeEventListener("keydown", handleKey, true);
  }, [onClose]);

  useEffect(() => {
    const runId = ++runIdRef.current;
    setPhase({ kind: "running", progress: null });
    store
      .exportMeshed(format, cellSize, (progress) => {
        if (runIdRef.current === runId) {
          setPhase({ kind: "running", progress });
        }
      })
      .then(({ bytes, report }) => {
        if (runIdRef.current === runId) {
          setPhase({ kind: "done", report, bytes });
        }
      })
      .catch((err: unknown) => {
        if (runIdRef.current === runId) {
          setPhase({
            kind: "error",
            message: err instanceof Error ? err.message : String(err),
          });
        }
      });
  }, [store, format, cellSize]);

  const applyCellSize = useCallback(() => {
    const parsed = Number(cellInput.replace(",", "."));
    if (!Number.isFinite(parsed)) return;
    const clamped = Math.min(5, Math.max(0.05, parsed));
    setCellInput(String(clamped));
    try {
      localStorage.setItem(CELL_SIZE_STORAGE_KEY, String(clamped));
    } catch {
      // ignore
    }
    setCellSize(clamped);
  }, [cellInput]);

  const save = useCallback(() => {
    if (phase.kind !== "done") return;
    store.downloadExport(phase.bytes, format);
  }, [phase, store, format]);

  const featureName = useCallback(
    (bodyId: number): string => {
      const f = store
        .getState()
        .features.find((feat) => feat.id === bodyId);
      return f?.name ?? `Body ${bodyId}`;
    },
    [store],
  );

  return (
    <div
      className="fd-export-dialog-overlay"
      data-testid="export-dialog-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="fd-export-dialog"
        role="dialog"
        aria-label={`Export ${format.toUpperCase()}`}
        data-testid="export-dialog"
      >
        <div className="dialog-header">
          <h2 className="dialog-title">Export {format.toUpperCase()}</h2>
          <button
            type="button"
            className="dialog-close"
            aria-label="Close"
            onClick={onClose}
          >
            ×
          </button>
        </div>

        <div className="settings-row">
          <label htmlFor="export-cell-size">Cell size (mm)</label>
          <input
            id="export-cell-size"
            data-testid="export-cell-size"
            type="number"
            min={0.05}
            max={5}
            step={0.05}
            value={cellInput}
            onChange={(e) => setCellInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") applyCellSize();
            }}
            onBlur={applyCellSize}
          />
          <span className="hint">
            Grid cell edge per body — smaller is finer. Vertices sit on
            the exact surface; the report shows the measured deviation.
          </span>
        </div>

        {phase.kind === "running" && (
          <div className="progress-block" data-testid="export-progress">
            <progress
              max={phase.progress?.total ?? 1}
              value={phase.progress?.done ?? 0}
            />
            <span>
              Meshing body {phase.progress?.done ?? 0} /{" "}
              {phase.progress?.total ?? "…"}
            </span>
          </div>
        )}

        {phase.kind === "error" && (
          <div className="error-block" data-testid="export-error">
            Export failed: {phase.message}
          </div>
        )}

        {phase.kind === "done" && (
          <ReportView report={phase.report} featureName={featureName} />
        )}

        <div className="dialog-footer">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="primary"
            data-testid="export-save"
            disabled={phase.kind !== "done"}
            onClick={save}
          >
            Save {format.toUpperCase()}
          </button>
        </div>
      </div>
    </div>
  );
}

function formatMm(v: number): string {
  if (v === 0) return "0";
  if (v < 0.0001) return "< 0.0001";
  return v >= 0.01 ? v.toFixed(3) : v.toFixed(4);
}

function ReportView({
  report,
  featureName,
}: {
  report: ExportReport;
  featureName: (bodyId: number) => string;
}) {
  const empty = report.bodies.length === 0;
  return (
    <div className="report" data-testid="export-report">
      {empty ? (
        <div className="verdict warn">Nothing to export — no bodies.</div>
      ) : (
        <div
          className={`verdict ${report.all_watertight ? "ok" : "warn"}`}
          data-testid="export-verdict"
        >
          {report.all_watertight ? "✓ Watertight" : "✗ Not watertight"}
          {" · "}max deviation {formatMm(report.max_deviation_mm)} mm
          {" · "}
          {report.total_triangles.toLocaleString()} triangles
        </div>
      )}

      {!empty && (
        <table className="body-table">
          <thead>
            <tr>
              <th>Body</th>
              <th>Mesh</th>
              <th>Triangles</th>
              <th>Watertight</th>
              <th>Max dev.</th>
            </tr>
          </thead>
          <tbody>
            {report.bodies.map((b) => (
              <tr key={b.body_id}>
                <td>{featureName(b.body_id)}</td>
                <td
                  title={
                    b.parametric_skip_reason
                      ? `Parametric route skipped: ${b.parametric_skip_reason}`
                      : undefined
                  }
                >
                  {b.passthrough
                    ? "source mesh"
                    : b.parametric
                      ? "parametric"
                      : b.adaptive
                        ? `adaptive ${formatMm(b.actual_cell_mm)} mm`
                        : `grid ${formatMm(b.actual_cell_mm)} mm`}
                  {b.cell_limited ? " (limited)" : ""}
                </td>
                <td>{b.triangles.toLocaleString()}</td>
                <td>{b.watertight ? "✓" : defects(b)}</td>
                <td>
                  {b.passthrough ? "—" : `${formatMm(b.max_deviation_mm)} mm`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function defects(b: {
  boundary_edges: number;
  non_manifold_edges: number;
  misoriented_edges: number;
  degenerate_triangles: number;
}): string {
  const parts: string[] = [];
  if (b.boundary_edges > 0) parts.push(`${b.boundary_edges} open`);
  if (b.non_manifold_edges > 0) parts.push(`${b.non_manifold_edges} non-manifold`);
  if (b.misoriented_edges > 0) parts.push(`${b.misoriented_edges} misoriented`);
  if (b.degenerate_triangles > 0) parts.push(`${b.degenerate_triangles} degenerate`);
  return parts.length > 0 ? `✗ ${parts.join(", ")}` : "✗";
}
