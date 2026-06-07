import { useEffect, useState } from "react";
import { Outliner } from "./Outliner";
import { Toolbar } from "./Toolbar";
import { Viewport } from "./Viewport";
import { FieldSkriptEditor } from "./FieldSkriptEditor";
import { BottomTabs } from "./BottomTabs";
import { useKernelStore } from "./store";
import { requestView } from "./viewCommands";
import { isTextEntryTarget } from "./keyboardTarget";
import { SettingsMenu, loadSettings, WorkspaceUnitsContext } from "./SettingsMenu";
import type { SettingsState } from "./SettingsMenu";

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
  useEffect(() => {
    if (store) store.setDcMeshingEnabled(isMeshMode, settings.meshAlgorithm);
  }, [store, isMeshMode, settings.meshAlgorithm]);

  useEffect(() => {
    if (store) store.setParametricWeave(settings.parametricWeave);
  }, [store, settings.parametricWeave]);

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
        if (store.removeSelected()) e.preventDefault();
        return;
      }
      if (ctrl && (e.key === "z" || e.key === "Z")) {
        if (e.shiftKey) {
          if (store.redo()) e.preventDefault();
        } else {
          if (store.undo()) e.preventDefault();
        }
        return;
      }
      if (ctrl && (e.key === "y" || e.key === "Y")) {
        if (store.redo()) e.preventDefault();
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
        switch (e.key) {
          case "Escape": {
            const cur = store.getState();
            if (cur.facePickMode) {
              store.cancelFacePickMode();
              store.enterSketch("qCreatedBy(origin_plane_xy)");
            } else if (cur.mateMode) {
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
          case "b":
            store.addPrimitive("box");
            e.preventDefault();
            return;
          case "s":
            store.addPrimitive("sphere");
            e.preventDefault();
            return;
          case "c":
            store.addPrimitive("cylinder");
            e.preventDefault();
            return;
          case "t":
            store.addPrimitive("torus");
            e.preventDefault();
            return;
          case "h":
            store.addHole();
            e.preventDefault();
            return;
          case "g":
            store.setGizmoMode("translate");
            e.preventDefault();
            return;
          case "r":
            store.setGizmoMode("rotate");
            e.preventDefault();
            return;
          case "1":
            requestView("front");
            e.preventDefault();
            return;
          case "2":
            requestView("top");
            e.preventDefault();
            return;
          case "3":
            requestView("right");
            e.preventDefault();
            return;
          case "0":
            requestView("iso");
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
  }, [store]);

  const activeProjectTab = state?.project.tabs.find(
    (t) => t.id === state.project.activeTabId,
  );
  const activeScriptTab =
    activeProjectTab?.kind === "script" ? activeProjectTab : null;

  return (
    <WorkspaceUnitsContext.Provider value={settings.units}>
    <div className={"fd-shell" + (compact ? " fd-compact" : "")}>
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
      {/* MT-A: both workspaces stay mounted; we toggle visibility with
          the `hidden` attr so the Viewport's GPU context and Monaco are
          never torn down on a tab switch. The active tab is derived from
          the project; part AND assembly both use the part workspace
          (Viewport + Outliner render the assembly when active). */}
      <div className="fd-workspace" hidden={activeProjectTab?.kind === "script"}>
        {store && state ? <Toolbar store={store} state={state} /> : null}
        <main className="main">
          <Viewport renderMode={settings.renderMode} showEdges={settings.showEdges} brepEdgeMode={settings.brepEdgeMode} frameLoop={settings.frameLoop} />
          {mobileNavOpen ? (
            <div
              className="fd-nav-scrim"
              data-testid="mobile-nav-scrim"
              onClick={() => setMobileNavOpen(false)}
            />
          ) : null}
          {store && state ? (
            <Outliner store={store} state={state} mobileNavOpen={mobileNavOpen} />
          ) : null}
        </main>
      </div>
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
                <tr><td><kbd>1</kbd> / <kbd>2</kbd> / <kbd>3</kbd></td><td>Front / Top / Right view</td></tr>
                <tr><td><kbd>0</kbd></td><td>Isometric view</td></tr>
                <tr><td><kbd>Arrows</kbd></td><td>Nudge selection</td></tr>
                <tr><td><kbd>F1</kbd></td><td>Toggle this help</td></tr>
              </tbody>
            </table>
            <button type="button" onClick={() => setShowHelp(false)}>Close</button>
          </div>
        </div>
      ) : null}
    </div>
    </WorkspaceUnitsContext.Provider>
  );
}
