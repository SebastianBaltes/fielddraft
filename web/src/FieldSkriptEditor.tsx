/**
 * FieldSkript editor (MT-A): the inline "Feature Script" workspace tab.
 * Fills the whole inner area (App toggles its container's visibility via
 * the bottom tab strip — this component no longer gates on its own
 * state and is never a modal overlay).
 *
 * MT-A re-binds the editor to a *single* script tab: it seeds Monaco from
 * the tab's `record.source` and Commit calls `store.saveScriptTab`. The
 * old multi-record left sidebar is gone — each script is its own tab now.
 * (Chunk B / MT-B moves the Commit bar to the top and polishes layout.)
 *
 * Monaco is heavy and cannot run under jsdom, so it's imported
 * dynamically from {@link ./fieldskript/monacoSetup} only after the
 * panel mounts — keeping it out of the main bundle and the unit tests.
 */

import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import type { KernelStore } from "./store";
import type { ScriptTab } from "./project";
import type { ScriptEditorHandle } from "./fieldskript/monacoSetup";
import { FeatureDialog } from "./widgets/FeatureDialog";
import { parseFeatureSchema, buildDefaultDefinition } from "./widgets/schema";

const noop = () => {};

// ── Schema-preview panel resize (mirrors the Outliner splitter) ──
const SCHEMA_MIN_WIDTH = 240; // px
const SCHEMA_MAX_WIDTH = 760; // px
const SCHEMA_DEFAULT_REM = 22;
const SCHEMA_WIDTH_STORAGE_KEY = "fielddraft.fieldskriptSchemaWidth";

function clampSchemaWidth(px: number): number {
  if (!Number.isFinite(px)) return SCHEMA_MIN_WIDTH;
  return Math.max(SCHEMA_MIN_WIDTH, Math.min(SCHEMA_MAX_WIDTH, px));
}

function loadSchemaWidth(): number {
  try {
    const raw = localStorage.getItem(SCHEMA_WIDTH_STORAGE_KEY);
    if (raw != null) {
      const parsed = parseFloat(raw);
      if (Number.isFinite(parsed)) return clampSchemaWidth(parsed);
    }
  } catch {
    // localStorage blocked — fall through to default.
  }
  const rootFontSize =
    typeof window !== "undefined"
      ? parseFloat(getComputedStyle(document.documentElement).fontSize) || 16
      : 16;
  return clampSchemaWidth(SCHEMA_DEFAULT_REM * rootFontSize);
}

export function FieldSkriptEditor({
  store,
  tab,
  visible,
}: {
  store: KernelStore;
  /** The active script tab whose record this editor edits. */
  tab: ScriptTab;
  /** True while the Script tab is active. Used to relayout Monaco when
   *  the panel transitions from display:none back into view. */
  visible: boolean;
}) {
  // Key on tab id so switching script tabs remounts the panel and
  // re-seeds Monaco from the new record.
  return (
    <FieldSkriptEditorPanel key={tab.id} store={store} tab={tab} visible={visible} />
  );
}

function FieldSkriptEditorPanel({
  store,
  tab,
  visible,
}: {
  store: KernelStore;
  tab: ScriptTab;
  visible: boolean;
}) {
  const [buffer, setBuffer] = useState<string>(() => tab.record.source);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The schema preview tracks the last successfully-saved record.
  const [savedRecord, setSavedRecord] = useState(() => tab.record);

  const hostRef = useRef<HTMLDivElement | null>(null);
  const editorRef = useRef<ScriptEditorHandle | null>(null);

  // Resizable schema panel on the RIGHT (splitter on its left edge),
  // mirroring the Part editor's Outliner. Width persists in localStorage.
  const [schemaWidth, setSchemaWidth] = useState<number>(() => loadSchemaWidth());
  const dragStateRef = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    try {
      localStorage.setItem(SCHEMA_WIDTH_STORAGE_KEY, String(schemaWidth));
    } catch {
      // ignore
    }
  }, [schemaWidth]);

  const onSplitterDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    dragStateRef.current = { startX: e.clientX, startWidth: schemaWidth };
  };
  const onSplitterMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragStateRef.current;
    if (!drag) return;
    // Splitter is on the LEFT edge of the panel; dragging left widens it.
    const delta = drag.startX - e.clientX;
    setSchemaWidth(clampSchemaWidth(drag.startWidth + delta));
    editorRef.current?.layout();
  };
  const onSplitterUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    dragStateRef.current = null;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // ignore
    }
  };
  const onSplitterDoubleClick = () => {
    setSchemaWidth(clampSchemaWidth(SCHEMA_DEFAULT_REM * 16));
    editorRef.current?.layout();
  };

  // Lazy-mount Monaco once for this panel instance. The dynamic import
  // keeps Monaco + its `?worker` chunks out of the main bundle and the
  // jsdom test environment.
  useEffect(() => {
    let disposed = false;
    let handle: ScriptEditorHandle | null = null;
    const host = hostRef.current;
    if (!host) return;
    const seed = tab.record.source;
    import("./fieldskript/monacoSetup")
      .then(({ createScriptEditor }) => {
        if (disposed || !hostRef.current) return;
        handle = createScriptEditor(hostRef.current, seed, (value) => {
          setBuffer(value);
          setDirty(true);
        });
        editorRef.current = handle;
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.error("fielddraft: failed to load Monaco editor", err);
        setError("Failed to load the code editor.");
      });
    return () => {
      disposed = true;
      handle?.dispose();
      editorRef.current = null;
    };
    // Seed once on mount; live edits flow back through onChange.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = () => {
    const result = store.saveScriptTab(tab.id, buffer);
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setError(null);
    setDirty(false);
    setSavedRecord(result.record);
  };

  // Monaco's automaticLayout can't measure a display:none container, so
  // when the Script tab becomes visible again we force a relayout.
  useEffect(() => {
    if (visible) editorRef.current?.layout();
  }, [visible]);

  return (
    <div className="fd-fieldskript-editor" data-testid="fieldskript-editor">
      <header className="header">
        {error ? (
          <span className="error" role="alert" data-testid="fieldskript-error">
            {error}
          </span>
        ) : (
          <span className="status">
            {`Editing ${savedRecord.id}${dirty ? " (unsaved)" : ""}`}
          </span>
        )}
        <button
          type="button"
          className="save"
          data-testid="fieldskript-save"
          onClick={save}
        >
          Commit
        </button>
      </header>

      <div className="body">
        <section className="editor-pane">
          <div
            className="editor-host"
            data-testid="fieldskript-editor-host"
            ref={hostRef}
          />
        </section>
        <aside
          className="schema-panel"
          style={{ width: `${schemaWidth}px`, flex: `0 0 ${schemaWidth}px` }}
          data-testid="fieldskript-schema-panel"
        >
          <div
            className="splitter"
            data-testid="fieldskript-schema-splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize schema preview"
            title="Drag to resize (double-click to reset)"
            onPointerDown={onSplitterDown}
            onPointerMove={onSplitterMove}
            onPointerUp={onSplitterUp}
            onPointerCancel={onSplitterUp}
            onDoubleClick={onSplitterDoubleClick}
          />
          <div className="schema-preview">
            <h3>Schema preview</h3>
            <FeatureDialog
              schema={parseFeatureSchema(savedRecord.schema)}
              definition={buildDefaultDefinition(
                parseFeatureSchema(savedRecord.schema),
              )}
              onChange={noop}
            />
          </div>
        </aside>
      </div>
    </div>
  );
}
