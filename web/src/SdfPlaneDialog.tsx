/**
 * Debug ▸ SDF-Plane dialog.
 *
 * A non-modal, draggable window that defines a plane in space (a point + a
 * normal + a half-extent). While it is open the plane is published to
 * `sdfPlaneStore` and the viewport draws the scene SDF sampled on it (see
 * SdfPlaneOverlay) — a diagnostic window into the exact distance field the
 * meshers contour. Closing it clears the plane.
 */

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { setSdfPlane } from "./sdfPlaneStore";

type Vec3 = [number, number, number];

const AXES: { key: "x" | "y" | "z"; i: number }[] = [
  { key: "x", i: 0 },
  { key: "y", i: 1 },
  { key: "z", i: 2 },
];

export function SdfPlaneDialog({ onClose }: { onClose: () => void }) {
  const [point, setPoint] = useState<Vec3>([0, 0, 0]);
  const [normal, setNormal] = useState<Vec3>([0, 0, 1]);
  const [halfExtent, setHalfExtent] = useState(50);

  // Publish the plane while open; clear it on unmount (dialog closed).
  useEffect(() => {
    setSdfPlane({ point, normal, halfExtent });
  }, [point, normal, halfExtent]);
  useEffect(() => () => setSdfPlane(null), []);

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

  const setAxis = (
    set: (v: Vec3) => void,
    v: Vec3,
    i: number,
    raw: string,
  ) => {
    const n = Number(raw.replace(",", "."));
    if (!Number.isFinite(n)) return;
    const next: Vec3 = [...v];
    next[i] = n;
    set(next);
  };

  // ── Drag the header to reposition (non-modal window) ──────────────
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

  const presetNormal = (v: Vec3) => setNormal(v);

  return (
    <div
      className="fd-sdf-plane-dialog"
      role="dialog"
      aria-label="SDF Plane"
      data-testid="sdf-plane-dialog"
      style={pos ? { left: pos.x, top: pos.y, right: "auto" } : undefined}
    >
      <div
        className="dialog-header"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <h2 className="dialog-title">SDF Plane</h2>
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
        <label>Point</label>
        <div className="fields">
          {AXES.map(({ key, i }) => (
            <input
              key={key}
              type="number"
              step={1}
              aria-label={`point ${key}`}
              value={point[i]}
              onChange={(e) => setAxis(setPoint, point, i, e.target.value)}
            />
          ))}
        </div>
      </div>

      <div className="vec-row">
        <label>Normal</label>
        <div className="fields">
          {AXES.map(({ key, i }) => (
            <input
              key={key}
              type="number"
              step={0.1}
              aria-label={`normal ${key}`}
              value={normal[i]}
              onChange={(e) => setAxis(setNormal, normal, i, e.target.value)}
            />
          ))}
        </div>
      </div>

      <div className="preset-row">
        <button type="button" onClick={() => presetNormal([1, 0, 0])}>
          X
        </button>
        <button type="button" onClick={() => presetNormal([0, 1, 0])}>
          Y
        </button>
        <button type="button" onClick={() => presetNormal([0, 0, 1])}>
          Z
        </button>
      </div>

      <div className="vec-row">
        <label>Half size (mm)</label>
        <div className="fields">
          <input
            type="number"
            min={1}
            step={5}
            aria-label="half extent"
            value={halfExtent}
            onChange={(e) => {
              const n = Number(e.target.value.replace(",", "."));
              if (Number.isFinite(n) && n > 0) setHalfExtent(n);
            }}
          />
        </div>
      </div>

      <div className="legend">
        <span className="in">inside</span>
        <span className="zero">surface</span>
        <span className="out">outside</span>
        <span className="hint">iso-bands every {(halfExtent / 8).toFixed(2)} mm</span>
      </div>
    </div>
  );
}
