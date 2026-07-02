/**
 * Global keyboard-command router.
 *
 * The App shell registers ONE window keydown listener and forwards every
 * event here. The routing rules live in this module as an ordered pipeline
 * plus a key→command table, each with its context gates:
 *
 *   1. F1/F2 fire ALWAYS — on every tab, even while typing in an input.
 *   2. A focused text surface (form field, Monaco editor) swallows
 *      everything else.
 *   3. Ctrl+S (save) is app-level and valid in any workspace.
 *   4. Everything below edits the active part/assembly model, so it is
 *      gated OFF while a FieldSkript (script) tab is active — a bare `b`
 *      there is text the user is typing, not "add box".
 *   5. Inside a sketch session, Delete and undo/redo act on the SKETCH
 *      only; primitive/gizmo keys are blocked (they would edit invisible
 *      3D state behind the sketch plane); view keys stay live.
 */

import type { KernelStore } from "./store";
import { requestView } from "./viewCommands";
import {
  getActive as getActiveQueryPicker,
  deactivate as deactivateQueryPicker,
} from "./queryPickerBridge";
import { isTextEntryTarget } from "./keyboardTarget";
import { CONTROLS_PRESETS, type ControlsPresetId } from "./controlsPreset";

export interface KeyboardCommandContext {
  store: KernelStore;
  /** Active controls preset — supplies the view keys and the
   *  space-clears-selection idiom (docs/UX-PRESETS.md). */
  controlsPreset: ControlsPresetId;
  /** F1 — toggle the help overlay (owned by the App shell). */
  toggleHelp: () => void;
}

/* ── Single-key command table (no modifiers) ─────────────────── */

interface SingleKeyCommand {
  /** Blocked inside an active sketch session: these add 3D features or flip
   *  gizmo modes, which would happen invisibly behind the sketch plane. */
  notInSketch?: boolean;
  run: (ctx: KeyboardCommandContext) => void;
}

const SINGLE_KEY_COMMANDS: Record<string, SingleKeyCommand> = {
  b: { notInSketch: true, run: ({ store }) => store.addPrimitive("box") },
  s: { notInSketch: true, run: ({ store }) => store.addPrimitive("sphere") },
  c: { notInSketch: true, run: ({ store }) => store.addPrimitive("cylinder") },
  t: { notInSketch: true, run: ({ store }) => store.addPrimitive("torus") },
  h: { notInSketch: true, run: ({ store }) => store.addHole() },
  g: { notInSketch: true, run: ({ store }) => store.setGizmoMode("translate") },
  r: { notInSketch: true, run: ({ store }) => store.setGizmoMode("rotate") },
  // Onshape parity: K toggles mate-connector visibility.
  k: {
    notInSketch: true,
    run: ({ store }) => store.toggleMateConnectorVisibility(),
  },
};

/* ── Ctrl/Cmd command table ──────────────────────────────────── */

const CTRL_KEY_COMMANDS: Record<
  string,
  (ctx: KeyboardCommandContext, e: KeyboardEvent) => void
> = {
  // Ctrl+M = toggle the Mate Connector tool (Onshape parity).
  m: ({ store }, e) => {
    store.toggleMateMode();
    e.preventDefault();
  },
  // Ctrl+A = select all features.
  a: ({ store }, e) => {
    store.selectAll();
    e.preventDefault();
  },
  // Ctrl+D = duplicate selected feature.
  d: ({ store }, e) => {
    const sel = store.getState().selection;
    if (sel.size === 1) {
      const id = sel.values().next().value;
      if (id !== undefined) store.duplicateFeature(id);
      e.preventDefault();
    }
  },
};

/* ── Escape cascade ──────────────────────────────────────────── */

function runEscape(ctx: KeyboardCommandContext, e: KeyboardEvent): void {
  const { store } = ctx;
  const cur = store.getState();
  // An armed query picker owns the viewport clicks — Escape releases it
  // first, before any mode/tool fallbacks.
  if (getActiveQueryPicker()) {
    deactivateQueryPicker();
    e.preventDefault();
    return;
  }
  if (cur.mateMode) {
    store.toggleMateMode();
  } else if (cur.sketchSession.active && cur.sketchSession.selectedTool) {
    // P0.1: in a sketch session, Esc drops the active drawing tool and
    // returns to select mode so geometry can be picked.
    store.selectSketchTool(null);
  } else if (
    cur.sketchSession.active &&
    cur.sketchSession.selection.length > 0
  ) {
    store.clearSketchSelection();
  } else {
    store.clearSelection();
  }
  e.preventDefault();
}

/* ── Router ──────────────────────────────────────────────────── */

/**
 * Route one window keydown through the global command pipeline.
 * Handled commands call `e.preventDefault()` themselves.
 */
