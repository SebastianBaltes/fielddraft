/**
 * Top-of-viewport toolbar: add primitives, switch the next-add op,
 * undo/redo, delete selection. K6b.2 adds a Sketch entry button and
 * an alternate sketch-mode row shown while a sketch session is
 * active.
 *
 * K7.21b: ResizeObserver-based overflow. When the toolbar is narrower
 * than its content, low-priority buttons collapse into a "··· More"
 * dropdown. Priority 0 items (Primitive split-button, Sketch, Extrude)
 * are always visible.
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { buildDefaultDefinition } from "./widgets/schema";
import { requestView } from "./viewCommands";

/* ------------------------------------------------------------------ */
/*  Primitive split-button constants                                   */
/* ------------------------------------------------------------------ */

const PRIMITIVE_TOOLS: { kind: PrimitiveToolKind; glyph: string; label: string; title: string }[] = [
  { kind: "box", glyph: "▣", label: "Box", title: "Add Box" },
  { kind: "sphere", glyph: "●", label: "Sphere", title: "Add Sphere" },
  { kind: "cylinder", glyph: "⬢", label: "Cylinder", title: "Add Cylinder" },
  { kind: "hex_prism", glyph: "⬡", label: "Hex Prism", title: "Add Hex Prism (apothem 25 mm)" },
  { kind: "ellipsoid", glyph: "⬬", label: "Ellipsoid", title: "Add Ellipsoid (40×25×30 mm)" },
  { kind: "capsule", glyph: "⬭", label: "Capsule", title: "Add Capsule (R20 × H80 mm)" },
  { kind: "cone", glyph: "△", label: "Cone", title: "Add Cone (frustum R40/10 × H120 mm)" },
  { kind: "torus", glyph: "◎", label: "Torus", title: "Add Torus (R30 × r10 mm)" },
];

type PrimitiveToolKind =
  | "box"
  | "sphere"
  | "cylinder"
  | "hex_prism"
  | "ellipsoid"
  | "capsule"
  | "cone"
  | "torus";

const PRIMITIVE_LAST_USED_KEY = "fielddraft.lastUsedPrimitive";

function loadLastUsedPrimitive(): PrimitiveToolKind {
  try {
    const raw = localStorage.getItem(PRIMITIVE_LAST_USED_KEY);
    if (raw && PRIMITIVE_TOOLS.some((t) => t.kind === raw)) {
      return raw as PrimitiveToolKind;
    }
  } catch {
    // ignore
  }
  return "box";
}

function savelastUsedPrimitive(kind: PrimitiveToolKind): void {
  try {
    localStorage.setItem(PRIMITIVE_LAST_USED_KEY, kind);
  } catch {
    // ignore
  }
}

/* ------------------------------------------------------------------ */
/*  Sketch constants                                                   */
/* ------------------------------------------------------------------ */

const DEFAULT_SKETCH_PLANE_QUERY = "qCreatedBy(origin_plane_xy)";

/* ------------------------------------------------------------------ */
/*  ISO metric thread presets (DIN 13 / ISO 261)                       */
/* ------------------------------------------------------------------ */

interface ThreadPreset {
  key: string;
  label: string;
  radius: number;
  pitch: number;
  depth: number;
}

const THREAD_PRESETS: ThreadPreset[] = [
  { key: "M3",  label: "M3",  radius: 1.5,  pitch: 0.5,  depth: 0.307 },
  { key: "M4",  label: "M4",  radius: 2.0,  pitch: 0.7,  depth: 0.429 },
  { key: "M5",  label: "M5",  radius: 2.5,  pitch: 0.8,  depth: 0.491 },
  { key: "M6",  label: "M6",  radius: 3.0,  pitch: 1.0,  depth: 0.613 },
  { key: "M8",  label: "M8",  radius: 4.0,  pitch: 1.25, depth: 0.767 },
  { key: "M10", label: "M10", radius: 5.0,  pitch: 1.5,  depth: 0.920 },
  { key: "M12", label: "M12", radius: 6.0,  pitch: 1.75, depth: 1.074 },
];

const THREAD_LAST_USED_KEY = "fielddraft.lastUsedThread";

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

