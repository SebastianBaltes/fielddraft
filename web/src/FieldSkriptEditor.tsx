/**
 * FieldSkript editor (FT-W): the inline "Feature Script" workspace tab.
 * Fills the whole inner area (App toggles its container's visibility via
 * the bottom tab strip — this component no longer gates on its own
 * state and is never a modal overlay).
 *
 * Layout: a left column listing the document's FieldSkript records
 * (+ "Create"), and a right column hosting a lazily-loaded Monaco editor
 * plus a schema preview. Commit runs the record's transpile → eval →
 * schema pipeline through the store and surfaces any error inline.
 * Instantiation lives in the Part toolbar (one auto-button per saved
 * script), so there is no per-row Insert button here.
 *
 * Monaco is heavy and cannot run under jsdom, so it's imported
 * dynamically from {@link ./fieldskript/monacoSetup} only after the
 * panel mounts — keeping it out of the main bundle and the unit tests.
 */

import { useEffect, useRef, useState } from "react";
import type { KernelStore, StoreState } from "./store";
import type { FieldSkriptRecord } from "./fieldskript/record";
import type { ScriptEditorHandle } from "./fieldskript/monacoSetup";
import { FeatureDialog } from "./widgets/FeatureDialog";
import { parseFeatureSchema, buildDefaultDefinition } from "./widgets/schema";

const NEW_SCRIPT_TEMPLATE = `import { fd } from "fielddraft";

export const definition = fd.feature({
  name: "My Feature",
  icon: "⬡",
  parameters: [
    fd.length("size", { name: "Size", default: 20 }),
  ],
});

export default function build(context, id, def) {
  const s = Number(def.size.value);
  fd.opBox(context, id.sub("body"), {
    half_extents: [s / 2, s / 2, s / 2],
    center: [0, 0, 0],
  });
}
`;

const noop = () => {};

export function FieldSkriptEditor({
  store,
  visible,
}: {
  store: KernelStore;
  state: StoreState;
  /** True while the Script tab is active. Used to relayout Monaco when
   *  the panel transitions from display:none back into view. */
  visible: boolean;
}) {
  // Visibility is owned by App (the tab container). We always render the
  // panel so Monaco stays mounted across tab switches.
  return <FieldSkriptEditorPanel store={store} visible={visible} />;
}

function FieldSkriptEditorPanel({
  store,
  visible,
}: {
  store: KernelStore;
  visible: boolean;
}) {
  // The records list is read fresh from the store on each render-trigger;
  // saveScript/deleteScript both commit() → notify(), re-rendering App.
  const records = store.listScripts();

  // `selectedId === null` means the unsaved "New" buffer is active.
  const [selectedId, setSelectedId] = useState<string | null>(
    () => records[0]?.id ?? null,
  );
  const [buffer, setBuffer] = useState<string>(
    () => records[0]?.source ?? NEW_SCRIPT_TEMPLATE,
  );
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [savedRecord, setSavedRecord] = useState<FieldSkriptRecord | null>(
    () => records[0] ?? null,
  );

  const hostRef = useRef<HTMLDivElement | null>(null);
  const editorRef = useRef<ScriptEditorHandle | null>(null);
  // The buffer the editor should mount with — bumped on selection change
  // so the lazy-load effect re-seeds the model. We key off the selected id
  // (or a sentinel for the New buffer) plus a nonce for repeated "New".
  const [mountKey, setMountKey] = useState(0);

  const loadSource = (source: string, record: FieldSkriptRecord | null) => {
    setBuffer(source);
    setDirty(false);
    setError(null);
    setSavedRecord(record);
    setMountKey((k) => k + 1);
  };

  const selectRecord = (record: FieldSkriptRecord) => {
    setSelectedId(record.id);
    loadSource(record.source, record);
  };

  const newScript = () => {
    setSelectedId(null);
    loadSource(NEW_SCRIPT_TEMPLATE, null);
  };

  // Lazy-mount Monaco whenever the selection (mountKey) changes. The
  // dynamic import keeps Monaco + its `?worker` chunks out of the main
  // bundle and the jsdom test environment.
  useEffect(() => {
    let disposed = false;
    let handle: ScriptEditorHandle | null = null;
    const host = hostRef.current;
    if (!host) return;
    const seed = buffer;
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
    // We intentionally re-run only on mountKey: `buffer` is read once as
    // the seed; live edits flow back through onChange, not a remount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mountKey]);

  const save = () => {
    const name = savedRecord?.name;
    const result = store.saveScript({
      id: selectedId ?? undefined,
      source: buffer,
      name,
    });
    if (!result.ok) {
      setError(result.error);
      return;
    }
    setError(null);
    setDirty(false);
    setSelectedId(result.record.id);
    setSavedRecord(result.record);
  };

  const remove = (id: string) => {
    store.deleteScript(id);
    if (selectedId === id) newScript();
  };

  // Monaco's automaticLayout can't measure a display:none container, so
  // when the Script tab becomes visible again we force a relayout.
  useEffect(() => {
    if (visible) editorRef.current?.layout();
  }, [visible]);

  return (
    <div className="fd-fieldskript-editor" data-testid="fieldskript-editor">
      <div className="body">
        <aside className="list">
          <button
            type="button"
            className="new"
            data-testid="fieldskript-new"
            onClick={newScript}
          >
            + Add Feature Script
          </button>
          {records.length === 0 ? (
            <p className="empty">No scripts yet.</p>
          ) : (
            <ul>
              {records.map((r) => (
                <li
                  key={r.id}
                  className={"row" + (r.id === selectedId ? " is-active" : "")}
                >
                  <button
                    type="button"
                    className="select"
                    onClick={() => selectRecord(r)}
                  >
                    <span className="icon" aria-hidden="true">{r.icon}</span>
                    <span className="text">
                      <span className="name">{r.name}</span>
                      <span className="id">{r.id}</span>
                    </span>
                  </button>
                  <button
                    type="button"
                    className="delete"
                    title="Delete script"
                    onClick={() => remove(r.id)}
                  >
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          )}
        </aside>

        <section className="editor-pane">
          <div
            className="editor-host"
            data-testid="fieldskript-editor-host"
            ref={hostRef}
          />
          {savedRecord ? (
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
          ) : null}
        </section>
      </div>

      <footer className="footer">
        {error ? (
          <span className="error" role="alert" data-testid="fieldskript-error">
            {error}
          </span>
        ) : (
          <span className="status">
            {selectedId
              ? `Editing ${selectedId}${dirty ? " (unsaved)" : ""}`
              : `New script${dirty ? " (unsaved)" : ""}`}
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
      </footer>
    </div>
  );
}
