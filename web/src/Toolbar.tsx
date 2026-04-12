/**
 * Top-of-viewport toolbar: add primitives, switch the next-add op,
 * undo/redo, delete selection. K6b.2 adds a Sketch entry button and
 * an alternate sketch-mode row shown while a sketch session is
 * active.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { BoolOpTag, type BoolOpTagValue } from "./kernel";
import type { KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { buildDefaultDefinition } from "./widgets/schema";
import { requestView } from "./viewCommands";

/** K6b.2 placeholder: the face-lock UX comes in K6b.3. Until then,
 *  the Sketch button anchors every new session on the XY origin
 *  plane so the mode transition is testable end-to-end. */
const DEFAULT_PLANE_QUERY = "qCreatedBy(origin_plane_xy)";

/** Short human-readable labels for the sketch tools. K6b.1 ships
 *  `sketch_point` and `sketch_line`; the rest are listed as
 *  disabled placeholders so the toolbar shape already matches the
 *  K6b.4 end state. */
const SKETCH_TOOL_LABELS: Record<string, { label: string; glyph: string }> = {
  sketch_point: { label: "Point", glyph: "•" },
  sketch_line: { label: "Line", glyph: "╱" },
  sketch_rectangle: { label: "Rectangle", glyph: "▭" },
  sketch_circle: { label: "Circle", glyph: "○" },
};

