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

const SETTINGS_KEY = "fielddraft.debug.mesher";
const DEFAULTS: MesherDebug = {
  maxDepth: 6,
  bandGuard: 1,
  normalScale: 0.6,
  pointSize: 5,
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
      pointSize: clampNum(p.pointSize, 1, 20, DEFAULTS.pointSize),
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
  const [pointSize, setPointSize] = useState(initial.pointSize);

  const stats = useSyncExternalStore(subscribeMesherStats, getMesherStats);

  // Publish while open; persist; clear on unmount.
  useEffect(() => {
    setMesherDebug({ maxDepth, bandGuard, normalScale, pointSize });
    try {
      localStorage.setItem(
        SETTINGS_KEY,
        JSON.stringify({ maxDepth, bandGuard, normalScale, pointSize }),
      );
    } catch {
      // no localStorage (tests) — non-persistent is fine
    }
  }, [maxDepth, bandGuard, normalScale, pointSize]);
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
  ) => (
    <div className="vec-row">
      <label>{label}</label>
      <div className="fields">
        <input
          type="number"
          min={opts.min}
          max={opts.max}
          step={opts.step}
          aria-label={label}
          value={value}
          onChange={(e) => {
            const raw = Number(e.target.value.replace(",", "."));
            if (!Number.isFinite(raw)) return;
            const n = opts.round ? Math.round(raw) : raw;
            set(Math.min(opts.max, Math.max(opts.min, n)));
          }}
        />
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
      {numRow("Band guard", bandGuard, setBandGuard, {
        min: 0.1,
        max: 8,
        step: 0.1,
      })}
      {numRow("Normal length", normalScale, setNormalScale, {
        min: 0,
        max: 4,
        step: 0.1,
      })}
      {numRow("Point size", pointSize, setPointSize, {
        min: 1,
        max: 20,
        step: 1,
        round: true,
      })}

      <div className="readout">
        <span className="k">Leaf cells</span>
        <span className="v" data-testid="mesher-cells">
          {stats ? stats.cells.toLocaleString() + (stats.capped ? " (capped)" : "") : "—"}
        </span>
      </div>

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
