/**
 * Debug ▸ Mesher: Lichen dialog — the dedicated front-end of the new "Lichen"
 * debug mesher (`Kernel::lichenMesh`). Mirrors the Mesher-PASM dialog's
 * live-reactive pattern: while it is open it publishes its parameters into
 * `lichenDebugStore` and the viewport (`LichenOverlay`) draws the result. No
 * run button — every edit re-publishes.
 *
 * Lichen has its OWN store + overlay (separate from the PASM/generic mesher),
 * so it never fights the other Mesher dialogs over the shared singleton.
 *
 * Exposes the Lichen knobs the kernel reads (target edge length h, Waben-
 * Wachstum, curvature α, reach, particle cap) plus the display knobs (part
 * opacity, stroke width). Non-modal + draggable; settings persist in
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
  setLichenDebug,
  subscribeLichenStats,
  getLichenStats,
  type LichenDebug,
} from "./lichenDebugStore";
import { NumberInput } from "./NumberInput";

const SETTINGS_KEY = "fielddraft.debug.lichen";

interface LichenSettings {
  /** Target edge length h (mm) — THE resolution parameter. */
  targetEdgeLength: number;
  /** Waben-Wachstum: grow the particle set outward. */
  grow: boolean;
  /** Max. Normalen-Knickwinkel pro Kante in Grad — kleiner = dichter, wo die
   *  Fläche/Kante krümmt. */
  curvAngleDeg: number;
  /** Bound U(p) by the local reach (outward footpoint tracking). */
  useReach: boolean;
  /** Footpoint departure threshold (mm) for the reach march. */
  reachTol: number;
  /** Hard cap on the particle count. */
  maxPoints: number;
  partOpacity: number;
  strokeWidth: number;
  /** Draw the U(p) reach circle in the tangent plane at each point. */
  showCircles: boolean;
  /** Draw the point markers (corner/edge/surface). */
  showPoints: boolean;
  /** Draw the triangle mesh wireframe (once meshing emits it). */
  showTriangles: boolean;
  /** Edge length (mm) of the point-marker boxes. */
  pointSize: number;
  /** Draw a needle along each point's ∇f normal. */
  showNormals: boolean;
  /** Length (mm) of the normal needles. */
  normalLength: number;
  /** Lift the whole overlay this many mm toward the camera (view-space Z). */
  zOffset: number;
}

const DEFAULTS: LichenSettings = {
  targetEdgeLength: 10,
  grow: true,
  curvAngleDeg: 20,
  useReach: false,
  reachTol: 1,
  maxPoints: 50000,
  partOpacity: 0.4,
  strokeWidth: 2,
  showCircles: true,
  showPoints: true,
  showTriangles: true,
  pointSize: 1,
  showNormals: false,
  normalLength: 1,
  zOffset: 2,
};

const clampNum = (v: unknown, lo: number, hi: number, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v)
    ? Math.min(hi, Math.max(lo, v))
    : fallback;

function loadSettings(): LichenSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULTS;
    const p = JSON.parse(raw) as Partial<LichenSettings>;
    return {
      targetEdgeLength: clampNum(
        p.targetEdgeLength,
        0.1,
        1e3,
        DEFAULTS.targetEdgeLength,
      ),
      grow: p.grow === undefined ? DEFAULTS.grow : p.grow === true,
      curvAngleDeg: clampNum(p.curvAngleDeg, 0, 90, DEFAULTS.curvAngleDeg),
      useReach: p.useReach === true,
      reachTol: clampNum(p.reachTol, 0.001, 1e6, DEFAULTS.reachTol),
      maxPoints: Math.round(
        clampNum(p.maxPoints, 1, 2_000_000, DEFAULTS.maxPoints),
      ),
      partOpacity: clampNum(p.partOpacity, 0, 1, DEFAULTS.partOpacity),
      strokeWidth: clampNum(p.strokeWidth, 0.5, 12, DEFAULTS.strokeWidth),
      showCircles: p.showCircles === undefined ? DEFAULTS.showCircles : p.showCircles === true,
      showPoints: p.showPoints === undefined ? DEFAULTS.showPoints : p.showPoints === true,
      showTriangles:
        p.showTriangles === undefined ? DEFAULTS.showTriangles : p.showTriangles === true,
      pointSize: clampNum(p.pointSize, 0.5, 100, DEFAULTS.pointSize),
      showNormals: p.showNormals === true,
      normalLength: clampNum(p.normalLength, 0, 1000, DEFAULTS.normalLength),
      zOffset: clampNum(p.zOffset, 0, 1000, DEFAULTS.zOffset),
    };
  } catch {
    return DEFAULTS;
  }
}

/** Map the exposed knobs onto the published `LichenDebug` blob. The overlay
 *  reads the display fields (surfaceThreshold/normalScale fixed, like the PASM
 *  preset); the kernel reads targetEdgeLength/grow/curvAngleDeg/useReach/maxPoints. */