const SKETCH_TOOL_STUBS: { type: string; label: string; glyph: string }[] = [
  { type: "sketch_arc", label: "Arc", glyph: "◠" },
  { type: "sketch_trim", label: "Trim", glyph: "✂" },
  { type: "sketch_mirror", label: "Mirror", glyph: "◧" },
  { type: "sketch_offset", label: "Offset", glyph: "⇔" },
];

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
  const fd = useMemo(() => createFdApi(store.kernel), [store]);
  const sketchSession = state.sketchSession;

  // K6b.2: list of available sketch tools queried once per kernel.
  // The kernel-side registry is static, so caching the names + their
  // parsed schemas avoids re-parsing on every render.
  const [sketchToolTypes, setSketchToolTypes] = useState<readonly string[]>([]);
  useEffect(() => {
    try {
      setSketchToolTypes(fd.listSketchToolTypes());
    } catch {
      // Older wasm snapshots without the K6b.2 export — degrade
      // to an empty list, the Sketch button still renders.
      setSketchToolTypes([]);
    }
  }, [fd]);

  const handleSketchEnter = () => {
    store.enterSketch(DEFAULT_PLANE_QUERY);
  };

  const handleSketchToolPick = (typeName: string) => {
    try {
      const schema = fd.schemaForSketchToolType(typeName);
      store.selectSketchTool(typeName, buildDefaultDefinition(schema));
    } catch {
      // Unknown tool — don't crash the toolbar; the button just
      // stays as a pointless click. The tool list is kernel-
      // driven, so this branch should never fire in practice.
    }
  };

  if (sketchSession.active) {
    return (
      <div
        className="fd-toolbar is-sketch-mode"
        data-testid="fielddraft-toolbar"
      >
        <div className="group">
          <span className="label" data-testid="sketch-mode-indicator">
            Sketch
          </span>
          <button
            type="button"
            className="tool is-primary"
            title="Finish sketch"
            data-testid="sketch-finish"
            onClick={() => store.commitSketch()}
          >
            ✓ Finish
          </button>
          <button
            type="button"
            className="tool"
            title="Cancel sketch"
            data-testid="sketch-cancel"
            onClick={() => store.exitSketch()}
          >
            ✗ Cancel
          </button>
        </div>

        <div className="group sketch-tool-row">
          {sketchToolTypes.map((type) => {
            const label = SKETCH_TOOL_LABELS[type];
            if (!label) return null;
            const isActive = sketchSession.selectedTool === type;
            return (
              <button
                key={type}
                type="button"
                className={"tool" + (isActive ? " is-active" : "")}
                title={label.label}
                data-testid={`sketch-tool-${type}`}
                onClick={() => handleSketchToolPick(type)}
              >
                {label.glyph} {label.label}
              </button>
            );
          })}
          {SKETCH_TOOL_STUBS.map((stub) => (
            <button
              key={stub.type}
              type="button"
              className="tool is-stub"
              title={`${stub.label} — coming in K6b.3/K6b.4`}
              data-testid={`sketch-tool-${stub.type}`}
              disabled
            >
              {stub.glyph} {stub.label}
            </button>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="fd-toolbar" data-testid="fielddraft-toolbar">
      <div className="group">
        <button
          type="button"
          className="tool"
          title="Add Box"
          data-testid="add-box"
          onClick={() => store.addPrimitive("box")}
        >
          ▣ Box
        </button>
        <button
          type="button"
          className="tool"
          title="Add Sphere"
          data-testid="add-sphere"
          onClick={() => store.addPrimitive("sphere")}
        >
          ● Sphere
        </button>
        <button
          type="button"
          className="tool"
          title="Add Cylinder"
          data-testid="add-cylinder"
          onClick={() => store.addPrimitive("cylinder")}
        >
          ⬢ Cylinder
        </button>
        <button
          type="button"
          className="tool"
          title="Add Torus (R30 × r10 mm)"
          data-testid="add-torus"
          onClick={() => store.addPrimitive("torus")}
        >
          ◎ Torus
        </button>
        <button
          type="button"
          className="tool"
          title="Add Thread (M6 metric bolt)"
          data-testid="add-thread"
          onClick={() => store.addPrimitive("thread")}
        >
          ⏍ Thread
        </button>
        <button
          type="button"
          className="tool"
          title="Add Hole (Ø6 × 40 mm, subtract)"
          data-testid="add-hole"
          onClick={() => store.addHole()}
        >
          ⊖ Hole
        </button>
        <button
          type="button"
          className="tool"
          title="Add Gyroid infill (10 mm cells, 1 mm walls, intersect)"
          data-testid="add-gyroid"
          disabled={state.features.length === 0}
          onClick={() => store.addGyroid()}
        >
          🕸 Gyroid
        </button>
        <button
          type="button"
          className="tool"
          title="Sketch (lock XY origin plane)"
          data-testid="enter-sketch"
          onClick={handleSketchEnter}
        >
          ✎ Sketch
        </button>
        <button
          type="button"
          className="tool"
          title="Extrude the most recent sketch (20 mm depth)"
          data-testid="extrude-latest-sketch"
          disabled={
            !state.features.some((f) => f.kind === "sketch")
          }
          onClick={() => store.addExtrudeFromLatestSketch(20)}
        >
          ↥ Extrude
        </button>
        <button
          type="button"
          className="tool"
          title="Revolve the most recent sketch around the world Y axis"
          data-testid="revolve-latest-sketch"
          disabled={
            !state.features.some((f) => f.kind === "sketch")
          }
          onClick={() => store.addRevolveFromLatestSketch()}
        >
          ↻ Revolve
        </button>
        <button
          type="button"
          className="tool"
          title="Linear pattern the latest feature (+X, 3×, 60 mm)"
          data-testid="linear-pattern-latest"
          disabled={state.features.length === 0}
          onClick={() => store.addLinearPatternOfLatest()}
        >
          ⎡⎡⎡ Pattern
        </button>
        <button
          type="button"
          className="tool"
          title="Circular pattern the latest feature around Y axis (6×, 360°)"
          data-testid="circular-pattern-latest"
          disabled={state.features.length === 0}
          onClick={() => store.addCircularPatternOfLatest()}
        >
          ↻ Circ
        </button>
        <button
          type="button"
          className="tool"
          title="Mirror the latest feature through the world YZ plane"
          data-testid="mirror-latest"
          disabled={state.features.length === 0}
          onClick={() => store.addMirrorOfLatest()}
        >
          ◫ Mirror
        </button>
      </div>

      <div className="group">
        <span className="label">Op:</span>
        {OP_BUTTONS.map((b) => (
          <button
            key={b.tag}
            type="button"
            className={"op-toggle" + (state.nextOp === b.tag ? " is-active" : "")}
            title={b.tooltip}
            data-testid={`op-${b.tag}`}
            onClick={() => store.setNextOp(b.tag)}
          >
            {b.label}
          </button>
        ))}
      </div>

      <div className="group">
        <button
          type="button"
          className="tool"
          title="Delete selection (Del)"
          data-testid="delete-selection"
          disabled={state.selection.size === 0}
          onClick={() => store.removeSelected()}
        >
          Delete
        </button>
        <button
          type="button"
          className="tool"
          title="Undo (Ctrl+Z)"
          data-testid="undo"
          disabled={!state.canUndo}
          onClick={() => store.undo()}
        >
          ⟲ Undo
        </button>
        <button
          type="button"
          className="tool"
          title="Redo (Ctrl+Shift+Z)"
          data-testid="redo"
          disabled={!state.canRedo}
          onClick={() => store.redo()}
        >
          ⟳ Redo
        </button>
        <button
          type="button"
          className="tool"
          title="Export as binary STL"
          data-testid="export-stl"
          disabled={state.features.length === 0}
          onClick={() => store.downloadStl()}
        >
          ⇩ STL
        </button>
      </div>

      <div className="group">
        <input
          ref={useRef<HTMLInputElement>(null)}
          type="file"
          accept=".stl"
          className="hidden-file-input"
          data-testid="import-stl-input"
          onChange={(e) => {
            const file = e.currentTarget.files?.[0];
            if (!file) return;
            file.arrayBuffer().then((buf) => {
              store.loadReferenceMesh(buf);
            });
            e.currentTarget.value = "";
          }}
        />
        <button
          type="button"
          className="tool"
          title="Import STL as visual reference"
          data-testid="import-stl"
          onClick={() => {
            const input = document.querySelector<HTMLInputElement>(
              '[data-testid="import-stl-input"]',
            );
            input?.click();
          }}
        >
          📁 Import
        </button>
      </div>

      <div className="group">
        <span className="label">View:</span>
        <button
          type="button"
          className="tool"
          title="Front view"
          data-testid="view-front"
          onClick={() => requestView("front")}
        >
          ◱ Front
        </button>
        <button
          type="button"
          className="tool"
          title="Top view"
          data-testid="view-top"
          onClick={() => requestView("top")}
        >
          ◳ Top
        </button>
        <button
          type="button"
          className="tool"
          title="Right view"
          data-testid="view-right"
          onClick={() => requestView("right")}
        >
          ◲ Right
        </button>
        <button
          type="button"
          className="tool"
          title="Isometric view"
          data-testid="view-iso"
          onClick={() => requestView("iso")}
        >
          ◰ Iso
        </button>
      </div>
    </div>
  );
}