/* ------------------------------------------------------------------ */
/*  Drag helpers                                                       */
/* ------------------------------------------------------------------ */

function dragProps(kind: string) {
  return {
    draggable: true,
    onDragStart: (e: React.DragEvent<HTMLButtonElement>) => {
      e.dataTransfer.setData("fielddraft/primitive", kind);
      e.dataTransfer.effectAllowed = "copy";
    },
  };
}

function sketchDragProps() {
  return {
    draggable: true,
    onDragStart: (e: React.DragEvent<HTMLButtonElement>) => {
      e.dataTransfer.setData("fielddraft/sketch", "true");
      e.dataTransfer.effectAllowed = "copy";
    },
  };
}

/* ------------------------------------------------------------------ */
/*  K7.21b: Overflow toolbar system                                    */
/* ------------------------------------------------------------------ */

interface ToolItem {
  id: string;
  priority: number;
  glyph: string;
  label: string;
  title: string;
  testId: string;
  disabled: boolean;
  active: boolean;
  onClick: () => void;
  drag?: Record<string, unknown>;
  group: number;
}

const ALWAYS_VISIBLE_PRIORITY = 0;

function ToolButton({ item }: { item: ToolItem }) {
  return (
    <button
      type="button"
      className={"tool" + (item.active ? " is-active" : "")}
      title={item.title}
      data-testid={item.testId}
      disabled={item.disabled}
      onClick={item.onClick}
      {...(item.drag ?? {})}
    >
      {item.glyph && <span className="glyph" aria-hidden="true">{item.glyph}</span>}
      <span className="tool-label">{item.label}</span>
    </button>
  );
}

