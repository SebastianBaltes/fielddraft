import { StrictMode, useEffect, useState } from "react";
import { Outliner } from "./Outliner";
import { Toolbar } from "./Toolbar";
import { Viewport } from "./Viewport";
import { FieldSkriptEditor } from "./FieldSkriptEditor";
import { BottomTabs } from "./BottomTabs";
import { useKernelStore } from "./store";
import { requestView } from "./viewCommands";
import {
  getActive as getActiveQueryPicker,
  deactivate as deactivateQueryPicker,
} from "./queryPickerBridge";
import { isTextEntryTarget } from "./keyboardTarget";
import { SettingsMenu, loadSettings, WorkspaceUnitsContext } from "./SettingsMenu";
import type { SettingsState } from "./SettingsMenu";
import { CONTROLS_PRESETS, viewShortcutRow } from "./controlsPreset";

export function App() {
  const { store, state, error } = useKernelStore();
  const [showHelp, setShowHelp] = useState(false);
  // Mobile-only: the outliner is an off-canvas drawer toggled from the
  // topbar. On desktop the toggle/scrim are hidden by CSS and this stays
  // a no-op.
  const [mobileNavOpen, setMobileNavOpen] = useState(false);
  const [settings, setSettings] = useState<SettingsState>(() => loadSettings());

  const compact = settings.compact;

  useEffect(() => {
    document.documentElement.dataset.theme = settings.theme;
  }, [settings.theme]);

  const isMeshMode = settings.renderMode === "mesh";
  // The WebGL fallback (render mode "webgl") draws the kernel's Dual-Contouring
  // meshes so booleans are visible — those only exist while DC meshing is on, so
  // enable it for the WebGL path too (ticket gcvz).
  const webGLFallbackActive = settings.renderMode === "webgl";
  useEffect(() => {
    if (store)
      store.setDcMeshingEnabled(isMeshMode || webGLFallbackActive, settings.meshAlgorithm);
  }, [store, isMeshMode, webGLFallbackActive, settings.meshAlgorithm]);

  useEffect(() => {
    if (store) store.setParametricWeave(settings.parametricWeave);
  }, [store, settings.parametricWeave]);

  useEffect(() => {
    if (store) store.setDragBrepRecompute(settings.dragBrepRecompute);
  }, [store, settings.dragBrepRecompute]);

  useEffect(() => {
    if (store) store.setLiveExtrudeDrag(settings.liveExtrudeDrag);
  }, [store, settings.liveExtrudeDrag]);

  // Global keyboard shortcuts: Del = remove selection, Ctrl+Z = undo,
  // Ctrl+Shift+Z (or Ctrl+Y) = redo. Skipped if focus is in an input.
  useEffect(() => {
    if (!store) return;
    const onKey = (e: KeyboardEvent) => {
      // F1 toggles help overlay — always, on every tab, even in inputs.
      if (e.key === "F1") {
        setShowHelp((v) => !v);
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

      // Everything below edits the active part/assembly model, so it is
      // only valid in the modelling workspace — NOT while a FieldSkript
      // (script) tab is open, which has its own editor that owns the
      // keyboard there. The window listener is global, so we gate on the
      // active tab kind rather than on focus heuristics: a bare `b` on
      // the script tab is text the user is typing, not "add box".
      const cur = store.getState();
      const activeTab = cur.project.tabs.find(
        (t) => t.id === cur.project.activeTabId,
      );
      if (activeTab?.kind === "script") return;

      if (e.key === "Delete" || e.key === "Backspace") {
        // In a sketch session Delete acts on the SKETCH selection only —
        // never on 3D features, which would vanish invisibly behind the
        // sketch plane.
        if (store.getState().sketchSession.active) {
          if (store.deleteSketchSelection()) e.preventDefault();
          return;
        }
        if (store.removeSelected()) e.preventDefault();
        return;
      }
      // In a sketch session, undo/redo act on the SKETCH history only —
      // never on 3D features behind the sketch plane (ticket r3ci; mirrors
      // how Delete is scoped to the sketch selection above). Routed here
      // unconditionally so an empty sketch history is a no-op, not a fall-
      // through to the kernel undo.
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

      // Arrow keys nudge the selection along the world axes. Shift =
      // larger step. Alt = move along Z (up the world Y in CAD parlance
      // is screen-up; Z lifts the part). Plain arrows move on the XY
      // plane: Right/Left = X, Up/Down = Y.
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

      // Single-key shortcuts (no modifier). Skipped when
      // focus is in an input (guard at the top of onKey).
      if (!ctrl && !e.shiftKey && !e.altKey) {
        const cur2 = store.getState();
        switch (e.key) {
          case "Escape": {
            const cur = store.getState();
            // An armed query picker owns the viewport clicks — Escape
            // releases it first, before any mode/tool fallbacks.
            if (getActiveQueryPicker()) {
              deactivateQueryPicker();
              e.preventDefault();
              return;
            }
            if (cur.mateMode) {
              store.toggleMateMode();
            } else if (cur.sketchSession.active && cur.sketchSession.selectedTool) {
              // P0.1: in a sketch session, Esc drops the active drawing
              // tool and returns to select mode so geometry can be picked.
              store.selectSketchTool(null);
            } else if (cur.sketchSession.active && cur.sketchSession.selection.length > 0) {
              store.clearSketchSelection();
            } else {
              store.clearSelection();
            }
            e.preventDefault();
            return;
          }
          case " ":
            // Onshape idiom (preset-gated): Space clears the selection.
            if (CONTROLS_PRESETS[settings.controlsPreset].selection.spaceClears) {
              store.clearSelection();
              e.preventDefault();
            }
            return;
          // Primitive/gizmo keys are modelling-workspace shortcuts —
          // inside a sketch session they would silently add 3D features
          // (or flip gizmo modes) behind the sketch. View keys below
          // stay live: orienting the camera is useful while sketching.
          case "b":
            if (cur2.sketchSession.active) return;
            store.addPrimitive("box");
            e.preventDefault();
            return;
          case "s":
            if (cur2.sketchSession.active) return;
            store.addPrimitive("sphere");
            e.preventDefault();
            return;
          case "c":
            if (cur2.sketchSession.active) return;
            store.addPrimitive("cylinder");
            e.preventDefault();
            return;
          case "t":
            if (cur2.sketchSession.active) return;
            store.addPrimitive("torus");
            e.preventDefault();
            return;
          case "h":
            if (cur2.sketchSession.active) return;
            store.addHole();
            e.preventDefault();
            return;
          case "g":
            if (cur2.sketchSession.active) return;
            store.setGizmoMode("translate");
            e.preventDefault();
            return;
          case "r":
            if (cur2.sketchSession.active) return;
            store.setGizmoMode("rotate");
            e.preventDefault();
            return;
        }
        // View keys come from the controls preset (docs/UX-PRESETS.md) —
        // e.g. FreeCAD binds 1–6 + 0 to all six standard views.
        const view = CONTROLS_PRESETS[settings.controlsPreset].keys.views[e.key];
        if (view) {
          requestView(view);
          e.preventDefault();
          return;
        }
      }

      // Ctrl+A = select all features.
      if (ctrl && (e.key === "a" || e.key === "A")) {
        store.selectAll();
        e.preventDefault();
        return;
      }

      // Ctrl+D = duplicate selected feature.
      if (ctrl && (e.key === "d" || e.key === "D")) {
        const sel = store.getState().selection;
        if (sel.size === 1) {
          const id = sel.values().next().value;
          if (id !== undefined) store.duplicateFeature(id);
          e.preventDefault();
        }
        return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store, settings.controlsPreset]);

  const activeProjectTab = state?.project.tabs.find(
    (t) => t.id === state.project.activeTabId,
  );
  const activeScriptTab =
    activeProjectTab?.kind === "script" ? activeProjectTab : null;

  return (
    <WorkspaceUnitsContext.Provider value={settings.units}>
    <div className={"fd-shell" + (compact ? " fd-compact" : "")}>
      {/* StrictMode wraps the UI chrome but NOT the 3D <Viewport> below — its
          dev double-mount breaks the R3F canvas (blank-until-resize) and
          double-inits the renderer. See main.tsx. */}
      <StrictMode>
      <header className="topbar">
        <button
          type="button"
          className="nav-toggle"
          data-testid="mobile-nav-toggle"
          aria-label={mobileNavOpen ? "Close panel" : "Open panel"}
          aria-expanded={mobileNavOpen}
          onClick={() => setMobileNavOpen((v) => !v)}
        >
          ☰
        </button>
        <div className="title">
          FieldDraft <span className="version">v{__APP_VERSION__}</span>
          <SettingsMenu
            store={store}
            state={state}
            settings={settings}
            onSettingsChange={setSettings}
          />
        </div>
        <div className="meta">
          {error ? (
            <span className="error">kernel error: {error.message}</span>
          ) : state ? (
            <>
              <span data-testid="feature-count">
                {state.features.length} feature{state.features.length === 1 ? "" : "s"}
              </span>
              {isMeshMode && state.meshedBodies.length > 0 ? (
                <span className="tri-count" data-testid="tri-count">
                  {state.meshedBodies
                    .reduce((sum, b) => sum + b.indices.length / 3, 0)
                    .toLocaleString()}{" "}
                  △
                </span>
              ) : null}
              {state.sceneSize[0] > 0 || state.sceneSize[1] > 0 || state.sceneSize[2] > 0 ? (
                <span className="scene-size" data-testid="scene-size">
                  {state.sceneSize[0].toFixed(1)} × {state.sceneSize[1].toFixed(1)} × {state.sceneSize[2].toFixed(1)} mm
                </span>
              ) : null}
              {state.transientError ? (
                <span
                  className="transient-error"
                  data-testid="transient-error"
                  role="alert"
                >
                  ⚠ {state.transientError}
                </span>
              ) : null}
            </>
          ) : (
            <span>loading…</span>
          )}
        </div>
      </header>
      </StrictMode>
      {/* MT-A: both workspaces stay mounted; we toggle visibility with
          the `hidden` attr so the Viewport's GPU context and Monaco are
          never torn down on a tab switch. The active tab is derived from
          the project; part AND assembly both use the part workspace
          (Viewport + Outliner render the assembly when active). */}
      <div className="fd-workspace" hidden={activeProjectTab?.kind === "script"}>
        {store && state ? (
          <StrictMode>
            <Toolbar store={store} state={state} />
          </StrictMode>
        ) : null}
        <main className="main">
          <Viewport renderMode={settings.renderMode} brepEdgeMode={settings.brepEdgeMode} frameLoop={settings.frameLoop} showEdgePoints={settings.showEdgePoints} showOctreeCells={settings.showOctreeCells} controlsPreset={settings.controlsPreset} />
          {mobileNavOpen ? (
            <div
              className="fd-nav-scrim"
              data-testid="mobile-nav-scrim"
              onClick={() => setMobileNavOpen(false)}
            />
          ) : null}
          {store && state ? (
            <StrictMode>
              <Outliner store={store} state={state} mobileNavOpen={mobileNavOpen} />
            </StrictMode>
          ) : null}
        </main>
      </div>
      <StrictMode>
      <div className="fd-workspace fd-script-workspace" hidden={activeProjectTab?.kind !== "script"}>
        {store && state && activeScriptTab ? (
          <FieldSkriptEditor
            store={store}
            tab={activeScriptTab}
            visible={activeProjectTab?.kind === "script"}
          />
        ) : null}
      </div>
      {store && state ? <BottomTabs store={store} state={state} /> : null}
      {state?.loading ? (
        <div className="fd-loading-overlay">
          <div className="fd-loading-dialog">
            <div className="spinner" />
            <span className="phase">{state.loading.phase}</span>
          </div>
        </div>
      ) : null}
      {showHelp ? (
        <div
          className="fd-help-overlay"
          onClick={() => setShowHelp(false)}
        >
          <div
            className="fd-help-panel"
            onClick={(e) => e.stopPropagation()}
          >
            <h2>Keyboard Shortcuts</h2>
            <table>
              <tbody>
                <tr><td><kbd>B</kbd></td><td>Add Box</td></tr>
                <tr><td><kbd>S</kbd></td><td>Add Sphere</td></tr>
                <tr><td><kbd>C</kbd></td><td>Add Cylinder</td></tr>
                <tr><td><kbd>T</kbd></td><td>Add Torus</td></tr>
                <tr><td><kbd>H</kbd></td><td>Add Hole</td></tr>
                <tr><td><kbd>Del</kbd></td><td>Delete selection</td></tr>
                <tr><td><kbd>Esc</kbd></td><td>Clear selection</td></tr>
                <tr><td><kbd>Ctrl+Z</kbd></td><td>Undo</td></tr>
                <tr><td><kbd>Ctrl+Shift+Z</kbd></td><td>Redo</td></tr>
                <tr><td><kbd>Ctrl+A</kbd></td><td>Select all</td></tr>
                <tr><td><kbd>Ctrl+D</kbd></td><td>Duplicate</td></tr>
                <tr><td><kbd>Ctrl+S</kbd></td><td>Save project</td></tr>
                {(() => {
                  const row = viewShortcutRow(CONTROLS_PRESETS[settings.controlsPreset]);
                  return (
                    <tr>
                      <td>
                        {row.keys.split(" / ").map((k, i) => (
                          <span key={k}>
                            {i > 0 && " / "}
                            <kbd>{k}</kbd>
                          </span>
                        ))}
                      </td>
                      <td>{row.action}</td>
                    </tr>
                  );
                })()}
                <tr><td><kbd>Arrows</kbd></td><td>Nudge selection</td></tr>
                <tr><td><kbd>F1</kbd></td><td>Toggle this help</td></tr>
              </tbody>
            </table>
            <button type="button" onClick={() => setShowHelp(false)}>Close</button>
          </div>
        </div>
      ) : null}
      </StrictMode>
    </div>
    </WorkspaceUnitsContext.Provider>
  );
}
