import { describe, expect, it } from "vitest";
import {
  CONTROLS_PRESETS,
  CONTROLS_PRESET_OPTIONS,
  isAdditiveClick,
  isControlsPresetId,
  toOrbitMouseButtons,
  viewShortcutRow,
  type ControlsPreset,
} from "./controlsPreset";

const PRESETS = Object.values(CONTROLS_PRESETS) as ControlsPreset[];

describe("controls preset registry (docs/UX-PRESETS.md)", () => {
  it("covers all six programs from the doc", () => {
    expect(Object.keys(CONTROLS_PRESETS).sort()).toEqual([
      "fielddraft",
      "freecad",
      "fusion",
      "onshape",
      "threejs",
      "tinkercad",
    ]);
    expect(CONTROLS_PRESET_OPTIONS.map((o) => o.key)).toEqual(
      Object.keys(CONTROLS_PRESETS),
    );
  });

  it("ids are self-consistent and validate via isControlsPresetId", () => {
    for (const p of PRESETS) {
      expect(CONTROLS_PRESETS[p.id]).toBe(p);
      expect(isControlsPresetId(p.id)).toBe(true);
    }
    expect(isControlsPresetId("blender")).toBe(false);
    expect(isControlsPresetId(null)).toBe(false);
  });

  it("classic CAD presets free the left mouse button for selection", () => {
    for (const id of ["onshape", "freecad", "fusion", "tinkercad"] as const) {
      expect(CONTROLS_PRESETS[id].camera.mouseButtons.left).toBeNull();
    }
    // FieldDraft and the Three.js editor keep LMB orbit.
    expect(CONTROLS_PRESETS.fielddraft.camera.mouseButtons.left).toBe("rotate");
    expect(CONTROLS_PRESETS.threejs.camera.mouseButtons.left).toBe("rotate");
  });

  it("matches the doc's orbit/pan table", () => {
    expect(CONTROLS_PRESETS.onshape.camera.mouseButtons).toEqual({
      left: null,
      middle: "pan",
      right: "rotate",
    });
    expect(CONTROLS_PRESETS.freecad.camera.mouseButtons.middle).toBe("rotate");
    expect(CONTROLS_PRESETS.tinkercad.camera.mouseButtons).toEqual({
      left: null,
      middle: "pan",
      right: "rotate",
    });
    // Fusion: MMB pans, orbit only while Shift is held.
    expect(CONTROLS_PRESETS.fusion.camera.mouseButtons.middle).toBe("pan");
    expect(CONTROLS_PRESETS.fusion.camera.shiftMiddleOrbit).toBe(true);
  });

  it("object-mode presets skip sub-element picking and escalation", () => {
    for (const p of PRESETS) {
      if (p.selection.mode === "object") {
        expect(p.selection.bodyEscalation).toBe(false);
      }
    }
    expect(CONTROLS_PRESETS.threejs.selection.mode).toBe("object");
    expect(CONTROLS_PRESETS.tinkercad.selection.mode).toBe("object");
    expect(CONTROLS_PRESETS.fielddraft.selection.mode).toBe("element");
  });

  it("every preset binds at least the iso view key", () => {
    for (const p of PRESETS) {
      expect(Object.values(p.keys.views)).toContain("iso");
    }
    // FreeCAD binds all six standard views.
    expect(Object.keys(CONTROLS_PRESETS.freecad.keys.views)).toHaveLength(7);
  });
});

describe("toOrbitMouseButtons", () => {
  it("maps actions to THREE.MOUSE codes and frees unbound buttons", () => {
    expect(toOrbitMouseButtons(CONTROLS_PRESETS.fielddraft.camera)).toEqual({
      LEFT: 0, // ROTATE
      MIDDLE: 1, // DOLLY
      RIGHT: 2, // PAN
    });
    expect(toOrbitMouseButtons(CONTROLS_PRESETS.onshape.camera)).toEqual({
      LEFT: undefined,
      MIDDLE: 2,
      RIGHT: 0,
    });
  });
});

describe("isAdditiveClick", () => {
  const ev = (shift = false, ctrl = false, meta = false) => ({
    shiftKey: shift,
    ctrlKey: ctrl,
    metaKey: meta,
  });

  it("honours each preset's modifier list", () => {
    const fd = CONTROLS_PRESETS.fielddraft.selection;
    expect(isAdditiveClick(fd, ev(true))).toBe(true);
    expect(isAdditiveClick(fd, ev(false, true))).toBe(true);
    expect(isAdditiveClick(fd, ev(false, false, true))).toBe(true);
    expect(isAdditiveClick(fd, ev())).toBe(false);

    // FreeCAD/Fusion: Ctrl (or Cmd) only — Shift is not additive.
    const fc = CONTROLS_PRESETS.freecad.selection;
    expect(isAdditiveClick(fc, ev(true))).toBe(false);
    expect(isAdditiveClick(fc, ev(false, true))).toBe(true);

    // TinkerCAD: Shift only.
    const tc = CONTROLS_PRESETS.tinkercad.selection;
    expect(isAdditiveClick(tc, ev(true))).toBe(true);
    expect(isAdditiveClick(tc, ev(false, true))).toBe(false);
  });
});

describe("viewShortcutRow", () => {
  it("renders the default and FreeCAD view-key rows", () => {
    expect(viewShortcutRow(CONTROLS_PRESETS.fielddraft)).toEqual({
      keys: "1 / 2 / 3 / 0",
      action: "Front / Top / Right / Isometric view",
    });
    expect(viewShortcutRow(CONTROLS_PRESETS.freecad).keys).toBe(
      "1 / 2 / 3 / 4 / 5 / 6 / 0",
    );
  });
});
