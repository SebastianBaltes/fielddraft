import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
} from "react";
import type { KernelStore, StoreState } from "./store";
import { createFdApi } from "./api/fd";
import { buildDefaultDefinition } from "./widgets/schema";
import { FEATURE_TYPE_META } from "./widgets/FeatureDialog";
import { elementQueryString } from "./elementQuery";
import { DATUM_PLANE_QUERIES } from "./queryLabels";
import { buildDraftSketchState } from "./sketchSession";
import {
  applicableConstraints,
  CONSTRAINT_DIMENSION_TOOL,
  DIMENSION_TYPES_BY_SPECIFICITY,
} from "./sketchConstraints";
import { displayDegreesOfFreedom } from "./solver/dofPreflight";
import { useMediaQuery, MOBILE_QUERY } from "./useMediaQuery";

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
      <span className="glyph" aria-hidden="true">
        {m.glyph}
      </span>
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
  /** Optional stable test id stamped on the menu row. Sketch-tool groups
   *  use this so existing e2e selectors keep finding the variant buttons
   *  even though they now live inside the dropdown menu. */
  testId?: string;
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
      } catch {
        /* ignore */
      }
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
      if (wrapperRef.current && !wrapperRef.current.contains(e.target as Node))
        setOpen(false);
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
      try {
        localStorage.setItem(lastUsedStorageKey, key);
      } catch {
        /* ignore */
      }
    }
  };

  if (!current) return null;
  const cm = meta(current.kind);

  // When any item carries a test id (the sketch-tool groups) the menu stays
  // mounted even while closed, parked off-screen via `is-collapsed`, so those
  // ids remain directly reachable. Other dropdowns keep the open-only mount.
  const keepMounted = items.some((it) => it.testId);
  const showMenu = open ? pos !== null : keepMounted;

  return (
    <div className="split-button" data-testid={testId} ref={wrapperRef}>
      <button
        type="button"
        className={
          "tool primary" + (activeKey !== undefined ? " is-active" : "")
        }
        title={current.title ?? cm.label}
        draggable={draggable}
        onDragStart={
          draggable ? dragStartCopy(dragType, current.key) : undefined
        }
        onClick={() => invoke(current.key)}
      >
        <span className="glyph" aria-hidden="true">
          {cm.glyph}
        </span>
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
      {showMenu && (
        <div
          className={"split-dropdown" + (open && pos ? "" : " is-collapsed")}
          role="menu"
          style={open && pos ? { top: pos.top, left: pos.left } : undefined}
        >
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
                data-testid={item.testId}
                draggable={draggable}
                onDragStart={
                  draggable ? dragStartCopy(dragType, item.key) : undefined
                }
                onClick={() => {
                  invoke(item.key);
                  setOpen(false);
                }}
              >
                <span className="glyph" aria-hidden="true">
                  {im.glyph}
                </span>
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

type PrimitiveToolKind =
  | "box"
  | "sphere"
  | "cylinder"
  | "hex_prism"
  | "ellipsoid"
  | "capsule"
  | "cone"
  | "torus";

const PRIMITIVE_ITEMS: DropdownItem[] = [
  { key: "box", kind: "box", title: "Add Box" },
  { key: "sphere", kind: "sphere", title: "Add Sphere" },
  { key: "cylinder", kind: "cylinder", title: "Add Cylinder" },
  {
    key: "hex_prism",
    kind: "hex_prism",
    title: "Add Hex Prism (apothem 25 mm)",
  },
  { key: "ellipsoid", kind: "ellipsoid", title: "Add Ellipsoid (40x25x30 mm)" },
  { key: "capsule", kind: "capsule", title: "Add Capsule (R20 x H80 mm)" },
  { key: "cone", kind: "cone", title: "Add Cone (frustum R40/10 x H120 mm)" },
  { key: "torus", kind: "torus", title: "Add Torus (R30 x r10 mm)" },
];

interface ThreadPreset {
  key: string;
  label: string;
  radius: number;
  pitch: number;
  depth: number;
}

const THREAD_PRESETS: ThreadPreset[] = [
  { key: "M3", label: "M3", radius: 1.5, pitch: 0.5, depth: 0.307 },
  { key: "M4", label: "M4", radius: 2.0, pitch: 0.7, depth: 0.429 },
  { key: "M5", label: "M5", radius: 2.5, pitch: 0.8, depth: 0.491 },
  { key: "M6", label: "M6", radius: 3.0, pitch: 1.0, depth: 0.613 },
  { key: "M8", label: "M8", radius: 4.0, pitch: 1.25, depth: 0.767 },
  { key: "M10", label: "M10", radius: 5.0, pitch: 1.5, depth: 0.92 },
  { key: "M12", label: "M12", radius: 6.0, pitch: 1.75, depth: 1.074 },
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
  { key: "rotate", kind: "rotate", title: "Rotate (R)" },
  { key: "scale", kind: "scale", title: "Scale" },
];

// Allowlist of sketch tools the toolbar renders, regardless of what the
// kernel reports. FeatureButton resolves each one's label/glyph via meta().
const SKETCH_TOOL_KEYS = new Set<string>([
  "sketch_point",
  "sketch_line",
  "sketch_rectangle",
  "sketch_circle",
  "sketch_ellipse",
  "sketch_slot",
  "sketch_polygon",
  "sketch_regular_polygon",
  "sketch_spline",
  "sketch_arc",
  // `sketch_trim` (F14 `sketch_mirror`, F15 `sketch_offset`) are not in
  // this allowlist because they aren't kernel tools at all — they're
  // interactive editing modes rendered as separate buttons below the
  // kernel-driven row (see the JSX). The kernel `sketch_mirror` and
  // `sketch_offset` whole-profile stubs are no longer surfaced; the
  // interactive Mirror / Offset replace them.
]);

// Sketch-tool dropdown groups (ticket pqoe). Each variant keeps the test id
// it had as a standalone button so existing e2e selectors still resolve it
// (the menu stays mounted while closed — see FeatureDropdownButton). The
// dropdown `key` is the tool kind; the toolbar's onSelect dispatches it
// through the right activation path (kernel tool vs. UI-only variant).
const SKETCH_CIRCLE_ITEMS: DropdownItem[] = [
  {
    key: "sketch_circle",
    kind: "sketch_circle",
    testId: "sketch-tool-sketch_circle",
  },
  {
    key: "sketch_circle_3p",
    kind: "sketch_circle_3p",
    testId: "sketch-tool-sketch_circle_3p",
  },
  {
    key: "sketch_ellipse",
    kind: "sketch_ellipse",
    testId: "sketch-tool-sketch_ellipse",
  },
];

const SKETCH_RECTANGLE_ITEMS: DropdownItem[] = [
  {
    key: "sketch_rectangle",
    kind: "sketch_rectangle",
    testId: "sketch-tool-sketch_rectangle",
  },
  {
    key: "sketch_rectangle_center",
    kind: "sketch_rectangle_center",
    testId: "sketch-tool-sketch_rectangle_center",
  },
  {
    key: "sketch_rectangle_3p",
    kind: "sketch_rectangle_3p",
    testId: "sketch-tool-sketch_rectangle_3p",
  },
];

// Variants drawn through dropdowns are pulled out of the flat kernel-driven
// row so they aren't rendered twice.
const SKETCH_GROUPED_KEYS = new Set<string>([
  "sketch_circle",
  "sketch_rectangle",
  "sketch_ellipse",
]);

const DEFAULT_SKETCH_PLANE_QUERY = "qCreatedBy(origin_plane_xy)";

/* ------------------------------------------------------------------ */
/*  Sketch-constraint icons (ticket vk8q)                              */
/* ------------------------------------------------------------------ */

// vk8q: Onshape-style monochrome SVG glyphs for the constraint toolbar,
// replacing the old text-symbol `glyph` characters. Each is a crisp 16×16
// `currentColor` line drawing so it inherits the button's text colour and
// reads at the small toolbar size. Keyed by the resolved palette KEY (the
// `sketch_constraint_*` suffix, plus the synthetic `dimension`), so the
// row, the unified Dimension button, and the unsupported placeholders all
// pull from one map. `stroke-linecap="round"` keeps the strokes clean.
const ICON_PROPS = {
  viewBox: "0 0 16 16",
  width: 16,
  height: 16,
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.4,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
};

const CONSTRAINT_ICONS: Record<string, ReactElement> = {
  // Two dots merging into one — coincident points.
  coincident: (
    <svg {...ICON_PROPS}>
      <circle cx="6" cy="8" r="2.4" />
      <circle cx="10" cy="8" r="2.4" />
    </svg>
  ),
  // Concentric rings about a common centre.
  concentric: (
    <svg {...ICON_PROPS}>
      <circle cx="8" cy="8" r="5.5" />
      <circle cx="8" cy="8" r="2" />
    </svg>
  ),
  // Two parallel slashes.
  parallel: (
    <svg {...ICON_PROPS}>
      <line x1="5" y1="13" x2="9" y2="3" />
      <line x1="9" y1="13" x2="13" y2="3" />
    </svg>
  ),
  // A right angle — ⊥.
  perpendicular: (
    <svg {...ICON_PROPS}>
      <line x1="4" y1="3" x2="4" y2="12" />
      <line x1="4" y1="12" x2="13" y2="12" />
      <path d="M4 9 L7 9 L7 12" />
    </svg>
  ),
  // A straight line just touching an arc — tangent.
  tangent: (
    <svg {...ICON_PROPS}>
      <path d="M3 5 A4 4 0 0 0 13 5" />
      <line x1="2" y1="11" x2="14" y2="11" />
    </svg>
  ),
  // Equals sign.
  equal: (
    <svg {...ICON_PROPS}>
      <line x1="4" y1="6" x2="12" y2="6" />
      <line x1="4" y1="10" x2="12" y2="10" />
    </svg>
  ),
  // A horizontal bar.
  horizontal: (
    <svg {...ICON_PROPS}>
      <line x1="3" y1="8" x2="13" y2="8" />
    </svg>
  ),
  // A vertical bar.
  vertical: (
    <svg {...ICON_PROPS}>
      <line x1="8" y1="3" x2="8" y2="13" />
    </svg>
  ),
  // A dot at the middle of a segment — midpoint.
  midpoint: (
    <svg {...ICON_PROPS}>
      <line x1="2" y1="8" x2="14" y2="8" />
      <circle cx="8" cy="8" r="1.6" fill="currentColor" stroke="none" />
    </svg>
  ),
  // A point sitting on a curve — on-curve / point-on-object.
  point_on_object: (
    <svg {...ICON_PROPS}>
      <path d="M2 11 Q8 1 14 11" />
      <circle cx="8" cy="4.1" r="1.6" fill="currentColor" stroke="none" />
    </svg>
  ),
  // Two dots mirrored about a dashed axis — symmetric.
  symmetric: (
    <svg {...ICON_PROPS}>
      <line x1="8" y1="2" x2="8" y2="14" strokeDasharray="2 2" />
      <circle cx="4" cy="8" r="1.6" fill="currentColor" stroke="none" />
      <circle cx="12" cy="8" r="1.6" fill="currentColor" stroke="none" />
    </svg>
  ),
  // A padlock — fix.
  fix: (
    <svg {...ICON_PROPS}>
      <rect x="4" y="7" width="8" height="6" rx="1" />
      <path d="M6 7 V5 a2 2 0 0 1 4 0 V7" />
    </svg>
  ),
  // A double-headed arrow with end ticks — a single Dimension tool that
  // resolves to distance / length / angle / radius / diameter.
  dimension: (
    <svg {...ICON_PROPS}>
      <line x1="3" y1="4" x2="3" y2="12" />
      <line x1="13" y1="4" x2="13" y2="12" />
      <line x1="3" y1="8" x2="13" y2="8" />
      <path d="M5 6 L3 8 L5 10" />
      <path d="M11 6 L13 8 L11 10" />
    </svg>
  ),
  // The three unsupported Onshape constraints (see vk8q investigation):
  // rendered DISABLED, so a recognisable glyph is still helpful.
  // Normal — a line meeting a curve at a right-angle tick.
  normal: (
    <svg {...ICON_PROPS}>
      <path d="M2 12 Q8 2 14 12" />
      <line x1="8" y1="4" x2="8" y2="13" />
      <path d="M6 11 L6 13 L8 13" />
    </svg>
  ),
  // Pierce — a point at a crossing, an x-marks-the-spot.
  pierce: (
    <svg {...ICON_PROPS}>
      <line x1="3" y1="3" x2="13" y2="13" />
      <line x1="13" y1="3" x2="3" y2="13" />
      <circle cx="8" cy="8" r="1.8" fill="currentColor" stroke="none" />
    </svg>
  ),
  // Curvature — two arcs flowing smoothly into each other (G2).
  curvature: (
    <svg {...ICON_PROPS}>
      <path d="M2 12 Q5 4 8 8 T14 4" />
    </svg>
  ),
};

/* ------------------------------------------------------------------ */
/*  Sketch-constraint toolbar descriptors (ticket vk8q)                */
/* ------------------------------------------------------------------ */

// pqoe / vk8q / zz1r: the explicit constraint toolbar, Onshape-style. Each
// constraint is a sketch TOOL: it sits in the drawing-tool row with an icon
// + text label, is always enabled, and on click selects the tool — the user
// then clicks the entities one after another and the constraint applies the
// instant the running selection satisfies it (see store.selectSketchConstraintTool).
// A single "Dimension" tool leads the row and infers the dimensional type
// (distance / angle / radius / diameter) from the picks. `sketch_mirror` is
// deliberately absent: mirror is a sketch tool, not a constraint.
interface ConstraintButtonMeta {
  /** Sub-call type the button applies, e.g. `sketch_constraint_parallel`.
   *  Null for the unified Dimension button (its type is resolved from the
   *  selection at apply time). */
  type: string | null;
  /** Stable test-id suffix (`sketch-constraint-<testid>`). */
  testid: string;
  /** Short text label shown next to the icon (zz1r). */
  label: string;
  /** Icon key into `CONSTRAINT_ICONS`. */
  icon: string;
  /** Tooltip describing the constraint. */
  title: string;
}

// zz1r: the constraint buttons are sketch TOOLS now (tool-first constrain
// workflow) — pick the tool, then click the entities one after another and
// the constraint applies as soon as the selection fits. They carry text
// labels, live in the drawing-tool row, and are always enabled. The
// unsupported Onshape placeholders (Normal / Pierce / Curvature) are gone:
// they had no kernel constraint, and a tool you can never apply has no
// place as an always-enabled button.
const SKETCH_CONSTRAINT_BUTTONS: ConstraintButtonMeta[] = [
  // Unified Dimension tool — infers distance/length/angle/radius/diameter.
  {
    type: null,
    testid: "dimension",
    label: "Dimension",
    icon: "dimension",
    title:
      "Dimension — pick the entities to set distance / angle / radius / diameter",
  },
  // Geometric constraints, in Onshape order.
  {
    type: "sketch_constraint_coincident",
    testid: "coincident",
    label: "Coincident",
    icon: "coincident",
    title:
      "Coincident — pick two entities: point+point glue, point+curve on-curve, line+line collinear, circle/arc+circle/arc concentric",
  },
  {
    type: "sketch_constraint_concentric",
    testid: "concentric",
    label: "Concentric",
    icon: "concentric",
    title: "Concentric — pick two circles/arcs to share a centre",
  },
  {
    type: "sketch_constraint_parallel",
    testid: "parallel",
    label: "Parallel",
    icon: "parallel",
    title: "Parallel — pick two lines to make them parallel",
  },
  {
    type: "sketch_constraint_tangent",
    testid: "tangent",
    label: "Tangent",
    icon: "tangent",
    title: "Tangent — pick a line and a curve to make them tangent",
  },
  {
    type: "sketch_constraint_horizontal",
    testid: "horizontal",
    label: "Horizontal",
    icon: "horizontal",
    title: "Horizontal — pick a line to make it horizontal",
  },
  {
    type: "sketch_constraint_vertical",
    testid: "vertical",
    label: "Vertical",
    icon: "vertical",
    title: "Vertical — pick a line to make it vertical",
  },
  {
    type: "sketch_constraint_perpendicular",
    testid: "perpendicular",
    label: "Perpendicular",
    icon: "perpendicular",
    title: "Perpendicular — pick two lines to make them perpendicular",
  },
  {
    type: "sketch_constraint_equal",
    testid: "equal",
    label: "Equal",
    icon: "equal",
    title: "Equal — pick two entities to equate length / radius",
  },
  {
    type: "sketch_constraint_midpoint",
    testid: "midpoint",
    label: "Midpoint",
    icon: "midpoint",
    title:
      "Midpoint — pick a point and a line to pin it at the line's midpoint",
  },
  {
    type: "sketch_constraint_symmetric",
    testid: "symmetric",
    label: "Symmetric",
    icon: "symmetric",
    title: "Symmetric — pick two points and a line of symmetry",
  },
  {
    type: "sketch_constraint_fix",
    testid: "fix",
    label: "Fix",
    icon: "fix",
    title: "Fix — pick a point to fix it at its position",
  },
  {
    type: "sketch_constraint_point_on_object",
    testid: "point_on_object",
    label: "On Curve",
    icon: "point_on_object",
    title:
      "On Curve — pick a point and a line/circle/arc/ellipse/spline to constrain it onto",
  },
];

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

  // Ticket apr3: on phone-width viewports the full tool row covers the whole
  // screen. Below the mobile breakpoint the toolbar collapses to a single
  // "Tools" menu button that drops the tools down as an overlay on demand.
  const isMobile = useMediaQuery(MOBILE_QUERY);
  const [menuOpen, setMenuOpen] = useState(false);
  // Picking a real tool (not a split-button chevron, which opens its own
  // sub-menu) closes the drawer so the canvas is reachable again.
  const closeMenuOnPick = (e: ReactMouseEvent<HTMLDivElement>) => {
    if (!isMobile) return;
    const btn = (e.target as HTMLElement).closest("button");
    if (btn && btn.classList.contains("tool") && !btn.classList.contains("chevron")) {
      setMenuOpen(false);
    }
  };
  // Ticket qh4l: the collapsed mobile toggle is a DROPDOWN affordance — it
  // shows the currently active tool (icon + label) plus a caret, not a third
  // hamburger (three identical ☰ read as the same control). In sketch mode the
  // active sketch tool is shown (Select when none); otherwise just "Tools".
  const activeTool =
    sketchSession.active
      ? sketchSession.selectedTool
        ? meta(sketchSession.selectedTool)
        : { label: "Select", glyph: "↖" }
      : { label: "Tools", glyph: null as string | null };
  const wrapToolbar = (children: ReactElement) => (
    <div
      className={
        "fd-toolbar" +
        (isMobile ? " is-mobile" : "") +
        (isMobile && menuOpen ? " is-menu-open" : "")
      }
      data-testid="fielddraft-toolbar"
    >
      {isMobile ? (
        <button
          type="button"
          className="toolbar-menu-toggle"
          data-testid="toolbar-menu-toggle"
          title={menuOpen ? "Close tools" : "Open tools"}
          aria-expanded={menuOpen}
          aria-label={menuOpen ? "Close tools" : "Open tools"}
          onClick={() => setMenuOpen((v) => !v)}
        >
          {activeTool.glyph ? (
            <span className="glyph" aria-hidden="true">
              {activeTool.glyph}
            </span>
          ) : null}
          <span className="toolbar-menu-label">{activeTool.label}</span>
          <span className="caret" aria-hidden="true">
            ▾
          </span>
        </button>
      ) : null}
      <div className="toolbar-tools" onClick={closeMenuOnPick}>
        {children}
      </div>
    </div>
  );

  // F22: hidden file input for SVG / DXF import.
  const importInputRef = useRef<HTMLInputElement>(null);
  const [importHint, setImportHint] = useState<string | null>(null);
  const handleImportFile = async (file: File | null | undefined) => {
    if (!file) return;
    try {
      const text = await file.text();
      const count = store.importVectorSubCalls(text, file.name);
      setImportHint(
        count > 0
          ? `Imported ${count} element${count === 1 ? "" : "s"}`
          : "No geometry found in file",
      );
    } catch {
      setImportHint("Could not read file");
    }
  };

  const [sketchToolTypes, setSketchToolTypes] = useState<readonly string[]>([]);
  useEffect(() => {
    try {
      setSketchToolTypes(fd.listSketchToolTypes());
    } catch {
      setSketchToolTypes([]);
    }
  }, [fd]);

  // Ticket yx6a: the Sketch button opens the session IMMEDIATELY — no
  // face-pick mode first. A pre-selected face becomes the sketch plane
  // (Onshape convention); a pre-selected default-geometry plane
  // (Top/Front/Right, ticket 90f9b34a) likewise. Otherwise the session
  // starts on XY and the plane stays editable in the session panel's
  // query picker.
  const handleSketchEnter = () => {
    const el = state.selectedElement;
    const datumQuery = state.selectedDatum
      ? DATUM_PLANE_QUERIES[state.selectedDatum]
      : undefined;
    const planeQuery =
      el && el.kind === "face"
        ? `qFace(${el.idHash})`
        : (datumQuery ?? DEFAULT_SKETCH_PLANE_QUERY);
    store.enterSketch(planeQuery);
  };

  const handleSketchToolPick = (typeName: string) => {
    // Ticket daa8: re-clicking the ACTIVE tool detaches it (back to select
    // mode) — this is how the user ends a chained Line/polyline, and matches
    // the toggle convention of the other sketch tools.
    if (sketchSession.selectedTool === typeName) {
      store.selectSketchTool(null);
      return;
    }
    try {
      const schema = fd.schemaForSketchToolType(typeName);
      store.selectSketchTool(typeName, buildDefaultDefinition(schema));
    } catch {
      /* ignore */
    }
  };

  // Dispatch a sketch-tool dropdown selection. Kernel tools (circle,
  // rectangle, ellipse) go through schema defaults; the UI-only variants
  // (3-point / center) just carry an empty definition like their old
  // hard-coded buttons did.
  const handleSketchToolSelect = (kind: string) => {
    if (SKETCH_TOOL_KEYS.has(kind)) {
      handleSketchToolPick(kind);
    } else {
      store.selectSketchTool(kind, {});
    }
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

  // F5: degrees-of-freedom readout for the sketch HUD. Shows the
  // authoritative solver DoF from the last solve once it converged,
  // falling back to the naive tally over the draft sub-calls before
  // the first solve lands; null on failure so the readout disappears.
  const dof = useMemo(() => {
    try {
      return displayDegreesOfFreedom(
        buildDraftSketchState(sketchSession.subCalls),
        sketchSession.diagnostics,
      );
    } catch {
      return null;
    }
  }, [sketchSession.subCalls, sketchSession.diagnostics]);

  // Over-constraint is reported by the solver as conflicting/malformed
  // constraints, not a negative DoF (PlaneGCS' remaining-DoF is never
  // negative). The naive fallback can still produce a negative `dof`.
  const dofOver =
    sketchSession.diagnostics.conflicting.length > 0 ||
    sketchSession.diagnostics.malformed.length > 0 ||
    (dof !== null && dof < 0);

  const hasSketch = state.features.some((f) => f.kind === "sketch");
  const hasFeatures = state.features.length > 0;
  // Ticket 981k: Extrude works on the current face selection too — as long
  // as a face is selectable it is extrudable, sketch or not.
  const selectedFaceQueries = state.selectedElements
    .filter((e) => e.kind === "face")
    .map(elementQueryString);

  /* ---------- Sketch mode toolbar ---------- */

  if (sketchSession.active) {
    // Ticket yuaw: the separate "Sketch | Import | DoF" header row is gone —
    // the sketch tools themselves signal sketch mode. The mode indicator is
    // kept (tests rely on it) as a minimal chip at the head of the tool row,
    // Import moved INTO the tool row next to Project, and the DoF readout
    // relocated to the far right of the row.
    return wrapToolbar(
      <div className="group sketch-tool-row">
          {/* Ticket yuaw: minimal sketch-mode chip — was the old header row's
              title. Kept (restyled) because e2e selectors wait on it. */}
          <span className="sketch-chip" data-testid="sketch-mode-indicator">
            Sketch
          </span>
          {/* Ticket pqoe: a sketch is a feature like any other — commit /
              discard live on the ✓/✗ in its Detail-panel header, not in
              the toolbar. The toolbar only carries sketch tools + Import. */}
          {/* P0.1: the Select tool returns to select mode (no drawing
              tool active) so committed geometry can be picked and
              constrained. Without it the constraint palette is
              unreachable once any drawing tool is chosen. */}
          <button
            type="button"
            className={
              "tool" + (sketchSession.selectedTool === null ? " is-active" : "")
            }
            title="Select & constrain geometry (Esc)"
            data-testid="sketch-tool-select"
            onClick={() => store.selectSketchTool(null)}
          >
            <span className="glyph" aria-hidden="true">
              ↖
            </span>
            <span className="tool-label">Select</span>
          </button>
          {sketchToolTypes.map((type) => {
            if (!SKETCH_TOOL_KEYS.has(type)) return null;
            // Circle / rectangle / ellipse are surfaced through the grouped
            // dropdowns below instead of as flat buttons.
            if (SKETCH_GROUPED_KEYS.has(type)) return null;
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
          {/* pqoe: Circle (Circle / 3-pt / Ellipse) and Rectangle
              (Rectangle / center / 3-pt) variants grouped into split-button
              dropdowns, matching the main toolbar's primitive/transform
              dropdowns. activeKey is set only while a member of the group is
              the selected tool, so the primary button lights up and shows the
              live variant; otherwise the last-used variant stays the default. */}
          <FeatureDropdownButton
            items={SKETCH_CIRCLE_ITEMS}
            lastUsedStorageKey="fielddraft.lastUsedSketchCircle"
            defaultKey="sketch_circle"
            activeKey={
              SKETCH_CIRCLE_ITEMS.some(
                (it) => it.key === sketchSession.selectedTool,
              )
                ? (sketchSession.selectedTool ?? undefined)
                : undefined
            }
            onSelect={handleSketchToolSelect}
            testId="sketch-tool-circle-group"
          />
          <FeatureDropdownButton
            items={SKETCH_RECTANGLE_ITEMS}
            lastUsedStorageKey="fielddraft.lastUsedSketchRectangle"
            defaultKey="sketch_rectangle"
            activeKey={
              SKETCH_RECTANGLE_ITEMS.some(
                (it) => it.key === sketchSession.selectedTool,
              )
                ? (sketchSession.selectedTool ?? undefined)
                : undefined
            }
            onSelect={handleSketchToolSelect}
            testId="sketch-tool-rectangle-group"
          />
          {/* F23-full: spline-mode toggle. Shown only while the spline
              tool is active. `interpolate` = Catmull-Rom through the
              clicked points; `control` = a real cubic B-spline whose
              clicked points are the control polygon. The chosen mode is
              carried into the emitted `sketch_spline` sub-call. */}
          {sketchSession.selectedTool === "sketch_spline" && (
            <div
              className="spline-mode"
              data-testid="sketch-spline-mode"
              role="radiogroup"
              aria-label="Spline mode"
            >
              <button
                type="button"
                className={
                  "tool" +
                  (state.sketchToolParams.splineMode === "interpolate" ? " is-active" : "")
                }
                data-testid="sketch-spline-mode-interpolate"
                aria-pressed={state.sketchToolParams.splineMode === "interpolate"}
                title="Interpolating spline (passes through points)"
                onClick={() => store.setSketchSplineMode("interpolate")}
              >
                <span className="tool-label">Through pts</span>
              </button>
              <button
                type="button"
                className={
                  "tool" +
                  (state.sketchToolParams.splineMode === "control" ? " is-active" : "")
                }
                data-testid="sketch-spline-mode-control"
                aria-pressed={state.sketchToolParams.splineMode === "control"}
                title="Control-point cubic B-spline"
                onClick={() => store.setSketchSplineMode("control")}
              >
                <span className="tool-label">Control pts</span>
              </button>
            </div>
          )}
          {/* F7: primitive-construction variants (rect center / rect 3-pt /
              circle 3-pt) are UI-only tools — they aren't kernel feature
              tools but emit ordinary sub-calls at commit time (sketch_rectangle
              / sketch_circle / sketch_line + perpendicular constraints), with
              the gestures handled in SketchCanvas2d keyed on the selectedTool
              string. They now live inside the Circle / Rectangle dropdowns
              above (ticket pqoe). */}
          {/* Ticket 7hgr (c): draw tools | modify tools — separator. */}
          <span
            className="tool-sep"
            aria-hidden="true"
            data-testid="sketch-tool-sep-modify"
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
          {/* Ticket yuaw: Import moved off the old header row INTO the tool
              row, grouped with Project since both bring outside geometry into
              the sketch. F22: a visible button triggers the visually-hidden
              file input; the hint sits inline next to it. */}
          <button
            type="button"
            className="tool"
            title="Import SVG / DXF geometry"
            data-testid="sketch-import"
            onClick={() => importInputRef.current?.click()}
          >
            <span className="glyph" aria-hidden="true">
              📐
            </span>
            <span className="tool-label">Import</span>
          </button>
          <input
            ref={importInputRef}
            type="file"
            accept=".svg,.dxf"
            className="hidden-file-input"
            data-testid="sketch-import-input"
            onChange={(e) => {
              void handleImportFile(e.currentTarget.files?.[0]);
              e.currentTarget.value = "";
            }}
          />
          {importHint && (
            <span className="hint" data-testid="sketch-import-hint">
              {importHint}
            </span>
          )}
          {/* F21: Project / Use — interactive editing mode, not a kernel
              tool. Existing bodies' B-Rep edges are drawn as faint reference
              guides projected onto the sketch plane; a single click adopts
              the highlighted guide under the cursor as a CONSTRUCTION
              sketch_line the solver can constrain against (see
              nearestProjectedEdge / sketchProject.ts). */}
          <FeatureButton
            kind="sketch_project"
            active={sketchSession.selectedTool === "sketch_project"}
            testId="sketch-tool-sketch_project"
            onClick={() => store.selectSketchTool("sketch_project", {})}
          />
          {sketchSession.selectedTool === "sketch_project" && (
            <span className="hint" data-testid="sketch-project-hint">
              Click a highlighted reference edge to adopt it as construction
              geometry
            </span>
          )}
          {/* Split: the keep-both sibling of Trim — also an interactive
              editing mode, not a kernel tool. A single click cuts the curve
              under the cursor into two pieces meeting at the click point;
              both halves survive (see store.splitSketchAt /
              sketchSplit.ts). */}
          <FeatureButton
            kind="sketch_split"
            active={sketchSession.selectedTool === "sketch_split"}
            testId="sketch-tool-sketch_split"
            onClick={() => store.selectSketchTool("sketch_split", {})}
          />
          {/* Extend: the complement of Trim — also an interactive editing
              mode, not a kernel tool. A single click picks the nearest line
              and lengthens its grabbed end to the nearest other curve (see
              store.extendSketchAt / sketchExtend.ts). */}
          <FeatureButton
            kind="sketch_extend"
            active={sketchSession.selectedTool === "sketch_extend"}
            testId="sketch-tool-sketch_extend"
            onClick={() => store.selectSketchTool("sketch_extend", {})}
          />
          {/* Fillet: like Trim, an interactive editing mode, not a kernel
              tool. Two clicks pick the lines whose corner is rounded by a
              tangent arc of the radius below (see store.filletSketchAt /
              sketchFillet.ts). */}
          <FeatureButton
            kind="sketch_fillet"
            active={sketchSession.selectedTool === "sketch_fillet"}
            testId="sketch-tool-sketch_fillet"
            onClick={() => store.selectSketchTool("sketch_fillet", {})}
          />
          {/* Ticket pqoe: the Fillet radius field moved in-scene — it now
              floats in the canvas corner while the Fillet tool is active
              (see SketchCanvas2d's in-scene tool-param input). */}
          {/* Chamfer: straight-edge sibling of Fillet, an interactive
              editing mode, not a kernel tool. Two clicks pick the lines
              whose corner is bevelled by a straight line at the setback
              distance below (see store.chamferSketchAt /
              sketchChamfer.ts). */}
          <FeatureButton
            kind="sketch_chamfer"
            active={sketchSession.selectedTool === "sketch_chamfer"}
            testId="sketch-tool-sketch_chamfer"
            onClick={() => store.selectSketchTool("sketch_chamfer", {})}
          />
          {/* Ticket pqoe: the Chamfer distance field moved in-scene — it now
              floats in the canvas corner while the Chamfer tool is active
              (see SketchCanvas2d's in-scene tool-param input). */}
          {/* F15: Offset — interactive editing mode, not a kernel tool. A
              single click picks the nearest curve and appends a parallel
              copy at the distance below, toward the clicked side (see
              store.offsetSketchAt / sketchOffset.ts). */}
          <FeatureButton
            kind="sketch_offset"
            active={sketchSession.selectedTool === "sketch_offset"}
            testId="sketch-tool-sketch_offset"
            onClick={() => store.selectSketchTool("sketch_offset", {})}
          />
          {/* Ticket pqoe: the Offset distance field moved in-scene — it now
              floats in the canvas corner while the Offset tool is active
              (see SketchCanvas2d's in-scene tool-param input). */}
          {/* Ticket 7hgr (c): modify tools | transform tools — separator. */}
          <span
            className="tool-sep"
            aria-hidden="true"
            data-testid="sketch-tool-sep-transform"
          />
          {/* F14: Mirror — interactive editing mode, not a kernel tool.
              Select source geometry first (Select mode), then pick this
              tool and click the axis LINE: every selected line/circle/arc
              is reflected across it as an editable mirrored copy (see
              store.mirrorSketchSelection / sketchMirror.ts). */}
          <FeatureButton
            kind="sketch_mirror"
            active={sketchSession.selectedTool === "sketch_mirror"}
            testId="sketch-tool-sketch_mirror"
            onClick={() => store.selectSketchTool("sketch_mirror", {})}
          />
          {/* jq6i/D1: the mirror instruction now floats as the centered
              ToolHint pill (SketchCanvas2d drives it through the two-step
              axis→objects flow), not as inline text next to the button. */}
          {/* Ticket pqoe (plan item 2): the Pattern controls (count /
              spacing / angle + Linear X / Linear Y / Circular) moved in-scene
              — they now float in the canvas corner as part of the
              selection-transform panel, shown while a selection exists in
              Select mode (see SketchCanvas2d's .fd-sketch-transform-panel). */}
          {/* F17: Move / Copy — interactive transforms, not kernel tools.
              Select source geometry first (Select mode), pick Move or Copy,
              then click a base point and a target point: the selection is
              translated by (target − base), in place for Move or as appended
              copies for Copy (see store.moveSketchSelection /
              SketchCanvas2d). */}
          <FeatureButton
            kind="sketch_move"
            active={sketchSession.selectedTool === "sketch_move"}
            testId="sketch-tool-sketch_move"
            onClick={() => store.selectSketchTool("sketch_move", {})}
          />
          <FeatureButton
            kind="sketch_copy"
            active={sketchSession.selectedTool === "sketch_copy"}
            testId="sketch-tool-sketch_copy"
            onClick={() => store.selectSketchTool("sketch_copy", {})}
          />
          {/* 3rz2/D1: the move/copy instruction now floats as the ToolHint
              pill instead of inline text next to the button. */}
          {/* Ticket pqoe (plan item 3): the Rotate controls (angle + Rotate)
              moved in-scene — they now sit in the selection-transform panel in
              the canvas corner alongside the Pattern controls, shown while a
              selection exists in Select mode (see SketchCanvas2d's
              .fd-sketch-transform-panel). */}
          {/* Ticket 7hgr (c): transform tools | text — separator. */}
          <span
            className="tool-sep"
            aria-hidden="true"
            data-testid="sketch-tool-sep-text"
          />
          {/* F24-full: Text — interactive editing mode, not a kernel tool.
              Picking the tool then clicking the canvas opens an IN-SCENE
              input + param popover (height / font) right at the click
              location — the text value and params no longer live in this
              toolbar (ticket pqoe). Each drawn glyph still becomes one
              sketch_path sub-call (see SketchCanvas2d's in-scene text editor
              + store.placeSketchText / sketchText.ts). Ticket lycy: the
              "click to type" hint moved into the button tooltip. */}
          <FeatureButton
            kind="sketch_text"
            active={sketchSession.selectedTool === "sketch_text"}
            testId="sketch-tool-sketch_text"
            title="Click in the sketch to type text"
            onClick={() => store.selectSketchTool("sketch_text", {})}
          />
          {/* Ticket 7hgr (b+c): Extrude / Revolve are available in sketch
              mode too. Clicking one COMMITS the current sketch, exits sketch
              mode and starts that tool against the just-committed sketch as
              its profile, opening the new feature's Detail view
              (store.extrudeFromSketchSession / revolveFromSketchSession). A
              separator sets them apart from the drawing/modify tools. */}
          {/* zz1r: the constraint tools live IN the drawing-tool row (no
              separate, selection-gated palette anymore). Each is always
              enabled and is a tool-selector: clicking it activates the tool,
              and the user then clicks the entities one after another (see
              store.selectSketchConstraintTool). The Dimension button keeps a
              dynamic test-id (`sketch-constraint-distance/angle/…`) resolved
              from the current selection so the existing dimensional e2e
              selectors and the selection-first apply path keep working. */}
          <span
            className="tool-sep"
            aria-hidden="true"
            data-testid="sketch-tool-sep-constraint"
          />
          {SKETCH_CONSTRAINT_BUTTONS.map((c) => {
            const tool = c.type ?? CONSTRAINT_DIMENSION_TOOL;
            const active = sketchSession.selectedTool === tool;
            // The Dimension button stamps the resolved dimensional type's
            // test-id when the selection fits one (e.g. distance/angle),
            // falling back to `sketch-constraint-dimension` otherwise.
            const resolvedDim =
              c.type === null
                ? (DIMENSION_TYPES_BY_SPECIFICITY.find((t) =>
                    constraintOptions.some((o) => o.type === t),
                  ) ?? null)
                : null;
            const testid =
              c.testid === "dimension" && resolvedDim !== null
                ? resolvedDim.replace(/^sketch_constraint_/, "")
                : c.testid;
            return (
              <button
                key={c.testid}
                type="button"
                className={
                  "tool constraint-tool" + (active ? " is-active" : "")
                }
                title={c.title}
                data-testid={`sketch-constraint-${testid}`}
                onClick={() => store.selectSketchConstraintTool(tool)}
              >
                <span className="glyph" aria-hidden="true">
                  {CONSTRAINT_ICONS[c.icon]}
                </span>
                <span className="tool-label">{c.label}</span>
              </button>
            );
          })}
          <span
            className="tool-sep"
            aria-hidden="true"
            data-testid="sketch-tool-sep-feature"
          />
          <FeatureButton
            kind="extrude"
            testId="sketch-tool-extrude"
            title="Finish sketch & extrude it (25 mm)"
            onClick={() => store.extrudeFromSketchSession(25)}
          />
          <FeatureButton
            kind="revolve"
            testId="sketch-tool-revolve"
            title="Finish sketch & revolve it"
            onClick={() => store.revolveFromSketchSession()}
          />
          {/* Ticket yuaw: the DoF readout moved off the old header row to the
              far right of the tool row — pushed there by an auto-margin so it
              stays a non-intrusive status chip. F5 colours: neutral / green
              (fully constrained) / red (over-constrained). */}
          {dof !== null && (
            <span
              className={
                "dof" + (dofOver ? " is-over" : dof === 0 ? " is-ok" : "")
              }
              data-testid="sketch-dof"
              title="Remaining degrees of freedom"
            >
              {dofOver
                ? dof < 0
                  ? `Over-constrained (${dof})`
                  : "Over-constrained"
                : dof > 0
                  ? `${dof} DoF`
                  : "Fully constrained"}
            </span>
          )}
        </div>,
    );
  }

  /* ---------- Main toolbar ---------- */

  return wrapToolbar(
    <>
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
        <FeatureButton
          kind="hole"
          title="Add Hole (Ø6 x 40 mm, subtract)"
          onClick={() => store.addHole()}
          draggable
          dragType="fielddraft/primitive"
        />
        <FeatureDropdownButton
          items={THREAD_ITEMS}
          lastUsedStorageKey="fielddraft.lastUsedThread"
          defaultKey="M6"
          onSelect={(key) => {
            const p =
              THREAD_PRESETS.find((t) => t.key === key) ?? THREAD_PRESETS[3];
            store.addThreadPreset(p.radius, p.pitch, p.depth);
          }}
          testId="thread-split-button"
        />
        <FeatureButton
          kind="sketch"
          title="Sketch on XY plane"
          testId="enter-sketch"
          onClick={handleSketchEnter}
          draggable
          dragType="fielddraft/sketch"
        />
        {/* Extrude/Revolve stay enabled without a matching selection
            (ticket 1rax): clicking creates the feature with an EMPTY
            profile and the dialog's query picker fills it afterwards —
            until then the feature reports its error Onshape-style. */}
        <FeatureButton
          kind="extrude"
          title={
            selectedFaceQueries.length > 0
              ? "Extrude selected face(s) (25 mm)"
              : hasSketch
                ? "Extrude latest sketch (25 mm)"
                : "Extrude — pick faces or a sketch in the dialog"
          }
          testId="extrude-latest-sketch"
          disabled={!hasFeatures && !hasSketch}
          onClick={() => {
            if (selectedFaceQueries.length > 0) {
              store.addExtrudeFaces(selectedFaceQueries);
              store.clearElementSelection();
            } else if (hasSketch) {
              store.addExtrudeFromLatestSketch(25);
            } else {
              store.addExtrudeFaces([]);
            }
          }}
        />
        <FeatureButton
          kind="revolve"
          title={
            hasSketch
              ? "Revolve latest sketch"
              : "Revolve — pick a sketch in the dialog"
          }
          testId="revolve-latest-sketch"
          disabled={!hasFeatures && !hasSketch}
          onClick={() => store.addRevolveFromLatestSketch()}
        />
        <FeatureButton
          kind="helix"
          title={
            hasSketch
              ? "Helix sweep of the latest sketch (screw threads)"
              : "Helix — pick a sketch in the dialog"
          }
          testId="helix-latest-sketch"
          disabled={!hasFeatures && !hasSketch}
          onClick={() => store.addHelixFromLatestSketch()}
        />
        <FeatureDropdownButton
          items={[
            {
              key: "sweep_sketch",
              kind: "sweep",
              label: "Sweep",
              title:
                "Sweep the previous sketch (profile) along the latest sketch (path)",
            },
            {
              key: "sweep_gizmo",
              kind: "sweep",
              label: "Sweep 3D",
              title:
                "Sweep the latest sketch along a 3D control-point path (edit the points in Detail)",
            },
          ]}
          lastUsedStorageKey="fielddraft.lastUsedSweep"
          defaultKey="sweep_sketch"
          onSelect={(key) =>
            key === "sweep_gizmo"
              ? store.addSweepGizmoFromLatestSketch()
              : store.addSweepFromLatestSketches()
          }
          testId="sweep-split-button"
        />
        <FeatureButton
          kind="gyroid"
          title="Add Gyroid infill"
          disabled={!hasFeatures}
          onClick={() => store.addGyroid()}
          draggable
          dragType="fielddraft/primitive"
        />
        <FeatureButton
          kind="custom_sdf"
          title="Add Custom SDF (math expression)"
          onClick={() => store.addCustomSdf()}
          draggable
          dragType="fielddraft/primitive"
        />
        <FeatureButton
          kind="linear_pattern"
          title="Linear pattern (+X, 3x, 60 mm)"
          testId="linear-pattern-latest"
          disabled={!hasFeatures}
          onClick={() => store.addLinearPatternOfLatest()}
          label="Pattern"
        />
        <FeatureButton
          kind="circular_pattern"
          title="Circular pattern (6x, 360°)"
          testId="circular-pattern-latest"
          disabled={!hasFeatures}
          onClick={() => store.addCircularPatternOfLatest()}
          label="Circ"
        />
        <FeatureButton
          kind="mirror"
          title="Mirror through YZ plane"
          testId="mirror-latest"
          disabled={!hasFeatures}
          onClick={() => store.addMirrorOfLatest()}
        />
        <FeatureButton
          kind="boolean"
          title="Boolean (Union / Subtract / Intersect)"
          disabled={!hasFeatures}
          onClick={() => store.addBoolean()}
        />
        <FeatureButton
          kind="material"
          title="Assign material properties"
          disabled={!hasFeatures}
          onClick={() => store.addMaterial()}
        />
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
        <button
          type="button"
          className="tool"
          title="Delete selection (Del)"
          data-testid="delete-selection"
          disabled={state.selection.size === 0}
          onClick={() => store.removeSelected()}
        >
          <span className="glyph" aria-hidden="true">
            🗑
          </span>
          <span className="tool-label">Delete</span>
        </button>
        <FeatureButton
          kind="fillet_edge"
          title="Click an edge to fillet it"
          testId="fillet-toggle"
          active={state.filletMode}
          onClick={() => store.toggleFilletMode()}
          label="Fillet"
        />
        <FeatureButton
          kind="thicken"
          title="Click a curved face to thicken it into a wall"
          testId="thicken-toggle"
          active={state.thickenMode}
          onClick={() => store.toggleThickenMode()}
          label="Thicken"
        />
        <FeatureButton
          kind="shell"
          title="Hollow a body — pick faces to open in the dialog"
          testId="add-shell"
          disabled={!hasFeatures}
          onClick={() => store.addShell()}
          label="Shell"
        />
        <FeatureButton
          kind="round_all"
          title="Round every edge globally — set the radius in the dialog"
          testId="add-round-all"
          disabled={!hasFeatures}
          onClick={() => store.addRoundAll()}
          label="Round All"
        />
        <FeatureButton
          kind="offset"
          title="Offset a body's surface — pick bodies and a signed amount in the dialog"
          testId="add-offset"
          disabled={!hasFeatures}
          onClick={() => store.addOffset()}
          label="Offset"
        />
        <FeatureButton
          kind="mate_connector"
          title="Place a mate connector (Ctrl+M)"
          testId="add-mate-connector"
          disabled={!hasFeatures}
          active={state.mateMode}
          onClick={() => store.toggleMateMode()}
          label="Mate"
        />
        <button
          type="button"
          className={"tool" + (state.measureMode ? " is-active" : "")}
          title="Measure distance (click two points)"
          data-testid="measure-toggle"
          onClick={() => store.toggleMeasureMode()}
        >
          <span className="glyph" aria-hidden="true">
            📏
          </span>
          <span className="tool-label">Measure</span>
        </button>
        {state.referenceMesh && (
          <FeatureButton
            kind="imported_mesh"
            title="Convert reference mesh to solid"
            testId="convert-to-solid"
            onClick={() => store.convertReferenceToSolid()}
            label="Solid"
          />
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
              <span className="glyph" aria-hidden="true">
                {t.icon}
              </span>
              <span className="tool-label">{t.name}</span>
            </button>
          ))}
        </div>
      )}
    </>,
  );
}
