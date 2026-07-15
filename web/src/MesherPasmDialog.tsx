/**
 * Debug ▸ Mesher PASM dialog — the DEDICATED front-end of the hybrid
 * PASM+B-Rep meshing pipeline (docs/HYBRID-PASM-MESHING-PLAN.md, ticket
 * nwmi). Unlike the generic Debug ▸ Mesher scaffold with its per-stage
 * checkboxes, this dialog drives the one clean chain
 *
 *   B-Rep skeleton (corners pinned, chains on the exact curves)
 *   → PASM Gaussian relaxation (σ = h/5, tangential, convergence break)
 *   → per-face constrained Delaunay in the carrier charts
 *   → §6-F3 fallback for chartless faces (signed-DC patch + rim zipper)
 *
 * and exposes only what the method actually needs: the target edge length h
 * (everything else — seed octree depth, kernel radius, chain spacing, seam
 * sampling, boundary clearance — is derived from h, mostly in the kernel via
 * `targetEdgeLength`) plus an iteration cap and display knobs. A STEP
 * selector truncates the pipeline after any stage so each stage's output can
 * be inspected in the scene (seeds → skeleton → relaxed particles → CDT
 * wireframe → fallback patches in orange). Both Mesher dialogs publish into
 * the same `mesherDebugStore`, so `projectDebug` keeps them mutually
 * exclusive. Non-modal + draggable; settings persist in localStorage.
 */

import {
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";
import {
  setMesherDebug,
  subscribeMesherStats,
  getMesherStats,
  type MesherDebug,
} from "./mesherDebugStore";
import { NumberInput } from "./NumberInput";
import { useDialogDrag } from "./useDialogDrag";

const SETTINGS_KEY = "fielddraft.debug.mesherPasm";

/** Pipeline truncation for visual step-debugging — each step ADDS one stage
 *  on top of the previous ones (the algorithm is a strict chain). */
export type PasmStep = "seeds" | "skeleton" | "relax" | "cdt" | "full";

const STEPS: { key: PasmStep; label: string; hint: string }[] = [
  {
    key: "seeds",
    label: "Seeds",
    hint: "Stufe 1 — rohe Seed-Punkte: Zentren der oberflächennahen Octree-Zellen (Band ±h).",
  },
  {
    key: "skeleton",
    label: "Skelett",
    hint: "Stufe 2 — B-Rep-Skelett: Ecken exakt (rot), Kanten-Ketten bogenlängen-gesampelt (orange). Fixe Randbedingungen.",
  },
  {
    key: "relax",
    label: "Relax",
    hint: "Stufe 3 — PASM-Relaxation: Gauß-Kernel (σ = h/5), tangential, auf die Fläche projiziert; Skelett stößt ab, bewegt sich nie.",
  },
  {
    key: "cdt",
    label: "CDT",
    hint: "Stufe 4 — per-Face Constrained Delaunay im Träger-Chart (dunkles Drahtgitter); Nähte teilen globale Indizes → wasserdicht.",
  },
  {
    key: "full",
    label: "Fallback",
    hint: "Stufe 5 — chartlose Flächen (Torus/Gyroid/Custom-SDF): signed-DC-Patch + Reißverschluss an die Rim-Ketten (oranges Drahtgitter).",
  },
];

interface PasmSettings {
  /** Target edge length h (mm) — THE resolution parameter. */
  targetEdgeLength: number;
  /** Relaxation iteration cap (convergence usually breaks earlier). */
  iterations: number;
  /** Run the pipeline only up to this stage (visual debugging). */
  step: PasmStep;
  /** P4d-2: curvature-anisotropic relaxation (denser where curved). */
  anisotropy: boolean;
  partOpacity: number;
  strokeWidth: number;
}

const DEFAULTS: PasmSettings = {
  targetEdgeLength: 2,
  iterations: 40,
  step: "full",
  anisotropy: false,
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
      step: STEPS.some((s) => s.key === p.step) ? (p.step as PasmStep) : DEFAULTS.step,
      anisotropy: p.anisotropy === true,
      partOpacity: clampNum(p.partOpacity, 0, 1, DEFAULTS.partOpacity),
      strokeWidth: clampNum(p.strokeWidth, 0.5, 12, DEFAULTS.strokeWidth),
    };
  } catch {
    return DEFAULTS;
  }
}

/** The fixed PASM pipeline preset around the few exposed knobs. The kernel
 *  derives `maxDepth` and `interactionRadius` from `targetEdgeLength`; the
 *  STEP truncates the chain after a stage for visual debugging. */
