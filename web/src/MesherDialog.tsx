/**
 * Debug ▸ Mesher dialog — the scaffold for building a custom meshing method
 * step by step, with a debug visualisation from the start.
 *
 * Step 1: a narrow-band adaptive octree. While open it publishes the octree
 * parameters to `mesherDebugStore`; the viewport (`MesherOverlay`) samples the
 * kernel octree and draws each leaf cell's centre colour-coded by its SDF value
 * with a short line along its gradient (the surface normal). Non-modal +
 * draggable; settings persist in localStorage.
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
import type { MesherDampingCurve } from "./mesherDebugStore";

const SETTINGS_KEY = "fielddraft.debug.mesher";
const DEFAULTS: MesherDebug = {
  maxDepth: 6,
  bandGuard: 1,
  normalScale: 0.6,
  strokeWidth: 2,
  surfaceThreshold: 0.05,
  partOpacity: 1,
  nearSurfaceOnly: false,
  nearSurfaceDist: 1,
  signChangeNeighboursOnly: false,
  constrainToCell: true,
  iterations: 12,
  fitOnSurface: false,
  onlyOnSurface: false,
  fitTargetEpsilon: 0.001,
  fitStepMultiplier: 0.8,
  repulsion: false,
  interactionRadius: 2,
  repulsionStrength: 0.5,
  repulsionDamping: 0.2,
  dampingCurve: "linear",
  meshPoints: false,
  balanceOctree: true,
};

const clampNum = (v: unknown, lo: number, hi: number, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v)
    ? Math.min(hi, Math.max(lo, v))
    : fallback;

function loadSettings(): MesherDebug {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULTS;
    const p = JSON.parse(raw) as Partial<MesherDebug>;
    return {
      maxDepth: Math.round(clampNum(p.maxDepth, 1, 10, DEFAULTS.maxDepth)),
      bandGuard: clampNum(p.bandGuard, 0.1, 8, DEFAULTS.bandGuard),
      normalScale: clampNum(p.normalScale, 0, 4, DEFAULTS.normalScale),
      strokeWidth: clampNum(p.strokeWidth, 0.5, 12, DEFAULTS.strokeWidth),
      surfaceThreshold: clampNum(p.surfaceThreshold, 0, 1e6, DEFAULTS.surfaceThreshold),
      partOpacity: clampNum(p.partOpacity, 0, 1, DEFAULTS.partOpacity),
      nearSurfaceOnly:
        typeof p.nearSurfaceOnly === "boolean"
          ? p.nearSurfaceOnly
          : DEFAULTS.nearSurfaceOnly,
      nearSurfaceDist: clampNum(p.nearSurfaceDist, 0, 1e6, DEFAULTS.nearSurfaceDist),
      signChangeNeighboursOnly:
        typeof p.signChangeNeighboursOnly === "boolean"
          ? p.signChangeNeighboursOnly
          : DEFAULTS.signChangeNeighboursOnly,
      constrainToCell:
        typeof p.constrainToCell === "boolean" ? p.constrainToCell : DEFAULTS.constrainToCell,
      iterations: Math.round(clampNum(p.iterations, 1, 200, DEFAULTS.iterations)),
      fitOnSurface:
        typeof p.fitOnSurface === "boolean" ? p.fitOnSurface : DEFAULTS.fitOnSurface,
      onlyOnSurface:
        typeof p.onlyOnSurface === "boolean" ? p.onlyOnSurface : DEFAULTS.onlyOnSurface,
      fitTargetEpsilon: clampNum(p.fitTargetEpsilon, 0, 1e6, DEFAULTS.fitTargetEpsilon),
      fitStepMultiplier: clampNum(p.fitStepMultiplier, 0.1, 1, DEFAULTS.fitStepMultiplier),
      repulsion: typeof p.repulsion === "boolean" ? p.repulsion : DEFAULTS.repulsion,
      interactionRadius: clampNum(p.interactionRadius, 0, 1e6, DEFAULTS.interactionRadius),
      repulsionStrength: clampNum(p.repulsionStrength, 0, 100, DEFAULTS.repulsionStrength),
      repulsionDamping: clampNum(p.repulsionDamping, 0, 1, DEFAULTS.repulsionDamping),
      dampingCurve:
        p.dampingCurve === "exponential" ? "exponential" : DEFAULTS.dampingCurve,
      meshPoints: typeof p.meshPoints === "boolean" ? p.meshPoints : DEFAULTS.meshPoints,
      balanceOctree:
        typeof p.balanceOctree === "boolean" ? p.balanceOctree : DEFAULTS.balanceOctree,
    };
  } catch {
    return DEFAULTS;
  }
}

export function MesherDialog({ onClose }: { onClose: () => void }) {
  const initial = useMemo(loadSettings, []);
  const [maxDepth, setMaxDepth] = useState(initial.maxDepth);
  const [bandGuard, setBandGuard] = useState(initial.bandGuard);
  const [normalScale, setNormalScale] = useState(initial.normalScale);
  const [strokeWidth, setStrokeWidth] = useState(initial.strokeWidth);
  const [surfaceThreshold, setSurfaceThreshold] = useState(initial.surfaceThreshold);
  const [partOpacity, setPartOpacity] = useState(initial.partOpacity);
  const [nearSurfaceOnly, setNearSurfaceOnly] = useState(initial.nearSurfaceOnly);
  const [nearSurfaceDist, setNearSurfaceDist] = useState(initial.nearSurfaceDist);
  const [signChangeNeighboursOnly, setSignChangeNeighboursOnly] = useState(
    initial.signChangeNeighboursOnly,
  );
  const [constrainToCell, setConstrainToCell] = useState(initial.constrainToCell);
  const [iterations, setIterations] = useState(initial.iterations);
  const [fitOnSurface, setFitOnSurface] = useState(initial.fitOnSurface);
  const [onlyOnSurface, setOnlyOnSurface] = useState(initial.onlyOnSurface);
  const [fitTargetEpsilon, setFitTargetEpsilon] = useState(initial.fitTargetEpsilon);
  const [fitStepMultiplier, setFitStepMultiplier] = useState(initial.fitStepMultiplier);
  const [repulsion, setRepulsion] = useState(initial.repulsion);
  const [interactionRadius, setInteractionRadius] = useState(initial.interactionRadius);
  const [repulsionStrength, setRepulsionStrength] = useState(initial.repulsionStrength);
  const [repulsionDamping, setRepulsionDamping] = useState(initial.repulsionDamping);
  const [dampingCurve, setDampingCurve] = useState<MesherDampingCurve>(initial.dampingCurve);
  const [meshPoints, setMeshPoints] = useState(initial.meshPoints);
  const [balanceOctree, setBalanceOctree] = useState(initial.balanceOctree);

  const stats = useSyncExternalStore(subscribeMesherStats, getMesherStats);

  // Publish while open; persist; clear on unmount.
  useEffect(() => {
    const s: MesherDebug = {
      maxDepth,
      bandGuard,
      normalScale,
      strokeWidth,
      surfaceThreshold,
      partOpacity,
      nearSurfaceOnly,
      nearSurfaceDist,
      signChangeNeighboursOnly,
      constrainToCell,
      iterations,
      fitOnSurface,
      onlyOnSurface,
      fitTargetEpsilon,
      fitStepMultiplier,
      repulsion,
      interactionRadius,
      repulsionStrength,
      repulsionDamping,
      dampingCurve,
      meshPoints,
      balanceOctree,
    };
    setMesherDebug(s);
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
    } catch {
      // no localStorage (tests) — non-persistent is fine
    }
  }, [
    maxDepth,
    bandGuard,
    normalScale,
    strokeWidth,
    surfaceThreshold,
    partOpacity,
    nearSurfaceOnly,
    nearSurfaceDist,
    signChangeNeighboursOnly,
    constrainToCell,
    iterations,
    fitOnSurface,
    onlyOnSurface,
    fitTargetEpsilon,
    fitStepMultiplier,
    repulsion,
    interactionRadius,
    repulsionStrength,
    repulsionDamping,
    dampingCurve,
    meshPoints,
    balanceOctree,
  ]);
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

  const numRow = (
    label: string,
    value: number,
    set: (n: number) => void,
    opts: { min: number; max: number; step: number; round?: boolean },
    title?: string,
  ) => (
    <div className="vec-row">
      <label title={title}>{label}</label>
      <div className="fields">
        <NumberInput
          min={opts.min}
          max={opts.max}
          step={opts.step}
          integer={opts.round}
          aria-label={label}
          value={value}
          onChange={set}
        />
      </div>
    </div>
  );

  const sliderRow = (
    label: string,
    value: number,
    set: (n: number) => void,
    opts: { min: number; max: number; step: number },
    title?: string,
  ) => (
    <div className="vec-row">
      <label title={title}>{label}</label>
      <div className="fields">
        <input
          type="range"
          min={opts.min}
          max={opts.max}
          step={opts.step}
          aria-label={label}
          value={value}
          onChange={(e) => set(Number(e.currentTarget.value))}
        />
        <span className="slider-val">{value.toFixed(2)}</span>
      </div>
    </div>
  );

  return (
    <div
      className="fd-mesher-dialog"
      role="dialog"
      aria-label="Mesher"
      data-testid="mesher-dialog"
      style={pos ? { left: pos.x, top: pos.y, right: "auto" } : undefined}
    >
      <div
        className="dialog-header"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <h2 className="dialog-title">Mesher — adaptive octree</h2>
        <button
          type="button"
          className="dialog-close"
          aria-label="Close"
          onClick={onClose}
        >
          ×
        </button>
      </div>

      {numRow("Max depth", maxDepth, setMaxDepth, {
        min: 1,
        max: 10,
        step: 1,
        round: true,
      })}
      {numRow(
        "Band width",
        bandGuard,
        setBandGuard,
        { min: 0.1, max: 8, step: 0.1 },
        "Narrow-band width for octree subdivision (NOT an inside/outside colour " +
          "threshold). A cell keeps subdividing while |SDF| ≤ ½·cell-diagonal × " +
          "this value. Larger = a wider shell around the surface is refined — but " +
          "it saturates above ~1 (the surface band is already fully refined), so " +
          "1→8 looks the same. Below 1 the band tightens, so fewer near-surface " +
          "cells (and normals) appear.",
      )}
      {numRow("Normal length", normalScale, setNormalScale, {
        min: 0,
        max: 4,
        step: 0.1,
      })}
      {numRow("Stroke width", strokeWidth, setStrokeWidth, {
        min: 0.5,
        max: 12,
        step: 0.5,
      })}
      {numRow(
        "Surface ± (mm)",
        surfaceThreshold,
        setSurfaceThreshold,
        { min: 0, max: 100, step: 0.01 },
        "Colour threshold: samples with |SDF| ≤ this are green (surface); " +
          "beyond it the colour ramps to blue (inside) / red (outside).",
      )}
      {sliderRow(
        "Part opacity",
        partOpacity,
        setPartOpacity,
        { min: 0, max: 1, step: 0.01 },
        "Fade the solid so you can see inside. Below 1 the interior octree " +
          "samples/normals are shown too.",
      )}

      <div className="vec-row check-row">
        <label htmlFor="mesher-near-surface">Near surface only</label>
        <input
          id="mesher-near-surface"
          type="checkbox"
          data-testid="mesher-near-surface"
          checked={nearSurfaceOnly}
          onChange={(e) => setNearSurfaceOnly(e.target.checked)}
        />
      </div>
      {nearSurfaceOnly &&
        numRow("Distance (mm)", nearSurfaceDist, setNearSurfaceDist, {
          min: 0,
          max: 1e6,
          step: 0.5,
        })}

      <div className="vec-row check-row">
        <label
          htmlFor="mesher-sign-change"
          title="Processing filter B: only points in cells whose SDF sign flips towards a direct neighbour cell are optimised (ANDed with the near-surface filter). Everything else stays rigid."
        >
          Neighbour sign change only
        </label>
        <input
          id="mesher-sign-change"
          type="checkbox"
          data-testid="mesher-sign-change"
          checked={signChangeNeighboursOnly}
          onChange={(e) => setSignChangeNeighboursOnly(e.target.checked)}
        />
      </div>

      <div className="vec-row check-row">
        <label
          htmlFor="mesher-constrain-cell"
          title="Topological protection: a point may never leave the bounding box of its origin octree leaf cell — every position update is clamped to the cell bounds."
        >
          Constrain to cell
        </label>
        <input
          id="mesher-constrain-cell"
          type="checkbox"
          data-testid="mesher-constrain-cell"
          checked={constrainToCell}
          onChange={(e) => setConstrainToCell(e.target.checked)}
        />
      </div>

      {numRow(
        "Iterations",
        iterations,
        setIterations,
        { min: 1, max: 200, step: 1, round: true },
        "Iterations of the single relaxation loop (Repulsion → Fit → Clamp), " +
          "and the max budget of the final fit-to-surface snap. One loop now.",
      )}

      <div className="vec-row check-row">
        <label
          htmlFor="mesher-fit-surface"
          title="Damped Newton projection: p ← p − step·d·∇f until |d| < epsilon or the iteration cap."
        >
          Fit on surface
        </label>
        <input
          id="mesher-fit-surface"
          type="checkbox"
          data-testid="mesher-fit-surface"
          checked={fitOnSurface}
          onChange={(e) => setFitOnSurface(e.target.checked)}
        />
      </div>
      {fitOnSurface && (
        <>
          <div className="vec-row check-row">
            <label
              htmlFor="mesher-only-on-surface"
              title="Keep only cells whose point actually REACHED the surface after the projection: a point clamped at its cell bounds (surface out of reach) drops out of the set entirely — not drawn, not repelled, no mesh vertex."
            >
              Only on surface
            </label>
            <input
              id="mesher-only-on-surface"
              type="checkbox"
              data-testid="mesher-only-on-surface"
              checked={onlyOnSurface}
              onChange={(e) => setOnlyOnSurface(e.target.checked)}
            />
          </div>
          {numRow("Target ε (mm)", fitTargetEpsilon, setFitTargetEpsilon, {
            min: 0,
            max: 1e6,
            step: 0.001,
          })}
          {sliderRow(
            "Step multiplier",
            fitStepMultiplier,
            setFitStepMultiplier,
            { min: 0.1, max: 1, step: 0.05 },
            "Newton damping — 1 = full step, lower is more robust on non-1-Lipschitz fields.",
          )}
        </>
      )}

      <div className="vec-row check-row">
        <label
          htmlFor="mesher-repulsion"
          title="Tangential relaxation over the single loop: per iteration the neighbours within the interaction radius repel each point in the surface tangent plane, then a Fit step re-pins it. The repulsion strength is annealed down across the iterations (strong first, fading), so points spread early and settle onto the surface at the end."
        >
          Repulsion inside surface
        </label>
        <input
          id="mesher-repulsion"
          type="checkbox"
          data-testid="mesher-repulsion"
          checked={repulsion}
          onChange={(e) => setRepulsion(e.target.checked)}
        />
      </div>
      {repulsion && (
        <>
          {numRow("Interaction radius (mm)", interactionRadius, setInteractionRadius, {
            min: 0,
            max: 1e6,
            step: 0.5,
          })}
          {numRow(
            "Strength (initial)",
            repulsionStrength,
            setRepulsionStrength,
            { min: 0, max: 100, step: 0.05 },
            "Repulsion strength at the FIRST iteration; it is then annealed down. " +
              "The pairwise force follows an inverse-square law (∝ 1/d², like " +
              "equally-charged monopoles): closer points repel far harder, so " +
              "clusters break up and the spread stays even and local — start low.",
          )}
          {sliderRow(
            "Damping",
            repulsionDamping,
            setRepulsionDamping,
            { min: 0, max: 1, step: 0.05 },
            "Annealing: fraction of the repulsion strength lost per iteration " +
              "(0 = constant, 1 = gone after one linear step). Higher = the fit " +
              "takes over sooner, so points settle onto the surface earlier.",
          )}
          <div className="vec-row">
            <label
              htmlFor="mesher-damping-curve"
              title="How the repulsion strength decays over the iterations: Linear = strength·max(0, 1 − damping·t); Exponential = strength·(1 − damping)^t."
            >
              Damping curve
            </label>
            <div className="fields">
              <select
                id="mesher-damping-curve"
                data-testid="mesher-damping-curve"
                value={dampingCurve}
                onChange={(e) => setDampingCurve(e.target.value as MesherDampingCurve)}
              >
                <option value="linear">Linear</option>
                <option value="exponential">Exponential</option>
              </select>
            </div>
          </div>
        </>
      )}

      <div className="vec-row check-row">
        <label
          htmlFor="mesher-mesh-points"
          title="Adaptive dual contouring over the optimised points: one vertex per leaf cell, a quad wherever a minimal octree edge crosses the surface (crack-free on the adaptive octree via the recursive cell/face/edge walk)."
        >
          Mesh points (dual contour)
        </label>
        <input
          id="mesher-mesh-points"
          type="checkbox"
          data-testid="mesher-mesh-points"
          checked={meshPoints}
          onChange={(e) => setMeshPoints(e.target.checked)}
        />
      </div>
      {meshPoints && (
        <div className="vec-row check-row">
          <label
            htmlFor="mesher-balance"
            title="Enforce the 2:1 rule before meshing: face-adjacent leaves differ by at most one depth level — bounds the size contrast of neighbouring triangles."
          >
            Balance octree (2:1)
          </label>
          <input
            id="mesher-balance"
            type="checkbox"
            data-testid="mesher-balance"
            checked={balanceOctree}
            onChange={(e) => setBalanceOctree(e.target.checked)}
          />
        </div>
      )}

      <div className="readout">
        <span className="k">Leaf cells</span>
        <span className="v" data-testid="mesher-cells">
          {stats ? stats.cells.toLocaleString() + (stats.capped ? " (capped)" : "") : "—"}
        </span>
      </div>
      {meshPoints && (
        <div className="readout">
          <span className="k">Triangles</span>
          <span className="v" data-testid="mesher-tris">
            {stats ? stats.tris.toLocaleString() : "—"}
          </span>
        </div>
      )}

      <div className="legend">
        <span className="in">inside</span>
        <span className="zero">surface</span>
        <span className="out">outside</span>
        <span className="hint">
          each leaf: centre coloured by SDF, line along ∇f
        </span>
      </div>
    </div>
  );
}
