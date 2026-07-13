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
import { NumberInput } from "./NumberInput";
import type {
  ExportBodyReport,
  ExportEstimate,
  ExportFormat,
  ExportMethodOptions,
  ExportProgress,
  ExportReport,
  MeshMethod,
} from "./exportTypes";
import { DEFAULT_CURV_ANGLE_DEG } from "./exportTypes";
import { formatDuration } from "./exportDuration";
import {
  setExportPreview,
  setExportPreviewOpacity,
  setExportPreviewWireframe,
} from "./exportPreviewStore";
import {
  parseBinaryStl,
  runServerMesh,
  type ServerMeshReport,
} from "./serverMesh";

const CELL_SIZE_STORAGE_KEY = "fielddraft.export.cellSizeMm";
const METHOD_STORAGE_KEY = "fielddraft.export.method";
const CURV_ANGLE_STORAGE_KEY = "fielddraft.export.curvAngleDeg";
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
      stored === "marching-cubes-33" ||
      stored === "dual-marching-cubes" ||
      stored === "extended-marching-cubes" ||
      stored === "macet" ||
      stored === "manifold-dc" ||
      stored === "hybrid-pasm" ||
      stored === "lichen" ||
      stored === "cgal-server" ||
      stored === "adaptive"
    ) {
      return stored;
    }
  } catch {
    // no localStorage (tests) — use the default
  }
  return "adaptive";
}

/** Lichen's curvature angle (deg) — the only Lichen knob the export exposes
 *  besides the edge length. Everything else is the kernel's fixed export
 *  preset. */
function initialCurvAngle(): number {
  try {
    const stored = Number(localStorage.getItem(CURV_ANGLE_STORAGE_KEY));
    if (Number.isFinite(stored) && stored >= 0.5 && stored <= 30) {
      return stored;
    }
  } catch {
    // no localStorage (tests) — use the default
  }
  return DEFAULT_CURV_ANGLE_DEG;
}

/** What each mesher does, in the user's words. A lookup, not a ternary chain —
 *  the chain was 12 levels deep before Lichen arrived. */
const METHOD_HINTS: Record<MeshMethod, string> = {
  adaptive:
    "Parametric-first, then adaptive Manifold-DC — exact edges and the fewest triangles for regular CAD solids.",
  "surface-nets":
    "Dense grid (every cell evaluated) then QEM decimation, via fast-surface-nets + baby_shark — robust for non-1-Lipschitz fields: gyroid, TPMS, dense helix windings.",
  "surface-nets-builtin":
    "Same dense-grid + QEM pipeline, but our own from-scratch implementations — dependency-free alternative to diff against the library path.",
  "interval-dc":
    "Interval-arithmetic octree + Manifold DC (fidget): topology-safe subdivision (no Lipschitz assumption) and QEF sharp-edge preservation for closed-form solids. Watertight on smooth solids; sharp/dense feature edges may leave a few non-manifold edges (see the report) — use Surface Nets for guaranteed watertight.",
  "uniform-dc":
    "Dense uniform grid with QEF vertex placement — watertight by construction AND feature-preserving (keeps sharp edges), non-1-Lipschitz-safe, then QEM decimation. The all-rounder for sharp-edged solids.",
  "marching-cubes":
    "Marching tetrahedra: unambiguous cases → watertight + 2-manifold by construction for ANY field (the robust baseline). Rounds sharp edges; then QEM decimation.",
  "marching-cubes-33":
    "Marching Cubes 33 (Lewiner 2003): topologically-controlled MC — tests each ambiguous face/interior against the trilinear surface, so it's manifold, consistent AND watertight for any field, with fewer triangles than the tet mesher. A strong choice for gyroid/TPMS. Then QEM decimation.",
  "dual-marching-cubes":
    "Dual Marching Cubes (Nielson 2004): the dual of MC — one point per MC patch joined into quads (manifold variant). Fewer sliver triangles than MC, closed by construction. Then QEM decimation.",
  "extended-marching-cubes":
    "Extended Marching Cubes (Kobbelt 2001): MC33 base plus a QEF feature vertex inserted in cells that carry a sharp CSG edge (clamped into the cell against spikes) — recovers crisp boolean edges MC rounds off, while staying watertight.",
  macet:
    "MACET (Dietrich/Scheidegger 2009): MC33 topology, but each active edge is transformed to be perpendicular to the surface (gradient + tangential, using the analytic ∇f) and the crossing is re-found by bisection — eliminates skinny/degenerate triangles while staying watertight. Slower (root-finding per edge).",
  "manifold-dc":
    "The viewport's own uniform Manifold Dual Contouring with planar-face simplification — now exportable too.",
  "hybrid-pasm":
    "Hybrid PASM + B-Rep (kantenexakt): pins the B-Rep corners/edges as a fixed skeleton, relaxes surface particles (PASM Gaussian kernel) and closes each face by constrained Delaunay in its carrier chart; chartless faces (Torus/Gyroid/Custom-SDF) fall to a DC-patch zippered to the exact edges. Here the cell size is read as the target edge length h. Watertight by construction; a body that can't close falls back to the adaptive route.",
  lichen:
    "Lichen (SDF + B-Rep): B-Rep corners and edge chains seed the points, a curvature-adaptive growth fills each face and a global manifold walk extracts one closed sheet — sharp edges land exactly on the B-Rep curves, curved faces are resolved by the angle below. Everything else runs the acceptance preset the mesher is gated on; a body that comes back open or uncovered falls back to the adaptive route (the report names the route).",
  "cgal-server":
    "Server-side CGAL Mesh_3 (restricted Delaunay refinement): watertight, 2-manifold and exact protected B-Rep edges. Saves the project, meshes on the server and downloads the STL — needs a signed-in, server-saved project.",
};

