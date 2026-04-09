import { useEffect } from "react";
import { Outliner } from "./Outliner";
import { Toolbar } from "./Toolbar";
import { Viewport } from "./Viewport";
import { useKernelStore } from "./store";

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
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store]);

  return (
    <div className="fielddraft-shell">
      <header className="fielddraft-topbar">
        <div className="fielddraft-title">FieldDraft</div>
        <div className="fielddraft-meta">
          {error ? (
            <span className="fielddraft-error">kernel error: {error.message}</span>
          ) : state ? (
            <span data-testid="feature-count">
              {state.features.length} feature{state.features.length === 1 ? "" : "s"}
            </span>
          ) : (
            <span>loading…</span>
          )}
        </div>
      </header>
      {store && state ? <Toolbar store={store} state={state} /> : null}
      <main className="fielddraft-main">
        <div className="fielddraft-viewport">
          <Viewport />
        </div>
        {store && state ? <Outliner store={store} state={state} /> : null}
      </main>
    </div>
  );
}
