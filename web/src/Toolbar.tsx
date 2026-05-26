/**
 * Top-of-viewport toolbar: add primitives, switch the next-add op,
 * undo/redo, delete selection. K6b.2 adds a Sketch entry button and
 * an alternate sketch-mode row shown while a sketch session is
 * active.
 *
 * The toolbar uses flex-wrap so items flow to a second row when the
 * window is narrow — no overflow dropdown needed.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import type { KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { buildDefaultDefinition } from "./widgets/schema";
import { FEATURE_TYPE_META } from "./widgets/FeatureDialog";

/* ------------------------------------------------------------------ */
/*  Primitive split-button constants                                   */
/* ------------------------------------------------------------------ */

function meta(kind: string) {
  return FEATURE_TYPE_META[kind] ?? { label: kind, glyph: "?" };
}

const PRIMITIVE_TOOLS: { kind: PrimitiveToolKind; glyph: string; label: string; title: string }[] = [
  { kind: "box",       ...meta("box"),       title: "Add Box" },
  { kind: "sphere",    ...meta("sphere"),    title: "Add Sphere" },
  { kind: "cylinder",  ...meta("cylinder"),  title: "Add Cylinder" },
  { kind: "hex_prism", ...meta("hex_prism"), title: "Add Hex Prism (apothem 25 mm)" },
  { kind: "ellipsoid", ...meta("ellipsoid"), title: "Add Ellipsoid (40×25×30 mm)" },
  { kind: "capsule",   ...meta("capsule"),   title: "Add Capsule (R20 × H80 mm)" },
  { kind: "cone",      ...meta("cone"),      title: "Add Cone (frustum R40/10 × H120 mm)" },
  { kind: "torus",     ...meta("torus"),     title: "Add Torus (R30 × r10 mm)" },
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

/* ------------------------------------------------------------------ */
/*  Transform split-button constants                                   */
/* ------------------------------------------------------------------ */

type GizmoMode = "translate" | "rotate" | "scale";

const TRANSFORM_TOOLS: { mode: GizmoMode; glyph: string; label: string; title: string }[] = [
  { mode: "translate", ...meta("translate"), title: "Move (G)" },
  { mode: "rotate",    ...meta("rotate"),    title: "Rotate (R)" },
  { mode: "scale",     ...meta("scale"),     title: "Scale" },
];

const SKETCH_TOOL_LABELS: Record<string, { label: string; glyph: string }> = {
  sketch_point:     meta("sketch_point"),
  sketch_line:      meta("sketch_line"),
  sketch_rectangle: meta("sketch_rectangle"),
  sketch_circle:    meta("sketch_circle"),
  sketch_polygon:   meta("sketch_polygon"),
  sketch_arc:       meta("sketch_arc"),
  sketch_trim:      meta("sketch_trim"),
  sketch_mirror:    meta("sketch_mirror"),
  sketch_offset:    meta("sketch_offset"),
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
/*  Main toolbar                                                       */
/* ------------------------------------------------------------------ */

interface ToolbarProps {
  store: KernelStore;
  state: StoreState;
}

export function Toolbar({ store, state }: ToolbarProps) {
  const fd = useMemo(() => createFdApi(store.kernel), [store]);
  const sketchSession = state.sketchSession;

  const [sketchToolTypes, setSketchToolTypes] = useState<readonly string[]>([]);
  useEffect(() => {
    try {
      setSketchToolTypes(fd.listSketchToolTypes());
    } catch {
      setSketchToolTypes([]);
    }
  }, [fd]);

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

  const hasSketch = state.features.some((f) => f.kind === "sketch");
  const hasFeatures = state.features.length > 0;

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

  /* ---------- Main toolbar (flex-wrap, no overflow dropdown) ---------- */

  return (
    <div className="fd-toolbar" data-testid="fielddraft-toolbar">
      {/* Modeling */}
      <div className="group">
        <PrimitiveSplitButton store={store} />
        <button type="button" className="tool" title="Add Hole (Ø6 × 40 mm, subtract)" data-testid="add-hole" onClick={() => store.addHole()} {...dragProps("hole")}>
          <span className="glyph" aria-hidden="true">{meta("hole").glyph}</span>
          <span className="tool-label">Hole</span>
        </button>
        <ThreadSplitButton onAdd={handleAddThread} />
        <button type="button" className="tool" title="Sketch on XY plane" data-testid="enter-sketch" onClick={handleSketchEnter} {...sketchDragProps()}>
          <span className="glyph" aria-hidden="true">{meta("sketch").glyph}</span>
          <span className="tool-label">Sketch</span>
        </button>
        <button type="button" className="tool" title="Extrude the most recent sketch (20 mm depth)" data-testid="extrude-latest-sketch" disabled={!hasSketch} onClick={() => store.addExtrudeFromLatestSketch(20)}>
          <span className="glyph" aria-hidden="true">{meta("extrude").glyph}</span>
          <span className="tool-label">Extrude</span>
        </button>
        <button type="button" className="tool" title="Revolve the most recent sketch around the world Y axis" data-testid="revolve-latest-sketch" disabled={!hasSketch} onClick={() => store.addRevolveFromLatestSketch()}>
          <span className="glyph" aria-hidden="true">{meta("revolve").glyph}</span>
          <span className="tool-label">Revolve</span>
        </button>
        <button type="button" className="tool" title="Add Gyroid infill" data-testid="add-gyroid" disabled={!hasFeatures} onClick={() => store.addGyroid()} {...dragProps("gyroid")}>
          <span className="glyph" aria-hidden="true">{meta("gyroid").glyph}</span>
          <span className="tool-label">Gyroid</span>
        </button>
        <button type="button" className="tool" title="Linear pattern (+X, 3×, 60 mm)" data-testid="linear-pattern-latest" disabled={!hasFeatures} onClick={() => store.addLinearPatternOfLatest()}>
          <span className="glyph" aria-hidden="true">{meta("linear_pattern").glyph}</span>
          <span className="tool-label">Pattern</span>
        </button>
        <button type="button" className="tool" title="Circular pattern around Y axis (6×, 360°)" data-testid="circular-pattern-latest" disabled={!hasFeatures} onClick={() => store.addCircularPatternOfLatest()}>
          <span className="glyph" aria-hidden="true">{meta("circular_pattern").glyph}</span>
          <span className="tool-label">Circ</span>
        </button>
        <button type="button" className="tool" title="Mirror through the world YZ plane" data-testid="mirror-latest" disabled={!hasFeatures} onClick={() => store.addMirrorOfLatest()}>
          <span className="glyph" aria-hidden="true">{meta("mirror").glyph}</span>
          <span className="tool-label">Mirror</span>
        </button>
        <button type="button" className="tool" title="Boolean (Union / Subtract / Intersect)" data-testid="add-boolean" disabled={!hasFeatures} onClick={() => store.addBoolean()}>
          <span className="glyph" aria-hidden="true">{meta("boolean").glyph}</span>
          <span className="tool-label">Boolean</span>
        </button>
        <button type="button" className="tool" title="Assign material (color, roughness, metalness)" data-testid="add-material" disabled={!hasFeatures} onClick={() => store.addMaterial()}>
          <span className="glyph" aria-hidden="true">{meta("material").glyph}</span>
          <span className="tool-label">Material</span>
        </button>
      </div>

      {/* Transform */}
      <div className="group">
        <TransformSplitButton store={store} gizmoMode={state.gizmoMode} />
      </div>

      {/* Tools */}
      <div className="group">
        <button type="button" className="tool" title="Delete selection (Del)" data-testid="delete-selection" disabled={state.selection.size === 0} onClick={() => store.removeSelected()}>
          <span className="glyph" aria-hidden="true">🗑</span>
          <span className="tool-label">Delete</span>
        </button>
        <button type="button" className={"tool" + (state.filletMode ? " is-active" : "")} title="Click an edge to fillet it" data-testid="fillet-toggle" onClick={() => store.toggleFilletMode()}>
          <span className="glyph" aria-hidden="true">{meta("fillet_edge").glyph}</span>
          <span className="tool-label">Fillet</span>
        </button>
        <button type="button" className={"tool" + (state.mateMode ? " is-active" : "")} title="Click a surface to place a mate connector" data-testid="add-mate-connector" disabled={!hasFeatures} onClick={() => store.toggleMateMode()}>
          <span className="glyph" aria-hidden="true">{meta("mate_connector").glyph}</span>
          <span className="tool-label">Mate</span>
        </button>
        <button type="button" className={"tool" + (state.measureMode ? " is-active" : "")} title="Measure distance (click two points)" data-testid="measure-toggle" onClick={() => store.toggleMeasureMode()}>
          <span className="glyph" aria-hidden="true">📏</span>
          <span className="tool-label">Measure</span>
        </button>
        {state.referenceMesh && (
          <button type="button" className="tool" title="Convert reference mesh to a CSG solid" data-testid="convert-to-solid" onClick={() => store.convertReferenceToSolid()}>
            <span className="glyph" aria-hidden="true">{meta("imported_mesh").glyph}</span>
            <span className="tool-label">Solid</span>
          </button>
        )}
      </div>
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
        <span className="glyph" aria-hidden="true">{meta("thread").glyph}</span>
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
                <span className="glyph" aria-hidden="true">{meta("thread").glyph}</span>
                <span className="label">{preset.label}</span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Transform split-button (Move / Rotate / Scale)                     */
/* ------------------------------------------------------------------ */

function TransformSplitButton({ store, gizmoMode }: { store: KernelStore; gizmoMode: GizmoMode }) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t?.closest(".transform-split-button")) setOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);

  useEffect(() => {
    if (!open || !wrapperRef.current) return;
    const rect = wrapperRef.current.getBoundingClientRect();
    setPos({ top: rect.bottom + 4, left: rect.left });
  }, [open]);

  const current = TRANSFORM_TOOLS.find((t) => t.mode === gizmoMode) ?? TRANSFORM_TOOLS[0];

  return (
    <div className="transform-split-button" data-testid="transform-split-button" ref={wrapperRef}>
      <button
        type="button"
        className="tool primary is-active"
        title={current.title}
        data-testid={`gizmo-${current.mode}`}
        onClick={() => store.setGizmoMode(current.mode)}
      >
        <span className="glyph" aria-hidden="true">{current.glyph}</span>
        <span className="tool-label">{current.label}</span>
      </button>
      <button
        type="button"
        className="tool chevron"
        title="Transform modes"
        data-testid="transform-split-button-chevron"
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
        <div className="transform-dropdown" role="menu" style={{ top: pos.top, left: pos.left }}>
          {TRANSFORM_TOOLS.map((tool) => {
            const active = tool.mode === gizmoMode;
            return (
              <button
                key={tool.mode}
                type="button"
                role="menuitem"
                className={"primitive-row" + (active ? " is-active" : "")}
                title={tool.title}
                data-testid={`transform-dropdown-${tool.mode}`}
                onClick={() => {
                  store.setGizmoMode(tool.mode);
                  setOpen(false);
                }}
              >
                <span className="glyph" aria-hidden="true">{tool.glyph}</span>
                <span className="label">{tool.label}</span>
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
