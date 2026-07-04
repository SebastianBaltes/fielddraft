/**
 * Export dialog (Meshing Phase 0) — the STL/3MF export front-end.
 *
 * Two-step flow: the dialog first asks for the cell size and waits.
 * Meshing (potentially expensive for gyroids/blends) only starts when
 * the user confirms with "Mesh". The tolerance-driven export pass then
 * runs in the mesher worker (per-body progress) and shows the
 * validation report the kernel measured: watertight verdict, measured
 * max deviation against the exact SDF, triangle counts, per-body
 * detail. Save only hands the already-encoded bytes to the browser
 * download. Changing the cell size returns to the confirm step.
 */

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type { KernelStore } from "./store";
import type {
  ExportFormat,
  ExportProgress,
  ExportReport,
  MeshMethod,
} from "./exportTypes";
import {
  setExportPreview,
  setExportPreviewOpacity,
  setExportPreviewWireframe,
} from "./exportPreviewStore";

const CELL_SIZE_STORAGE_KEY = "fielddraft.export.cellSizeMm";
const METHOD_STORAGE_KEY = "fielddraft.export.method";
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

function initialMethod(): MeshMethod {
  try {
    const stored = localStorage.getItem(METHOD_STORAGE_KEY);
    if (
      stored === "surface-nets" ||
      stored === "surface-nets-builtin" ||
      stored === "interval-dc" ||
      stored === "adaptive"
    ) {
      return stored;
    }
  } catch {
    // no localStorage (tests) — use the default
  }
  return "adaptive";
}

interface ExportDialogProps {
  store: KernelStore;
  format: ExportFormat;
  onClose: () => void;
}

type Phase =
  | { kind: "idle" }
  | { kind: "running"; progress: ExportProgress | null }
  | { kind: "done"; report: ExportReport; bytes: Uint8Array }
  | { kind: "error"; message: string };

