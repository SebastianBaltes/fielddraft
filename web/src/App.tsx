import { useEffect } from "react";
import { Outliner } from "./Outliner";
import { Toolbar } from "./Toolbar";
import { Viewport } from "./Viewport";
import { useKernelStore } from "./store";
import { requestView } from "./viewCommands";

export function App() {
  const { store, state, error } = useKernelStore();

  // Global keyboard shortcuts: Del = remove selection, Ctrl+Z = undo,
  // Ctrl+Shift+Z (or Ctrl+Y) = redo. Skipped if focus is in an input.
  useEffect(() => {
    if (!store) return;
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) return;

      if (e.key === "Delete" || e.key === "Backspace") {
        if (store.removeSelected()) e.preventDefault();
        return;
      }
      const ctrl = e.ctrlKey || e.metaKey;
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
          case "Escape":
            store.clearSelection();
            e.preventDefault();
            return;
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
          case "h":
            store.addHole();
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

  return (
    <div className="fd-shell">
      <header className="topbar">
        <div className="title">FieldDraft</div>
        <div className="meta">
          {error ? (
            <span className="error">kernel error: {error.message}</span>
          ) : state ? (
            <>
              <span data-testid="feature-count">
                {state.features.length} feature{state.features.length === 1 ? "" : "s"}
              </span>
              {state.sceneSize[0] > 0 || state.sceneSize[1] > 0 || state.sceneSize[2] > 0 ? (
                <span className="scene-size" data-testid="scene-size">
                  {state.sceneSize[0].toFixed(1)} × {state.sceneSize[1].toFixed(1)} × {state.sceneSize[2].toFixed(1)} mm
                </span>
              ) : null}
            </>
          ) : (
            <span>loading…</span>
          )}
        </div>
      </header>
      {store && state ? <Toolbar store={store} state={state} /> : null}
      <main className="main">
        <Viewport />
        {store && state ? <Outliner store={store} state={state} /> : null}
      </main>
    </div>
  );
}
