import { useEffect } from "react";
import { Outliner } from "./Outliner";
import { Toolbar } from "./Toolbar";
import { Viewport } from "./Viewport";
import { useKernelStore } from "./store";
import "./App.css";

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