export function ExportDialog({ store, format, onClose }: ExportDialogProps) {
  const [cellInput, setCellInput] = useState(String(initialCellSize()));
  const [method, setMethod] = useState<MeshMethod>(initialMethod);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  // Preview display controls (crossfade mesh↔SDF solid, wireframe overlay).
  const [meshOpacity, setMeshOpacity] = useState(1);
  const [wireframe, setWireframe] = useState(false);
  // Monotonic run id — a re-run must not let a stale response overwrite
  // the fresh one (or one from a cancelled run).
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

  // Reads the current cell-size input, clamps + persists it, returns the
  // usable value (or null if unparseable).
  const commitCellSize = useCallback((): number | null => {
    const parsed = Number(cellInput.replace(",", "."));
    if (!Number.isFinite(parsed)) return null;
    const clamped = Math.min(5, Math.max(0.05, parsed));
    setCellInput(String(clamped));
    try {
      localStorage.setItem(CELL_SIZE_STORAGE_KEY, String(clamped));
    } catch {
      // ignore
    }
    return clamped;
  }, [cellInput]);

  // Step 2: run the meshing pass for the confirmed cell size.
  const runExport = useCallback(() => {
    const cellSize = commitCellSize();
    if (cellSize === null) return;
    const runId = ++runIdRef.current;
    setPhase({ kind: "running", progress: null });
    store
      .exportMeshed(format, cellSize, method, (progress) => {
        if (runIdRef.current === runId) {
          setPhase({ kind: "running", progress });
        }
      })
      .then(({ bytes, report, preview }) => {
        if (runIdRef.current === runId) {
          setPhase({ kind: "done", report, bytes });
          // Show the exact export triangles as a solid in the viewport
          // while the (non-modal) dialog stays open.
          setExportPreview(
            preview.positions.length > 0
              ? { positions: preview.positions, indices: preview.indices }
              : null,
          );
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
  }, [commitCellSize, store, format, method]);

  // Editing the cell size (or method) after a run discards the stale
  // mesh so the user must re-confirm (Prio: never save/preview a mesh
  // for settings the user no longer sees).
  const resetToIdle = useCallback(() => {
    runIdRef.current++; // invalidate any in-flight run
    setPhase({ kind: "idle" });
    setExportPreview(null); // drop the stale solid from the viewport
    // setExportPreview(null) resets the store's display controls; mirror
    // that in the local UI so the slider/checkbox match on the next mesh.
    setMeshOpacity(1);
    setWireframe(false);
  }, []);

  const onMeshOpacityChange = useCallback((value: number) => {
    setMeshOpacity(value);
    setExportPreviewOpacity(value);
  }, []);

  const onWireframeChange = useCallback((value: boolean) => {
    setWireframe(value);
    setExportPreviewWireframe(value);
  }, []);

  const onCellInputChange = useCallback(
    (value: string) => {
      setCellInput(value);
      resetToIdle();
    },
    [resetToIdle],
  );

  const onMethodChange = useCallback(
    (value: MeshMethod) => {
      setMethod(value);
      try {
        localStorage.setItem(METHOD_STORAGE_KEY, value);
      } catch {
        // ignore
      }
      resetToIdle();
    },
    [resetToIdle],
  );

  // The preview lives only while the dialog is open — clear it on close
  // so the normal renderer takes over again.
  useEffect(() => () => setExportPreview(null), []);

  // Free-floating (non-modal) window position; null = CSS default anchor
  // (top-right). Dragging the header sets an explicit left/top.
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const dragRef = useRef<{ dx: number; dy: number } | null>(null);

  const onHeaderPointerDown = useCallback((e: ReactPointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return; // not the ×
    const dialog = e.currentTarget.parentElement as HTMLElement | null;
    if (!dialog) return;
    const rect = dialog.getBoundingClientRect();
    dragRef.current = { dx: e.clientX - rect.left, dy: e.clientY - rect.top };
    setPos({ x: rect.left, y: rect.top });
    e.currentTarget.setPointerCapture(e.pointerId);
  }, []);

  const onHeaderPointerMove = useCallback((e: ReactPointerEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    const x = Math.max(
      0,
      Math.min(window.innerWidth - 80, e.clientX - drag.dx),
    );
    const y = Math.max(
      0,
      Math.min(window.innerHeight - 40, e.clientY - drag.dy),
    );
    setPos({ x, y });
  }, []);

  const onHeaderPointerUp = useCallback((e: ReactPointerEvent) => {
    dragRef.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
  }, []);

  const save = useCallback(() => {
    if (phase.kind !== "done") return;
    store.downloadExport(phase.bytes, format);
  }, [phase, store, format]);

  const featureName = useCallback(
    (bodyId: number): string => {
      const f = store.getState().features.find((feat) => feat.id === bodyId);
      return f?.name ?? `Body ${bodyId}`;
    },
    [store],
  );

  return (
    <div
      className="fd-export-dialog"
      role="dialog"
      aria-label={`Export ${format.toUpperCase()}`}
      data-testid="export-dialog"
      style={pos ? { left: pos.x, top: pos.y, right: "auto" } : undefined}
    >
      <div
        className="dialog-header"
        onPointerDown={onHeaderPointerDown}
        onPointerMove={onHeaderPointerMove}
        onPointerUp={onHeaderPointerUp}
      >
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
        <label htmlFor="export-method">Meshing method</label>
        <select
          id="export-method"
          data-testid="export-method"
          className="method-select"
          value={method}
          onChange={(e) => onMethodChange(e.target.value as MeshMethod)}
        >
          <option value="adaptive">Adaptive dual contouring</option>
          <option value="surface-nets">
            Surface Nets + decimation (library)
          </option>
          <option value="surface-nets-builtin">
            Surface Nets + decimation (built-in)
          </option>
          <option value="interval-dc">Interval-octree DC (fidget)</option>
          <option value="uniform-dc">Uniform Dual Contouring (QEF)</option>
          <option value="marching-cubes">Marching cubes (robust)</option>
        </select>
        <span className="hint">
          {method === "surface-nets"
            ? "Dense grid (every cell evaluated) then QEM decimation, via fast-surface-nets + baby_shark — robust for non-1-Lipschitz fields: gyroid, TPMS, dense helix windings."
            : method === "surface-nets-builtin"
              ? "Same dense-grid + QEM pipeline, but our own from-scratch implementations — dependency-free alternative to diff against the library path."
              : method === "interval-dc"
                ? "Interval-arithmetic octree + Manifold DC (fidget): topology-safe subdivision (no Lipschitz assumption) and QEF sharp-edge preservation for closed-form solids. Watertight on smooth solids; sharp/dense feature edges may leave a few non-manifold edges (see the report) — use Surface Nets for guaranteed watertight."
                : method === "uniform-dc"
                  ? "Dense uniform grid with QEF vertex placement — watertight by construction AND feature-preserving (keeps sharp edges), non-1-Lipschitz-safe, then QEM decimation. The all-rounder for sharp-edged solids."
                  : method === "marching-cubes"
                    ? "Marching tetrahedra: unambiguous cases → watertight + 2-manifold by construction for ANY field (the robust baseline). Rounds sharp edges; then QEM decimation."
                    : "Parametric-first, then adaptive Manifold-DC — exact edges and the fewest triangles for regular CAD solids."}
        </span>
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
          onChange={(e) => onCellInputChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && phase.kind !== "running") runExport();
          }}
        />
        <span className="hint">
          Grid cell edge per body — smaller is finer. Vertices sit on the exact
          surface; the report shows the measured deviation.
        </span>
      </div>

      {phase.kind === "idle" && (
        <div className="hint-block" data-testid="export-idle">
          Confirm the cell size, then <strong>Mesh</strong> to build the export
          mesh and check it before saving.
        </div>
      )}

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
        <>
          <ReportView report={phase.report} featureName={featureName} />
          <div
            className="preview-controls"
            data-testid="export-preview-controls"
          >
            <label htmlFor="export-preview-opacity">SDF ↔ mesh</label>
            <input
              id="export-preview-opacity"
              data-testid="export-preview-opacity"
              type="range"
              min={0}
              max={1}
              step={0.01}
              value={meshOpacity}
              onChange={(e) => onMeshOpacityChange(Number(e.target.value))}
            />
            <label className="checkbox">
              <input
                type="checkbox"
                data-testid="export-preview-wireframe"
                checked={wireframe}
                onChange={(e) => onWireframeChange(e.target.checked)}
              />
              Wireframe
            </label>
          </div>
        </>
      )}

      <div className="dialog-footer">
        <button type="button" className="secondary" onClick={onClose}>
          Cancel
        </button>
        {phase.kind === "done" ? (
          <button
            type="button"
            className="primary"
            data-testid="export-save"
            onClick={save}
          >
            Save {format.toUpperCase()}
          </button>
        ) : (
          <button
            type="button"
            className="primary"
            data-testid="export-mesh"
            disabled={phase.kind === "running"}
            onClick={runExport}
          >
            {phase.kind === "running"
              ? "Meshing…"
              : phase.kind === "error"
                ? "Retry"
                : "Mesh"}
          </button>
        )}
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
          {report.bodies.some((b) => b.self_intersections > 0) && (
            <>
              {" · "}⚠{" "}
              {report.bodies.reduce((s, b) => s + b.self_intersections, 0)}
              {report.bodies.some((b) => !b.self_intersections_complete)
                ? "+"
                : ""}
              {" self-intersections"}
            </>
          )}
          {report.bodies.some((b) => b.thin_cells > 0) && (
            <>
              {" · "}⚠ thin features under-resolved (
              {report.bodies.reduce((s, b) => s + b.thin_cells, 0)} cells) —
              reduce the cell size
            </>
          )}
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
                  {b.route
                    ? b.route
                    : b.passthrough
                      ? "source mesh"
                      : b.parametric
                        ? "parametric"
                        : b.interval_dc
                          ? `interval-DC ${formatMm(b.actual_cell_mm)} mm`
                          : b.surface_nets
                            ? `surface nets ${formatMm(b.actual_cell_mm)} mm`
                            : b.adaptive
                              ? `adaptive ${formatMm(b.actual_cell_mm)} mm`
                              : `grid ${formatMm(b.actual_cell_mm)} mm`}
                  {b.cell_limited ? " (limited)" : ""}
                  {(b.surface_nets || b.route) && b.dense_triangles > 0
                    ? ` (${b.dense_triangles.toLocaleString()} → ${b.triangles.toLocaleString()})`
                    : ""}
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
  if (b.non_manifold_edges > 0)
    parts.push(`${b.non_manifold_edges} non-manifold`);
  if (b.misoriented_edges > 0) parts.push(`${b.misoriented_edges} misoriented`);
  if (b.degenerate_triangles > 0)
    parts.push(`${b.degenerate_triangles} degenerate`);
  return parts.length > 0 ? `✗ ${parts.join(", ")}` : "✗";
}
