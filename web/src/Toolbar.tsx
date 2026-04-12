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

/** ISO metric thread presets (external, bolt-side). Radius is
 *  nominal/2, pitch and depth per ISO 261. */
const METRIC_THREADS = [
  { label: "M3",  radius: 1.5,  pitch: 0.5,  depth: 0.307 },
  { label: "M4",  radius: 2.0,  pitch: 0.7,  depth: 0.429 },
  { label: "M5",  radius: 2.5,  pitch: 0.8,  depth: 0.491 },
  { label: "M6",  radius: 3.0,  pitch: 1.0,  depth: 0.613 },
  { label: "M8",  radius: 4.0,  pitch: 1.25, depth: 0.767 },
  { label: "M10", radius: 5.0,  pitch: 1.5,  depth: 0.920 },
  { label: "M12", radius: 6.0,  pitch: 1.75, depth: 1.074 },
];

/** K6b.2 placeholder: the face-lock UX comes in K6b.3. Until then,
 *  the Sketch button anchors every new session on the XY origin
 *  plane so the mode transition is testable end-to-end. */
const SKETCH_PLANES: { query: string; label: string; title: string }[] = [
  { query: "qCreatedBy(origin_plane_xy)", label: "XY", title: "Sketch on XY plane" },
  { query: "qCreatedBy(origin_plane_xz)", label: "XZ", title: "Sketch on XZ plane (top-down)" },
  { query: "qCreatedBy(origin_plane_yz)", label: "YZ", title: "Sketch on YZ plane" },
];

/** Short human-readable labels for the sketch tools. K6b.1 ships
 *  `sketch_point` and `sketch_line`; the rest are listed as
 *  disabled placeholders so the toolbar shape already matches the
 *  K6b.4 end state. */
const SKETCH_TOOL_LABELS: Record<string, { label: string; glyph: string }> = {
  sketch_point: { label: "Point", glyph: "•" },
  sketch_line: { label: "Line", glyph: "╱" },
  sketch_rectangle: { label: "Rectangle", glyph: "▭" },
  sketch_circle: { label: "Circle", glyph: "○" },
  sketch_polygon: { label: "Polygon", glyph: "⬠" },
  sketch_arc: { label: "Arc", glyph: "◠" },
  sketch_trim: { label: "Trim", glyph: "✂" },
  sketch_mirror: { label: "Mirror", glyph: "◧" },
  sketch_offset: { label: "Offset", glyph: "⇔" },
};

const SKETCH_TOOL_STUBS: { type: string; label: string; glyph: string }[] = [];