function pasmParams(s: PasmSettings): MesherDebug {
  const at = STEPS.findIndex((st) => st.key === s.step);
  return {
    targetEdgeLength: s.targetEdgeLength,
    iterations: s.iterations,
    partOpacity: s.partOpacity,
    strokeWidth: s.strokeWidth,
    // Derived by the kernel from targetEdgeLength; values here are inert.
    maxDepth: 6,
    interactionRadius: s.targetEdgeLength,
    // Pipeline: skeleton → PASM relaxation → per-face CDT → F3 fallback,
    // cumulatively enabled up to the selected step.
    brepSkeleton: at >= 1,
    fitOnSurface: at >= 2,
    onlyOnSurface: at >= 2,
    repulsion: at >= 2,
    pasmKernel: at >= 2,
    cdtFaces: at >= 3,
    fallbackPatches: at >= 4,
    repulsionStrength: 0.5,
    repulsionDamping: 0.2,
    dampingCurve: "linear",
    fitTargetEpsilon: 0.001,
    fitStepMultiplier: 0.8,
    // P4d-2: curvature anisotropy (opt-in — denser where curved, coarser
    // where flat; higher mesh cost from the per-particle shape operator).
    anisotropy: s.anisotropy,
    anisoSag: 0.02,
    anisoSmax: 8,
    // Seeding: the near-surface band at the target spacing.
    bandGuard: 1,
    nearSurfaceOnly: true,
    nearSurfaceDist: s.targetEdgeLength,
    signChangeNeighboursOnly: false,
    constrainToCell: false,
    // Display: particles + wireframes, no gradient needles.
    normalScale: 0,
    surfaceThreshold: 0.05,
    // Legacy scaffold stages stay off; balance serves the fallback DC.
    featureSnap: false,
    featureAngleDeg: 30,
    meshPoints: false,
    balanceOctree: true,
  };
}

export function MesherPasmDialog({ onClose }: { onClose: () => void }) {
  const initial = useMemo(loadSettings, []);
  const [targetEdgeLength, setTargetEdgeLength] = useState(initial.targetEdgeLength);
  const [iterations, setIterations] = useState(initial.iterations);
  const [step, setStep] = useState<PasmStep>(initial.step);
  const [anisotropy, setAnisotropy] = useState(initial.anisotropy);
  const [partOpacity, setPartOpacity] = useState(initial.partOpacity);
  const [strokeWidth, setStrokeWidth] = useState(initial.strokeWidth);

  const stats = useSyncExternalStore(subscribeMesherStats, getMesherStats);

  // Publish while open; persist; clear on unmount.
  useEffect(() => {
    const s: PasmSettings = {
      targetEdgeLength,
      iterations,
      step,
      anisotropy,
      partOpacity,
      strokeWidth,
    };
    setMesherDebug(pasmParams(s));
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
    } catch {
      // no localStorage (tests) — non-persistent is fine
    }
  }, [targetEdgeLength, iterations, step, anisotropy, partOpacity, strokeWidth]);
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
  const { positionStyle, onHeaderDown, onHeaderMove, onHeaderUp } = useDialogDrag();

  return (
    <div
      className="fd-mesher-dialog fd-mesher-pasm-dialog"
      role="dialog"
      aria-label="Mesher PASM"
      data-testid="mesher-pasm-dialog"
      style={positionStyle}
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
            // Ticket 0018657: re-weaves on change → commit on blur/Enter so a
            // half-typed "0," can't clamp to the min and hang on a fine mesh.
            commitOnChange={false}
          />
        </div>
      </div>

      <div className="step-row" role="radiogroup" aria-label="Pipeline step">
        {STEPS.map((st, i) => (
          <button
            key={st.key}
            type="button"
            role="radio"
            aria-checked={step === st.key}
            className={`step${step === st.key ? " active" : ""}`}
            title={st.hint}
            data-testid={`mesher-pasm-step-${st.key}`}
            onClick={() => setStep(st.key)}
          >
            {i + 1} {st.label}
          </button>
        ))}
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
            commitOnChange={false}
          />
        </div>
      </div>

      <div className="vec-row">
        <label
          title={
            "Curvature anisotropy (P4d-2): the metric-warped Gaussian force " +
            "(paper Gl. 21, Q not M) packs particles denser where the surface " +
            "curves and coarser where it's flat — fewer triangles at the same " +
            "sag (ε = 0.02 mm), at a higher mesh cost (shape operator per " +
            "particle). Off = the isotropic kernel."
          }
        >
          Anisotropy
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="Anisotropy"
              data-testid="mesher-pasm-anisotropy"
              checked={anisotropy}
              onChange={(e) => setAnisotropy(e.currentTarget.checked)}
            />
          </label>
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
      <div className="readout">
        <span className="k">Fallback tris</span>
        <span className="v" data-testid="mesher-pasm-fallback-tris">
          {stats ? (stats.fallbackTris ?? 0).toLocaleString() : "—"}
        </span>
      </div>

      <div className="legend">
        <span className="corner">corner (pinned)</span>
        <span className="chain">edge chain (fixed)</span>
        <span className="particle">surface particle</span>
        <span className="fallback">fallback patch</span>
        <span className="hint">
          skeleton → PASM relaxation (σ = h/5) → per-face CDT → DC fallback
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