function lichenParams(s: LichenSettings): LichenDebug {
  return {
    targetEdgeLength: s.targetEdgeLength,
    grow: s.grow,
    curvAngleDeg: s.curvAngleDeg,
    useReach: s.useReach,
    reachTol: s.reachTol,
    maxPoints: s.maxPoints,
    partOpacity: s.partOpacity,
    strokeWidth: s.strokeWidth,
    showCircles: s.showCircles,
    showPoints: s.showPoints,
    showTriangles: s.showTriangles,
    pointSize: s.pointSize,
    showNormals: s.showNormals,
    normalLength: s.normalLength,
    zOffset: s.zOffset,
    // Display: 3-way SDF colouring band (unused for markers now).
    surfaceThreshold: 0.05,
    normalScale: 0,
  };
}

export function MesherLichenDialog({ onClose }: { onClose: () => void }) {
  const initial = useMemo(loadSettings, []);
  const [targetEdgeLength, setTargetEdgeLength] = useState(initial.targetEdgeLength);
  const [grow, setGrow] = useState(initial.grow);
  const [curvAngleDeg, setCurvAngleDeg] = useState(initial.curvAngleDeg);
  const [useReach, setUseReach] = useState(initial.useReach);
  const [reachTol, setReachTol] = useState(initial.reachTol);
  const [maxPoints, setMaxPoints] = useState(initial.maxPoints);
  const [partOpacity, setPartOpacity] = useState(initial.partOpacity);
  const [strokeWidth, setStrokeWidth] = useState(initial.strokeWidth);
  const [showCircles, setShowCircles] = useState(initial.showCircles);
  const [showPoints, setShowPoints] = useState(initial.showPoints);
  const [showTriangles, setShowTriangles] = useState(initial.showTriangles);
  const [pointSize, setPointSize] = useState(initial.pointSize);
  const [showNormals, setShowNormals] = useState(initial.showNormals);
  const [normalLength, setNormalLength] = useState(initial.normalLength);
  const [zOffset, setZOffset] = useState(initial.zOffset);

  const stats = useSyncExternalStore(subscribeLichenStats, getLichenStats);

  // Publish while open; persist; clear on unmount.
  useEffect(() => {
    const s: LichenSettings = {
      targetEdgeLength,
      grow,
      curvAngleDeg,
      useReach,
      reachTol,
      maxPoints,
      partOpacity,
      strokeWidth,
      showCircles,
      showPoints,
      showTriangles,
      pointSize,
      showNormals,
      normalLength,
      zOffset,
    };
    setLichenDebug(lichenParams(s));
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
    } catch {
      // no localStorage (tests) — non-persistent is fine
    }
  }, [targetEdgeLength, grow, curvAngleDeg, useReach, reachTol, maxPoints, partOpacity, strokeWidth, showCircles, showPoints, showTriangles, pointSize, showNormals, normalLength, zOffset]);
  useEffect(() => () => setLichenDebug(null), []);

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
      className="fd-mesher-dialog fd-mesher-lichen-dialog"
      role="dialog"
      aria-label="Mesher Lichen"
      data-testid="mesher-lichen-dialog"
      style={pos ? { left: pos.x, top: pos.y, right: "auto" } : undefined}
    >
      <div
        className="dialog-header"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <h2 className="dialog-title">Mesher — Lichen</h2>
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
            "The Lichen growth spacing scales off it."
          }
        >
          h (mm)
        </label>
        <div className="fields">
          <NumberInput
            min={0.1}
            max={1e3}
            step={0.5}
            aria-label="h (mm)"
            value={targetEdgeLength}
            onChange={setTargetEdgeLength}
            // Re-weaves on change → commit on blur/Enter so a half-typed "0,"
            // can't clamp to the min and hang on a fine mesh.
            commitOnChange={false}
          />
        </div>
      </div>

      <div className="vec-row">
        <label
          title={
            "Waben-Wachstum: grow the particle set outward from a seed instead " +
            "of seeding the whole band at once."
          }
        >
          Grow
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="Grow"
              data-testid="mesher-lichen-grow"
              checked={grow}
              onChange={(e) => setGrow(e.currentTarget.checked)}
            />
          </label>
        </div>
      </div>

      <div className="vec-row">
        <label
          title={
            "Max. Normalen-Knickwinkel pro Kante in Grad — kleiner = dichter, wo " +
            "die Fläche/Kante krümmt. Greift erst, wenn der Krümmungsradius " +
            "kleiner als h/α wird; auf leicht gekrümmten Flächen wirkungslos."
          }
        >
          Krümmungswinkel (°)
        </label>
        <div className="fields">
          <NumberInput
            min={0}
            max={90}
            step={1}
            aria-label="Krümmungswinkel in Grad"
            value={curvAngleDeg}
            onChange={setCurvAngleDeg}
            commitOnChange={false}
          />
        </div>
      </div>

      <div className="vec-row">
        <label
          title={
            "Bound the growth radius by the local reach (feature size) so the " +
            "particles do not jump across thin gaps."
          }
        >
          Use reach
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="Use reach"
              data-testid="mesher-lichen-use-reach"
              checked={useReach}
              onChange={(e) => setUseReach(e.currentTarget.checked)}
            />
          </label>
        </div>
      </div>

      <div className="vec-row">
        <label title="Footpoint departure threshold (mm): how far the outward footpoint may drift before the reach caps U(p).">
          Reach tol (mm)
        </label>
        <div className="fields">
          <NumberInput
            min={0.001}
            max={1e6}
            step={0.25}
            aria-label="Reach tol"
            value={reachTol}
            onChange={setReachTol}
            commitOnChange={false}
          />
        </div>
      </div>

      <div className="vec-row">
        <label title="Hard cap on the particle count — a safety valve against runaway growth.">
          Max points
        </label>
        <div className="fields">
          <NumberInput
            min={1}
            max={2_000_000}
            step={1000}
            integer
            aria-label="Max points"
            value={maxPoints}
            onChange={setMaxPoints}
            commitOnChange={false}
          />
        </div>
      </div>

      {numRowLikeSlider(
        "Part opacity",
        partOpacity,
        setPartOpacity,
        "Fade the solid so the particles and wireframe read clearly.",
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

      <div className="vec-row">
        <label
          title={
            "Lift the whole overlay this many mm TOWARD the camera so the " +
            "points/circles/wireframe float in front of the surface instead of " +
            "z-fighting with it. A real geometric shift — the gap grows as you " +
            "zoom in (unlike a depth-buffer bias, which does nothing here)."
          }
        >
          Z-Offset (mm)
        </label>
        <div className="fields">
          <NumberInput
            min={0}
            max={1000}
            step={0.5}
            aria-label="Z-Offset"
            value={zOffset}
            onChange={setZOffset}
            // Re-meshes on change (shared overlay effect) → commit on blur/Enter
            // so scrubbing the value can't re-weave a fine mesh per keystroke.
            commitOnChange={false}
          />
        </div>
      </div>

      <div className="vec-row">
        <label title="Draw the point markers, colour-coded: red corner, orange edge chain, green/blue/red surface particle.">
          Points
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="Points"
              data-testid="mesher-lichen-points-toggle"
              checked={showPoints}
              onChange={(e) => setShowPoints(e.currentTarget.checked)}
            />
          </label>
        </div>
      </div>

      <div className="vec-row">
        <label title="On-screen edge length (px) of the point-marker boxes — zoom-independent.">
          Point size (px)
        </label>
        <div className="fields">
          <NumberInput
            min={0.5}
            max={100}
            step={0.5}
            aria-label="Point size"
            value={pointSize}
            onChange={setPointSize}
          />
        </div>
      </div>

      <div className="vec-row">
        <label title="Draw the triangle mesh wireframe (once the triangulation stage produces triangles).">
          Triangles
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="Triangles"
              data-testid="mesher-lichen-triangles-toggle"
              checked={showTriangles}
              onChange={(e) => setShowTriangles(e.currentTarget.checked)}
            />
          </label>
        </div>
      </div>

      <div className="vec-row">
        <label title="Draw the U(p) reach circle (radius = local U) in the tangent plane at each point.">
          U(p) circles
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="U(p) circles"
              data-testid="mesher-lichen-circles"
              checked={showCircles}
              onChange={(e) => setShowCircles(e.currentTarget.checked)}
            />
          </label>
        </div>
      </div>

      <div className="vec-row">
        <label title="Draw a needle along each point's ∇f normal to inspect its orientation.">
          Normals
        </label>
        <div className="fields">
          <label className="checkbox">
            <input
              type="checkbox"
              aria-label="Normals"
              data-testid="mesher-lichen-normals"
              checked={showNormals}
              onChange={(e) => setShowNormals(e.currentTarget.checked)}
            />
          </label>
        </div>
      </div>

      <div className="vec-row">
        <label title="Length (mm) of the normal needles.">Normal length</label>
        <div className="fields">
          <NumberInput
            min={0}
            max={1000}
            step={0.5}
            aria-label="Normal length"
            value={normalLength}
            onChange={setNormalLength}
          />
        </div>
      </div>

      <div className="readout">
        <span className="k">Points</span>
        <span className="v" data-testid="mesher-lichen-points">
          {stats ? stats.cells.toLocaleString() + (stats.capped ? " (capped)" : "") : "—"}
        </span>
      </div>
      <div className="readout">
        <span className="k">Triangles</span>
        <span className="v" data-testid="mesher-lichen-tris">
          {stats ? stats.tris.toLocaleString() : "—"}
        </span>
      </div>
      <div className="readout">
        <span className="k">Fallback tris</span>
        <span className="v" data-testid="mesher-lichen-fallback-tris">
          {stats ? (stats.fallbackTris ?? 0).toLocaleString() : "—"}
        </span>
      </div>

      <div className="legend">
        <span className="seed">seed</span>
        <span className="corner">corner (pinned)</span>
        <span className="chain">edge chain (fixed)</span>
        <span className="particle">surface particle</span>
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
