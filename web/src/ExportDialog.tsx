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
  ExportBodyReport,
  ExportEstimate,
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
const OPACITY_STORAGE_KEY = "fielddraft.export.previewOpacity";
const WIREFRAME_STORAGE_KEY = "fielddraft.export.previewWireframe";
const DEFAULT_CELL_MM = 0.4;

function initialOpacity(): number {
  try {
    const stored = Number(localStorage.getItem(OPACITY_STORAGE_KEY));
    if (Number.isFinite(stored) && stored >= 0 && stored <= 1) return stored;
  } catch {
    // no localStorage (tests) — use the default
  }
  return 1;
}

function initialWireframe(): boolean {
  try {
    return localStorage.getItem(WIREFRAME_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

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
      stored === "uniform-dc" ||
      stored === "marching-cubes" ||
      stored === "manifold-dc" ||
      stored === "rfta" ||
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
  // Preview display controls (crossfade mesh↔SDF solid, wireframe overlay),
  // persisted across sessions.
  const [meshOpacity, setMeshOpacity] = useState(initialOpacity);
  const [wireframe, setWireframe] = useState(initialWireframe);
  // Pre-mesh estimate for dense-grid methods: the finest achievable cell size
  // (memory-guard lower bound) and a rough triangle count at the chosen cell,
  // so cost is visible BEFORE committing to a mesh. Recomputed (debounced) as
  // the cell size / method changes; null for non-dense methods.
  const [estimate, setEstimate] = useState<ExportEstimate | null>(null);
  // Monotonic run id — a re-run must not let a stale response overwrite
  // the fresh one (or one from a cancelled run).
  const runIdRef = useRef(0);

  // Live estimate: debounced so it doesn't mesh a coarse grid on every
  // keystroke. The kernel returns `dense: false` for octree / parametric /
  // RFTA routes, and the hint below renders nothing for those.
  useEffect(() => {
    const parsed = Number(cellInput.replace(",", "."));
    if (!Number.isFinite(parsed)) {
      setEstimate(null);
      return;
    }
    const clamped = Math.min(5, Math.max(0.05, parsed));
    const handle = setTimeout(() => {
      setEstimate(store.exportEstimate(clamped, method));
    }, 250);
    return () => clearTimeout(handle);
  }, [cellInput, method, store]);

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
          // while the (non-modal) dialog stays open — with the persisted
          // crossfade opacity + wireframe state applied.
          setExportPreview(
            preview.positions.length > 0
              ? { positions: preview.positions, indices: preview.indices }
              : null,
          );
          setExportPreviewOpacity(meshOpacity);
          setExportPreviewWireframe(wireframe);
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
  }, [commitCellSize, store, format, method, meshOpacity, wireframe]);

  // Editing the cell size (or method) after a run discards the stale
  // mesh so the user must re-confirm (Prio: never save/preview a mesh
  // for settings the user no longer sees). The crossfade/wireframe
  // preferences are intentionally KEPT (they're persisted user settings).
  const resetToIdle = useCallback(() => {
    runIdRef.current++; // invalidate any in-flight run
    setPhase({ kind: "idle" });
    setExportPreview(null); // drop the stale solid from the viewport
  }, []);

  const onMeshOpacityChange = useCallback((value: number) => {
    setMeshOpacity(value);
    setExportPreviewOpacity(value);
    try {
      localStorage.setItem(OPACITY_STORAGE_KEY, String(value));
    } catch {
      // ignore
    }
  }, []);

  const onWireframeChange = useCallback((value: boolean) => {
    setWireframe(value);
    setExportPreviewWireframe(value);
    try {
      localStorage.setItem(WIREFRAME_STORAGE_KEY, String(value));
    } catch {
      // ignore
    }
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
          <option value="manifold-dc">Manifold DC (viewport)</option>
          <option value="rfta">Reach for the Arcs</option>
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
                    : method === "manifold-dc"
                      ? "The viewport's own uniform Manifold Dual Contouring with planar-face simplification — now exportable too."
                      : method === "rfta"
                        ? "Reach for the Arcs (Sellán et al. 2024): spheres → tangent points → screened Poisson reconstruction. A sampled-grid reconstruction (internal resolution capped) — robust on blurry/imperfect fields."
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
        {estimate?.dense && (
          <span className="hint estimate" data-testid="export-estimate">
            Lower bound {formatMm(estimate.min_cell_mm)} mm · ≈{" "}
            {estimate.total_est_triangles.toLocaleString()} triangles (raw grid,
            before decimation)
            {estimate.cell_limited && (
              <> — cell clamped to the memory budget</>
            )}
          </span>
        )}
      </div>

      {phase.kind === "idle" && (
        <div className="hint-block" data-testid="export-idle">
          Confirm the cell size, then <strong>Mesh</strong> to build the export
          mesh and check it before saving.
        </div>
      )}

      {phase.kind === "running" && (
        <div className="progress-block" data-testid="export-progress">
          <span className="spinner" aria-hidden="true" />
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
        // Transposed: attributes down the side, one column per body — so a
        // long "Mesh" label wraps in its cell instead of forcing a
        // horizontal scroll.
        <table className="body-table">
          <tbody>
            <tr>
              <th>Body</th>
              {report.bodies.map((b) => (
                <td key={b.body_id}>{featureName(b.body_id)}</td>
              ))}
            </tr>
            <tr>
              <th>Mesh</th>
              {report.bodies.map((b) => (
                <td key={b.body_id} className="mesh-cell">
                  {meshLabel(b)}
                </td>
              ))}
            </tr>
            <tr>
              <th>Triangles</th>
              {report.bodies.map((b) => (
                <td key={b.body_id}>{b.triangles.toLocaleString()}</td>
              ))}
            </tr>
            <tr>
              <th>Watertight</th>
              {report.bodies.map((b) => (
                <td key={b.body_id}>{b.watertight ? "✓" : defects(b)}</td>
              ))}
            </tr>
            <tr>
              <th>Max dev.</th>
              {report.bodies.map((b) => (
                <td key={b.body_id}>
                  {b.passthrough ? "—" : `${formatMm(b.max_deviation_mm)} mm`}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      )}
    </div>
  );
}

/** The mesher-route label for a body's report row. */
function meshLabel(b: ExportBodyReport): string {
  const cell = formatMm(b.actual_cell_mm);
  const base = b.route
    ? b.route
    : b.passthrough
      ? "source mesh"
      : b.parametric
        ? "parametric"
        : b.interval_dc
          ? `interval-DC ${cell} mm`
          : b.surface_nets
            ? `surface nets ${cell} mm`
            : b.adaptive
              ? `adaptive ${cell} mm`
              : `grid ${cell} mm`;
  const limited = b.cell_limited ? " (limited)" : "";
  const reduction =
    (b.surface_nets || b.route) && b.dense_triangles > 0
      ? ` (${b.dense_triangles.toLocaleString()} → ${b.triangles.toLocaleString()})`
      : "";
  return `${base}${limited}${reduction}`;
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