/** K14: makes a toolbar button draggable into the viewport. */
function dragProps(kind: string) {
  return {
    draggable: true,
    onDragStart: (e: React.DragEvent<HTMLButtonElement>) => {
      e.dataTransfer.setData("fielddraft/primitive", kind);
      e.dataTransfer.effectAllowed = "copy";
    },
  };
}

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

  const [sketchPlaneOpen, setSketchPlaneOpen] = useState(false);
  const [threadMenuOpen, setThreadMenuOpen] = useState(false);

  const handleSketchEnter = (planeQuery: string) => {
    store.enterSketch(planeQuery);
    setSketchPlaneOpen(false);
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
          data-testid="add-box" {...dragProps("box")}
          onClick={() => store.addPrimitive("box")}
        >
          ▣ Box
        </button>
        <button
          type="button"
          className="tool"
          title="Add Sphere"
          data-testid="add-sphere" {...dragProps("sphere")}
          onClick={() => store.addPrimitive("sphere")}
        >
          ● Sphere
        </button>
        <button
          type="button"
          className="tool"
          title="Add Cylinder"
          data-testid="add-cylinder" {...dragProps("cylinder")}
          onClick={() => store.addPrimitive("cylinder")}
        >
          ⬢ Cylinder
        </button>
        <button
          type="button"
          className="tool"
          title="Add Hex Prism (apothem 25 mm)"
          data-testid="add-hex-prism" {...dragProps("hex_prism")}
          onClick={() => store.addPrimitive("hex_prism")}
        >
          ⬡ Hex
        </button>
        <button
          type="button"
          className="tool"
          title="Add Capsule (R20 × H80 mm)"
          data-testid="add-capsule" {...dragProps("capsule")}
          onClick={() => store.addPrimitive("capsule")}
        >
          ⬭ Capsule
        </button>
        <button
          type="button"
          className="tool"
          title="Add Cone (frustum R40/10 × H120 mm)"
          data-testid="add-cone" {...dragProps("cone")}
          onClick={() => store.addPrimitive("cone")}
        >
          △ Cone
        </button>
        <button
          type="button"
          className="tool"
          title="Add Torus (R30 × r10 mm)"
          data-testid="add-torus" {...dragProps("torus")}
          onClick={() => store.addPrimitive("torus")}
        >
          ◎ Torus
        </button>
        <button
          type="button"
          className="tool"
          title="Add Thread (M6 metric bolt)"
          data-testid="add-thread"
          onClick={() => setThreadMenuOpen(!threadMenuOpen)}
        >
          ⏍ Thread ▾
        </button>
        {threadMenuOpen ? (
          <div className="sketch-plane-dropdown">
            {METRIC_THREADS.map((t) => (
              <button
                key={t.label}
                type="button"
                className="tool"
                title={`${t.label}: Ø${t.radius * 2} pitch ${t.pitch}`}
                onClick={() => {
                  store.addThreadPreset(t.radius, t.pitch, t.depth);
                  setThreadMenuOpen(false);
                }}
              >
                {t.label}
              </button>
            ))}
          </div>
        ) : null}
        <button
          type="button"
          className="tool"
          title="Add Hole (Ø6 × 40 mm, subtract)"
          data-testid="add-hole" {...dragProps("hole")}
          onClick={() => store.addHole()}
        >
          ⊖ Hole
        </button>
        <button
          type="button"
          className="tool"
          title="Add Gyroid infill (10 mm cells, 1 mm walls, intersect)"
          data-testid="add-gyroid" {...dragProps("gyroid")}
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
          onClick={() => setSketchPlaneOpen(!sketchPlaneOpen)}
        >
          ✎ Sketch ▾
        </button>
        {sketchPlaneOpen ? (
          <div className="sketch-plane-dropdown">
            {SKETCH_PLANES.map((plane) => (
              <button
                key={plane.query}
                type="button"
                className="tool"
                title={plane.title}
                data-testid={`sketch-plane-${plane.label.toLowerCase()}`}
                onClick={() => handleSketchEnter(plane.query)}
              >
                {plane.label}
              </button>
            ))}
          </div>
        ) : null}
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
          className={"tool" + (state.measureMode ? " is-active" : "")}
          title="Measure distance (click two points)"
          data-testid="measure-toggle"
          onClick={() => store.toggleMeasureMode()}
        >
          📏 Measure
        </button>
        <button
          type="button"
          className="tool"
          title="Undo (Ctrl+Z)"
          data-testid="undo"
          disabled={!state.canUndo}
          onClick={() => store.undo()}
        >
          ⟲ Undo{state.undoDepth > 0 ? ` (${state.undoDepth})` : ""}
        </button>
        <button
          type="button"
          className="tool"
          title="Redo (Ctrl+Shift+Z)"
          data-testid="redo"
          disabled={!state.canRedo}
          onClick={() => store.redo()}
        >
          ⟳ Redo{state.redoDepth > 0 ? ` (${state.redoDepth})` : ""}
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
        <button
          type="button"
          className="tool"
          title="Export as binary GLB (glTF 2.0)"
          data-testid="export-glb"
          disabled={state.features.length === 0}
          onClick={() => store.downloadGlb()}
        >
          ⇩ GLB
        </button>
        <button
          type="button"
          className="tool"
          title="Export as Wavefront OBJ"
          data-testid="export-obj"
          disabled={state.features.length === 0}
          onClick={() => store.downloadObj()}
        >
          ⇩ OBJ
        </button>
      </div>

      <div className="group">
        <button
          type="button"
          className="tool"
          title="Save project as .fdraft file"
          data-testid="save-project"
          onClick={() => store.saveProjectToFile()}
        >
          💾 Save
        </button>
        <input
          type="file"
          accept=".fdraft,.json"
          className="hidden-file-input"
          data-testid="load-project-input"
          onChange={(e) => {
            const file = e.currentTarget.files?.[0];
            if (!file) return;
            file.text().then((json) => {
              store.loadProjectFromJson(json);
            });
            e.currentTarget.value = "";
          }}
        />
        <button
          type="button"
          className="tool"
          title="Open a .fdraft project file"
          data-testid="load-project"
          onClick={() => {
            const input = document.querySelector<HTMLInputElement>(
              '[data-testid="load-project-input"]',
            );
            input?.click();
          }}
        >
          📂 Open
        </button>
        <button
          type="button"
          className="tool"
          title="New empty project"
          data-testid="new-project"
          onClick={() => {
            if (
              state.features.length === 0 ||
              confirm("Discard current project?")
            ) {
              store.newProject();
            }
          }}
        >
          📄 New
        </button>
        <input
          ref={useRef<HTMLInputElement>(null)}
          type="file"
          accept=".stl,.glb,.gltf,.obj"
          className="hidden-file-input"
          data-testid="import-stl-input"
          onChange={(e) => {
            const file = e.currentTarget.files?.[0];
            if (!file) return;
            const name = file.name.toLowerCase();
            if (name.endsWith(".glb") || name.endsWith(".gltf")) {
              file.arrayBuffer().then((buf) => {
                store.loadReferenceGlb(buf);
              });
            } else if (name.endsWith(".obj")) {
              file.text().then((text) => {
                store.loadReferenceObj(text);
              });
            } else {
              file.arrayBuffer().then((buf) => {
                store.loadReferenceMesh(buf);
              });
            }
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
        <button
          type="button"
          className="tool"
          title="Convert reference mesh to a CSG solid"
          data-testid="convert-to-solid"
          disabled={!state.referenceMesh}
          onClick={() => store.convertReferenceToSolid()}
        >
          ⊕ Solid
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
