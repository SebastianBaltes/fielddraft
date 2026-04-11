/**
 * Feature outliner — flat list of features with selection + per-row op
 * switching. Mirrors Onshape's part-studio feature list, scoped down to
 * a flat spine for M2.
 */

import {
  useEffect,
  useMemo,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent,
} from "react";
import { BoolOpTag, type BoolOpName, type BoolOpTagValue, type FeatureSummary } from "./kernel";
import type { KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { FeatureDialog } from "./widgets/FeatureDialog";
import {
  buildDefaultDefinition,
  type FeatureSchema,
} from "./widgets/schema";

const OP_LABEL: Record<BoolOpName, string> = {
  union: "+",
  difference: "−",
  intersect: "∩",
};

const OP_TOOLTIP: Record<BoolOpName, string> = {
  union: "Union (add)",
  difference: "Difference (subtract)",
  intersect: "Intersect",
};

const KIND_GLYPH: Record<string, string> = {
  box: "▣",
  sphere: "●",
  cylinder: "⬢",
  sketch: "✎",
  extrude: "↥",
};

function nameToTag(op: BoolOpName): BoolOpTagValue {
  switch (op) {
    case "union":
      return BoolOpTag.Union;
    case "difference":
      return BoolOpTag.Difference;
    case "intersect":
      return BoolOpTag.Intersect;
  }
}

function nextOp(op: BoolOpName): BoolOpTagValue {
  // Cycle: union → difference → intersect → union
  switch (op) {
    case "union":
      return BoolOpTag.Difference;
    case "difference":
      return BoolOpTag.Intersect;
    case "intersect":
      return BoolOpTag.Union;
  }
}

interface OutlinerProps {
  store: KernelStore;
  state: StoreState;
}

export function Outliner({ store, state }: OutlinerProps) {
  // K6a: memoised Outer API façade over the store's kernel. The
  // `createFdApi` return value is a tiny object of method refs,
  // so re-creating it per render is cheap — but memoising keeps
  // React's reference identity stable for downstream hooks.
  const fd = useMemo(() => createFdApi(store.kernel), [store]);

  // K7.5: the FeatureDialog's working copy mirrors the
  // currently-selected feature's real params. Whenever the
  // selection changes we reload the draft from the kernel,
  // and every dialog onChange commits the new blob back via
  // store.updateFeatureParams. Keyed on the feature id so
  // switching between two features of the same kind picks up
  // each one's own values, not a shared default.
  const [dialogDraft, setDialogDraft] = useState<
    Record<string, unknown>
  >({});
  const selectedFeature = useMemo<FeatureSummary | undefined>(
    () =>
      state.selection.size === 1
        ? state.features.find((f) => state.selection.has(f.id))
        : undefined,
    [state.selection, state.features],
  );
  const selectedSchema = useMemo<FeatureSchema | null>(() => {
    if (!selectedFeature) return null;
    try {
      return fd.schemaForType(selectedFeature.kind);
    } catch {
      // The kernel may report a kind name the schema lookup
      // doesn't know about (future feature types, stale caches).
      // Silently skip the dialog in that case — the rest of the
      // outliner still functions.
      return null;
    }
  }, [fd, selectedFeature]);

  // Reload the draft from the kernel whenever the selected
  // feature id changes. The `draftKey` is the feature id plus
  // the schema type so a delete+re-add of the same id still
  // reinitialises cleanly.
  const draftKey =
    selectedFeature && selectedSchema
      ? `${selectedFeature.id}:${selectedSchema.type}`
      : null;
  const [lastDraftKey, setLastDraftKey] = useState<string | null>(null);
  if (draftKey !== lastDraftKey) {
    setLastDraftKey(draftKey);
    if (selectedFeature && selectedSchema) {
      try {
        setDialogDraft(store.getFeatureParams(selectedFeature.id));
      } catch {
        setDialogDraft(buildDefaultDefinition(selectedSchema));
      }
    } else {
      setDialogDraft({});
    }
  }

  // K7.5: committing a dialog change writes back through the
  // store, which re-runs regeneration. The commit bumps the
  // store version, which forces a re-render and a new
  // `selectedFeature` reference. The `draftKey` equality
  // guard above prevents the reload from clobbering an
  // in-progress edit because the feature id hasn't changed.
  const handleDialogChange = (next: Record<string, unknown>) => {
    setDialogDraft(next);
    if (!selectedFeature) return;
    // Sketches have their own reentry session, so the generic
    // edit path is skipped for them. Every other feature kind
    // (primitives, extrude, linear/circular/mirror patterns)
    // round-trips through replaceFeatureCallParams.
    if (selectedFeature.kind === "sketch") return;
    store.updateFeatureParams(selectedFeature.id, next);
  };

  const handleRowClick = (e: MouseEvent, id: number) => {
    if (e.shiftKey) {
      store.selectRangeTo(id);
    } else if (e.ctrlKey || e.metaKey) {
      store.toggleSelect(id);
    } else {
      store.selectOnly(id);
    }
  };

  // K8.5: inline rename state. Holds the id of the row
  // currently in rename mode plus the working-copy string.
  // Null means no row is being renamed.
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameDraft, setRenameDraft] = useState<string>("");

  const beginRename = (feature: FeatureSummary) => {
    setRenamingId(feature.id);
    setRenameDraft(feature.name);
  };

  const commitRename = () => {
    if (renamingId === null) return;
    const next = renameDraft.trim();
    if (next.length > 0) {
      store.renameFeature(renamingId, next);
    }
    setRenamingId(null);
    setRenameDraft("");
  };

  const cancelRename = () => {
    setRenamingId(null);
    setRenameDraft("");
  };

  const handleRenameKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      commitRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelRename();
    }
    // Swallow so global shortcuts (Delete, Ctrl-Z, …) don't
    // fire while typing.
    e.stopPropagation();
  };

  /** K6b.3b: double-click on a sketch row reopens it for editing.
   *  Onshape convention — single-click selects, double-click
   *  enters the feature's own edit mode. K8.5 reuses double-click
   *  on every non-sketch row for inline rename. */
  const handleRowDoubleClick = (feature: FeatureSummary) => {
    if (feature.kind === "sketch") {
      store.editSketch(feature.id);
      return;
    }
    beginRename(feature);
  };

  const handleOpClick = (e: MouseEvent, feature: FeatureSummary) => {
    e.stopPropagation();
    store.setFeatureOp(feature.id, nextOp(feature.op));
  };

  const handleDelete = (e: MouseEvent, id: number) => {
    e.stopPropagation();
    store.removeFeature(id);
  };

  // K6b.2: when a sketch session is active, the outliner leads with a
  // sketch-session panel (tool FeatureDialog + sub-spine list) so the
  // user never loses the feature list but sees the sketch workspace
  // prominently on top. Schema fetching for the selected sketch tool
  // happens through the same fd facade as top-level features.
  const sketchSession = state.sketchSession;
  const [sketchToolSchema, setSketchToolSchema] =
    useState<FeatureSchema | null>(null);
  useEffect(() => {
    const tool = sketchSession.selectedTool;
    if (!tool) {
      setSketchToolSchema(null);
      return;
    }
    try {
      setSketchToolSchema(fd.schemaForSketchToolType(tool));
    } catch {
      setSketchToolSchema(null);
    }
  }, [fd, sketchSession.selectedTool]);

  return (
    <aside className="fd-outliner" data-testid="fielddraft-outliner">
      {sketchSession.active ? (
        <section
          className="sketch-session"
          data-testid="sketch-session-panel"
        >
          <div className="header">
            <span>Sketch</span>
            <span
              className="plane-query"
              data-testid="sketch-session-plane"
            >
              {sketchSession.planeQuery ?? "—"}
            </span>
          </div>
          {sketchSession.selectedTool && sketchToolSchema ? (
            <>
              <FeatureDialog
                schema={sketchToolSchema}
                definition={sketchSession.toolDraft}
                onChange={(next) => store.setSketchToolDraft(next)}
              />
              <div className="actions">
                <button
                  type="button"
                  className="apply"
                  data-testid="sketch-tool-apply"
                  onClick={() => store.applySketchToolDraft()}
                >
                  Apply
                </button>
                <button
                  type="button"
                  className="clear"
                  data-testid="sketch-tool-clear"
                  onClick={() => store.selectSketchTool(null)}
                >
                  Clear
                </button>
              </div>
            </>
          ) : (
            <div className="empty" data-testid="sketch-session-empty">
              Pick a sketch tool in the toolbar.
            </div>
          )}
          {sketchSession.subCalls.length > 0 ? (
            <ul className="sub-spine" data-testid="sketch-sub-spine">
              {sketchSession.subCalls.map((call, i) => (
                <li key={i} className="row">
                  <span className="kind">{call.type}</span>
                  <button
                    type="button"
                    className="row-delete"
                    title="Delete"
                    data-testid={`sketch-sub-row-delete-${i}`}
                    onClick={() => store.removeSketchSubCall(i)}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}
      {!sketchSession.active && selectedSchema ? (
        <FeatureDialog
          schema={selectedSchema}
          definition={dialogDraft}
          onChange={handleDialogChange}
        />
      ) : null}
      <div className="header">Features</div>
      {state.features.length === 0 ? (
        <div className="empty">No features yet.</div>
      ) : (
        <ul className="list">
          {state.features.map((feature) => {
            const selected = state.selection.has(feature.id);
            const tagOp = feature.op;
            return (
              <li
                key={feature.id}
                className={
                  "row" +
                  (selected ? " is-selected" : "") +
                  (feature.suppressed ? " is-suppressed" : "")
                }
                data-testid={`feature-row-${feature.id}`}
                data-feature-id={feature.id}
                data-feature-op={tagOp}
                data-feature-kind={feature.kind}
                data-feature-suppressed={
                  feature.suppressed ? "true" : "false"
                }
                onClick={(e) => handleRowClick(e, feature.id)}
                onDoubleClick={() => handleRowDoubleClick(feature)}
              >
                <button
                  type="button"
                  className={`op op-${tagOp}`}
                  title={OP_TOOLTIP[tagOp]}
                  onClick={(e) => handleOpClick(e, feature)}
                >
                  {OP_LABEL[tagOp]}
                </button>
                <span className="kind">
                  {KIND_GLYPH[feature.kind] ?? "?"}
                </span>
                {renamingId === feature.id ? (
                  <input
                    className="feature-name-input"
                    data-testid={`feature-rename-${feature.id}`}
                    value={renameDraft}
                    autoFocus
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setRenameDraft(e.currentTarget.value)}
                    onKeyDown={handleRenameKey}
                    onBlur={commitRename}
                  />
                ) : (
                  <span className="feature-name">{feature.name}</span>
                )}
                {(feature.translate[0] !== 0 ||
                  feature.translate[1] !== 0 ||
                  feature.translate[2] !== 0 ||
                  feature.scale !== 1) && (
                  <span
                    className="feature-meta"
                    data-testid={`feature-meta-${feature.id}`}
                  >
                    [{feature.translate[0].toFixed(0)},{" "}
                    {feature.translate[1].toFixed(0)},{" "}
                    {feature.translate[2].toFixed(0)}]
                  </span>
                )}
                <button
                  type="button"
                  className="row-suppress"
                  title={feature.suppressed ? "Unsuppress" : "Suppress"}
                  data-testid={`feature-suppress-${feature.id}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    store.toggleFeatureSuppressed(feature.id);
                  }}
                >
                  {feature.suppressed ? "◎" : "◉"}
                </button>
                <button
                  type="button"
                  className="row-reorder"
                  title="Move up"
                  data-testid={`feature-move-up-${feature.id}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    store.moveFeatureUp(feature.id);
                  }}
                >
                  ▲
                </button>
                <button
                  type="button"
                  className="row-reorder"
                  title="Move down"
                  data-testid={`feature-move-down-${feature.id}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    store.moveFeatureDown(feature.id);
                  }}
                >
                  ▼
                </button>
                <button
                  type="button"
                  className="row-reorder"
                  title="Duplicate"
                  data-testid={`feature-duplicate-${feature.id}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    store.duplicateFeature(feature.id);
                  }}
                >
                  ⧉
                </button>
                <button
                  type="button"
                  className="row-delete"
                  title="Delete"
                  onClick={(e) => handleDelete(e, feature.id)}
                >
                  ×
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </aside>
  );
}

export const __testing = { nameToTag, nextOp };
