/**
 * Top-of-viewport toolbar: add primitives, switch the next-add op,
 * undo/redo, delete selection.
 */

import { BoolOpTag, type BoolOpTagValue } from "./kernel";
import type { KernelStore, StoreState } from "./store";

interface ToolbarProps {
  store: KernelStore;
  state: StoreState;
}

const OP_BUTTONS: { tag: BoolOpTagValue; label: string; tooltip: string }[] = [
  { tag: BoolOpTag.Union, label: "+", tooltip: "Union (add)" },
  { tag: BoolOpTag.Difference, label: "−", tooltip: "Difference (subtract)" },
  { tag: BoolOpTag.Intersect, label: "∩", tooltip: "Intersect" },
];

export function Toolbar({ store, state }: ToolbarProps) {
  return (
    <div className="fielddraft-toolbar" data-testid="fielddraft-toolbar">
      <div className="fielddraft-toolbar-group">
        <button
          type="button"
          className="fielddraft-tool"
          title="Add Box"
          data-testid="add-box"
          onClick={() => store.addPrimitive("box")}
        >
          ▣ Box
        </button>
        <button
          type="button"
          className="fielddraft-tool"
          title="Add Sphere"
          data-testid="add-sphere"
          onClick={() => store.addPrimitive("sphere")}
        >
          ● Sphere
        </button>
        <button
          type="button"
          className="fielddraft-tool"
          title="Add Cylinder"
          data-testid="add-cylinder"
          onClick={() => store.addPrimitive("cylinder")}
        >
          ⬢ Cylinder
        </button>
      </div>

      <div className="fielddraft-toolbar-group">
        <span className="fielddraft-toolbar-label">Op:</span>
        {OP_BUTTONS.map((b) => (
          <button
            key={b.tag}
            type="button"
            className={
              "fielddraft-op-toggle" + (state.nextOp === b.tag ? " is-active" : "")
            }
            title={b.tooltip}
            data-testid={`op-${b.tag}`}
            onClick={() => store.setNextOp(b.tag)}
          >
            {b.label}
          </button>
        ))}
      </div>

      <div className="fielddraft-toolbar-group">
        <button
          type="button"
          className="fielddraft-tool"
          title="Delete selection (Del)"
          data-testid="delete-selection"
          disabled={state.selection.size === 0}
          onClick={() => store.removeSelected()}
        >
          Delete
        </button>
        <button
          type="button"
          className="fielddraft-tool"
          title="Undo (Ctrl+Z)"
          data-testid="undo"
          disabled={!state.canUndo}
          onClick={() => store.undo()}
        >
          ⟲ Undo
        </button>
        <button
          type="button"
          className="fielddraft-tool"
          title="Redo (Ctrl+Shift+Z)"
          data-testid="redo"
          disabled={!state.canRedo}
          onClick={() => store.redo()}
        >
          ⟳ Redo
        </button>
      </div>
    </div>
  );
}
