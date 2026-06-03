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

import { useEffect, useRef, useState } from "react";
import type { KernelStore } from "./store";
import type { ScriptTab } from "./project";
import type { ScriptEditorHandle } from "./fieldskript/monacoSetup";
import { FeatureDialog } from "./widgets/FeatureDialog";
import { parseFeatureSchema, buildDefaultDefinition } from "./widgets/schema";

const noop = () => {};

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
        </section>
      </div>
    </div>
  );
}