export function dispatchKeyboardCommand(
  e: KeyboardEvent,
  ctx: KeyboardCommandContext,
): void {
  const { store } = ctx;

  // F1 toggles the help overlay — always, on every tab, even in inputs.
  if (e.key === "F1") {
    ctx.toggleHelp();
    e.preventDefault();
    return;
  }
  // F2 opens the Report Issue dialog — also global, even in inputs, so it
  // works mid-task. SettingsMenu owns the dialog; bridge via a window event.
  if (e.key === "F2") {
    window.dispatchEvent(new Event("fd:report-issue"));
    e.preventDefault();
    return;
  }
  // Never steal keystrokes from a focused text surface (form fields,
  // and the Monaco script editor).
  if (isTextEntryTarget(e.target)) return;

  const ctrl = e.ctrlKey || e.metaKey;

  // Ctrl+S saves the whole project — an app-level action valid in
  // any workspace.
  if (ctrl && (e.key === "s" || e.key === "S")) {
    store.saveProjectToFile();
    e.preventDefault();
    return;
  }

  // Everything below edits the active part/assembly model, so it is only
  // valid in the modelling workspace — NOT while a FieldSkript (script) tab
  // is open, which has its own editor that owns the keyboard there. The
  // window listener is global, so we gate on the active tab kind rather
  // than on focus heuristics.
  const cur = store.getState();
  const activeTab = cur.project.tabs.find(
    (t) => t.id === cur.project.activeTabId,
  );
  if (activeTab?.kind === "script") return;

  if (e.key === "Delete" || e.key === "Backspace") {
    // In a sketch session Delete acts on the SKETCH selection only — never
    // on 3D features, which would vanish invisibly behind the sketch plane.
    if (store.getState().sketchSession.active) {
      if (store.deleteSketchSelection()) e.preventDefault();
      return;
    }
    if (store.removeSelected()) e.preventDefault();
    return;
  }

  // In a sketch session, undo/redo act on the SKETCH history only — never
  // on 3D features behind the sketch plane (ticket r3ci; mirrors how Delete
  // is scoped to the sketch selection above). Routed here unconditionally
  // so an empty sketch history is a no-op, not a fall-through to the
  // kernel undo.
  const inSketch = store.getState().sketchSession.active;
  if (ctrl && (e.key === "z" || e.key === "Z")) {
    if (inSketch) {
      if (e.shiftKey ? store.sketchRedo() : store.sketchUndo())
        e.preventDefault();
    } else if (e.shiftKey) {
      if (store.redo()) e.preventDefault();
    } else {
      if (store.undo()) e.preventDefault();
    }
    return;
  }
  if (ctrl && (e.key === "y" || e.key === "Y")) {
    if (inSketch ? store.sketchRedo() : store.redo()) e.preventDefault();
    return;
  }

  // Arrow keys nudge the selection along the world axes. Shift = larger
  // step. Alt = move along Z (up the world Y in CAD parlance is screen-up;
  // Z lifts the part). Plain arrows move on the XY plane: Right/Left = X,
  // Up/Down = Y.
  const step = e.shiftKey ? 10 : 1;
  let dx = 0,
    dy = 0,
    dz = 0;
  if (e.key === "ArrowRight") dx = step;
  else if (e.key === "ArrowLeft") dx = -step;
  else if (e.key === "ArrowUp") {
    if (e.altKey) dz = step;
    else dy = step;
  } else if (e.key === "ArrowDown") {
    if (e.altKey) dz = -step;
    else dy = -step;
  }
  if (dx !== 0 || dy !== 0 || dz !== 0) {
    if (store.nudgeSelected(dx, dy, dz)) e.preventDefault();
    return;
  }

  // Single-key shortcuts (no modifier).
  if (!ctrl && !e.shiftKey && !e.altKey) {
    if (e.key === "Escape") {
      runEscape(ctx, e);
      return;
    }
    if (e.key === " ") {
      // Onshape idiom (preset-gated): Space clears the selection.
      if (CONTROLS_PRESETS[ctx.controlsPreset].selection.spaceClears) {
        store.clearSelection();
        e.preventDefault();
      }
      return;
    }
    const command = SINGLE_KEY_COMMANDS[e.key];
    if (command) {
      if (command.notInSketch && inSketch) return;
      command.run(ctx);
      e.preventDefault();
      return;
    }
    // View keys come from the controls preset (docs/UX-PRESETS.md) —
    // e.g. FreeCAD binds 1–6 + 0 to all six standard views. They stay
    // live while sketching: orienting the camera is useful there.
    const view = CONTROLS_PRESETS[ctx.controlsPreset].keys.views[e.key];
    if (view) {
      requestView(view);
      e.preventDefault();
      return;
    }
  }

  if (ctrl) {
    const command = CTRL_KEY_COMMANDS[e.key.toLowerCase()];
    if (command) {
      command(ctx, e);
      return;
    }
  }
}
