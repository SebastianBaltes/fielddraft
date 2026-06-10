/**
 * Controls-preset registry (docs/UX-PRESETS.md).
 *
 * Users bring mouse/keyboard habits from their previous CAD package; one
 * setting (`fielddraft.controlsPreset`) switches the whole control scheme.
 * Each preset bundles three sections:
 *   - `camera`: OrbitControls button mapping + speeds. Data-only — the
 *     string actions are mapped to `THREE.MOUSE` codes by
 *     `toOrbitMouseButtons` so this module stays three-free and testable.
 *   - `selection`: which modifiers multi-select, whether a second click on
 *     the selected face escalates to the body, and whether the preset
 *     selects whole objects instead of faces/edges/vertices.
 *   - `keys`: bare-digit view shortcuts (front/top/… per program).
 *
 * The main conflict axis is the LEFT mouse button: FieldDraft and the
 * Three.js editor orbit with LMB; classic CAD packages keep LMB for
 * selection and orbit with RMB/MMB (`left: null` frees the button).
 */

import type { ViewPreset } from "./viewCommands";

export type ControlsPresetId =
  | "fielddraft"
  | "threejs"
  | "onshape"
  | "freecad"
  | "fusion"
  | "tinkercad";

/** What a mouse button does to the camera. `null` = button is free
 *  (OrbitControls ignores the drag; clicks still select). */
export type MouseAction = "rotate" | "pan" | "dolly" | null;

export interface CameraBindings {
  mouseButtons: { left: MouseAction; middle: MouseAction; right: MouseAction };
  /** Fusion 360 idiom: MMB pans, orbit only while Shift is held. The
   *  camera rig swaps the MIDDLE action between pan and rotate on
   *  Shift down/up. */
  shiftMiddleOrbit?: boolean;
  rotateSpeed: number;
  panSpeed: number;
  zoomSpeed: number;
}

export type SelectionModifier = "shift" | "ctrl" | "meta";

export interface SelectionBindings {
  /** "element": clicks pick faces/edges/vertices (FieldDraft default);
   *  "object": clicks select whole objects, no sub-elements (Three.js
   *  editor, TinkerCAD). */
  mode: "element" | "object";
  /** Modifiers that toggle the clicked element in the multi-select set. */
  additive: SelectionModifier[];
  /** Second plain click on the already-selected face selects the body. */
  bodyEscalation: boolean;
}

export interface KeyBindings {
  /** Bare-key (no modifier) view shortcuts: key → standard view. */
  views: Record<string, Exclude<ViewPreset, null>>;
}

export interface ControlsPreset {
  id: ControlsPresetId;
  label: string;
  camera: CameraBindings;
  selection: SelectionBindings;
  keys: KeyBindings;
}

const DEFAULT_VIEW_KEYS: KeyBindings["views"] = {
  "1": "front",
  "2": "top",
  "3": "right",
  "0": "iso",
};

export const CONTROLS_PRESETS: Record<ControlsPresetId, ControlsPreset> = {
  fielddraft: {
    id: "fielddraft",
    label: "FieldDraft",
    camera: {
      mouseButtons: { left: "rotate", middle: "dolly", right: "pan" },
      rotateSpeed: 1.4,
      panSpeed: 1.2,
      zoomSpeed: 1.6,
    },
    selection: {
      mode: "element",
      additive: ["shift", "ctrl", "meta"],
      bodyEscalation: true,
    },
    keys: { views: DEFAULT_VIEW_KEYS },
  },
  threejs: {
    id: "threejs",
    label: "Three.js Editor",
    camera: {
      mouseButtons: { left: "rotate", middle: "dolly", right: "pan" },
      rotateSpeed: 1.4,
      panSpeed: 1.2,
      zoomSpeed: 1.6,
    },
    // The editor selects whole objects — no face/edge/vertex picking.
    selection: {
      mode: "object",
      additive: ["shift", "ctrl", "meta"],
      bodyEscalation: false,
    },
    keys: { views: DEFAULT_VIEW_KEYS },
  },
  onshape: {
    id: "onshape",
    label: "Onshape",
    camera: {
      mouseButtons: { left: null, middle: "pan", right: "rotate" },
      rotateSpeed: 1.4,
      panSpeed: 1.2,
      zoomSpeed: 1.6,
    },
    selection: {
      mode: "element",
      additive: ["shift", "ctrl", "meta"],
      bodyEscalation: true,
    },
    keys: { views: DEFAULT_VIEW_KEYS },
  },
  freecad: {
    id: "freecad",
    label: "FreeCAD",
    camera: {
      // CAD navigation orbits with MMB; the MMB+LMB pan chord has no
      // OrbitControls equivalent, so pan approximates onto RMB.
      mouseButtons: { left: null, middle: "rotate", right: "pan" },
      rotateSpeed: 1.4,
      panSpeed: 1.2,
      zoomSpeed: 1.6,
    },
    selection: {
      mode: "element",
      additive: ["ctrl", "meta"],
      bodyEscalation: true,
    },
    keys: {
      views: {
        "1": "front",
        "2": "top",
        "3": "right",
        "4": "back",
        "5": "bottom",
        "6": "left",
        "0": "iso",
      },
    },
  },
  fusion: {
    id: "fusion",
    label: "Fusion 360",
    camera: {
      mouseButtons: { left: null, middle: "pan", right: null },
      shiftMiddleOrbit: true,
      rotateSpeed: 1.4,
      panSpeed: 1.2,
      zoomSpeed: 1.6,
    },
    selection: {
      mode: "element",
      additive: ["ctrl", "meta"],
      bodyEscalation: true,
    },
    keys: { views: DEFAULT_VIEW_KEYS },
  },
  tinkercad: {
    id: "tinkercad",
    label: "TinkerCAD",
    camera: {
      mouseButtons: { left: null, middle: "pan", right: "rotate" },
      rotateSpeed: 1.4,
      panSpeed: 1.2,
      zoomSpeed: 1.6,
    },
    selection: {
      mode: "object",
      additive: ["shift"],
      bodyEscalation: false,
    },
    keys: { views: DEFAULT_VIEW_KEYS },
  },
};

