import { useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import type { KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { buildDefaultDefinition } from "./widgets/schema";
import { FEATURE_TYPE_META } from "./widgets/FeatureDialog";
import { buildDraftSketchState } from "./sketchSession";
import { applicableConstraints } from "./sketchConstraints";
import { countDegreesOfFreedom } from "./solver/dofPreflight";

/* ------------------------------------------------------------------ */
/*  Meta helper                                                        */
/* ------------------------------------------------------------------ */

function meta(kind: string) {
  return FEATURE_TYPE_META[kind] ?? { label: kind, glyph: "?" };
}

/** Shared HTML5 drag-start handler: stamps the payload onto the
 *  dataTransfer and marks the drag as a copy. */
function dragStartCopy(dragType: string | undefined, payload: string) {
  return (e: DragEvent) => {
    e.dataTransfer.setData(dragType ?? "fielddraft/feature", payload);
    e.dataTransfer.effectAllowed = "copy";
  };
}

/* ------------------------------------------------------------------ */
/*  FeatureButton — generic single-action toolbar button               */
/* ------------------------------------------------------------------ */

export function FeatureButton({
  kind,
  onClick,
  disabled,
  title,
  active,
  testId,
  draggable,
  dragType,
  label: labelOverride,
}: {
  kind: string;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  active?: boolean;
  testId?: string;
  draggable?: boolean;
  dragType?: string;
  label?: string;
}) {
  const m = meta(kind);
  return (
    <button
      type="button"
      className={"tool" + (active ? " is-active" : "")}
      title={title ?? m.label}
      data-testid={testId ?? `add-${kind.replace(/_/g, "-")}`}
      disabled={disabled}
      onClick={onClick}
      draggable={draggable}
      onDragStart={draggable ? dragStartCopy(dragType, kind) : undefined}
    >
      <span className="glyph" aria-hidden="true">{m.glyph}</span>
      <span className="tool-label">{labelOverride ?? m.label}</span>
    </button>
  );
}

/* ------------------------------------------------------------------ */
/*  FeatureDropdownButton — generic split-button with dropdown         */
/* ------------------------------------------------------------------ */

export interface DropdownItem {
  key: string;
  kind: string;
  label?: string;
  title?: string;
}

export function FeatureDropdownButton({
  items,
  activeKey,
  lastUsedStorageKey,
  defaultKey,
  onSelect,
  testId,
  draggable,
  dragType,
}: {
  items: DropdownItem[];
  activeKey?: string;
  lastUsedStorageKey?: string;
  defaultKey?: string;
  onSelect: (key: string) => void;
  testId?: string;
  draggable?: boolean;
  dragType?: string;
}) {
  const [lastUsed, setLastUsed] = useState<string>(() => {
    if (lastUsedStorageKey) {
      try {
        const raw = localStorage.getItem(lastUsedStorageKey);
        if (raw && items.some((it) => it.key === raw)) return raw;
      } catch { /* ignore */ }
    }
    return defaultKey ?? items[0]?.key ?? "";
  });

  const currentKey = activeKey ?? lastUsed;
  const current = items.find((it) => it.key === currentKey) ?? items[0];

  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (wrapperRef.current && !wrapperRef.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [open]);

  useEffect(() => {
    if (!open || !wrapperRef.current) return;
    const rect = wrapperRef.current.getBoundingClientRect();
    setPos({ top: rect.bottom + 4, left: rect.left });
  }, [open]);

  const invoke = (key: string) => {
    onSelect(key);
    if (lastUsedStorageKey && key !== lastUsed) {
      setLastUsed(key);
      try { localStorage.setItem(lastUsedStorageKey, key); } catch { /* ignore */ }
    }
  };

  if (!current) return null;
  const cm = meta(current.kind);

  return (
    <div className="split-button" data-testid={testId} ref={wrapperRef}>
      <button
        type="button"
        className={"tool primary" + (activeKey !== undefined ? " is-active" : "")}
        title={current.title ?? cm.label}
        draggable={draggable}
        onDragStart={draggable ? dragStartCopy(dragType, current.key) : undefined}
        onClick={() => invoke(current.key)}
      >
        <span className="glyph" aria-hidden="true">{cm.glyph}</span>
        <span className="tool-label">{current.label ?? cm.label}</span>
      </button>
      <button
        type="button"
        className="tool chevron"
        title="More options"
        aria-expanded={open}
        aria-haspopup="menu"
        data-testid={testId ? `${testId}-chevron` : undefined}
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        ▾
      </button>
      {open && pos && (
        <div className="split-dropdown" role="menu" style={{ top: pos.top, left: pos.left }}>
          {items.map((item) => {
            const im = meta(item.kind);
            const isActive = item.key === currentKey;
            return (
              <button
                key={item.key}
                type="button"
                role="menuitem"
                className={"split-row" + (isActive ? " is-active" : "")}
                title={item.title ?? im.label}
                draggable={draggable}
                onDragStart={draggable ? dragStartCopy(dragType, item.key) : undefined}
                onClick={() => {
                  invoke(item.key);
                  setOpen(false);
                }}
              >
                <span className="glyph" aria-hidden="true">{im.glyph}</span>
                <span className="label">{item.label ?? im.label}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ */
/*  Toolbar item descriptors                                           */
/* ------------------------------------------------------------------ */

type PrimitiveToolKind = "box" | "sphere" | "cylinder" | "hex_prism" | "ellipsoid" | "capsule" | "cone" | "torus";

const PRIMITIVE_ITEMS: DropdownItem[] = [
  { key: "box",       kind: "box",       title: "Add Box" },
  { key: "sphere",    kind: "sphere",    title: "Add Sphere" },
  { key: "cylinder",  kind: "cylinder",  title: "Add Cylinder" },
  { key: "hex_prism", kind: "hex_prism", title: "Add Hex Prism (apothem 25 mm)" },
  { key: "ellipsoid", kind: "ellipsoid", title: "Add Ellipsoid (40x25x30 mm)" },
  { key: "capsule",   kind: "capsule",   title: "Add Capsule (R20 x H80 mm)" },
  { key: "cone",      kind: "cone",      title: "Add Cone (frustum R40/10 x H120 mm)" },
  { key: "torus",     kind: "torus",     title: "Add Torus (R30 x r10 mm)" },
];

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

const THREAD_ITEMS: DropdownItem[] = THREAD_PRESETS.map((p) => ({
  key: p.key,
  kind: "thread",
  label: p.label,
  title: `${p.label}: Ø${p.radius * 2} mm, pitch ${p.pitch} mm`,
}));

type GizmoMode = "translate" | "rotate" | "scale";

const TRANSFORM_ITEMS: DropdownItem[] = [
  { key: "translate", kind: "translate", title: "Move (G)" },
  { key: "rotate",    kind: "rotate",    title: "Rotate (R)" },
  { key: "scale",     kind: "scale",     title: "Scale" },
];

// Allowlist of sketch tools the toolbar renders, regardless of what the
// kernel reports. FeatureButton resolves each one's label/glyph via meta().
const SKETCH_TOOL_KEYS = new Set<string>([
  "sketch_point",
  "sketch_line",
  "sketch_rectangle",
  "sketch_circle",
  "sketch_polygon",
  "sketch_regular_polygon",
  "sketch_arc",
  "sketch_mirror",
  "sketch_offset",
  // `sketch_trim` is not in this allowlist because it isn't a kernel tool
  // at all — it's an interactive editing mode rendered as a separate
  // button below the kernel-driven row (see the JSX).
]);

const DEFAULT_SKETCH_PLANE_QUERY = "qCreatedBy(origin_plane_xy)";

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

  const handleSketchToolPick = (typeName: string) => {
    try {
      const schema = fd.schemaForSketchToolType(typeName);
      store.selectSketchTool(typeName, buildDefaultDefinition(schema));
    } catch { /* ignore */ }
  };

  // P0.1: constraints applicable to the current select-mode pick.
  const sketchSelection = sketchSession.selection;
  const constraintOptions = useMemo(() => {
    if (!sketchSession.active || sketchSelection.length === 0) return [];
    try {
      return applicableConstraints(
        buildDraftSketchState(sketchSession.subCalls),
        sketchSelection,
      );
    } catch {
      return [];
    }
  }, [sketchSession.active, sketchSession.subCalls, sketchSelection]);

  // F5: live degrees-of-freedom tally for the sketch HUD. Rebuilds the
  // draft SketchState from the session sub-calls and runs the naive DoF
  // count; null on failure so the readout simply disappears.
  const dof = useMemo(() => {
    try {
      return countDegreesOfFreedom(buildDraftSketchState(sketchSession.subCalls));
    } catch {
      return null;
    }
  }, [sketchSession.subCalls]);

  const hasSketch = state.features.some((f) => f.kind === "sketch");
  const hasFeatures = state.features.length > 0;

  /* ---------- Sketch mode toolbar ---------- */

  if (sketchSession.active) {
    return (
      <div className="fd-toolbar is-sketch-mode" data-testid="fielddraft-toolbar">
        <div className="group">
          <span className="label" data-testid="sketch-mode-indicator">Sketch</span>
          <button type="button" className="tool is-primary" title="Finish sketch" data-testid="sketch-finish" onClick={() => store.commitSketch()}>
            ✓ Finish
          </button>
          <button type="button" className="tool" title="Cancel sketch" data-testid="sketch-cancel" onClick={() => store.exitSketch()}>
            ✗ Cancel
          </button>
          {dof !== null && (
            <span
              className={
                "dof" + (dof === 0 ? " is-ok" : dof < 0 ? " is-over" : "")
              }
              data-testid="sketch-dof"
              title="Remaining degrees of freedom"
            >
              {dof > 0
                ? `${dof} DoF`
                : dof === 0
                  ? "Fully constrained"
                  : `Over-constrained (${dof})`}
            </span>
          )}
        </div>

        <div className="group sketch-tool-row">
          {/* P0.1: the Select tool returns to select mode (no drawing
              tool active) so committed geometry can be picked and
              constrained. Without it the constraint palette is
              unreachable once any drawing tool is chosen. */}
          <button
            type="button"
            className={"tool" + (sketchSession.selectedTool === null ? " is-active" : "")}
            title="Select & constrain geometry (Esc)"
            data-testid="sketch-tool-select"
            onClick={() => store.selectSketchTool(null)}
          >
            <span className="glyph" aria-hidden="true">↖</span>
            <span className="tool-label">Select</span>
          </button>
          {sketchToolTypes.map((type) => {
            if (!SKETCH_TOOL_KEYS.has(type)) return null;
            return (
              <FeatureButton
                key={type}
                kind={type}
                active={sketchSession.selectedTool === type}
                testId={`sketch-tool-${type}`}
                onClick={() => handleSketchToolPick(type)}
              />
            );
          })}
          {/* F7: primitive-construction variants. These are UI-only tools
              (like trim) — they aren't kernel feature tools but emit
              ordinary sub-calls at commit time (sketch_rectangle /
              sketch_circle / sketch_line + perpendicular constraints), so
              they're hard-coded buttons here and the gestures are handled
              in SketchCanvas2d keyed on the selectedTool string. */}
          <FeatureButton
            kind="sketch_rectangle_center"
            active={sketchSession.selectedTool === "sketch_rectangle_center"}
            testId="sketch-tool-sketch_rectangle_center"
            onClick={() => store.selectSketchTool("sketch_rectangle_center", {})}
          />
          <FeatureButton
            kind="sketch_rectangle_3p"
            active={sketchSession.selectedTool === "sketch_rectangle_3p"}
            testId="sketch-tool-sketch_rectangle_3p"
            onClick={() => store.selectSketchTool("sketch_rectangle_3p", {})}
          />
          <FeatureButton
            kind="sketch_circle_3p"
            active={sketchSession.selectedTool === "sketch_circle_3p"}
            testId="sketch-tool-sketch_circle_3p"
            onClick={() => store.selectSketchTool("sketch_circle_3p", {})}
          />
          {/* Trim is an interactive editing mode, not a kernel feature
              tool, so it isn't in `sketchToolTypes`. Selecting it makes a
              canvas click cut the curve under the cursor at its nearest
              intersections (see store.trimSketchAt / sketchTrim.ts). */}
          <FeatureButton
            kind="sketch_trim"
            active={sketchSession.selectedTool === "sketch_trim"}
            testId="sketch-tool-sketch_trim"
            onClick={() => store.selectSketchTool("sketch_trim", {})}
          />
        </div>

        {!sketchSession.selectedTool ? (
          <div
            className="group sketch-constraint-row"
            data-testid="sketch-constraint-row"
          >
            {sketchSelection.length === 0 ? (
              <span className="hint">Select geometry to add a constraint</span>
            ) : constraintOptions.length === 0 ? (
              <span className="hint">No constraint fits this selection</span>
            ) : (
              constraintOptions.map((o) => (
                <button
                  key={o.type}
                  type="button"
                  className="tool"
                  title={o.title}
                  data-testid={`sketch-constraint-${o.type.replace(/^sketch_constraint_/, "")}`}
                  onClick={() => store.applySketchConstraint(o.type)}
                >
                  <span className="glyph" aria-hidden="true">{o.glyph}</span>
                  {o.label}
                </button>
              ))
            )}
          </div>
        ) : null}
      </div>
    );
  }

  /* ---------- Main toolbar ---------- */

  return (
    <div className="fd-toolbar" data-testid="fielddraft-toolbar">
      <div className="group">
        <FeatureDropdownButton
          items={PRIMITIVE_ITEMS}
          lastUsedStorageKey="fielddraft.lastUsedPrimitive"
          defaultKey="box"
          onSelect={(key) => store.addPrimitive(key as PrimitiveToolKind)}
          testId="primitive-split-button"
          draggable
          dragType="fielddraft/primitive"
        />
        <FeatureButton kind="hole" title="Add Hole (Ø6 x 40 mm, subtract)" onClick={() => store.addHole()} draggable dragType="fielddraft/primitive" />
        <FeatureDropdownButton
          items={THREAD_ITEMS}
          lastUsedStorageKey="fielddraft.lastUsedThread"
          defaultKey="M6"
          onSelect={(key) => {
            const p = THREAD_PRESETS.find((t) => t.key === key) ?? THREAD_PRESETS[3];
            store.addThreadPreset(p.radius, p.pitch, p.depth);
          }}
          testId="thread-split-button"
        />
        <FeatureButton kind="sketch" title="Sketch on XY plane" testId="enter-sketch" onClick={handleSketchEnter} draggable dragType="fielddraft/sketch" />
        <FeatureButton kind="extrude" title="Extrude latest sketch (20 mm)" testId="extrude-latest-sketch" disabled={!hasSketch} onClick={() => store.addExtrudeFromLatestSketch(20)} />
        <FeatureButton kind="revolve" title="Revolve latest sketch" testId="revolve-latest-sketch" disabled={!hasSketch} onClick={() => store.addRevolveFromLatestSketch()} />
        <FeatureButton kind="gyroid" title="Add Gyroid infill" disabled={!hasFeatures} onClick={() => store.addGyroid()} draggable dragType="fielddraft/primitive" />
        <FeatureButton kind="linear_pattern" title="Linear pattern (+X, 3x, 60 mm)" testId="linear-pattern-latest" disabled={!hasFeatures} onClick={() => store.addLinearPatternOfLatest()} label="Pattern" />
        <FeatureButton kind="circular_pattern" title="Circular pattern (6x, 360°)" testId="circular-pattern-latest" disabled={!hasFeatures} onClick={() => store.addCircularPatternOfLatest()} label="Circ" />
        <FeatureButton kind="mirror" title="Mirror through YZ plane" testId="mirror-latest" disabled={!hasFeatures} onClick={() => store.addMirrorOfLatest()} />
        <FeatureButton kind="boolean" title="Boolean (Union / Subtract / Intersect)" disabled={!hasFeatures} onClick={() => store.addBoolean()} />
        <FeatureButton kind="material" title="Assign material properties" disabled={!hasFeatures} onClick={() => store.addMaterial()} />
      </div>

      <div className="group">
        <FeatureDropdownButton
          items={TRANSFORM_ITEMS}
          activeKey={state.gizmoMode}
          onSelect={(key) => store.setGizmoMode(key as GizmoMode)}
          testId="transform-split-button"
        />
      </div>

      <div className="group">
        <button type="button" className="tool" title="Delete selection (Del)" data-testid="delete-selection" disabled={state.selection.size === 0} onClick={() => store.removeSelected()}>
          <span className="glyph" aria-hidden="true">🗑</span>
          <span className="tool-label">Delete</span>
        </button>
        <FeatureButton kind="fillet_edge" title="Click an edge to fillet it" testId="fillet-toggle" active={state.filletMode} onClick={() => store.toggleFilletMode()} label="Fillet" />
        <FeatureButton kind="mate_connector" title="Place a mate connector" testId="add-mate-connector" disabled={!hasFeatures} active={state.mateMode} onClick={() => store.toggleMateMode()} label="Mate" />
        <button type="button" className={"tool" + (state.measureMode ? " is-active" : "")} title="Measure distance (click two points)" data-testid="measure-toggle" onClick={() => store.toggleMeasureMode()}>
          <span className="glyph" aria-hidden="true">📏</span>
          <span className="tool-label">Measure</span>
        </button>
        {state.referenceMesh && (
          <FeatureButton kind="imported_mesh" title="Convert reference mesh to solid" testId="convert-to-solid" onClick={() => store.convertReferenceToSolid()} label="Solid" />
        )}
      </div>

      {/* FT-W: one button per saved FieldSkript — glyph = the script's
          icon, label = its name. Clicking instantiates the script into
          the part. The "Feature Script" bottom tab opens the IDE. */}
      {state.scriptTypes.length > 0 && (
        <div className="group">
          {state.scriptTypes.map((t) => (
            <button
              key={t.id}
              type="button"
              className="tool"
              title={`Insert ${t.name}`}
              data-testid={`add-script-${t.id}`}
              onClick={() => store.insertScriptInstance(t.id)}
            >
              <span className="glyph" aria-hidden="true">{t.icon}</span>
              <span className="tool-label">{t.name}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
