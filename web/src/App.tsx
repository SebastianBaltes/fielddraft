import { Viewport } from "./Viewport";
import { useKernelStore } from "./store";
import "./App.css";

export function App() {
  const { primitiveCount, error } = useKernelStore();

  return (
    <div className="fielddraft-shell">
      <header className="fielddraft-topbar">
        <div className="fielddraft-title">FieldDraft</div>
        <div className="fielddraft-meta">
          {error ? (
            <span className="fielddraft-error">kernel error: {error.message}</span>
          ) : (
            <span>{primitiveCount} primitive{primitiveCount === 1 ? "" : "s"}</span>
          )}
        </div>
      </header>
      <main className="fielddraft-viewport">
        <Viewport />
      </main>
    </div>
  );
}
