/**
 * Debug ▸ Mesher PASM dialog — the DEDICATED front-end of the hybrid
 * PASM+B-Rep meshing pipeline (docs/HYBRID-PASM-MESHING-PLAN.md, ticket
 * nwmi). Unlike the generic Debug ▸ Mesher scaffold with its per-stage
 * checkboxes, this dialog always runs the full chain
 *
 *   B-Rep skeleton (corners pinned, chains on the exact curves)
 *   → PASM Gaussian relaxation (σ = h/5, tangential, convergence break)
 *   → per-face constrained Delaunay in the carrier charts
 *
 * and exposes only what the method actually needs: the target edge length h
 * (everything else — seed octree depth, kernel radius, chain spacing, seam
 * sampling, boundary clearance — is derived from h, mostly in the kernel via
 * `targetEdgeLength`) plus an iteration cap and display knobs. Both Mesher
 * dialogs publish into the same `mesherDebugStore`, so `projectDebug` keeps
 * them mutually exclusive. Non-modal + draggable; settings persist in
 * localStorage.
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  setMesherDebug,
  subscribeMesherStats,
  getMesherStats,
  type MesherDebug,
} from "./mesherDebugStore";
import { NumberInput } from "./NumberInput";

const SETTINGS_KEY = "fielddraft.debug.mesherPasm";

interface PasmSettings {
  /** Target edge length h (mm) — THE resolution parameter. */
  targetEdgeLength: number;
  /** Relaxation iteration cap (convergence usually breaks earlier). */
  iterations: number;
  partOpacity: number;
  strokeWidth: number;
}

const DEFAULTS: PasmSettings = {
  targetEdgeLength: 2,
  iterations: 40,
  partOpacity: 0.4,
  strokeWidth: 2,
};

const clampNum = (v: unknown, lo: number, hi: number, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v)
    ? Math.min(hi, Math.max(lo, v))
    : fallback;

function loadSettings(): PasmSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULTS;
    const p = JSON.parse(raw) as Partial<PasmSettings>;
    return {
      targetEdgeLength: clampNum(
        p.targetEdgeLength,
        0.1,
        1e3,
        DEFAULTS.targetEdgeLength,
      ),
      iterations: Math.round(clampNum(p.iterations, 1, 200, DEFAULTS.iterations)),
      partOpacity: clampNum(p.partOpacity, 0, 1, DEFAULTS.partOpacity),
      strokeWidth: clampNum(p.strokeWidth, 0.5, 12, DEFAULTS.strokeWidth),
    };
  } catch {
    return DEFAULTS;
  }
}

/** The fixed PASM pipeline preset around the few exposed knobs. The kernel
 *  derives `maxDepth` and `interactionRadius` from `targetEdgeLength`. */
function pasmParams(s: PasmSettings): MesherDebug {
  return {
    targetEdgeLength: s.targetEdgeLength,
    iterations: s.iterations,
    partOpacity: s.partOpacity,
    strokeWidth: s.strokeWidth,
    // Derived by the kernel from targetEdgeLength; values here are inert.
    maxDepth: 6,
    interactionRadius: s.targetEdgeLength,
    // Pipeline: skeleton → PASM Gaussian relaxation → per-face CDT.
    brepSkeleton: true,
    pasmKernel: true,
    cdtFaces: true,
    repulsion: true,
    repulsionStrength: 0.5,
    repulsionDamping: 0.2,
    dampingCurve: "linear",
    fitOnSurface: true,
    onlyOnSurface: true,
    fitTargetEpsilon: 0.001,
    fitStepMultiplier: 0.8,
    // Seeding: the near-surface band at the target spacing.
    bandGuard: 1,
    nearSurfaceOnly: true,
    nearSurfaceDist: s.targetEdgeLength,
    signChangeNeighboursOnly: false,
    constrainToCell: false,
    // Display: particles + CDT wireframe, no gradient needles.
    normalScale: 0,
    surfaceThreshold: 0.05,
    // Legacy scaffold stages stay off.
    featureSnap: false,
    featureAngleDeg: 30,
    meshPoints: false,
    balanceOctree: false,
  };
}

