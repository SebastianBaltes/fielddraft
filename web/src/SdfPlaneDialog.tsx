/**
 * Debug ▸ SDF-Plane dialog.
 *
 * A non-modal, draggable window that defines a plane in space (a point + a
 * normal + a half-extent) plus the colour-coding controls (sampling resolution
 * and iso-band count). While it is open the plane is published to
 * `sdfPlaneStore` and the viewport draws the scene SDF sampled on it (see
 * SdfPlaneOverlay) — a diagnostic window into the exact distance field the
 * meshers contour. The cursor's projection onto the plane reports the numeric
 * SDF value there. Closing it clears the plane.
 */

import {
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";
import {
  setSdfPlane,
  subscribeSdfPlaneHover,
  getSdfPlaneHover,
} from "./sdfPlaneStore";
import { NumberInput } from "./NumberInput";
import { useDialogDrag } from "./useDialogDrag";

type Vec3 = [number, number, number];

const AXES: { key: "x" | "y" | "z"; i: number }[] = [
  { key: "x", i: 0 },
  { key: "y", i: 1 },
  { key: "z", i: 2 },
];

// ── Settings persistence (localStorage) ──────────────────────────────
interface SdfPlaneSettings {
  point: Vec3;
  normal: Vec3;
  halfExtent: number;
  resolution: number;
  bands: number;
  step: number;
}

const SETTINGS_KEY = "fielddraft.debug.sdfPlane";
const DEFAULT_SETTINGS: SdfPlaneSettings = {
  point: [0, 0, 0],
  normal: [0, 0, 1],
  halfExtent: 50,
  resolution: 256,
  bands: 16,
  step: 1,
};

const isVec3 = (v: unknown): v is Vec3 =>
  Array.isArray(v) && v.length === 3 && v.every((n) => Number.isFinite(n));
const posOr = (v: unknown, lo: number, hi: number, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) && v > 0
    ? Math.min(hi, Math.max(lo, v))
    : fallback;

/** Load persisted settings, falling back to defaults for anything missing or
 *  malformed (and when localStorage is unavailable, e.g. tests). */
function loadSettings(): SdfPlaneSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return DEFAULT_SETTINGS;
    const p = JSON.parse(raw) as Partial<SdfPlaneSettings>;
    return {
      point: isVec3(p.point) ? p.point : DEFAULT_SETTINGS.point,
      normal: isVec3(p.normal) ? p.normal : DEFAULT_SETTINGS.normal,
      halfExtent: posOr(p.halfExtent, 1e-3, 1e6, DEFAULT_SETTINGS.halfExtent),
      resolution: Math.round(posOr(p.resolution, 16, 1024, DEFAULT_SETTINGS.resolution)),
      bands: Math.round(posOr(p.bands, 1, 256, DEFAULT_SETTINGS.bands)),
      step: posOr(p.step, 1e-6, 1e6, DEFAULT_SETTINGS.step),
    };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export function SdfPlaneDialog({ onClose }: { onClose: () => void }) {
  // Restore persisted settings once on mount.
  const initial = useMemo(loadSettings, []);
  const [point, setPoint] = useState<Vec3>(initial.point);
  const [normal, setNormal] = useState<Vec3>(initial.normal);
  const [halfExtent, setHalfExtent] = useState(initial.halfExtent);
  const [resolution, setResolution] = useState(initial.resolution);
  const [bands, setBands] = useState(initial.bands);
  // Spinner/arrow-key increment for the Point & Normal xyz fields (settable so
  // the plane can be scanned through the model in coarse or fine steps).
  const [step, setStep] = useState(initial.step);

  // Persist on any change so the settings survive close/reopen and reload.
  useEffect(() => {
    try {
      localStorage.setItem(
        SETTINGS_KEY,
        JSON.stringify({ point, normal, halfExtent, resolution, bands, step }),
      );
    } catch {
      // no localStorage (tests) — non-persistent is fine
    }
  }, [point, normal, halfExtent, resolution, bands, step]);

  const hover = useSyncExternalStore(subscribeSdfPlaneHover, getSdfPlaneHover);

  // Publish the plane while open; clear it on unmount (dialog closed).
  useEffect(() => {
    setSdfPlane({ point, normal, halfExtent, resolution, bands });
  }, [point, normal, halfExtent, resolution, bands]);
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

  const setAxis = (set: (v: Vec3) => void, v: Vec3, i: number, n: number) => {
    const next: Vec3 = [...v];
    next[i] = n;
    set(next);
  };

  // ── Drag the header to reposition (non-modal window) ──────────────
  const { positionStyle, onHeaderDown, onHeaderMove, onHeaderUp } = useDialogDrag();

  const presetNormal = (v: Vec3) => setNormal(v);
  const bandSpacing = halfExtent / Math.max(1, bands);

  return (
    <div
      className="fd-sdf-plane-dialog"
      role="dialog"
      aria-label="SDF Plane"
      data-testid="sdf-plane-dialog"
      style={positionStyle}
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

      {/* Point / Normal — label-less xyz triples (the fields speak for
          themselves; the X/Y/Z presets sit under the normal). */}
      <div className="vec-row">
        <div className="fields">
          {AXES.map(({ key, i }) => (
            <NumberInput
              key={key}
              step={step}
              aria-label={`point ${key}`}
              value={point[i]}
              onChange={(n) => setAxis(setPoint, point, i, n)}
            />
          ))}
        </div>
      </div>

      <div className="vec-row">
        <div className="fields">
          {AXES.map(({ key, i }) => (
            <NumberInput
              key={key}
              step={step}
              aria-label={`normal ${key}`}
              value={normal[i]}
              onChange={(n) => setAxis(setNormal, normal, i, n)}
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
        <label>Step</label>
        <div className="fields">
          <NumberInput
            step={0.1}
            aria-label="step"
            value={step}
            onChange={(n) => {
              if (n > 0) setStep(n);
            }}
          />
        </div>
      </div>

      <div className="vec-row">
        <label>Half size (mm)</label>
        <div className="fields">
          <NumberInput
            step={5}
            aria-label="half extent"
            value={halfExtent}
            onChange={(n) => {
              if (n > 0) setHalfExtent(n);
            }}
          />
        </div>
      </div>

      <div className="vec-row">
        <label>Resolution</label>
        <div className="fields">
          <NumberInput
            integer
            min={16}
            max={1024}
            step={16}
            aria-label="resolution"
            value={resolution}
            onChange={setResolution}
          />
        </div>
      </div>

      <div className="vec-row">
        <label>Iso-bands</label>
        <div className="fields">
          <NumberInput
            integer
            min={1}
            max={256}
            step={1}
            aria-label="iso-bands"
            value={bands}
            onChange={setBands}
          />
        </div>
      </div>

      <div className="readout">
        <span className="k">SDF @ cursor</span>
        <span className="v" data-testid="sdf-plane-hover">
          {hover ? `${hover.value.toFixed(3)} mm` : "—"}
        </span>
      </div>

      <div className="legend">
        <span className="in">inside</span>
        <span className="zero">surface</span>
        <span className="out">outside</span>
        <span className="hint">iso-bands every {bandSpacing.toFixed(2)} mm</span>
      </div>
    </div>
  );
}
