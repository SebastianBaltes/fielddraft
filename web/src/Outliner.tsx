/**
 * Feature outliner — flat list of features with selection + per-row op
 * switching. Mirrors Onshape's part-studio feature list, scoped down to
 * a flat spine for M2.
 */

import type { MouseEvent } from "react";
import { BoolOpTag, type BoolOpName, type BoolOpTagValue, type FeatureSummary } from "./kernel";
import type { KernelStore, StoreState } from "./store";

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

  return (
    <aside className="fd-outliner" data-testid="fielddraft-outliner">
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