export function MesherPasmDialog({ onClose }: { onClose: () => void }) {
  const initial = useMemo(loadSettings, []);
  const [targetEdgeLength, setTargetEdgeLength] = useState(initial.targetEdgeLength);
  const [iterations, setIterations] = useState(initial.iterations);
  const [partOpacity, setPartOpacity] = useState(initial.partOpacity);
  const [strokeWidth, setStrokeWidth] = useState(initial.strokeWidth);

  const stats = useSyncExternalStore(subscribeMesherStats, getMesherStats);

  // Publish while open; persist; clear on unmount.
  useEffect(() => {
    const s: PasmSettings = { targetEdgeLength, iterations, partOpacity, strokeWidth };
    setMesherDebug(pasmParams(s));
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
    } catch {
      // no localStorage (tests) — non-persistent is fine
    }
  }, [targetEdgeLength, iterations, partOpacity, strokeWidth]);
  useEffect(() => () => setMesherDebug(null), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  // ── Drag the header ───────────────────────────────────────────────
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const dragRef = useRef<{ dx: number; dy: number } | null>(null);
  const onHeaderDown = useCallback((e: ReactPointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    const dialog = e.currentTarget.parentElement as HTMLElement | null;
    if (!dialog) return;
    const r = dialog.getBoundingClientRect();
    dragRef.current = { dx: e.clientX - r.left, dy: e.clientY - r.top };
    setPos({ x: r.left, y: r.top });
    e.currentTarget.setPointerCapture(e.pointerId);
  }, []);
  const onHeaderMove = useCallback((e: ReactPointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    setPos({
      x: Math.max(0, Math.min(window.innerWidth - 80, e.clientX - d.dx)),
      y: Math.max(0, Math.min(window.innerHeight - 40, e.clientY - d.dy)),
    });
  }, []);
  const onHeaderUp = useCallback((e: ReactPointerEvent) => {
    dragRef.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
  }, []);

  return (
    <div
      className="fd-mesher-dialog fd-mesher-pasm-dialog"
      role="dialog"
      aria-label="Mesher PASM"
      data-testid="mesher-pasm-dialog"
      style={pos ? { left: pos.x, top: pos.y, right: "auto" } : undefined}
    >
      <div
        className="dialog-header"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <h2 className="dialog-title">Mesher — PASM + B-Rep</h2>
        <button
          type="button"
          className="dialog-close"
          aria-label="Close"
          onClick={onClose}
        >
          ×
        </button>
      </div>

      <div className="vec-row">
        <label
          title={
            "THE resolution parameter: the edge length the mesh aims for (mm). " +
            "Everything scales off it — skeleton chain + seam spacing = h, " +
            "Gaussian kernel σ = h/5 (cutoff 5σ = h), seed octree cell ≈ h, " +
            "CDT boundary clearance 0.25·h."
          }
        >
          Edge length h (mm)
        </label>
        <div className="fields">
          <NumberInput
            min={0.1}
            max={1e3}
            step={0.5}
            aria-label="Edge length h (mm)"
            value={targetEdgeLength}
            onChange={setTargetEdgeLength}
          />
        </div>
      </div>

      <div className="vec-row">
        <label
          title={
            "Cap on the relaxation iterations — the loop stops earlier once the " +
            "max particle displacement falls below 1e-3·h (converged). The " +
            "paper needs ~100 without population control."
          }
        >
          Iterations (max)
        </label>
        <div className="fields">
          <NumberInput
            min={1}
            max={200}
            step={1}
            integer
            aria-label="Iterations (max)"
            value={iterations}
            onChange={setIterations}
          />
        </div>
      </div>

      {numRowLikeSlider(
        "Part opacity",
        partOpacity,
        setPartOpacity,
        "Fade the solid so the skeleton, particles and wireframe read clearly.",
      )}

      <div className="vec-row">
        <label title="Screen-space stroke width (px) of the wireframe; dots are 3×.">
          Stroke width
        </label>
        <div className="fields">
          <NumberInput
            min={0.5}
            max={12}
            step={0.5}
            aria-label="Stroke width"
            value={strokeWidth}
            onChange={setStrokeWidth}
          />
        </div>
      </div>

      <div className="readout">
        <span className="k">Points</span>
        <span className="v" data-testid="mesher-pasm-points">
          {stats ? stats.cells.toLocaleString() + (stats.capped ? " (capped)" : "") : "—"}
        </span>
      </div>
      <div className="readout">
        <span className="k">Triangles</span>
        <span className="v" data-testid="mesher-pasm-tris">
          {stats ? stats.tris.toLocaleString() : "—"}
        </span>
      </div>

      <div className="legend">
        <span className="corner">corner (pinned)</span>
        <span className="chain">edge chain (fixed)</span>
        <span className="particle">surface particle</span>
        <span className="hint">
          skeleton → PASM relaxation (σ = h/5) → per-face CDT
        </span>
      </div>
    </div>
  );
}

function numRowLikeSlider(
  label: string,
  value: number,
  set: (n: number) => void,
  title: string,
) {
  return (
    <div className="vec-row">
      <label title={title}>{label}</label>
      <div className="fields">
        <input
          type="range"
          min={0}
          max={1}
          step={0.01}
          aria-label={label}
          value={value}
          onChange={(e) => set(Number(e.currentTarget.value))}
        />
        <span className="slider-val">{value.toFixed(2)}</span>
      </div>
    </div>
  );
}
