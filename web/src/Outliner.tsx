/**
 * Feature outliner — flat list of features with selection + per-row op
 * switching. Mirrors Onshape's part-studio feature list, scoped down to
 * a flat spine for M2.
 */

import { useEffect, useMemo, useState, type MouseEvent } from "react";
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

  // K6a: the generic FeatureDialog's working copy. Keyed on the
  // currently-selected feature's kind so switching between two
  // features of the same kind reuses the edit buffer, while
  // switching to a different kind reinitialises it from the
  // schema defaults. Full "commit edits back to the kernel"
  // plumbing is K6b territory — the dialog in this slice
  // renders the schema and lets the user type into it, but
  // doesn't yet round-trip values into the feature spine.
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

  // Reset the dialog draft whenever the selection changes to a
  // feature of a different kind. Keying by kind rather than id
  // means navigating through a row of identical primitives
  // reuses the in-progress edits without the React reconciler
  // thrashing.
  const draftKey = selectedSchema?.type ?? null;
  const [lastDraftKey, setLastDraftKey] = useState<string | null>(null);
  if (draftKey !== lastDraftKey) {
    setLastDraftKey(draftKey);
    setDialogDraft(
      selectedSchema ? buildDefaultDefinition(selectedSchema) : {},
    );
  }

  const handleRowClick = (e: MouseEvent, id: number) => {
    if (e.shiftKey) {
      store.selectRangeTo(id);
    } else if (e.ctrlKey || e.metaKey) {
      store.toggleSelect(id);
    } else {
      store.selectOnly(id);
    }
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
          onChange={setDialogDraft}
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
                className={"row" + (selected ? " is-selected" : "")}
                data-testid={`feature-row-${feature.id}`}
                data-feature-id={feature.id}
                data-feature-op={tagOp}
                onClick={(e) => handleRowClick(e, feature.id)}
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
                <span className="feature-name">{feature.name}</span>
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