function MoreDropdown({ items }: { items: ToolItem[] }) {
  const [open, setOpen] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState<{ top: number; right: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t?.closest(".more-dropdown-wrapper")) setOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);

  useEffect(() => {
    if (!open || !btnRef.current) return;
    const rect = btnRef.current.getBoundingClientRect();
    setPos({ top: rect.bottom + 4, right: window.innerWidth - rect.right });
  }, [open]);

  if (items.length === 0) return null;

  return (
    <div className="more-dropdown-wrapper">
      <button
        ref={btnRef}
        type="button"
        className="tool"
        title="More tools"
        data-testid="toolbar-more"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        ··· More
      </button>
      {open && pos && (
        <div
          className="more-dropdown"
          role="menu"
          style={{ top: pos.top, right: pos.right }}
        >
          {items.map((item) => (
            <button
              key={item.id}
              type="button"
              role="menuitem"
              className={"more-row" + (item.active ? " is-active" : "")}
              title={item.title}
              data-testid={item.testId}
              disabled={item.disabled}
              onClick={() => {
                item.onClick();
                setOpen(false);
              }}
            >
              <span className="glyph" aria-hidden="true">
                {item.glyph}
              </span>
              <span className="label">{item.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Main toolbar                                                       */
/* ------------------------------------------------------------------ */

interface ToolbarProps {
  store: KernelStore;
  state: StoreState;
}

export function Toolbar({ store, state }: ToolbarProps) {
  const fd = useMemo(() => createFdApi(store.kernel), [store]);
  const sketchSession = state.sketchSession;
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [hideCount, setHideCount] = useState(0);
  const prevWidth = useRef(0);

  const [sketchToolTypes, setSketchToolTypes] = useState<readonly string[]>([]);
  useEffect(() => {
    try {
      setSketchToolTypes(fd.listSketchToolTypes());
    } catch {
      setSketchToolTypes([]);
    }
  }, [fd]);

  useEffect(() => {
    const el = toolbarRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => {
      const w = entry.contentRect.width;
      if (w > prevWidth.current) {
        setHideCount(0);
      }
      prevWidth.current = w;
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useLayoutEffect(() => {
    const el = toolbarRef.current;
    if (!el) return;
    if (el.scrollWidth > el.clientWidth + 1) {
      setHideCount((c) => c + 1);
    }
  });

  const handleSketchEnter = () => {
    if (state.features.length > 0) {
      store.enterFacePickMode();
    } else {
      store.enterSketch(DEFAULT_SKETCH_PLANE_QUERY);
    }
  };

  const handleAddThread = (preset?: ThreadPreset) => {
    const p = preset ?? THREAD_PRESETS.find((t) => t.key === "M6")!;
    store.addThreadPreset(p.radius, p.pitch, p.depth);
  };

  const handleSketchToolPick = (typeName: string) => {
    try {
      const schema = fd.schemaForSketchToolType(typeName);
      store.selectSketchTool(typeName, buildDefaultDefinition(schema));
    } catch {
      // ignore
    }
  };

  /* ---------- Items + overflow (must be before early returns to keep hook order) ---------- */

  const hasSketch = state.features.some((f) => f.kind === "sketch");
  const hasFeatures = state.features.length > 0;

  const items: ToolItem[] = [
    // Group 0: Modeling
    { id: "thread", priority: 2, group: 0, glyph: "⏍", label: "Thread", title: "Add Thread (M6 default)", testId: "add-thread", disabled: false, active: false, onClick: () => handleAddThread() },
    { id: "hole", priority: 2, group: 0, glyph: "⊖", label: "Hole", title: "Add Hole (Ø6 × 40 mm, subtract)", testId: "add-hole", disabled: false, active: false, onClick: () => store.addHole(), drag: dragProps("hole") },
    { id: "gyroid", priority: 3, group: 0, glyph: "🕸", label: "Gyroid", title: "Add Gyroid infill (10 mm cells, 1 mm walls, intersect)", testId: "add-gyroid", disabled: !hasFeatures, active: false, onClick: () => store.addGyroid(), drag: dragProps("gyroid") },
    { id: "sketch", priority: 0, group: 0, glyph: "✎", label: "Sketch", title: "Sketch on XY plane (plane editable via feature dialog)", testId: "enter-sketch", disabled: false, active: false, onClick: handleSketchEnter, drag: sketchDragProps() },
    { id: "extrude", priority: 0, group: 0, glyph: "↥", label: "Extrude", title: "Extrude the most recent sketch (20 mm depth)", testId: "extrude-latest-sketch", disabled: !hasSketch, active: false, onClick: () => store.addExtrudeFromLatestSketch(20) },
    { id: "revolve", priority: 2, group: 0, glyph: "↻", label: "Revolve", title: "Revolve the most recent sketch around the world Y axis", testId: "revolve-latest-sketch", disabled: !hasSketch, active: false, onClick: () => store.addRevolveFromLatestSketch() },
    { id: "linear-pattern", priority: 3, group: 0, glyph: "⎡⎡⎡", label: "Pattern", title: "Linear pattern the latest feature (+X, 3×, 60 mm)", testId: "linear-pattern-latest", disabled: !hasFeatures, active: false, onClick: () => store.addLinearPatternOfLatest() },
    { id: "circular-pattern", priority: 3, group: 0, glyph: "↻", label: "Circ", title: "Circular pattern the latest feature around Y axis (6×, 360°)", testId: "circular-pattern-latest", disabled: !hasFeatures, active: false, onClick: () => store.addCircularPatternOfLatest() },
    { id: "mirror", priority: 3, group: 0, glyph: "◫", label: "Mirror", title: "Mirror the latest feature through the world YZ plane", testId: "mirror-latest", disabled: !hasFeatures, active: false, onClick: () => store.addMirrorOfLatest() },
    // Group 1: Tools
    { id: "delete", priority: 1, group: 1, glyph: "", label: "Delete", title: "Delete selection (Del)", testId: "delete-selection", disabled: state.selection.size === 0, active: false, onClick: () => store.removeSelected() },
    { id: "mate", priority: 3, group: 1, glyph: "⊕", label: "Mate", title: "Click a surface to place a mate connector", testId: "add-mate-connector", disabled: !hasFeatures, active: state.mateMode, onClick: () => store.toggleMateMode() },
    { id: "fillet", priority: 2, group: 1, glyph: "◠", label: "Fillet", title: "Click an edge to fillet it", testId: "fillet-toggle", disabled: false, active: state.filletMode, onClick: () => store.toggleFilletMode() },
    { id: "measure", priority: 3, group: 1, glyph: "📏", label: "Measure", title: "Measure distance (click two points)", testId: "measure-toggle", disabled: false, active: state.measureMode, onClick: () => store.toggleMeasureMode() },
    { id: "undo", priority: 1, group: 1, glyph: "⟲", label: `Undo${state.undoDepth > 0 ? ` (${state.undoDepth})` : ""}`, title: "Undo (Ctrl+Z)", testId: "undo", disabled: !state.canUndo, active: false, onClick: () => store.undo() },
    { id: "redo", priority: 1, group: 1, glyph: "⟳", label: `Redo${state.redoDepth > 0 ? ` (${state.redoDepth})` : ""}`, title: "Redo (Ctrl+Shift+Z)", testId: "redo", disabled: !state.canRedo, active: false, onClick: () => store.redo() },
    { id: "export-stl", priority: 4, group: 1, glyph: "⇩", label: "STL", title: "Export as binary STL", testId: "export-stl", disabled: !hasFeatures, active: false, onClick: () => store.downloadStl() },
    { id: "export-glb", priority: 4, group: 1, glyph: "⇩", label: "GLB", title: "Export as binary GLB (glTF 2.0)", testId: "export-glb", disabled: !hasFeatures, active: false, onClick: () => store.downloadGlb() },
    { id: "export-obj", priority: 4, group: 1, glyph: "⇩", label: "OBJ", title: "Export as Wavefront OBJ", testId: "export-obj", disabled: !hasFeatures, active: false, onClick: () => store.downloadObj() },
    { id: "export-3mf", priority: 4, group: 1, glyph: "⇩", label: "3MF", title: "Export as 3MF (3D Manufacturing Format, preserves slicer settings if imported)", testId: "export-3mf", disabled: !hasFeatures, active: false, onClick: () => store.download3mfRoundtrip() },
    { id: "export-step", priority: 5, group: 1, glyph: "⇩", label: "STEP", title: "Export mechanical core as STEP (via OpenCASCADE)", testId: "export-step", disabled: !hasFeatures, active: false, onClick: () => store.downloadStep() },
    // Group 2: File
    { id: "save", priority: 4, group: 2, glyph: "💾", label: "Save", title: "Save project as .fdraft file", testId: "save-project", disabled: false, active: false, onClick: () => store.saveProjectToFile() },
    { id: "open", priority: 4, group: 2, glyph: "📂", label: "Open", title: "Open a .fdraft project file", testId: "load-project", disabled: false, active: false, onClick: () => document.querySelector<HTMLInputElement>('[data-testid="load-project-input"]')?.click() },
    { id: "new", priority: 4, group: 2, glyph: "📄", label: "New", title: "New empty project", testId: "new-project", disabled: false, active: false, onClick: () => { if (state.features.length === 0 || confirm("Discard current project?")) store.newProject(); } },
    { id: "import", priority: 3, group: 2, glyph: "📁", label: "Import", title: "Import STL/OBJ/GLB/STEP file", testId: "import-stl", disabled: false, active: false, onClick: () => document.querySelector<HTMLInputElement>('[data-testid="import-stl-input"]')?.click() },
    { id: "convert-solid", priority: 5, group: 2, glyph: "⊕", label: "Solid", title: "Convert reference mesh to a CSG solid", testId: "convert-to-solid", disabled: !state.referenceMesh, active: false, onClick: () => store.convertReferenceToSolid() },
    { id: "assembly", priority: 4, group: 2, glyph: "🔗", label: "Assembly", title: "Enter assembly mode (multi-part scene)", testId: "enter-assembly", disabled: false, active: state.assembly.active, onClick: () => { if (state.assembly.active) store.exitAssembly(); else store.enterAssembly(); } },
    { id: "share", priority: 4, group: 2, glyph: "🔗", label: "Share", title: "Publish and get a shareable link", testId: "share-project", disabled: !hasFeatures || !state.projectId, active: false, onClick: () => store.publishProject() },
    { id: "export-r3f", priority: 5, group: 2, glyph: "⚛", label: "R3F", title: "Export assembly as React Three Fiber component", testId: "export-r3f", disabled: !state.assembly.active || state.assembly.document.parts.length === 0, active: false, onClick: () => store.exportR3F() },
    // Group 3: View
    { id: "view-front", priority: 5, group: 3, glyph: "◱", label: "Front", title: "Front view", testId: "view-front", disabled: false, active: false, onClick: () => requestView("front") },
    { id: "view-top", priority: 5, group: 3, glyph: "◳", label: "Top", title: "Top view", testId: "view-top", disabled: false, active: false, onClick: () => requestView("top") },
    { id: "view-right", priority: 5, group: 3, glyph: "◲", label: "Right", title: "Right view", testId: "view-right", disabled: false, active: false, onClick: () => requestView("right") },
    { id: "view-iso", priority: 5, group: 3, glyph: "◰", label: "Iso", title: "Isometric view", testId: "view-iso", disabled: false, active: false, onClick: () => requestView("iso") },
  ];

  const hiddenIds = useMemo(() => {
    const collapsible = items
      .filter((i) => i.priority > ALWAYS_VISIBLE_PRIORITY)
      .sort((a, b) => b.priority - a.priority || items.indexOf(b) - items.indexOf(a));
    const ids = new Set<string>();
    for (let i = 0; i < hideCount && i < collapsible.length; i++) {
      ids.add(collapsible[i].id);
    }
    return ids;
  }, [items, hideCount]);

  const visible = items.filter((i) => !hiddenIds.has(i.id));
  const overflow = items.filter((i) => hiddenIds.has(i.id));

  /* ---------- Sketch mode toolbar ---------- */

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
                <span className="glyph" aria-hidden="true">{label.glyph}</span>
                <span className="tool-label">{label.label}</span>
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
              <span className="glyph" aria-hidden="true">{stub.glyph}</span>
              <span className="tool-label">{stub.label}</span>
            </button>
          ))}
        </div>
      </div>
    );
  }

  /* ---------- Main toolbar with overflow ---------- */

  const groups = [0, 1, 2, 3] as const;

  return (
    <div className="fd-toolbar" data-testid="fielddraft-toolbar" ref={toolbarRef}>
      {/* Group 0 always starts with PrimitiveSplitButton */}
      <div className="group">
        <PrimitiveSplitButton store={store} />
        {visible
          .filter((i) => i.group === 0 && i.id !== "thread")
          .map((item) => (
            <ToolButton key={item.id} item={item} />
          ))}
        {visible.some((i) => i.id === "thread") && (
          <ThreadSplitButton onAdd={handleAddThread} />
        )}
      </div>

      {/* Remaining groups render only when they have visible items */}
      {groups.slice(1).map((g) => {
        const groupVisible = visible.filter((i) => i.group === g);
        if (groupVisible.length === 0) return null;
        return (
          <div className="group" key={g}>
            {g === 3 && <span className="label">View:</span>}
            {groupVisible.map((item) => (
              <ToolButton key={item.id} item={item} />
            ))}
          </div>
        );
      })}

      {/* More dropdown for overflow items */}
      {overflow.length > 0 && (
        <div className="group">
          <MoreDropdown items={overflow} />
        </div>
      )}

      {/* Hidden file inputs (always in DOM for querySelector refs) */}
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
      <input
        type="file"
        accept=".stl,.glb,.gltf,.obj,.step,.stp,.3mf"
        className="hidden-file-input"
        data-testid="import-stl-input"
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          if (!file) return;
          const name = file.name.toLowerCase();
          if (name.endsWith(".3mf")) {
            file.arrayBuffer().then((buf) => {
              store.import3mfFile(buf);
            });
          } else if (name.endsWith(".step") || name.endsWith(".stp")) {
            file.arrayBuffer().then((buf) => {
              store.importStepFile(buf);
            });
          } else if (name.endsWith(".glb") || name.endsWith(".gltf")) {
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
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Primitive split-button                                             */
/* ------------------------------------------------------------------ */

function PrimitiveSplitButton({ store }: { store: KernelStore }) {
  const [lastUsed, setLastUsed] = useState<PrimitiveToolKind>(() =>
    loadLastUsedPrimitive(),
  );
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t?.closest(".primitive-split-button")) setOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);

  useEffect(() => {
    if (!open || !wrapperRef.current) return;
    const rect = wrapperRef.current.getBoundingClientRect();
    setPos({ top: rect.bottom + 4, left: rect.left });
  }, [open]);

  const current =
    PRIMITIVE_TOOLS.find((t) => t.kind === lastUsed) ?? PRIMITIVE_TOOLS[0];

  const invoke = (kind: PrimitiveToolKind) => {
    store.addPrimitive(kind);
    if (kind !== lastUsed) {
      setLastUsed(kind);
      savelastUsedPrimitive(kind);
    }
  };

  return (
    <div className="primitive-split-button" data-testid="primitive-split-button" ref={wrapperRef}>
      <button
        type="button"
        className="tool primary"
        title={current.title}
        data-testid={`add-${current.kind.replace("_", "-")}`}
        {...dragProps(current.kind)}
        onClick={() => invoke(current.kind)}
      >
        <span className="glyph" aria-hidden="true">{current.glyph}</span>
        <span className="tool-label">{current.label}</span>
      </button>
      <button
        type="button"
        className="tool chevron"
        title="Other primitives"
        data-testid="primitive-split-button-chevron"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        ▾
      </button>
      {open && pos ? (
        <div className="primitive-dropdown" role="menu" style={{ top: pos.top, left: pos.left }}>
          {PRIMITIVE_TOOLS.map((tool) => {
            const active = tool.kind === lastUsed;
            return (
              <button
                key={tool.kind}
                type="button"
                role="menuitem"
                className={"primitive-row" + (active ? " is-active" : "")}
                title={tool.title}
                data-testid={`primitive-dropdown-${tool.kind.replace("_", "-")}`}
                {...dragProps(tool.kind)}
                onClick={() => {
                  invoke(tool.kind);
                  setOpen(false);
                }}
              >
                <span className="glyph" aria-hidden="true">
                  {tool.glyph}
                </span>
                <span className="label">{tool.label}</span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Thread preset split-button (K12.2)                                 */
/* ------------------------------------------------------------------ */

function ThreadSplitButton({ onAdd }: { onAdd: (preset: ThreadPreset) => void }) {
  const [lastUsed, setLastUsed] = useState<string>(() => {
    try {
      const raw = localStorage.getItem(THREAD_LAST_USED_KEY);
      if (raw && THREAD_PRESETS.some((t) => t.key === raw)) return raw;
    } catch { /* ignore */ }
    return "M6";
  });
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t?.closest(".thread-split-button")) setOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);

  useEffect(() => {
    if (!open || !wrapperRef.current) return;
    const rect = wrapperRef.current.getBoundingClientRect();
    setPos({ top: rect.bottom + 4, left: rect.left });
  }, [open]);

  const current = THREAD_PRESETS.find((t) => t.key === lastUsed) ?? THREAD_PRESETS[3];

  const invoke = (preset: ThreadPreset) => {
    onAdd(preset);
    if (preset.key !== lastUsed) {
      setLastUsed(preset.key);
      try { localStorage.setItem(THREAD_LAST_USED_KEY, preset.key); } catch { /* ignore */ }
    }
  };

  return (
    <div className="thread-split-button" data-testid="thread-split-button" ref={wrapperRef}>
      <button
        type="button"
        className="tool primary"
        title={`Add Thread (${current.label})`}
        data-testid="add-thread"
        onClick={() => invoke(current)}
      >
        <span className="glyph" aria-hidden="true">⏍</span>
        <span className="tool-label">{current.label}</span>
      </button>
      <button
        type="button"
        className="tool chevron"
        title="Other thread sizes"
        data-testid="thread-split-button-chevron"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        ▾
      </button>
      {open && pos ? (
        <div className="thread-dropdown" role="menu" style={{ top: pos.top, left: pos.left }}>
          {THREAD_PRESETS.map((preset) => {
            const active = preset.key === lastUsed;
            return (
              <button
                key={preset.key}
                type="button"
                role="menuitem"
                className={"primitive-row" + (active ? " is-active" : "")}
                title={`${preset.label}: Ø${preset.radius * 2} mm, pitch ${preset.pitch} mm`}
                data-testid={`thread-dropdown-${preset.key.toLowerCase()}`}
                onClick={() => {
                  invoke(preset);
                  setOpen(false);
                }}
              >
                <span className="glyph" aria-hidden="true">⏍</span>
                <span className="label">{preset.label}</span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