/** Methods that read the cell-size field as a target EDGE LENGTH h (a mesh
 *  spacing), not as a grid cell. */
function isEdgeLengthMethod(method: MeshMethod): boolean {
  return method === "hybrid-pasm" || method === "lichen";
}

interface ExportDialogProps {
  store: KernelStore;
  format: ExportFormat;
  onClose: () => void;
}

// `startedAt`/`durationMs` (ticket 6a0354e8): the meshing runtime, measured in
// the WEB around the kernel/worker call — the kernel must not touch
// `std::time` (panics on wasm32). Applies to every method, local or server.
type Phase =
  | { kind: "idle" }
  | { kind: "running"; progress: ExportProgress | null; startedAt: number }
  | { kind: "server-running"; status: string; startedAt: number }
  | {
      kind: "done";
      report: ExportReport;
      bytes: Uint8Array;
      durationMs: number;
    }
  | {
      kind: "server-done";
      report: ServerMeshReport;
      bytes: Uint8Array;
      durationMs: number;
    }
  | { kind: "error"; message: string };

export function ExportDialog({ store, format, onClose }: ExportDialogProps) {
  const [cellSizeMm, setCellSizeMm] = useState(initialCellSize());
  const [method, setMethod] = useState<MeshMethod>(initialMethod);
  const [curvAngleDeg, setCurvAngleDeg] = useState(initialCurvAngle);
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
  // routes, and the hint below renders nothing for those.
  useEffect(() => {
    if (method === "cgal-server") {
      setEstimate(null); // meshing happens server-side, no local estimate
      return;
    }
    const handle = setTimeout(() => {
      setEstimate(store.exportEstimate(cellSizeMm, method));
    }, 250);
    return () => clearTimeout(handle);
  }, [cellSizeMm, method, store]);

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

  // Persist the current (already-clamped by NumberInput) cell size and return
  // it for the meshing pass.
  const commitCellSize = useCallback((): number => {
    try {
      localStorage.setItem(CELL_SIZE_STORAGE_KEY, String(cellSizeMm));
    } catch {
      // ignore
    }
    return cellSizeMm;
  }, [cellSizeMm]);

  // Server route (method "cgal-server"): sync the project, submit a mesh
  // job, poll it to done and preview the downloaded STL. The abort
  // controller cancels the poll loop AND the job on reset/close.
  const serverAbortRef = useRef<AbortController | null>(null);

  const runServerExport = useCallback(() => {
    const cellSize = commitCellSize();
    const runId = ++runIdRef.current;
    const abort = new AbortController();
    serverAbortRef.current?.abort();
    serverAbortRef.current = abort;
    const startedAt = performance.now();
    setPhase({ kind: "server-running", status: "Saving project…", startedAt });
    (async () => {
      const synced = await store.syncToServer();
      const state = store.getState();
      if (!synced || !state.projectId) {
        throw new Error(
          "Server meshing needs a project saved to the server — sign in and save first.",
        );
      }
      // Mesh the active part tab; if a script/assembly tab is active, fall
      // back to the first part tab.
      const tabs = state.project.tabs;
      const active = tabs.find((t) => t.id === state.project.activeTabId);
      const part = active?.kind === "part" ? active : tabs.find((t) => t.kind === "part");
      if (!part) throw new Error("No part tab to mesh.");
      const result = await runServerMesh(
        state.projectId,
        part.id,
        { cellSizeMm: cellSize, facetDistMult: 0.15 },
        (status) => {
          if (runIdRef.current === runId) {
            setPhase({ kind: "server-running", status, startedAt });
          }
        },
        abort.signal,
      );
      if (runIdRef.current !== runId) return;
      const preview = parseBinaryStl(result.bytes);
      // The server always meshes to binary STL; a 3MF export converts the
      // triangles through the kernel's 3MF encoder client-side (M6).
      const bytes =
        format === "3mf"
          ? store.mesh3mfFromBuffers(preview.positions, preview.indices)
          : result.bytes;
      setPhase({
        kind: "server-done",
        report: result.report,
        bytes,
        durationMs: performance.now() - startedAt,
      });
      setExportPreview(
        preview.positions.length > 0
          ? { positions: preview.positions, indices: preview.indices }
          : null,
      );
      setExportPreviewOpacity(meshOpacity);
      setExportPreviewWireframe(wireframe);
    })().catch((err: unknown) => {
      if (runIdRef.current === runId && !abort.signal.aborted) {
        setPhase({
          kind: "error",
          message: err instanceof Error ? err.message : String(err),
        });
      }
    });
  }, [commitCellSize, store, format, meshOpacity, wireframe]);

  // Step 2: run the meshing pass for the confirmed cell size.
  const runExport = useCallback(() => {
    if (method === "cgal-server") {
      runServerExport();
      return;
    }
    const cellSize = commitCellSize();
    const options: ExportMethodOptions = { curvAngleDeg };
    try {
      localStorage.setItem(CURV_ANGLE_STORAGE_KEY, String(curvAngleDeg));
    } catch {
      // ignore
    }
    const runId = ++runIdRef.current;
    const startedAt = performance.now();
    setPhase({ kind: "running", progress: null, startedAt });
    store
      .exportMeshed(format, cellSize, method, options, (progress) => {
        if (runIdRef.current === runId) {
          setPhase({ kind: "running", progress, startedAt });
        }
      })
      .then(({ bytes, report, preview }) => {
        if (runIdRef.current === runId) {
          setPhase({
            kind: "done",
            report,
            bytes,
            durationMs: performance.now() - startedAt,
          });
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
  }, [
    commitCellSize,
    curvAngleDeg,
    store,
    format,
    method,
    meshOpacity,
    wireframe,
    runServerExport,
  ]);

  // Editing the cell size (or method) after a run discards the stale
  // mesh so the user must re-confirm (Prio: never save/preview a mesh
  // for settings the user no longer sees). The crossfade/wireframe
  // preferences are intentionally KEPT (they're persisted user settings).
  const resetToIdle = useCallback(() => {
    runIdRef.current++; // invalidate any in-flight run
    serverAbortRef.current?.abort(); // cancels a pending server job too
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

  const onCellSizeChange = useCallback(
    (value: number) => {
      setCellSizeMm(value);
      resetToIdle();
    },
    [resetToIdle],
  );

  const onCurvAngleChange = useCallback(
    (value: number) => {
      setCurvAngleDeg(value);
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
  // so the normal renderer takes over again. Closing also cancels a
  // pending server mesh job.
  useEffect(
    () => () => {
      setExportPreview(null);
      serverAbortRef.current?.abort();
    },
    [],
  );

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
    if (phase.kind !== "done" && phase.kind !== "server-done") return;
    store.downloadExport(phase.bytes, format);
  }, [phase, store, format]);

  const featureName = useCallback(
    (bodyId: number): string => {
      const f = store.getState().features.find((feat) => feat.id === bodyId);
      return f?.name ?? `Body ${bodyId}`;
    },
    [store],
  );

  // Server meshing needs a signed-in, server-saved project (read per render;
  // the submit path re-checks after the sync).
  const { projectId, syncState } = store.getState();
  const serverReady = projectId !== null && syncState !== "offline";

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
          <option value="marching-cubes">Marching Tetrahedra (robust)</option>
          <option value="marching-cubes-33">Marching Cubes 33 (Lewiner)</option>
          <option value="dual-marching-cubes">
            Dual Marching Cubes (Nielson)
          </option>
          <option value="extended-marching-cubes">
            Extended Marching Cubes (features)
          </option>
          <option value="macet">MACET (edge transforms, quality)</option>
          <option value="manifold-dc">Manifold DC (viewport)</option>
          <option value="hybrid-pasm">CAD (kantenexakt, PASM+B-Rep)</option>
          <option value="lichen">Lichen (SDF + B-Rep, kantenexakt)</option>
          <option value="cgal-server" disabled={!serverReady}>
            CGAL Mesh_3 (server)
            {!serverReady ? " — save to server first" : ""}
          </option>
        </select>
        <span className="hint">{METHOD_HINTS[method]}</span>
      </div>

      <div className="settings-row">
        <label htmlFor="export-cell-size">
          {isEdgeLengthMethod(method) ? "Edge length h (mm)" : "Cell size (mm)"}
        </label>
        <NumberInput
          id="export-cell-size"
          data-testid="export-cell-size"
          min={0.05}
          max={5}
          step={0.05}
          value={cellSizeMm}
          onChange={onCellSizeChange}
          // Ticket 0018657: the pre-mesh estimate recomputes on change — commit
          // on blur/Enter so a half-typed value can't clamp to the min and stall.
          commitOnChange={false}
          onKeyDown={(e) => {
            if (e.key === "Enter" && phase.kind !== "running") runExport();
          }}
        />
        <span className="hint">
          {method === "hybrid-pasm"
            ? "Target edge length h the mesh aims for — smaller is finer. The whole PASM pipeline (skeleton spacing, kernel radius, CDT clearance) scales off it."
            : method === "lichen"
              ? "Target edge length h the mesh aims for — smaller is finer. It is Lichen's global spacing: the point growth, the edge chains and the sizing field all scale off it."
              : "Grid cell edge per body — smaller is finer. Vertices sit on the exact surface; the report shows the measured deviation."}
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

      {/* Lichen's second (and last) knob. Everything else about Lichen is a
          fixed kernel preset — the Debug ▸ Mesher: Lichen dialog stays the
          place to turn the other twenty screws. */}
      {method === "lichen" && (
        <div className="settings-row">
          <label htmlFor="export-curv-angle">Curvature angle (°)</label>
          <NumberInput
            id="export-curv-angle"
            data-testid="export-curv-angle"
            min={0.5}
            max={30}
            step={0.5}
            value={curvAngleDeg}
            onChange={onCurvAngleChange}
            commitOnChange={false}
            onKeyDown={(e) => {
              if (e.key === "Enter" && phase.kind !== "running") runExport();
            }}
          />
          <span className="hint">
            How far the surface normal may turn across one triangle edge —
            smaller resolves curved faces finer (and costs triangles and time).
            Scale-free: a 2 mm hole gets as many segments as a 200 mm one. Flat
            faces are unaffected; there the edge length h rules.
          </span>
        </div>
      )}

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
          <Elapsed since={phase.startedAt} />
        </div>
      )}

      {phase.kind === "server-running" && (
        <div className="progress-block" data-testid="export-server-progress">
          <span className="spinner" aria-hidden="true" />
          <span>{phase.status}</span>
          <Elapsed since={phase.startedAt} />
          <button type="button" className="secondary" onClick={resetToIdle}>
            Cancel job
          </button>
        </div>
      )}

      {phase.kind === "error" && (
        <div className="error-block" data-testid="export-error">
          Export failed: {phase.message}
        </div>
      )}

      {phase.kind === "server-done" && (
        <>
          <div className="report" data-testid="export-report">
            <div className="verdict ok" data-testid="export-verdict">
              ✓ Watertight &amp; 2-manifold (CGAL Mesh_3)
              {" · "}
              {(phase.report.triangles ?? 0).toLocaleString()} triangles
              {" · "}
              {(phase.report.vertices ?? 0).toLocaleString()} vertices
            </div>
            <div className="runtime" data-testid="export-duration">
              Runtime {formatDuration(phase.durationMs)}
            </div>
          </div>
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

      {phase.kind === "done" && (
        <>
          <ReportView
            report={phase.report}
            durationMs={phase.durationMs}
            featureName={featureName}
          />
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
        {phase.kind === "done" || phase.kind === "server-done" ? (
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
            disabled={
              phase.kind === "running" || phase.kind === "server-running"
            }
            onClick={runExport}
          >
            {phase.kind === "running" || phase.kind === "server-running"
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

/** Live runtime of a pass in flight — the dialog must never look hung while a
 *  fine Lichen/CGAL mesh grinds away (ticket 6a0354e8). */
function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(() => performance.now());
  useEffect(() => {
    setNow(performance.now());
    const handle = setInterval(() => setNow(performance.now()), 200);
    return () => clearInterval(handle);
  }, [since]);
  return (
    <span className="runtime" data-testid="export-elapsed">
      {formatDuration(Math.max(0, now - since))}
    </span>
  );
}

function ReportView({
  report,
  durationMs,
  featureName,
}: {
  report: ExportReport;
  durationMs: number;
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

      <div className="runtime" data-testid="export-duration">
        Runtime {formatDuration(durationMs)}
      </div>

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