export const CONTROLS_PRESET_OPTIONS: { key: ControlsPresetId; label: string }[] =
  (Object.values(CONTROLS_PRESETS) as ControlsPreset[]).map((p) => ({
    key: p.id,
    label: p.label,
  }));

export const DEFAULT_CONTROLS_PRESET: ControlsPresetId = "fielddraft";

export function isControlsPresetId(val: unknown): val is ControlsPresetId {
  return typeof val === "string" && val in CONTROLS_PRESETS;
}

export function getControlsPreset(id: ControlsPresetId): ControlsPreset {
  return CONTROLS_PRESETS[id];
}

/* ── Camera mapping ─────────────────────────────────────────── */

/** `THREE.MOUSE` codes — duplicated as plain numbers so this data module
 *  needs no three import. Frozen three.js API: ROTATE=0, DOLLY=1, PAN=2. */
export const ORBIT_ACTION_CODE: Record<Exclude<MouseAction, null>, number> = {
  rotate: 0,
  dolly: 1,
  pan: 2,
};

export interface OrbitMouseButtons {
  LEFT?: number;
  MIDDLE?: number;
  RIGHT?: number;
}

/** Map a preset's button bindings to the `mouseButtons` object drei's
 *  OrbitControls expects. Unbound buttons stay `undefined` → the controls
 *  ignore the drag and the click falls through to selection. */
export function toOrbitMouseButtons(camera: CameraBindings): OrbitMouseButtons {
  const code = (a: MouseAction) => (a === null ? undefined : ORBIT_ACTION_CODE[a]);
  return {
    LEFT: code(camera.mouseButtons.left),
    MIDDLE: code(camera.mouseButtons.middle),
    RIGHT: code(camera.mouseButtons.right),
  };
}

/* ── Selection mapping ──────────────────────────────────────── */

/** Does this click event carry one of the preset's multi-select modifiers? */
export function isAdditiveClick(
  selection: SelectionBindings,
  ev: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean },
): boolean {
  return selection.additive.some(
    (m) =>
      (m === "shift" && ev.shiftKey) ||
      (m === "ctrl" && ev.ctrlKey) ||
      (m === "meta" && ev.metaKey),
  );
}

/* ── Shortcut display ───────────────────────────────────────── */

const VIEW_LABEL: Record<Exclude<ViewPreset, null>, string> = {
  front: "Front",
  back: "Back",
  top: "Top",
  bottom: "Bottom",
  right: "Right",
  left: "Left",
  iso: "Isometric",
};

/** Canonical display order — `Object.entries` would sort digit keys
 *  numerically and put the iso "0" first. */
const VIEW_ORDER: Exclude<ViewPreset, null>[] = [
  "front", "top", "right", "back", "bottom", "left", "iso",
];

/** One shortcut-table row describing the preset's view keys,
 *  e.g. `{ keys: "1 / 2 / 3 / 0", action: "Front / Top / Right / Isometric view" }`. */
export function viewShortcutRow(preset: ControlsPreset): { keys: string; action: string } {
  const views = preset.keys.views;
  const entries = VIEW_ORDER.flatMap((v) => {
    const key = Object.keys(views).find((k) => views[k] === v);
    return key ? [[key, v] as const] : [];
  });
  return {
    keys: entries.map(([k]) => k).join(" / "),
    action: entries.map(([, v]) => VIEW_LABEL[v]).join(" / ") + " view",
  };
}
