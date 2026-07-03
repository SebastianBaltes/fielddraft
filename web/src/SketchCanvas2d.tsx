/**
 * K6b.3 opening: 2D canvas overlay for the sketch editor.
 *
 * Sits absolutely positioned above the WebGPU `<Canvas>` in the
 * viewport DOM, same bounding box. Draws sketch points, lines,
 * the active drag preview and the live distance label. Reads
 * camera state from the `sketchCameraBridge` (published from a
 * tiny R3F component inside the Canvas, see
 * `CameraBridgePublisher` in `Viewport.tsx`) so the overlay
 * stays in sync with pan/zoom without any React prop churn.
 *
 * Why a 2D canvas and not SVG: the K6b.3+ face-lock feature
 * lands tilted sketch planes, and the plane→screen transform
 * in the general case is a 2×2 affine map that shears text and
 * icons along with the geometry. With canvas we transform the
 * geometry explicitly and draw labels/icons screen-aligned at
 * the projected anchor — no per-element counter-transform. For
 * the K6b.3 opening slice the transform is trivial (top-down
 * ortho), but the abstraction already takes a matrix so the
 * general case drops in later.
 */

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactElement,
} from "react";
import {
  getActive as getActiveQueryPicker,
  subscribe as subscribeQueryPicker,
} from "./queryPickerBridge";
import type { KernelStore } from "./store";
import type { SketchSessionState } from "./sketchSession";
import {
  readSketchCamera,
  screenToSketch,
  setSketchPlaneFrame,
  sketchToScreen,
  type SketchCameraState,
} from "./sketchCameraBridge";
import { resolvePlaneFrame } from "./sketchPlaneFrame";
import { BUNDLED_FONTS, UPLOADED_FONT_PREFIX } from "./sketchTextFont";
import {
  buildDraftSketchState,
  predictNextLineIds,
} from "./sketchSession";
import { measuredLength, parseField } from "./sketchLiveInput";
import { isConstraintTool } from "./sketchConstraints";
import { useWorkspaceUnits, decimalPlaces } from "./settings";
import {
  getSketchLengthUnit,
  setSketchLengthUnit,
  sketchDisplayLength,
  sketchLengthToMm,
} from "./sketchUnitsBridge";
import {
  entitiesInBox,
  hitTestSketchEntity,
  hitTestTaggedCorner,
  hitTestTaggedText,
  type TaggedCornerHit,
  type TaggedTextHit,
} from "./sketchHitTest";
import {
  allGrips,
  entityIdForGrip,
  gripsForSelection,
  pickGripAt,
  type Grip,
} from "./sketchGrips";
import { planSelectDrag } from "./sketchSelectDrag";
import { angleAbout, buildArcSubCall, unwrapSweep } from "./sketchArc";
import {
  circleThrough3,
  rectCenterCorners,
  rect3pCorners,
} from "./sketchPrimitiveVariants";
import { setOverlayRedraw } from "./sketchOverlayBridge";
import { setToolHint } from "./toolHintBridge";
import { setHoveredSketchEntity } from "./sketchEntityHoverBridge";
import { clearSketchLabels } from "./sketchLabelBridge";
import { curveOfSubCall, distToCurve, nearestTrimTarget } from "./sketchTrim";
import { nearestProjectedEdge } from "./sketchProject";
import { solveAndReplace } from "./solver/replaySolve";
import { DragController } from "./solver/dragController";
import {
  anchorConstraints,
  classifyLineAnchor,
  projectEdgesToSketchPlane,
  resolveSnap,
  snapRadiusMmFor,
  type ProjectedEdge,
  type SnapResolution,
} from "./sketchSnapResolve";
import {
  DIM_LABEL_HIT_PX,
  distanceLabelAnchorPx,
  drawOverlay,
  linearDimLabelPx,
  resizeToContainer,
  type RecentLineGeometry,
} from "./sketchCanvasHud";
import {
  forwardPointerToViewport,
  SketchTouchPinch,
  viewportCanvas,
} from "./sketchCanvasTouch";
import {
  GRIP_PICK_PX,
  SELECT_PICK_PX,
  gridSnapped,
  type SketchToolController,
} from "./sketchTools/toolTypes";

interface Props {
  store: KernelStore | null;
  session: SketchSessionState;
}

/**
 * F10 fillet pick: index of the line sub-call nearest `click` (within
 * `tolMm`), or null. Restricted to `sketch_line` calls — fillet only
 * rounds the corner between two plain lines, so composites and curves
 * are skipped. Mirrors `nearestTrimTarget` but line-only.
 */
function nearestLineSubCall(
  subCalls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  click: [number, number],
  tolMm: number,
): number | null {
  let bestIdx: number | null = null;
  let bestDist = tolMm;
  subCalls.forEach((call, i) => {
    const cv = curveOfSubCall(call);
    if (!cv || cv.kind !== "line") return;
    const d = distToCurve(click, cv);
    if (d < bestDist) {
      bestDist = d;
      bestIdx = i;
    }
  });
  return bestIdx;
}

/** Ticket pqoe: the slice of store state the in-scene text editor reads. */
interface TextParams {
  value: string;
  height: number;
  fontId: string;
  fontReady: boolean;
}

const EMPTY_TEXT_PARAMS: TextParams = {
  value: "",
  height: 10,
  fontId: "",
  fontReady: false,
};

/** No-op subscribe for `useSyncExternalStore` when the store is absent. */
const NOOP_SUBSCRIBE = (): (() => void) => () => {};

/**
 * Ticket pqoe: the single numeric parameters the corner-op tools (fillet /
 * chamfer / offset) read live in top-level store state, not in `session`,
 * so the in-scene tool-param input subscribes to the store. Snapshot the
 * slice it reads; identity-stable so React bails out when nothing changed.
 */
interface ToolParams {
  filletRadius: number;
  chamferDistance: number;
  offsetDistance: number;
}

const EMPTY_TOOL_PARAMS: ToolParams = {
  filletRadius: 5,
  chamferDistance: 5,
  offsetDistance: 5,
};

let toolParamsCache: ToolParams = EMPTY_TOOL_PARAMS;
function toolParamsSnapshot(store: KernelStore): ToolParams {
  const s = store.getState();
  const c = toolParamsCache;
  if (
    c.filletRadius === s.sketchToolParams.filletRadius &&
    c.chamferDistance === s.sketchToolParams.chamferDistance &&
    c.offsetDistance === s.sketchToolParams.offsetDistance
  ) {
    return c;
  }
  toolParamsCache = {
    filletRadius: s.sketchToolParams.filletRadius,
    chamferDistance: s.sketchToolParams.chamferDistance,
    offsetDistance: s.sketchToolParams.offsetDistance,
  };
  return toolParamsCache;
}

/**
 * Ticket pqoe (plan items 2 + 3): the selection-transform values — pattern
 * count / spacing / angle (F16) and rotate angle (F17) — live in top-level
 * store state, not in `session`. The in-scene transform panel subscribes to
 * the store; snapshot the slice it reads, identity-stable so React bails out
 * when nothing it cares about changed.
 */
interface TransformParams {
  patternCount: number;
  patternSpacing: number;
  patternAngle: number;
  rotateAngle: number;
}

const EMPTY_TRANSFORM_PARAMS: TransformParams = {
  patternCount: 3,
  patternSpacing: 20,
  patternAngle: 360,
  rotateAngle: 90,
};

let transformParamsCache: TransformParams = EMPTY_TRANSFORM_PARAMS;
function transformParamsSnapshot(store: KernelStore): TransformParams {
  const s = store.getState();
  const c = transformParamsCache;
  if (
    c.patternCount === s.sketchToolParams.patternCount &&
    c.patternSpacing === s.sketchToolParams.patternSpacing &&
    c.patternAngle === s.sketchToolParams.patternAngle &&
    c.rotateAngle === s.sketchToolParams.rotateAngle
  ) {
    return c;
  }
  transformParamsCache = {
    patternCount: s.sketchToolParams.patternCount,
    patternSpacing: s.sketchToolParams.patternSpacing,
    patternAngle: s.sketchToolParams.patternAngle,
    rotateAngle: s.sketchToolParams.rotateAngle,
  };
  return transformParamsCache;
}

/**
 * Ticket pqoe: descriptor for the active corner-op tool's single numeric
 * param — which testid/label/setter the in-scene input binds to, and the
 * live value pulled from the `ToolParams` snapshot. `null` for any tool
 * that has no in-scene param (most tools).
 */
interface ToolParamField {
  testId: string;
  /** Short label glyph shown before the input (matches the old toolbar). */
  label: string;
  /** Tooltip describing the param. */
  title: string;
  value: number;
  set: (store: KernelStore, v: number) => void;
}

function toolParamField(
  tool: string | null,
  params: ToolParams,
): ToolParamField | null {
  switch (tool) {
    case "sketch_fillet":
      return {
        testId: "sketch-fillet-radius",
        label: "R",
        title: "Fillet radius (mm)",
        value: params.filletRadius,
        set: (store, v) => store.setSketchFilletRadius(v),
      };
    case "sketch_chamfer":
      return {
        testId: "sketch-chamfer-distance",
        label: "D",
        title: "Chamfer distance (mm)",
        value: params.chamferDistance,
        set: (store, v) => store.setSketchChamferDistance(v),
      };
    case "sketch_offset":
      return {
        testId: "sketch-offset-distance",
        label: "D",
        title: "Offset distance (mm)",
        value: params.offsetDistance,
        set: (store, v) => store.setSketchOffsetDistance(v),
      };
    default:
      return null;
  }
}

/**
 * Cached `TextParams` snapshot keyed off the store's current values, so
 * `useSyncExternalStore` gets a referentially-stable object while nothing
 * the editor cares about changed (re-creating it every call would loop).
 */
let textParamsCache: TextParams = EMPTY_TEXT_PARAMS;
function textParamsSnapshot(store: KernelStore): TextParams {
  const s = store.getState();
  const c = textParamsCache;
  if (
    c.value === s.sketchToolParams.textValue &&
    c.height === s.sketchToolParams.textHeight &&
    c.fontId === s.sketchToolParams.textFontId &&
    c.fontReady === s.sketchToolParams.textFontReady
  ) {
    return c;
  }
  textParamsCache = {
    value: s.sketchToolParams.textValue,
    height: s.sketchToolParams.textHeight,
    fontId: s.sketchToolParams.textFontId,
    fontReady: s.sketchToolParams.textFontReady,
  };
  return textParamsCache;
}

/**
 * The in-flight pointer gesture. Tool controllers under `sketchTools/` own
 * their drag state through the `owned` variant; the remaining variants are
 * the legacy inline state machines, dissolved tool by tool as the migration
 * proceeds (architecture review, Befund 2). Exported for the transitional
 * `drawOverlay` signature in `sketchCanvasHud.ts`.
 */
export type DragState =
  | { kind: "idle" }
  | {
      /** A drag owned by a tool controller from the registry. `state` is
       *  the controller's private drag type; the canvas only routes events
       *  and HUD requests to `owner`. */
      kind: "owned";
      owner: SketchToolController;
      state: unknown;
    }
  | {
      /** Ticket 3rz2: a direct click-drag of the current selection with the
       *  Move/Copy tool. Pressing an entity selects it (if needed) and arms
       *  this; dragging past a small threshold and releasing translates the
       *  selection by (current − start). A release without drag is a plain
       *  select. `copy` distinguishes Move (in place) from Copy (appended). */
      kind: "move_drag";
      start: [number, number];
      current: [number, number];
      copy: boolean;
      moved: boolean;
    }
  | {
      /** Ticket matf: a plain Select-mode click-drag that rigidly translates
       *  geometry — a single entity body (line / circle / arc / region) or a
       *  whole multi-selection grabbed and dragged together. `entityIds` are
       *  the committed entities to translate; the translate lands on release
       *  by (current − start), then the solver settles constraints. A release
       *  without a real drag was a plain pick (the down already selected). */
      kind: "select_drag";
      entityIds: number[];
      /** Selection to apply once a real drag begins (so the moved group
       *  highlights). Applied lazily on the first move, not at arm time, to
       *  preserve a no-drag click's additive toggle. */
      selectIds: number[];
      start: [number, number];
      current: [number, number];
      moved: boolean;
    }
  | {
      kind: "line";
      start: [number, number];
      current: [number, number];
      /** Entity id the start snapped to at pointerdown time, if
       *  the snap was a coincident-to-point hit. Used by the
       *  commit path to emit a matching coincident constraint. */
      startAnchorId: number | null;
    }
  | {
      /** F18: a click-without-drag started a line; it now rubber-bands
       *  to the cursor with NO button held, and a HUD lets the user type
       *  an exact length / angle. A second click OR Enter commits; Esc
       *  cancels. `startAnchorId` carries any coincident-to-point hit
       *  from the first click, same as the drag path. */
      kind: "pending_line";
      start: [number, number];
      current: [number, number];
      startAnchorId: number | null;
    }
  | {
      kind: "rectangle";
      start: [number, number];
      current: [number, number];
    }
  | {
      /** F7: rectangle-from-centre. Drag from the centre outward to a
       *  corner; the axis-aligned rectangle spans center ∓ |delta|. On
       *  drop we emit a single `sketch_rectangle` sub-call. */
      kind: "rect_center";
      center: [number, number];
      current: [number, number];
    }
  | {
      kind: "circle";
      center: [number, number];
      current: [number, number];
    }
  | {
      /** F8: axis-aligned ellipse. Drag from the centre outward to a
       *  corner; radius_x = |dx|, radius_y = |dy|. Rotation defaults to
       *  0 and is adjustable via the FeatureDialog. On drop we emit a
       *  single `sketch_ellipse` sub-call. */
      kind: "ellipse";
      center: [number, number];
      current: [number, number];
    }
  | {
      /** F9: centre-to-centre slot (stadium). Drag from end-centre A to
       *  end-centre B; the radius comes from the FeatureDialog (default
       *  8 mm). On drop we emit a single `sketch_slot` sub-call. */
      kind: "slot";
      center_a: [number, number];
      current: [number, number];
    }
  | {
      /** F7: circle through three points. Click 1 and 2 fix two points
       *  on the circumference; the 3rd click fixes the third and commits
       *  the circumcircle as a `sketch_circle` (aborted if collinear).
       *  `p2` is null until the 2nd click. */
      kind: "circle_3p";
      p1: [number, number];
      p2: [number, number] | null;
      current: [number, number];
    }
  | {
      /** F7: rotated rectangle from three points. p1→p2 is one edge;
       *  the 3rd click sets the perpendicular width. On the 3rd click we
       *  emit four `sketch_line` sub-calls forming the loop plus
       *  perpendicular constraints. `p2` is null until the 2nd click. */
      kind: "rect_3p";
      p1: [number, number];
      p2: [number, number] | null;
      current: [number, number];
    }
  | {
      /** F6: regular N-gon. Drag from the centre outward to set the
       *  radius; the side count + inscribed/circumscribed come from
       *  the tool's parameter dialog. On drop we emit a single
       *  `sketch_regular_polygon` sub-call. */
      kind: "regular_polygon";
      center: [number, number];
      current: [number, number];
    }
  | {
      kind: "polygon";
      vertices: [number, number][];
      current: [number, number];
    }
  | {
      /** F23: interpolating spline. Click to drop control points; the
       *  smooth Catmull-Rom curve is previewed live. Finish by double-
       *  clicking (or clicking within 8 px of) the last point for an
       *  OPEN spline, or by clicking within 8 px of the FIRST point to
       *  CLOSE it. On finish we emit one `sketch_spline` sub-call with
       *  the flat `points` array and the `closed` flag. */
      kind: "spline";
      points: [number, number][];
      current: [number, number];
      /** F23-full: `interpolate` (Catmull-Rom) or `control` (cubic
       *  B-spline control polygon). Snapshotted when the gesture
       *  starts so the live preview + commit stay consistent. */
      mode: "interpolate" | "control";
    }
  | {
      /** Arc: a centre → start → end three-click gesture. `start` is
       *  null until the 2nd click fixes the radius + start angle; after
       *  that the 3rd click fixes the sweep. `sweep` is unwrapped live
       *  (see `sketchArc.unwrapSweep`) so arcs past 180° are reachable. */
      kind: "arc";
      center: [number, number];
      start: [number, number] | null;
      current: [number, number];
      startAngle: number;
      sweep: number;
    }
  | {
      /** P0.3: dragging a grip handle on committed geometry. The
       *  geometry follows the cursor live; the solver re-runs only
       *  on drop so existing constraints snap the sketch back. */
      kind: "grip";
      grip: Grip;
    }
  | {
      /** F4: box-select / marquee in select mode. The user drags a
       *  rectangle on empty canvas; on drop we pick every entity the
       *  box matches. Drag direction picks the mode: left→right is an
       *  enclosing "window" select, right→left a "crossing" select.
       *  `start` is captured in sketch mm at pointerdown; `current`
       *  tracks the cursor. `startPx`/`currentPx` are the matching
       *  client-pixel corners, used to decide whether the gesture was
       *  a real drag or just a click. */
      kind: "marquee";
      start: [number, number];
      current: [number, number];
      startPx: [number, number];
      currentPx: [number, number];
    };

/**
 * F7: expand a rotated rectangle (four corners) into the sub-calls that
 * realise it: four `sketch_line` edges around the loop, coincident
 * constraints joining adjacent corners so the loop stays closed under
 * solving, and perpendicular constraints on adjacent edges so the right
 * angles are locked (three suffice to fix all four corners). Entity ids
 * are predicted the way the line-commit path does — each `sketch_line`
 * allocates start, end, line in that order — so the constraints reference
 * the lines/points the kernel is about to create.
 */
function buildRect3pSubCalls(
  corners: [
    [number, number],
    [number, number],
    [number, number],
    [number, number],
  ],
  subCalls: SketchSessionState["subCalls"],
): Array<{ type: string; params: Record<string, unknown> }> {
  const base = predictNextLineIds(subCalls).start;
  // Per line i: startPt = base + 3i, endPt = base + 3i + 1, line = base + 3i + 2.
  const startPt = (i: number) => base + 3 * i;
  const endPt = (i: number) => base + 3 * i + 1;
  const lineId = (i: number) => base + 3 * i + 2;

  const calls: Array<{ type: string; params: Record<string, unknown> }> = [];
  for (let i = 0; i < 4; i++) {
    const a = corners[i];
    const b = corners[(i + 1) % 4];
    calls.push({
      type: "sketch_line",
      params: {
        start: [a[0], a[1]] as [number, number],
        end: [b[0], b[1]] as [number, number],
      },
    });
  }
  // Close the loop: line i's end coincides with line (i+1)'s start.
  for (let i = 0; i < 4; i++) {
    calls.push({
      type: "sketch_constraint_coincident",
      params: { a: endPt(i), b: startPt((i + 1) % 4) },
    });
  }
  // Lock the right angles: perpendicular between consecutive edges. Three
  // constraints pin all four corners (the fourth angle follows).
  for (let i = 0; i < 3; i++) {
    calls.push({
      type: "sketch_constraint_perpendicular",
      params: { line_a: lineId(i), line_b: lineId(i + 1) },
    });
  }
  return calls;
}

/**
 * K8.0 reliability fix: read the camera bridge state but
 * override `width/height` with the canvas element's real
 * `getBoundingClientRect()` dimensions. The R3F publisher
 * sometimes publishes stale dimensions while the WebGL canvas
 * is still resizing to its container, which mis-maps pointer
 * coordinates and drawing positions. The overlay owns its own
 * DOM node and knows its size reliably via ResizeObserver.
 */
function readCamForCanvas(
  canvas: HTMLCanvasElement,
): Readonly<SketchCameraState> {
  const cam = readSketchCamera();
  const rect = canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return cam;
  return { ...cam, width: rect.width, height: rect.height };
}

/** Ticket D1 (jq6i / 3rz2): the active sketch tool's first-step instruction for
 *  the floating ToolHint pill, or null for tools that need no guidance. */
function initialToolHint(tool: string | null): string | null {
  switch (tool) {
    case "sketch_mirror":
      return "Spiegeln: Spiegelachse anklicken.";
    case "sketch_move":
      return "Verschieben: selektierte Geometrie ziehen, oder Basis- dann Zielpunkt anklicken.";
    case "sketch_copy":
      return "Kopieren: Basispunkt anklicken, dann Zielpunkt.";
    default:
      return null;
  }
}

export function SketchCanvas2d({ store, session }: Props): ReactElement | null {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const dragRef = useRef<DragState>({ kind: "idle" });
  // Live grip-drag solver: re-solves the sketch every pointer frame so
  // constraints hold visually while dragging (continuation model), not
  // just at drop. One stable instance for the component's lifetime.
  const dragControllerRef = useRef<DragController>(new DragController());
  const hoverRef = useRef<[number, number] | null>(null);
  const snapRef = useRef<SnapResolution>({
    snapped: [0, 0],
    candidate: null,
    anchor: null,
    coincidentTargetId: null,
  });
  const rafRef = useRef<number | null>(null);
  const lastCamVersionRef = useRef<number>(-1);
  const lastSessionRef = useRef<SketchSessionState | null>(null);
  const projectedEdgesRef = useRef<ProjectedEdge[]>([]);
  // Ticket czqf: the line under the cursor while the Mirror tool is active,
  // highlighted as the axis candidate. Endpoints in sketch (u,v).
  const mirrorAxisHoverRef = useRef<
    [[number, number], [number, number]] | null
  >(null);
  // Ticket jq6i: the Mirror tool is a two-step Onshape flow now — first click
  // picks the axis (sub-call index stored here), then clicks toggle the objects
  // to mirror into the sketch selection; Enter applies, Esc cancels. `null`
  // means "no axis yet" (step 1).
  const mirrorAxisRef = useRef<number | null>(null);
  // F10: the first line picked in the two-click fillet flow (sub-call
  // index), pending the second pick. Reset whenever the tool changes.
  const filletFirstRef = useRef<number | null>(null);
  // F11: the first line picked in the two-click chamfer flow (sub-call
  // index), pending the second pick. Reset whenever the tool changes.
  const chamferFirstRef = useRef<number | null>(null);
  // F17: the base point of the two-click Move / Copy flow (sketch mm),
  // pending the target click. Reset whenever the tool changes.
  const moveBaseRef = useRef<[number, number] | null>(null);
  // Latest session, readable from imperative (non-React) listeners
  // like the native wheel handler below.
  const sessionRef = useRef<SketchSessionState>(session);
  sessionRef.current = session;

  // Overlay redraw, driven from the R3F render frame (CameraBridgePublisher)
  // so the HUD tracks the camera in lockstep with the WebGPU render instead
  // of lagging a frame behind on this component's own rAF. `redrawRef` is
  // refreshed each render below with a closure over the current helpers.
  const redrawRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    setOverlayRedraw(() => redrawRef.current?.());
    return () => setOverlayRedraw(null);
  }, []);

  // Touch pinch-to-zoom — the two-finger camera gesture with its lockout
  // lives in `sketchCanvasTouch.ts`; one stable instance per canvas.
  const touchPinchRef = useRef<SketchTouchPinch>(new SketchTouchPinch());

  // F18 (reworked): click-to-edit dimension. While drawing, the canvas
  // shows only the unobtrusive numeric readout; once a line is placed,
  // its sub-call index lands in `recentLineRef` and the readout stays
  // visible at the committed line. Clicking the readout opens `editor`
  // — a small input at the label anchor; Enter applies the value,
  // Esc / click-away dismisses. Two kinds share the same widget:
  //   - "line": the just-drawn line's exact length (store.setSketchLineLength)
  //   - "dimension": an existing distance-dimension constraint placed by the
  //     Dimension tool, edited as a CONSTRAINT (store.setSketchConstraintValue).
  // `value`/`unit` are in the workspace length unit, e.g. "15"/"cm".
  const recentLineRef = useRef<number | null>(null);
  const [editor, setEditor] = useState<{
    kind: "line" | "dimension";
    index: number;
    left: number;
    top: number;
    value: string;
    unit: string;
  } | null>(null);
  const lengthInputRef = useRef<HTMLInputElement | null>(null);

  // Push the workspace length unit into the overlay's units bridge so the
  // module-level dimension drawing (drawLinearDimension, the live readouts)
  // renders "15 cm" etc., and redraw once when it changes.
  const ws = useWorkspaceUnits();
  useEffect(() => {
    setSketchLengthUnit(ws.lengthUnit, decimalPlaces(ws.lengthDecimals));
    redrawRef.current?.();
  }, [ws.lengthUnit, ws.lengthDecimals]);

  // Ticket pqoe: in-scene text editor. While the Text tool is active a
  // click on empty canvas opens an input field + param popover positioned
  // at the click point (screen px) anchored to the sketch-plane location
  // `(u, v)` so it tracks pan/zoom on redraw. Typing drives the store's
  // pending text value; Enter / blur commits a `sketch_text` placement at
  // `(u, v)` via store.placeSketchText (the same path the toolbar drove
  // before). Mirrors the line-length `editor` mechanism above.
  // Plan item 5: the editor runs in two modes. `group === null` is the
  // existing "place new" mode (a click on empty canvas while the Text tool
  // is active → store.placeSketchText). `group` set is "edit existing" mode
  // (a Select-mode click on a baked text → store.updateSketchTextGroup),
  // entered with the editor pre-filled from the clicked text's recipe.
  const [textEditor, setTextEditor] = useState<{
    left: number;
    top: number;
    u: number;
    v: number;
    group: string | null;
  } | null>(null);
  const textInputRef = useRef<HTMLInputElement | null>(null);

  // Plan item 4: in-scene re-edit of a baked fillet radius / chamfer
  // setback. In Select mode, clicking a recipe-tagged rounding arc /
  // bevel line opens a small input at the corner's anchor (arc midpoint /
  // bevel midpoint) pre-filled with the current value. Enter commits via
  // store.setFilletRadiusAt / setChamferDistanceAt, Esc / tool-change
  // dismisses. Mirrors the line-length `editor` mechanism above; the
  // anchor `(u, v)` is re-projected on redraw so it tracks pan/zoom.
  const [cornerEditor, setCornerEditor] = useState<{
    kind: "fillet" | "chamfer";
    subCallId: string;
    u: number;
    v: number;
    left: number;
    top: number;
    value: string;
  } | null>(null);
  const cornerInputRef = useRef<HTMLInputElement | null>(null);

  // The text params (value / height / font) live in top-level store state,
  // not in `session`, so subscribe to the store to re-render the in-scene
  // fields when the user types or switches fonts. Snapshot the slice the
  // editor reads; identity-stable so React bails out when it's unchanged.
  const textParams = useSyncExternalStore(
    store ? store.subscribe : NOOP_SUBSCRIBE,
    () => (store ? textParamsSnapshot(store) : EMPTY_TEXT_PARAMS),
    () => EMPTY_TEXT_PARAMS,
  );

  // Ticket pqoe: the corner-op tools (fillet / chamfer / offset) carry a
  // single numeric param that used to live in the toolbar. It now shows as
  // an in-scene input pinned to the canvas corner while that tool is active
  // — same store binding, so the two-click flow is unchanged. Subscribe to
  // the store so the field re-renders when the user edits it.
  const toolParams = useSyncExternalStore(
    store ? store.subscribe : NOOP_SUBSCRIBE,
    () => (store ? toolParamsSnapshot(store) : EMPTY_TOOL_PARAMS),
    () => EMPTY_TOOL_PARAMS,
  );

  // Ticket pqoe (plan items 2 + 3): the selection-transform values (pattern
  // count / spacing / angle, rotate angle) used to live in the toolbar. They
  // now show in an in-scene transform panel, shown while a selection exists in
  // Select mode. Same store binding (setSketchPattern* / setSketchRotateAngle
  // + linearPatternSketch / circularPatternSketch / rotateSketchSelection), so
  // the actions are unchanged — only the controls moved. Subscribe so the panel
  // re-renders when the user edits a field.
  const transformParams = useSyncExternalStore(
    store ? store.subscribe : NOOP_SUBSCRIBE,
    () => (store ? transformParamsSnapshot(store) : EMPTY_TRANSFORM_PARAMS),
    () => EMPTY_TRANSFORM_PARAMS,
  );

  // Tickets yx6a/bxoo: resolve the session's plane query to its world
  // frame and publish it to the camera bridge — sketchToScreen /
  // screenToSketch project through it with the live 3D camera.
  useEffect(() => {
    if (!store || !session.active) {
      setSketchPlaneFrame(null);
      return;
    }
    setSketchPlaneFrame(
      resolvePlaneFrame(
        session.planeQuery ?? "qCreatedBy(origin_plane_xy)",
        store.kernel,
      ),
    );
    return () => setSketchPlaneFrame(null);
  }, [store, session.active, session.planeQuery]);

  // Clear the published 3D text labels when the sketch session ends (this
  // component unmounts) — unlike the HUD they are non-empty at idle (constraint
  // glyphs, recent-line readout), so they would otherwise linger in the scene.
  useEffect(() => () => clearSketchLabels(), []);

  // K6b: project analytic B-Rep edges onto the sketch plane for snap targets.
  useEffect(() => {
    if (!store) {
      projectedEdgesRef.current = [];
      return;
    }
    try {
      const segs: { p0: number[]; p1: number[]; idHash?: string }[] = [];
      const count = store.kernel.bodyCount();
      for (let i = 0; i < count; i++) {
        let bodyEdges: { a: number; b: number; points: number[]; idHash?: string }[];
        try {
          bodyEdges = store.kernel.bodyBrepEdges(i);
        } catch {
          continue;
        }
        for (const e of bodyEdges) {
          const n = e.points.length / 3;
          for (let p = 0; p < n - 1; p++) {
            // Every flattened segment carries its parent edge's id-hash so a
            // Project-tool click resolves to an associative `qEdge(...)` (jcb7).
            segs.push({
              p0: [e.points[p * 3], e.points[p * 3 + 1], e.points[p * 3 + 2]],
              p1: [
                e.points[(p + 1) * 3],
                e.points[(p + 1) * 3 + 1],
                e.points[(p + 1) * 3 + 2],
              ],
              idHash: e.idHash,
            });
          }
        }
      }
      projectedEdgesRef.current = projectEdgesToSketchPlane(segs);
    } catch {
      projectedEdgesRef.current = [];
    }
  }, [store, session.planeQuery]);

  // Forward wheel-zoom to the WebGPU canvas so OrbitControls keeps
  // zooming under the (pointer-events: auto) overlay — in select mode
  // AND while a drawing tool is active (ticket bouv: Onshape zooms with
  // the wheel mid-tool too; the cursor keeps rubber-banding because the
  // overlay only forwards, never swallows, the pointer). Native listener
  // with { passive: false } so we can stop the page from scrolling.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      const s = sessionRef.current;
      if (!s.active) return;
      // Zooming moves the label under an open dimension editor — the
      // DOM field can't follow the canvas transform, so dismiss it.
      setEditor(null);
      // Same for the in-scene text editor (ticket pqoe).
      setTextEditor(null);
      // …and the corner radius / distance editor (plan item 4): its
      // anchor is positioned once at open, so a zoom would orphan it.
      setCornerEditor(null);
      const target = viewportCanvas(canvas);
      if (!target) return;
      target.dispatchEvent(
        new WheelEvent("wheel", {
          deltaX: e.deltaX,
          deltaY: e.deltaY,
          deltaZ: e.deltaZ,
          deltaMode: e.deltaMode,
          clientX: e.clientX,
          clientY: e.clientY,
          bubbles: true,
          cancelable: true,
        }),
      );
      e.preventDefault();
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, []);

  // Reset an in-progress multi-click draft (arc / circle-3p / rect-3p)
  // when the active tool changes, so a stale point can't get committed as
  // the first click of the next gesture.
  useEffect(() => {
    const drag = dragRef.current;
    const k = drag.kind;
    // Ticket egia: leaving the Spline tool (switching tool, Esc-to-select)
    // FINISHES the in-progress spline — its placed control points commit as an
    // open spline instead of vanishing. < 2 points has nothing to commit.
    if (k === "spline" && drag.points.length >= 2 && store) {
      store.pushSketchSubCalls([
        {
          type: "sketch_spline",
          params: {
            points: drag.points.flat(),
            closed: false,
            mode: drag.mode,
          },
        },
      ]);
      scheduleSolverReplay();
      dragRef.current = { kind: "idle" };
      lastCamVersionRef.current = -1;
    }
    if (k === "arc" || k === "circle_3p" || k === "rect_3p") {
      dragRef.current = { kind: "idle" };
      lastCamVersionRef.current = -1;
    }
    // F18: drop an in-flight pending line when leaving the tool.
    if (k === "pending_line") {
      dragRef.current = { kind: "idle" };
      lastCamVersionRef.current = -1;
    }
    // The click-to-edit dimension belongs to the line tool — drop it
    // (and any open editor) on tool change.
    recentLineRef.current = null;
    setEditor(null);
    // Ticket pqoe: the in-scene text editor belongs to the Text tool —
    // drop it on any tool change.
    setTextEditor(null);
    // Plan item 4: the corner editor belongs to Select mode — drop it on
    // any tool change.
    setCornerEditor(null);
    // F10: drop a pending first fillet pick when leaving the tool.
    filletFirstRef.current = null;
    // F17: drop a pending Move / Copy base point when leaving the tool.
    moveBaseRef.current = null;
    // F11: drop a pending first chamfer pick when leaving the tool.
    chamferFirstRef.current = null;
    // jq6i: drop a pending mirror axis when leaving the tool.
    mirrorAxisRef.current = null;
    // o6hx: drop any mirror step-2 hover highlight when leaving the tool.
    setHoveredSketchEntity(null);
    // D1: surface the new tool's first-step instruction as the hint pill.
    setToolHint(initialToolHint(session.selectedTool));
  }, [session.selectedTool]);

  // D1: clear the tool hint when the sketch canvas unmounts (session exit).
  useEffect(() => () => setToolHint(null), []);

  // Size the canvas to its container on mount and on resize.
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() => {
      resizeToContainer(canvas);
      // Force redraw on next frame.
      lastCamVersionRef.current = -1;
    });
    observer.observe(canvas);
    resizeToContainer(canvas);
    return () => observer.disconnect();
  }, []);

  // RAF loop: redraw whenever the camera version or session
  // identity changed. Runs unconditionally, but the redraw is
  // skipped when nothing moved — cheap.
  useEffect(() => {
    const tick = () => {
      const canvas = canvasRef.current;
      if (canvas) {
        const bridge = readSketchCamera();
        if (
          bridge.version !== lastCamVersionRef.current ||
          lastSessionRef.current !== session
        ) {
          lastCamVersionRef.current = bridge.version;
          lastSessionRef.current = session;
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    return () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    };
  }, [session]);

  // Pointer handlers — only fire while the canvas has
  // `pointer-events: auto`, which the CSS only sets when a
  // sketch tool is active (see `.is-tool-active` below).
  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    // Touch: track the pointer; a second finger turns the gesture into a
    // camera pinch. Abort whatever the first finger started, drop any
    // pointer capture it took, and let the pinch own the gesture until all
    // fingers lift.
    if (
      touchPinchRef.current.onPointerDown(e, () => {
        if (dragRef.current.kind !== "idle") {
          dragRef.current = { kind: "idle" };
          hoverRef.current = null;
        }
      })
    ) {
      lastCamVersionRef.current = -1;
      return;
    }

    // Select mode: no drawing tool active. Left-click hit-tests the
    // committed geometry and toggles it in the selection; clicking
    // empty space clears. Other buttons (pan) are forwarded to the
    // R3F canvas so OrbitControls keeps working under the overlay.
    // zz1r: a constraint TOOL turns committed-entity clicks into a running
    // selection that applies the constraint the moment it fits — the
    // tool-first constrain workflow. Same entity hit-test as select mode,
    // but no grips / marquee / in-scene editors (those are select-mode
    // edits). A miss clears the running selection.
    if (store && session.active && isConstraintTool(session.selectedTool)) {
      if (e.button !== 0) {
        forwardPointerToViewport(e);
        return;
      }
      e.preventDefault();
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [u, v] = screenToSketchFromEvent(e, cam);
      // A click on an existing dimension's value label edits it (constraint),
      // rather than starting a new constraint pick. A click while the editor
      // is open just dismisses it.
      const rect = (e.currentTarget as HTMLCanvasElement).getBoundingClientRect();
      const clickPx: [number, number] = [
        e.clientX - rect.left,
        e.clientY - rect.top,
      ];
      if (editor !== null) {
        setEditor(null);
        lastCamVersionRef.current = -1;
        return;
      }
      if (tryOpenDimensionEditor(clickPx, cam)) {
        lastCamVersionRef.current = -1;
        return;
      }
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const state = buildDraftSketchState(session.subCalls);
      const hit = hitTestSketchEntity(state, [u, v], tolMm);
      if (hit === null) {
        store.clearSketchSelection();
      } else {
        store.pickSketchConstraintEntity(hit);
      }
      lastCamVersionRef.current = -1;
      return;
    }
    if (store && session.active && !session.selectedTool) {
      if (e.button !== 0) {
        forwardPointerToViewport(e);
        return;
      }
      e.preventDefault();
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [u, v] = screenToSketchFromEvent(e, cam);

      // Ticket matf: resolve the precise HANDLE under the cursor (a free
      // point, a line endpoint, a polygon/spline vertex, a circle/arc
      // centre/rim) up-front. Whether grabbing it reshapes that one degree of
      // freedom (a lone handle) or rigid-translates a group (a handle inside a
      // multi-selection) is decided by `planSelectDrag` below, once the broad
      // entity hit is also known. Searches ALL committed entities so a direct
      // pointer-down grabs without a prior selecting click (ticket t11x).
      let directGrip: Grip | null = null;
      {
        const grip = pickGrip(e, cam, session);
        if (grip) {
          directGrip = grip;
        } else {
          const rect = (
            e.currentTarget as HTMLCanvasElement
          ).getBoundingClientRect();
          const pointerPx: [number, number] = [
            e.clientX - rect.left,
            e.clientY - rect.top,
          ];
          directGrip = pickGripAt(
            allGrips(session.subCalls),
            pointerPx,
            (pos) => sketchToScreen(pos[0], pos[1], cam),
            GRIP_PICK_PX,
          );
        }
      }
      const handleEntityId =
        directGrip !== null
          ? entityIdForGrip(session.subCalls, directGrip)
          : null;

      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);

      // Plan item 4: a click on a recipe-tagged corner (fillet arc /
      // chamfer bevel) opens the in-scene radius / distance editor instead
      // of selecting. The tagged-corner test reads sub-call geometry
      // directly (a fillet arc isn't a picked entity), so it must run
      // before the entity hit-test. A click elsewhere while the editor is
      // open just dismisses it.
      if (cornerEditor !== null) {
        setCornerEditor(null);
        lastCamVersionRef.current = -1;
        return;
      }
      const corner = hitTestTaggedCorner(session.subCalls, [u, v], tolMm);
      if (corner !== null) {
        openCornerEditor(corner, cam);
        lastCamVersionRef.current = -1;
        return;
      }

      // Plan item 5: a click on a recipe-tagged baked text reopens the
      // in-scene text editor pre-filled with the stored value/height/font,
      // in "edit existing group" mode. Runs after the corner test (a tagged
      // corner is a small precise target) but before the entity hit-test
      // (clicking a glyph fill should edit the text, not pick a region). A
      // click while the editor is already open just commits + dismisses
      // (mirrors the corner editor), rather than falling through to select.
      if (textEditor !== null) {
        commitTextEditor();
        lastCamVersionRef.current = -1;
        return;
      }
      const text = hitTestTaggedText(session.subCalls, [u, v], tolMm);
      if (text !== null) {
        openTextEditor(text, cam);
        lastCamVersionRef.current = -1;
        return;
      }

      // A click on an existing dimension's value label re-edits it (constraint
      // value), before the entity hit-test so it wins over selecting the line.
      // A click while the editor is open just dismisses it.
      {
        const rect = (e.currentTarget as HTMLCanvasElement).getBoundingClientRect();
        const clickPx: [number, number] = [
          e.clientX - rect.left,
          e.clientY - rect.top,
        ];
        if (editor !== null) {
          setEditor(null);
          lastCamVersionRef.current = -1;
          return;
        }
        if (tryOpenDimensionEditor(clickPx, cam)) {
          lastCamVersionRef.current = -1;
          return;
        }
      }

      const state = buildDraftSketchState(session.subCalls);
      const hit = hitTestSketchEntity(state, [u, v], tolMm);

      // Ticket matf: decide the drag gesture from {handle, entity hit,
      // selection}. A lone handle reshapes its DoF (grip); a handle inside a
      // multi-selection, or any entity body, rigid-translates a group.
      const plan = planSelectDrag({
        selection: session.selection,
        handleEntityId,
        hitEntityId: hit,
      });

      if (plan.kind === "grip" && directGrip) {
        (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
        if (plan.selectId !== null) store.setSketchSelection([plan.selectId]);
        dragRef.current = { kind: "grip", grip: directGrip };
        // Live-solve from here: every move re-solves so constraints stay
        // satisfied as the grip moves, and drop just settles the last frame.
        dragControllerRef.current.start(store, directGrip);
        lastCamVersionRef.current = -1;
        return;
      }

      if (plan.kind === "translate") {
        // Arm a rigid-translate of `entityIds`. The move lands on release
        // (current − start) so the solver runs once; a release WITHOUT a real
        // drag falls back to the historic additive toggle (handled in
        // onPointerUp), so plain click-to-build-a-selection still works. The
        // selection is set lazily on the first real move (below), not here —
        // arming must not disturb a no-drag click's toggle.
        (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
        dragRef.current = {
          kind: "select_drag",
          entityIds: plan.entityIds,
          selectIds: plan.selectIds,
          start: [u, v],
          current: [u, v],
          moved: false,
        };
        lastCamVersionRef.current = -1;
        return;
      }

      // Nothing under the cursor: begin a marquee. We DON'T clear the
      // selection yet — that happens on pointerup if the gesture turns out to
      // be a plain click (no drag).
      {
        const canvas = e.currentTarget as HTMLCanvasElement;
        canvas.setPointerCapture(e.pointerId);
        const rect = canvas.getBoundingClientRect();
        const startPx: [number, number] = [
          e.clientX - rect.left,
          e.clientY - rect.top,
        ];
        dragRef.current = {
          kind: "marquee",
          start: [u, v],
          current: [u, v],
          startPx,
          currentPx: startPx,
        };
      }
      lastCamVersionRef.current = -1;
      return;
    }
    if (!store || !session.active || !session.selectedTool) return;
    if (e.button !== 0) {
      // Ticket bxoo: the normal camera stays live during the session —
      // forward pan/rotate gestures (right/middle drag) to OrbitControls
      // mid-tool too, same as select mode.
      forwardPointerToViewport(e);
      return;
    }
    e.preventDefault();
    (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    let [u, v] = screenToSketchFromEvent(e, cam);
    // Trim: not a drawing tool — a single click cuts the curve under the
    // cursor at its nearest intersections and drops the picked piece. No
    // drag, no snap, no grid; the raw click decides which segment dies.
    if (session.selectedTool === "sketch_trim") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      store.trimSketchAt([u, v], tolMm);
      lastCamVersionRef.current = -1;
      return;
    }
    // Text (ticket pqoe): not a drawing tool — a click opens an in-scene
    // input field + param popover at the click point so the user types /
    // tweaks the text right where it lands instead of in the toolbar.
    // Enter / blur commits the placement via store.placeSketchText with the
    // baseline-left at this sketch-plane location (see the JSX below and
    // sketchText.ts). A second click while the editor is open just moves it
    // to the new location.
    if (session.selectedTool === "sketch_text") {
      // A click while an editor is already open commits the current text
      // first, then re-opens at the new location (Onshape-style).
      if (textEditor !== null) {
        commitTextEditor();
      }
      // Ticket lycy: every fresh create-mode open starts with an EMPTY
      // value — the persisted store value must not leak the previously
      // typed text into a new drop (clicking elsewhere would otherwise
      // re-place the last string). Edit mode (openTextEditor) still seeds
      // the clicked text's value, unaffected.
      store.setSketchTextValue("");
      const canvas = e.currentTarget as HTMLCanvasElement;
      const rect = canvas.getBoundingClientRect();
      setTextEditor({
        left: e.clientX - rect.left,
        top: e.clientY - rect.top,
        u,
        v,
        group: null,
      });
      requestAnimationFrame(() => {
        textInputRef.current?.focus();
        textInputRef.current?.select();
      });
      lastCamVersionRef.current = -1;
      return;
    }
    // Split: keep-both sibling of Trim — also a single-click editing mode
    // (not a kernel tool). The click picks the nearest curve under the
    // cursor and cuts it into two pieces meeting at the click point; both
    // halves survive (see store.splitSketchAt / sketchSplit.ts).
    if (session.selectedTool === "sketch_split") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const pick = nearestTrimTarget(session.subCalls, [u, v], tolMm);
      if (pick !== null) store.splitSketchAt(pick, [u, v], tolMm);
      lastCamVersionRef.current = -1;
      return;
    }
    // Extend: complement of Trim, also a single-click editing mode (not a
    // kernel tool). The click picks the nearest LINE under the cursor and
    // lengthens its grabbed end (the one nearer the click) toward the
    // nearest other curve (see store.extendSketchAt / sketchExtend.ts).
    if (session.selectedTool === "sketch_extend") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const pick = nearestLineSubCall(session.subCalls, [u, v], tolMm);
      if (pick !== null) store.extendSketchAt(pick, [u, v], tolMm);
      lastCamVersionRef.current = -1;
      return;
    }
    // Fillet (tickets tqi4/chfp): single CLICK on a CORNER rounds it. The
    // corner under the cursor resolves to the two line edges meeting there —
    // exploding a Polygon / N-gon / Rectangle into lines first if needed — and
    // the fillet applies with the current radius. A generous corner tolerance
    // (vertices are small targets).
    if (session.selectedTool === "sketch_fillet") {
      const tolMm = (SELECT_PICK_PX * 2.5) / Math.max(cam.zoom, 1e-6);
      store.filletCornerAt([u, v], store.getState().sketchToolParams.filletRadius, tolMm);
      lastCamVersionRef.current = -1;
      return;
    }
    // Chamfer (tickets tqi4/chfp): single CLICK on a corner bevels it — same
    // corner resolution as fillet, applied with the current setback distance.
    if (session.selectedTool === "sketch_chamfer") {
      const tolMm = (SELECT_PICK_PX * 2.5) / Math.max(cam.zoom, 1e-6);
      store.chamferCornerAt(
        [u, v],
        store.getState().sketchToolParams.chamferDistance,
        tolMm,
      );
      lastCamVersionRef.current = -1;
      return;
    }
    // F17: Move / Copy — two-click transform of the current selection (not a
    // kernel tool). The user selects geometry first (Select mode), picks
    // Move or Copy, then clicks a BASE point and a TARGET point: the
    // selection is translated by (target − base), in place for Move or as
    // appended copies for Copy. The first click records the base in
    // `moveBaseRef`; the second click applies the delta and clears it.
    if (
      session.selectedTool === "sketch_move" ||
      session.selectedTool === "sketch_copy"
    ) {
      const copy = session.selectedTool === "sketch_copy";
      // Ticket 3rz2: if a base point is already pending (Onshape base→target
      // path), the second click applies the delta. Otherwise, a press ON an
      // entity selects it (if needed) and arms a direct click-drag; a press on
      // empty space records the base point for the two-click path.
      const base = moveBaseRef.current;
      if (base !== null) {
        store.moveSketchSelection(u - base[0], v - base[1], copy);
        moveBaseRef.current = null;
        lastCamVersionRef.current = -1;
        return;
      }
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const state = buildDraftSketchState(session.subCalls);
      const hit = hitTestSketchEntity(state, [u, v], tolMm);
      if (hit !== null) {
        if (!session.selection.includes(hit)) store.setSketchSelection([hit]);
        (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
        dragRef.current = {
          kind: "move_drag",
          start: [u, v],
          current: [u, v],
          copy,
          moved: false,
        };
      } else {
        // Empty space → start the two-click base→target transform.
        moveBaseRef.current = [u, v];
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // K6b.8: Ctrl = snap to grid. Round to nearest grid step.
    if (e.ctrlKey || e.metaKey) {
      [u, v] = gridSnapped(u, v, cam.zoom);
    }
    // Resolve snap at pointerdown so the placed point/line-start
    // respects endpoint / origin magnetism even on a dead-stop
    // click (no prior move events).
    const snap = resolveSnap(
      [u, v],
      session,
      null,
      snapRadiusMmFor(cam),
      e.shiftKey,
      projectedEdgesRef.current,
    );
    snapRef.current = snap;
    const [su, sv] = snap.snapped;
    if (session.selectedTool === "sketch_point") {
      store.setSketchToolDraft({ at: [su, sv] });
      store.applySketchToolDraft();
      return;
    }
    if (session.selectedTool === "sketch_line") {
      // F18: a second click while a pending (click-started) line is in
      // flight commits it as a free line to the click point.
      if (dragRef.current.kind === "pending_line") {
        const pending: Extract<DragState, { kind: "pending_line" }> = {
          ...dragRef.current,
          current: [su, sv],
        };
        const committed = commitPendingLine(pending);
        if (!committed) {
          // Degenerate (clicked the start again): keep the pending line
          // alive rather than dropping it silently.
          dragRef.current = pending;
        }
        lastCamVersionRef.current = -1;
        return;
      }
      // Click on the recent line's dimension readout → open the editor
      // instead of starting a new line. A click anywhere else while the
      // editor is open only dismisses it.
      const canvas = e.currentTarget as HTMLCanvasElement;
      const rect = canvas.getBoundingClientRect();
      const clickPx: [number, number] = [
        e.clientX - rect.left,
        e.clientY - rect.top,
      ];
      if (editor !== null) {
        setEditor(null);
        lastCamVersionRef.current = -1;
        return;
      }
      const recent = recentLineGeometry();
      if (recent !== null) {
        const anchor = distanceLabelAnchorPx(recent.start, recent.end, cam);
        if (
          anchor !== null &&
          Math.hypot(clickPx[0] - anchor[0], clickPx[1] - anchor[1]) <=
            DIM_LABEL_HIT_PX
        ) {
          const u = getSketchLengthUnit();
          setEditor({
            kind: "line",
            index: recentLineRef.current!,
            left: anchor[0],
            top: anchor[1],
            value: sketchDisplayLength(
              measuredLength(recent.start, recent.end),
            ).toFixed(u.decimals),
            unit: u.unit,
          });
          requestAnimationFrame(() => {
            lengthInputRef.current?.focus();
            lengthInputRef.current?.select();
          });
          return;
        }
      }
      // Starting a new line retires the previous line's readout.
      recentLineRef.current = null;
      dragRef.current = {
        kind: "line",
        start: [su, sv],
        current: [su, sv],
        startAnchorId: snap.coincidentTargetId,
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_rectangle") {
      dragRef.current = {
        kind: "rectangle",
        start: [su, sv],
        current: [su, sv],
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_circle") {
      dragRef.current = {
        kind: "circle",
        center: [su, sv],
        current: [su, sv],
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_ellipse") {
      dragRef.current = {
        kind: "ellipse",
        center: [su, sv],
        current: [su, sv],
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_slot") {
      dragRef.current = {
        kind: "slot",
        center_a: [su, sv],
        current: [su, sv],
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_rectangle_center") {
      dragRef.current = {
        kind: "rect_center",
        center: [su, sv],
        current: [su, sv],
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_regular_polygon") {
      dragRef.current = {
        kind: "regular_polygon",
        center: [su, sv],
        current: [su, sv],
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_polygon") {
      // If the previous polygon was applied (draft cleared),
      // start fresh rather than appending to the old one.
      const draftVerts = session.toolDraft?.vertices;
      const draftEmpty =
        !draftVerts || (Array.isArray(draftVerts) && draftVerts.length === 0);
      const existing =
        dragRef.current.kind === "polygon" && !draftEmpty
          ? dragRef.current.vertices
          : [];

      // Close-on-first-vertex: if we have ≥ 3 vertices and
      // the click is within 8 px of the first vertex, auto-
      // close and commit instead of adding another vertex.
      if (existing.length >= 3) {
        const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
        const [fpx, fpy] = sketchToScreen(existing[0][0], existing[0][1], cam);
        const [cpx, cpy] = sketchToScreen(su, sv, cam);
        const dist = Math.hypot(cpx - fpx, cpy - fpy);
        if (dist < 8) {
          // Commit the polygon as-is (without the closing
          // duplicate vertex).
          store.setSketchToolDraft({ vertices: existing.flat() });
          store.applySketchToolDraft();
          dragRef.current = { kind: "idle" };
          lastCamVersionRef.current = -1;
          return;
        }
      }

      const verts: [number, number][] = [...existing, [su, sv]];
      dragRef.current = {
        kind: "polygon",
        vertices: verts,
        current: [su, sv],
      };
      store.setSketchToolDraft({
        vertices: verts.flat(),
      });
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_spline") {
      // Multi-click control-point placement (mirrors the polygon
      // gesture). Finish gestures:
      //   • click within 8 px of the FIRST point (≥ 3 pts) → CLOSE
      //   • double-click, or click within 8 px of the LAST point
      //     (≥ 2 pts) → finish OPEN
      const existing =
        dragRef.current.kind === "spline" ? dragRef.current.points : [];
      // Mode is locked in when the gesture begins (first click); a
      // mid-gesture toggle would desync the preview, so reuse the
      // in-flight drag's mode if one exists.
      const splineMode =
        dragRef.current.kind === "spline"
          ? dragRef.current.mode
          : store.getState().sketchToolParams.splineMode;
      const cam2 = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [cpx, cpy] = sketchToScreen(su, sv, cam2);
      const commitSpline = (closed: boolean) => {
        store.pushSketchSubCalls([
          {
            type: "sketch_spline",
            params: { points: existing.flat(), closed, mode: splineMode },
          },
        ]);
        scheduleSolverReplay();
        dragRef.current = { kind: "idle" };
        lastCamVersionRef.current = -1;
      };
      // Close on first point.
      if (existing.length >= 3) {
        const [fpx, fpy] = sketchToScreen(existing[0][0], existing[0][1], cam2);
        if (Math.hypot(cpx - fpx, cpy - fpy) < 8) {
          commitSpline(true);
          return;
        }
      }
      // Finish open: double-click, or click near the last point.
      if (existing.length >= 2) {
        const last = existing[existing.length - 1];
        const [lpx, lpy] = sketchToScreen(last[0], last[1], cam2);
        if (e.detail >= 2 || Math.hypot(cpx - lpx, cpy - lpy) < 8) {
          commitSpline(false);
          return;
        }
      }
      const points: [number, number][] = [...existing, [su, sv]];
      dragRef.current = {
        kind: "spline",
        points,
        current: [su, sv],
        mode: splineMode,
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_arc") {
      const drag = dragRef.current;
      if (drag.kind !== "arc") {
        // Click 1 — place the centre.
        dragRef.current = {
          kind: "arc",
          center: [su, sv],
          start: null,
          current: [su, sv],
          startAngle: 0,
          sweep: 0,
        };
      } else if (drag.start === null) {
        // Click 2 — fix radius + start angle.
        dragRef.current = {
          ...drag,
          start: [su, sv],
          current: [su, sv],
          startAngle: angleAbout(drag.center, [su, sv]),
          sweep: 0,
        };
      } else {
        // Click 3 — fix the sweep and commit (or discard if degenerate).
        const call = buildArcSubCall(
          drag.center,
          drag.start,
          [su, sv],
          drag.sweep,
        );
        dragRef.current = { kind: "idle" };
        if (call) {
          store.pushSketchSubCalls([call]);
          scheduleSolverReplay();
        }
      }
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_circle_3p") {
      const drag = dragRef.current;
      if (drag.kind !== "circle_3p") {
        // Click 1 — first point on the circumference.
        dragRef.current = {
          kind: "circle_3p",
          p1: [su, sv],
          p2: null,
          current: [su, sv],
        };
      } else if (drag.p2 === null) {
        // Click 2 — second point.
        dragRef.current = { ...drag, p2: [su, sv], current: [su, sv] };
      } else {
        // Click 3 — solve the circumcircle and commit (or discard if
        // the three points are collinear).
        const res = circleThrough3(drag.p1, drag.p2, [su, sv]);
        dragRef.current = { kind: "idle" };
        if (res) {
          // Ticket ybnl: keep the three defining points as real entities (so
          // they render as draggable dots) and bind the circle through them
          // with point-on-object constraints, instead of collapsing to a bare
          // centre+radius circle. Dragging any point re-fits the circle.
          // Id allocation order (mirrors buildDraftSketchState): three points,
          // then the circle's centre + entity.
          const base = predictNextLineIds(session.subCalls).start;
          const circleId = base + 4; // base..+2 points, +3 centre, +4 circle
          store.pushSketchSubCalls([
            { type: "sketch_point", params: { at: drag.p1 } },
            { type: "sketch_point", params: { at: drag.p2 } },
            { type: "sketch_point", params: { at: [su, sv] } },
            {
              type: "sketch_circle",
              params: { center: res.center, radius: res.radius },
            },
            {
              type: "sketch_constraint_point_on_object",
              params: { point: base, curve: circleId },
            },
            {
              type: "sketch_constraint_point_on_object",
              params: { point: base + 1, curve: circleId },
            },
            {
              type: "sketch_constraint_point_on_object",
              params: { point: base + 2, curve: circleId },
            },
          ]);
          scheduleSolverReplay();
        }
      }
      lastCamVersionRef.current = -1;
      return;
    }
    if (session.selectedTool === "sketch_rectangle_3p") {
      const drag = dragRef.current;
      if (drag.kind !== "rect_3p") {
        // Click 1 — first edge endpoint.
        dragRef.current = {
          kind: "rect_3p",
          p1: [su, sv],
          p2: null,
          current: [su, sv],
        };
      } else if (drag.p2 === null) {
        // Click 2 — second edge endpoint (fixes the edge direction).
        dragRef.current = { ...drag, p2: [su, sv], current: [su, sv] };
      } else {
        // Click 3 — the perpendicular width; commit four lines + two
        // perpendicular constraints (or discard if degenerate).
        const corners = rect3pCorners(drag.p1, drag.p2, [su, sv]);
        dragRef.current = { kind: "idle" };
        if (corners) {
          store.pushSketchSubCalls(
            buildRect3pSubCalls(corners, session.subCalls),
          );
          scheduleSolverReplay();
        }
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // F14: Mirror is an interactive editing mode, not a kernel tool. The
    // user first selects the source geometry (Select mode: click / box-
    // select), then picks the Mirror tool, then clicks the AXIS LINE here.
    // The click resolves the nearest line sub-call and reflects every
    // selected simple-geometry sub-call across it, appending editable
    // mirrored copies (see store.mirrorSketchSelection / sketchMirror.ts).
    if (session.selectedTool === "sketch_mirror") {
      // Ticket jq6i: Onshape-style two-step flow.
      // Step 1 (no axis yet): the click picks the mirror axis line.
      if (mirrorAxisRef.current === null) {
        const tolMm = (SELECT_PICK_PX * 2.5) / Math.max(cam.zoom, 1e-6);
        const axis = nearestLineSubCall(session.subCalls, [u, v], tolMm);
        if (axis !== null) {
          mirrorAxisRef.current = axis;
          store.clearSketchSelection();
          setToolHint("Spiegeln: Teile anklicken zum Spiegeln · Esc beendet.");
        }
        lastCamVersionRef.current = -1;
        return;
      }
      // Step 2 (axis set): ticket ps21 — each click on a part mirrors it
      // LIVE across the axis (Onshape pick-to-apply), no Enter, no
      // selection collecting. Esc ends the tool (keyboard handler below).
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const state = buildDraftSketchState(session.subCalls);
      const hit = hitTestSketchEntity(state, [u, v], tolMm);
      if (hit !== null) store.mirrorSketchEntityAt(mirrorAxisRef.current, hit);
      lastCamVersionRef.current = -1;
      return;
    }
    // F15: Offset is an interactive editing mode, not a kernel tool. A
    // single click picks the nearest curve and appends a parallel copy at
    // the toolbar distance toward the clicked side — the click point
    // doubles as the side selector (a line offsets to the click's side; a
    // circle/arc grows if the click is outside its rim, shrinks if inside).
    // See store.offsetSketchAt / sketchOffset.ts.
    if (session.selectedTool === "sketch_offset") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const pick = nearestTrimTarget(session.subCalls, [u, v], tolMm);
      if (pick !== null) {
        store.offsetSketchAt(
          pick,
          [u, v],
          store.getState().sketchToolParams.offsetDistance,
        );
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // F21 / jcb7: Project / Use external geometry. The faint guides drawn from
    // existing bodies' B-Rep edges (projected onto the sketch plane in
    // `projectedEdgesRef`) are inert reference lines. A single click adopts the
    // nearest guide as an ASSOCIATIVE CONSTRUCTION projection: a `sketch_project`
    // sub-call that stores the edge QUERY (`qEdge(idHash)`), not a frozen copy,
    // so the kernel re-resolves it every regen and the line FOLLOWS the body.
    // Construction matches the CAD "projected reference" convention (visible +
    // snappable, excluded from the profile). A guide whose edge has no exported
    // id-hash falls back to a frozen `sketch_line` (non-associative, but still
    // usable) rather than failing the pick.
    if (session.selectedTool === "sketch_project") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const idx = nearestProjectedEdge(
        projectedEdgesRef.current,
        [u, v],
        tolMm,
      );
      if (idx !== null) {
        const edge = projectedEdgesRef.current[idx];
        store.pushSketchSubCalls([
          edge.idHash
            ? {
                type: "sketch_project",
                params: {
                  edge: `qEdge(${edge.idHash})`,
                  start: edge.p0,
                  end: edge.p1,
                  construction: true,
                },
              }
            : {
                type: "sketch_line",
                params: { start: edge.p0, end: edge.p1, construction: true },
              },
        ]);
        scheduleSolverReplay();
      }
      lastCamVersionRef.current = -1;
      return;
    }
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    // Pinch gesture owns the move while it's live. With both fingers down we
    // dolly by the change in finger spread; with one finger left (mid-lift)
    // we stay inert until it too lifts, so no stray sketch action leaks out.
    if (touchPinchRef.current.onPointerMove(e)) return;

    // P0.3: grip drag lives in select mode (no tool active), so it
    // runs before the drawing-tool guard below. The controller solves
    // every frame (continuation model), so constraints stay satisfied
    // as the grip moves — the geometry the user sees is already solved.
    if (dragRef.current.kind === "grip") {
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      let [u, v] = screenToSketchFromEvent(e, cam);
      if (e.ctrlKey || e.metaKey) {
        [u, v] = gridSnapped(u, v, cam.zoom);
      }
      void dragControllerRef.current.update([u, v]);
      lastCamVersionRef.current = -1;
      return;
    }
    // Ticket 3rz2: direct click-drag of the Move/Copy selection. Track the
    // cursor; the actual translate lands on release (onPointerUp) so the
    // constraint solver runs once, not per frame. The `moved` flag separates a
    // real drag from a plain select-click.
    if (dragRef.current.kind === "move_drag") {
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [u, v] = screenToSketchFromEvent(e, cam);
      const { start } = dragRef.current;
      const movedPx =
        Math.hypot(u - start[0], v - start[1]) * Math.max(cam.zoom, 1e-6);
      dragRef.current = {
        ...dragRef.current,
        current: [u, v],
        moved: dragRef.current.moved || movedPx > 4,
      };
      lastCamVersionRef.current = -1;
      return;
    }
    // Ticket matf: plain Select-mode rigid translate of an entity / group.
    // Track the cursor; the translate lands on release (onPointerUp) so the
    // solver runs once. The `moved` flag separates a real drag from a plain
    // select-click (which falls back to a toggle).
    if (dragRef.current.kind === "select_drag") {
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [u, v] = screenToSketchFromEvent(e, cam);
      const { start } = dragRef.current;
      const movedPx =
        Math.hypot(u - start[0], v - start[1]) * Math.max(cam.zoom, 1e-6);
      const nowMoved = dragRef.current.moved || movedPx > 4;
      // On the FIRST frame that becomes a real drag, set the selection to the
      // grabbed group so it highlights while moving (deferred from arm time so
      // a no-drag click keeps its additive toggle).
      if (nowMoved && !dragRef.current.moved && store) {
        store.setSketchSelection(dragRef.current.selectIds);
      }
      dragRef.current = {
        ...dragRef.current,
        current: [u, v],
        moved: nowMoved,
      };
      lastCamVersionRef.current = -1;
      return;
    }
    // F4: marquee drag also lives in select mode (no tool active), so
    // it runs before the drawing-tool guard. Track both the sketch-mm
    // corner and the client-pixel corner, then redraw.
    if (dragRef.current.kind === "marquee") {
      const canvas = e.currentTarget as HTMLCanvasElement;
      const cam = readCamForCanvas(canvas);
      const [u, v] = screenToSketchFromEvent(e, cam);
      const rect = canvas.getBoundingClientRect();
      dragRef.current = {
        ...dragRef.current,
        current: [u, v],
        currentPx: [e.clientX - rect.left, e.clientY - rect.top],
      };
      lastCamVersionRef.current = -1;
      drawOverlay(
        canvas,
        session,
        cam,
        dragRef.current,
        hoverRef.current,
        snapRef.current,
        projectedEdgesRef.current,
        recentLineGeometry(),
      );
      return;
    }
    // Select mode (no drawing/constraint tool active): highlight the entity
    // under the cursor so the user sees what a click will pick (ticket zadt).
    // SketchSession3D draws the published id amber, mirroring the orange
    // click-selection. Done before the tool guard below — hover is a
    // select-mode-only affordance.
    if (session.active && !session.selectedTool) {
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [u, v] = screenToSketchFromEvent(e, cam);
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const hit = hitTestSketchEntity(
        buildDraftSketchState(session.subCalls),
        [u, v],
        tolMm,
      );
      setHoveredSketchEntity(hit);
      return;
    }
    if (!session.active || !session.selectedTool) return;
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    let [u, v] = screenToSketchFromEvent(e, cam);
    // K6b.8: Ctrl = snap to grid on move too.
    if (e.ctrlKey || e.metaKey) {
      [u, v] = gridSnapped(u, v, cam.zoom);
    }
    if (dragRef.current.kind === "line") {
      const snap = resolveSnap(
        [u, v],
        session,
        dragRef.current.start,
        snapRadiusMmFor(cam),
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
      dragRef.current = { ...dragRef.current, current: snap.snapped };
    } else if (dragRef.current.kind === "pending_line") {
      // F18: rubber-band the click-started line to the cursor (no button
      // held). The length readout is canvas-drawn (drawDistanceLabel).
      const snap = resolveSnap(
        [u, v],
        session,
        dragRef.current.start,
        snapRadiusMmFor(cam),
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
      dragRef.current = { ...dragRef.current, current: snap.snapped };
    } else if (dragRef.current.kind === "rectangle") {
      // Rectangles don't run inference at the drag endpoint —
      // the H/V constraints are implicit in the tool itself, so
      // an orientation snap would be redundant. Shift still
      // suppresses; hover snap is also disabled while dragging.
      dragRef.current = { ...dragRef.current, current: [u, v] };
      snapRef.current = {
        snapped: [u, v],
        candidate: null,
        anchor: null,
        coincidentTargetId: null,
      };
    } else if (dragRef.current.kind === "circle") {
      dragRef.current = { ...dragRef.current, current: [u, v] };
    } else if (dragRef.current.kind === "ellipse") {
      dragRef.current = { ...dragRef.current, current: [u, v] };
    } else if (dragRef.current.kind === "slot") {
      dragRef.current = { ...dragRef.current, current: [u, v] };
    } else if (dragRef.current.kind === "rect_center") {
      dragRef.current = { ...dragRef.current, current: [u, v] };
      snapRef.current = {
        snapped: [u, v],
        candidate: null,
        anchor: null,
        coincidentTargetId: null,
      };
    } else if (
      dragRef.current.kind === "circle_3p" ||
      dragRef.current.kind === "rect_3p"
    ) {
      dragRef.current = { ...dragRef.current, current: [u, v] };
    } else if (dragRef.current.kind === "regular_polygon") {
      dragRef.current = { ...dragRef.current, current: [u, v] };
      snapRef.current = {
        snapped: [u, v],
        candidate: null,
        anchor: null,
        coincidentTargetId: null,
      };
    } else if (dragRef.current.kind === "arc") {
      const d = dragRef.current;
      if (d.start === null) {
        // Stage 1: rubber-band the radius/start point.
        dragRef.current = { ...d, current: [u, v] };
      } else {
        // Stage 2: sweep, unwrapped so it tracks past ±180°.
        const sweep = unwrapSweep(
          d.startAngle,
          angleAbout(d.center, [u, v]),
          d.sweep,
        );
        dragRef.current = { ...d, current: [u, v], sweep };
      }
      snapRef.current = {
        snapped: [u, v],
        candidate: null,
        anchor: null,
        coincidentTargetId: null,
      };
    } else if (dragRef.current.kind === "polygon") {
      dragRef.current = { ...dragRef.current, current: [u, v] };
      snapRef.current = {
        snapped: [u, v],
        candidate: null,
        anchor: null,
        coincidentTargetId: null,
      };
    } else if (dragRef.current.kind === "spline") {
      // Ticket 8vpe: rubber-band the spline preview to the cursor on EVERY
      // move — including while dragging (button held) after the first point —
      // so the curve continuously follows the pointer, not only between clicks.
      const snap = resolveSnap(
        [u, v],
        session,
        null,
        snapRadiusMmFor(cam),
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
      dragRef.current = { ...dragRef.current, current: snap.snapped };
    } else {
      // Hover path: show a preview point with snap magnetism so
      // the user sees the same feedback a click-to-place would
      // land on.
      const snap = resolveSnap(
        [u, v],
        session,
        null,
        snapRadiusMmFor(cam),
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
      hoverRef.current = snap.snapped;
    }
    // Highlight the line under the cursor as a pick candidate while the Mirror
    // tool (czqf) OR a constraint tool — e.g. Dimension (7lxk) — is armed, so
    // the user sees what a click will target. (Point hover comes from the snap
    // dot above.)
    //
    // Ticket o6hx: the Mirror flow has two steps. STEP 1 (no axis yet) the
    // candidate is the AXIS LINE → highlight the nearest line. STEP 2 (axis
    // set) the candidate is the OBJECT TO MIRROR (point / line / polygon) →
    // highlight the entity under the cursor (same hover as Select mode), and
    // drop the axis-line highlight.
    if (session.selectedTool === "sketch_mirror" && mirrorAxisRef.current !== null) {
      // Step 2: hover the mirror-source entity under the cursor.
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const hovered = hitTestSketchEntity(
        buildDraftSketchState(session.subCalls),
        [u, v],
        tolMm,
      );
      setHoveredSketchEntity(hovered);
      mirrorAxisHoverRef.current = null;
    } else if (
      session.selectedTool === "sketch_mirror" ||
      isConstraintTool(session.selectedTool)
    ) {
      const tolMm = (SELECT_PICK_PX * 2.5) / Math.max(cam.zoom, 1e-6);
      const idx = nearestLineSubCall(session.subCalls, [u, v], tolMm);
      const cv = idx !== null ? curveOfSubCall(session.subCalls[idx]) : null;
      mirrorAxisHoverRef.current =
        cv && cv.kind === "line" ? [cv.a, cv.b] : null;
      setHoveredSketchEntity(null);
    } else {
      mirrorAxisHoverRef.current = null;
    }
    const canvas = canvasRef.current;
    if (canvas) {
      drawOverlay(
        canvas,
        session,
        cam,
        dragRef.current,
        hoverRef.current,
        snapRef.current,
        projectedEdgesRef.current,
        recentLineGeometry(),
        mirrorAxisHoverRef.current,
      );
    }
  };

  const scheduleSolverReplay = () => {
    if (!store) return;
    void solveAndReplace(store);
  };

  // F18: tear down the pending-line draft without committing.
  const cancelPendingLine = () => {
    dragRef.current = { kind: "idle" };
    lastCamVersionRef.current = -1;
  };

  // The committed geometry of the most recently placed line, for the
  // click-to-edit dimension readout. Reads the (post-solve) sub-call
  // params, so it tracks solver rewrites. Clears a stale index.
  const recentLineGeometry = (): RecentLineGeometry | null => {
    const index = recentLineRef.current;
    if (index === null) return null;
    const call = session.subCalls[index];
    if (!call || call.type !== "sketch_line") {
      recentLineRef.current = null;
      return null;
    }
    const start = toXy(call.params.start);
    const end = toXy(call.params.end);
    if (!start || !end) return null;
    return { start, end };
  };

  // Refreshed every render: the redraw the R3F frame calls (via
  // sketchOverlayBridge) to keep the HUD in lockstep with the camera.
  redrawRef.current = () => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    drawOverlay(
      canvas,
      sessionRef.current,
      readCamForCanvas(canvas),
      dragRef.current,
      hoverRef.current,
      snapRef.current,
      projectedEdgesRef.current,
      recentLineGeometry(),
      mirrorAxisHoverRef.current,
    );
  };

  // F18: commit the in-flight pending line as a free line. Returns true
  // when something was committed. The placed line becomes the
  // click-to-edit dimension target.
  const commitPendingLine = (
    drag: Extract<DragState, { kind: "pending_line" }>,
  ): boolean => {
    if (!store) return false;
    const { start, current, startAnchorId } = drag;
    // Reject a degenerate zero-length line.
    if (measuredLength(start, current) < 1e-3) return false;
    const ids = predictNextLineIds(session.subCalls);
    const state = buildDraftSketchState(session.subCalls);
    const calls: Array<{ type: string; params: Record<string, unknown> }> = [
      {
        type: "sketch_line",
        params: {
          start: [start[0], start[1]] as [number, number],
          end: [current[0], current[1]] as [number, number],
        },
      },
    ];
    // Start: an explicit coincident captured at pointerdown (the chain / a
    // point snap) wins; otherwise classify where it landed (ticket inej).
    if (startAnchorId !== null) {
      calls.push({
        type: "sketch_constraint_coincident",
        params: { a: ids.start, b: startAnchorId },
      });
    } else {
      calls.push(
        ...anchorConstraints(ids.start, classifyLineAnchor(start, state)),
      );
    }
    // End: bind it to whatever it snapped onto — a vertex (coincident, e.g.
    // closing a loop) or a line interior (point-on-object). Without this the
    // endpoint drifts off the target when the sketch is later dimensioned.
    calls.push(
      ...anchorConstraints(ids.end, classifyLineAnchor(current, state)),
    );
    recentLineRef.current = session.subCalls.length;
    store.pushSketchSubCalls(calls);
    // Ticket daa8: chain — instead of ending, start a NEW rubber-band line from
    // the just-placed endpoint, coincident with it, so clicking repeatedly draws
    // a connected polyline (Esc / tool switch / re-clicking Line ends it via the
    // tool-change effect + the spline/line Escape handlers).
    dragRef.current = {
      kind: "pending_line",
      start: [current[0], current[1]],
      current: [current[0], current[1]],
      startAnchorId: ids.end,
    };
    lastCamVersionRef.current = -1;
    scheduleSolverReplay();
    return true;
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    // Consume every touch-up for the duration of the pinch lockout; the
    // gesture only fully ends (and sketching resumes) once all fingers
    // are off the glass.
    if (touchPinchRef.current.onPointerUp(e)) return;
    // Ticket 3rz2: commit a direct Move/Copy click-drag. A real drag
    // translates the selection by (current − start); a release without drag was
    // just a select-click (the press already selected the entity).
    if (dragRef.current.kind === "move_drag") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { start, current, copy, moved } = dragRef.current;
      dragRef.current = { kind: "idle" };
      if (moved && store) {
        store.moveSketchSelection(current[0] - start[0], current[1] - start[1], copy);
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // Ticket matf: commit a plain Select-mode rigid translate. A real drag
    // moves the grabbed entity / group by (current − start); a release without
    // a real drag was just a select-click — fall back to toggling the entity
    // (and clearing the rest), so a plain click still selects exactly one.
    if (dragRef.current.kind === "select_drag") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { start, current, moved, entityIds } = dragRef.current;
      dragRef.current = { kind: "idle" };
      if (store) {
        if (moved) {
          store.translateSketchEntities(
            entityIds,
            current[0] - start[0],
            current[1] - start[1],
          );
        } else if (entityIds.length === 1) {
          // A plain click without a drag keeps the historic additive toggle
          // (click entities one by one to build a multi-selection). The drag
          // arming did NOT touch the selection, so a straight toggle is right.
          store.toggleSketchSelection(entityIds[0]);
        }
      }
      lastCamVersionRef.current = -1;
      return;
    }
    if (dragRef.current.kind === "grip") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      dragRef.current = { kind: "idle" };
      // Settle the drag: one authoritative solve at the final cursor
      // (coalescing may have dropped the last live frame), then commit.
      void dragControllerRef.current.finish();
      lastCamVersionRef.current = -1;
      return;
    }
    if (dragRef.current.kind === "marquee") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { start, current, startPx, currentPx } = dragRef.current;
      dragRef.current = { kind: "idle" };
      lastCamVersionRef.current = -1;
      // A gesture under a few pixels is a plain click → fall back to
      // the existing clear-selection behaviour.
      const movedPx = Math.hypot(
        currentPx[0] - startPx[0],
        currentPx[1] - startPx[1],
      );
      if (movedPx < 4) {
        if (store) store.clearSketchSelection();
        return;
      }
      // Drag direction picks the mode: left→right (startU ≤ endU) is an
      // enclosing "window" select; right→left is a crossing select.
      const mode = start[0] <= current[0] ? "enclosing" : "crossing";
      const min: [number, number] = [
        Math.min(start[0], current[0]),
        Math.min(start[1], current[1]),
      ];
      const max: [number, number] = [
        Math.max(start[0], current[0]),
        Math.max(start[1], current[1]),
      ];
      if (store) {
        const state = buildDraftSketchState(session.subCalls);
        store.setSketchSelection(entitiesInBox(state, min, max, mode));
      }
      return;
    }
    if (dragRef.current.kind === "regular_polygon") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { center, current } = dragRef.current;
      dragRef.current = { kind: "idle" };
      const radius = Math.hypot(current[0] - center[0], current[1] - center[1]);
      if (radius < 1e-2) {
        const canvas = canvasRef.current;
        if (canvas) {
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
        return;
      }
      if (!store) return;
      store.pushSketchSubCalls([
        {
          type: "sketch_regular_polygon",
          params: {
            center: [center[0], center[1]] as [number, number],
            radius,
            sides: Number(session.toolDraft?.sides) || 6,
            inscribed: session.toolDraft?.inscribed ?? true,
          },
        },
      ]);
      scheduleSolverReplay();
      return;
    }
    if (dragRef.current.kind === "ellipse") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { center, current } = dragRef.current;
      dragRef.current = { kind: "idle" };
      const radiusX = Math.abs(current[0] - center[0]);
      const radiusY = Math.abs(current[1] - center[1]);
      if (radiusX < 1e-2 || radiusY < 1e-2) {
        const canvas = canvasRef.current;
        if (canvas) {
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
        return;
      }
      if (!store) return;
      store.pushSketchSubCalls([
        {
          type: "sketch_ellipse",
          params: {
            center: [center[0], center[1]] as [number, number],
            radius_x: radiusX,
            radius_y: radiusY,
            rotation: Number(session.toolDraft?.rotation) || 0,
            segments: Number(session.toolDraft?.segments) || 0,
          },
        },
      ]);
      scheduleSolverReplay();
      return;
    }
    if (dragRef.current.kind === "slot") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { center_a, current } = dragRef.current;
      dragRef.current = { kind: "idle" };
      const len = Math.hypot(
        current[0] - center_a[0],
        current[1] - center_a[1],
      );
      if (len < 1e-2) {
        const canvas = canvasRef.current;
        if (canvas) {
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
        return;
      }
      if (!store) return;
      const radius = Number(session.toolDraft?.radius) || 8;
      store.pushSketchSubCalls([
        {
          type: "sketch_slot",
          params: {
            center_a: [center_a[0], center_a[1]] as [number, number],
            center_b: [current[0], current[1]] as [number, number],
            radius,
          },
        },
      ]);
      scheduleSolverReplay();
      return;
    }
    if (dragRef.current.kind === "circle") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { center, current } = dragRef.current;
      dragRef.current = { kind: "idle" };
      const dx = current[0] - center[0];
      const dy = current[1] - center[1];
      const radius = Math.hypot(dx, dy);
      if (radius < 1e-2) {
        const canvas = canvasRef.current;
        if (canvas) {
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
        return;
      }
      if (!store) return;
      store.pushSketchSubCalls([
        {
          type: "sketch_circle",
          params: {
            center: [center[0], center[1]] as [number, number],
            radius,
          },
        },
      ]);
      scheduleSolverReplay();
      return;
    }
    if (dragRef.current.kind === "rect_center") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { center, current } = dragRef.current;
      dragRef.current = { kind: "idle" };
      const res = rectCenterCorners(center, current);
      if (!res) {
        const canvas = canvasRef.current;
        if (canvas) {
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
        return;
      }
      if (!store) return;
      store.pushSketchSubCalls([
        {
          type: "sketch_rectangle",
          params: { corner_a: res.cornerA, corner_b: res.cornerB },
        },
      ]);
      scheduleSolverReplay();
      return;
    }
    if (dragRef.current.kind === "rectangle") {
      (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
      const { start, current } = dragRef.current;
      dragRef.current = { kind: "idle" };
      const [cx, cy] = current;
      // Reject near-zero-area rectangles.
      const dx = cx - start[0];
      const dy = cy - start[1];
      if (dx * dx + dy * dy < 1e-4) {
        const canvas = canvasRef.current;
        if (canvas) {
          drawOverlay(
            canvas,
            session,
            readCamForCanvas(canvas),
            dragRef.current,
            hoverRef.current,
            snapRef.current,
            projectedEdgesRef.current,
            recentLineGeometry(),
          );
        }
        return;
      }
      if (!store) return;
      store.pushSketchSubCalls([
        {
          type: "sketch_rectangle",
          params: {
            corner_a: [start[0], start[1]] as [number, number],
            corner_b: [cx, cy] as [number, number],
          },
        },
      ]);
      scheduleSolverReplay();
      return;
    }
    if (dragRef.current.kind !== "line") return;
    (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    const [u, v] = screenToSketchFromEvent(e, cam);
    const snap = resolveSnap(
      [u, v],
      session,
      dragRef.current.start,
      snapRadiusMmFor(cam),
      e.shiftKey,
      projectedEdgesRef.current,
    );
    snapRef.current = snap;
    const { start, startAnchorId } = dragRef.current;
    dragRef.current = { kind: "idle" };
    const [su, sv] = snap.snapped;
    // F18: a click-without-drag (the pointer barely moved between down and
    // up) starts a PENDING line that rubber-bands to the cursor; a second
    // click commits, Esc cancels. A real drag (moved past the pixel
    // threshold) commits a free line below, exactly as before.
    const [sx0, sy0] = sketchToScreen(start[0], start[1], cam);
    const [sx1, sy1] = sketchToScreen(su, sv, cam);
    const movedPx = Math.hypot(sx1 - sx0, sy1 - sy0);
    if (movedPx < 4) {
      dragRef.current = {
        kind: "pending_line",
        start: [start[0], start[1]],
        current: [start[0], start[1]],
        startAnchorId,
      };
      lastCamVersionRef.current = -1;
      return;
    }
    if (!store) return;

    // K6b.3d: emit the sketch_line call plus any constraint
    // calls that materialise from the snap candidate at the
    // END of the drag (horizontal / vertical / endpoint) and
    // from the anchor match at the START of the drag (another
    // endpoint coincident hit). Entity ids are predicted by
    // replicating the kernel's allocation order so the
    // constraint calls reference the correct line/point.
    const predictedIds = predictNextLineIds(session.subCalls);
    const priorState = buildDraftSketchState(session.subCalls);
    const lineCall = {
      type: "sketch_line",
      params: {
        start: [start[0], start[1]] as [number, number],
        end: [su, sv] as [number, number],
      },
    };
    const newCalls: Array<{ type: string; params: Record<string, unknown> }> = [
      lineCall,
    ];

    // Start: an explicit coincident captured at pointerdown wins; otherwise
    // classify where the start landed (vertex → coincident, line interior →
    // point-on-object) so it stays put under later edits (ticket inej).
    if (startAnchorId !== null) {
      newCalls.push({
        type: "sketch_constraint_coincident",
        params: { a: predictedIds.start, b: startAnchorId },
      });
    } else {
      newCalls.push(
        ...anchorConstraints(
          predictedIds.start,
          classifyLineAnchor(start, priorState),
        ),
      );
    }

    // End snap: orientation (horizontal / vertical) plus a binding. The live
    // candidate covers H/V and an inferred vertex coincident; anything else
    // (a line-interior snap, or a snap the inference missed) is bound by
    // classifying the landed position — ticket inej.
    let endBound = false;
    if (snap.candidate) {
      const c = snap.candidate.constraint;
      if (c.kind === "horizontal") {
        newCalls.push({
          type: "sketch_constraint_horizontal",
          params: { line: predictedIds.line },
        });
      } else if (c.kind === "vertical") {
        newCalls.push({
          type: "sketch_constraint_vertical",
          params: { line: predictedIds.line },
        });
      } else if (c.kind === "coincident" && snap.coincidentTargetId !== null) {
        newCalls.push({
          type: "sketch_constraint_coincident",
          params: { a: predictedIds.end, b: snap.coincidentTargetId },
        });
        endBound = true;
      }
    }
    if (!endBound) {
      newCalls.push(
        ...anchorConstraints(
          predictedIds.end,
          classifyLineAnchor([su, sv], priorState),
        ),
      );
    }

    // The placed line becomes the click-to-edit dimension target.
    recentLineRef.current = session.subCalls.length;
    store.pushSketchSubCalls(newCalls);
    scheduleSolverReplay();
  };

  const onPointerCancel = (e: React.PointerEvent<HTMLCanvasElement>) => {
    touchPinchRef.current.onPointerCancel(e);
    dragRef.current = { kind: "idle" };
    hoverRef.current = null;
    snapRef.current = {
      snapped: [0, 0],
      candidate: null,
      anchor: null,
      coincidentTargetId: null,
    };
    lastCamVersionRef.current = -1;
  };

  // zadt: the entity-hover highlight is a select-mode-only affordance. Drop it
  // whenever the session ends or a tool takes over, so a stale amber stroke
  // can't linger past the gesture that produced it.
  useEffect(() => {
    if (!session.active || session.selectedTool !== null) {
      setHoveredSketchEntity(null);
    }
  }, [session.active, session.selectedTool]);

  const onPointerLeave = () => {
    hoverRef.current = null;
    setHoveredSketchEntity(null); // zadt: drop the entity hover highlight

    snapRef.current = {
      snapped: [0, 0],
      candidate: null,
      anchor: null,
      coincidentTargetId: null,
    };
    lastCamVersionRef.current = -1;
  };

  // F18: Esc dismisses an open dimension editor, else cancels an
  // in-flight pending line. Window-level so it works even when the
  // canvas has focus; the editor input wires its own key handler too.
  useEffect(() => {
    if (!session.active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (editor !== null) {
        e.preventDefault();
        setEditor(null);
        return;
      }
      // Ticket pqoe: Esc dismisses an open in-scene text editor first.
      if (textEditor !== null) {
        e.preventDefault();
        setTextEditor(null);
        return;
      }
      // Plan item 4: Esc dismisses an open corner radius / distance editor.
      if (cornerEditor !== null) {
        e.preventDefault();
        setCornerEditor(null);
        return;
      }
      if (dragRef.current.kind === "pending_line") {
        e.preventDefault();
        cancelPendingLine();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [session.active, editor, textEditor, cornerEditor]);

  // Ticket ps21: Mirror tool — pick-to-apply. Clicks on parts mirror them
  // live across the picked axis (pointerdown handler), so there is no
  // Enter-applies step. Esc ENDS the tool: with an axis set it drops back
  // to the pick-axis step; pressed again (no axis) it leaves Mirror for the
  // default Select tool. Switching to another toolbar button also ends it
  // (the selectedTool effect resets mirrorAxisRef).
  useEffect(() => {
    if (!store || session.selectedTool !== "sketch_mirror") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (mirrorAxisRef.current !== null) {
        mirrorAxisRef.current = null;
        store.clearSketchSelection();
        setHoveredSketchEntity(null); // o6hx: drop step-2 hover highlight
        setToolHint(initialToolHint("sketch_mirror"));
      } else {
        store.selectSketchTool(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store, session.selectedTool]);

  // F23: keyboard finish for the spline gesture — Enter commits the
  // in-progress spline (open, needs ≥ 2 control points); Escape cancels
  // it. Mirrors the click/double-click finish in onPointerDown.
  useEffect(() => {
    if (!store || session.selectedTool !== "sketch_spline") return;
    const onKey = (e: KeyboardEvent) => {
      const drag = dragRef.current;
      if (drag.kind !== "spline") return;
      // Ticket egia: BOTH Enter and Escape FINISH the spline — they commit the
      // control points placed so far as an open spline instead of discarding
      // them. Only an Escape with too few points (< 2) cancels outright.
      if (
        (e.key === "Enter" || e.key === "Escape") &&
        drag.points.length >= 2
      ) {
        e.preventDefault();
        store.pushSketchSubCalls([
          {
            type: "sketch_spline",
            params: {
              points: drag.points.flat(),
              closed: false,
              mode: drag.mode,
            },
          },
        ]);
        scheduleSolverReplay();
        dragRef.current = { kind: "idle" };
        lastCamVersionRef.current = -1;
      } else if (e.key === "Escape") {
        e.preventDefault();
        dragRef.current = { kind: "idle" };
        lastCamVersionRef.current = -1;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store, session.selectedTool]);

  if (!session.active) return null;

  const toolActive = session.selectedTool !== null;
  const selectMode = session.active && !toolActive;

  // Ticket yx6a: while a query picker holds the viewport click focus
  // (session panel's "Sketch plane" field), the overlay must not eat
  // the click — pointer-events drop to none via `.is-query-pick`.
  const queryPickActive = useSyncExternalStore(
    subscribeQueryPicker,
    () => getActiveQueryPicker() !== null,
    () => false,
  );

  // F18 / ticket: apply the typed value from the dimension editor. The field
  // is in the workspace unit, so convert back to mm before it reaches the
  // kernel. "line" rewrites the committed line's exact length; "dimension"
  // edits an existing distance-constraint value. Blank / invalid dismisses.
  const commitEditor = () => {
    if (editor === null || !store) return;
    const typed = parseField(editor.value);
    if (typed !== null && typed > 0) {
      const mm = sketchLengthToMm(typed);
      if (editor.kind === "line") {
        store.setSketchLineLength(editor.index, mm);
      } else {
        store.setSketchConstraintValue(editor.index, mm);
      }
    }
    setEditor(null);
    lastCamVersionRef.current = -1;
  };

  // Ticket: click an existing distance-dimension's value label to edit it as a
  // CONSTRAINT. Hit-tests every committed distance constraint's drawn label
  // against the click; on a hit opens the shared editor pre-filled in the
  // workspace unit. Returns true when it handled the click.
  const tryOpenDimensionEditor = (
    clickPx: [number, number],
    cam: Readonly<SketchCameraState>,
  ): boolean => {
    const skState = buildDraftSketchState(session.subCalls);
    const posOf = (id: number): [number, number] | null => {
      const ent = skState.entities.find((x) => x.id === id);
      if (!ent) return null;
      if (ent.kind === "point") return [ent.at.x, ent.at.y];
      if (ent.kind === "origin") return [0, 0];
      return null;
    };
    for (let i = 0; i < session.subCalls.length; i++) {
      const c = session.subCalls[i];
      if (c.type !== "sketch_constraint_distance") continue;
      // Driven dimensions are informational readouts (the solver doesn't honour
      // a typed value), so they aren't editable — matches the Outliner panel.
      if (c.params.driven === true) continue;
      const a = posOf(c.params.entity_a as number);
      const b = posOf(c.params.entity_b as number);
      if (!a || !b) continue;
      const labelPx = linearDimLabelPx(a, b, cam);
      if (!labelPx) continue;
      if (
        Math.hypot(clickPx[0] - labelPx[0], clickPx[1] - labelPx[1]) <=
        DIM_LABEL_HIT_PX
      ) {
        const mm = c.params.value_mm as number;
        const u = getSketchLengthUnit();
        setEditor({
          kind: "dimension",
          index: i,
          left: labelPx[0],
          top: labelPx[1],
          value: sketchDisplayLength(mm).toFixed(u.decimals),
          unit: u.unit,
        });
        requestAnimationFrame(() => {
          lengthInputRef.current?.focus();
          lengthInputRef.current?.select();
        });
        return true;
      }
    }
    return false;
  };

  // Ticket pqoe: commit the in-scene text editor — place the current text
  // value at the anchored sketch-plane location. Empty / geometry-less text
  // is a no-op inside placeSketchText, so we just close the editor either
  // way. Keeps the editor mechanism symmetric with commitEditor above.
  const commitTextEditor = () => {
    if (textEditor === null || !store) return;
    // Ticket lycy / g91j: an empty / whitespace-only value behaves by mode.
    // In CREATE mode (group === null) it never drops an object — a stray
    // click-away just dismisses (lycy). In EDIT mode (a group is set) the
    // user cleared an existing text, which now DELETES it: remove the group's
    // glyph sub-calls and re-solve, then dismiss (g91j).
    if (store.getState().sketchToolParams.textValue.trim() === "") {
      if (textEditor.group !== null) {
        store.deleteSketchTextGroup(textEditor.group);
      }
      setTextEditor(null);
      lastCamVersionRef.current = -1;
      return;
    }
    if (textEditor.group !== null) {
      // Plan item 5 "edit existing" mode: update the clicked text group in
      // place from the current store params. A graceful no-op (font not
      // loaded / empty text) leaves the old geometry untouched.
      const s = store.getState();
      store.updateSketchTextGroup(textEditor.group, {
        value: s.sketchToolParams.textValue,
        height: s.sketchToolParams.textHeight,
        fontId: s.sketchToolParams.textFontId,
      });
    } else {
      store.placeSketchText([textEditor.u, textEditor.v]);
    }
    setTextEditor(null);
    lastCamVersionRef.current = -1;
  };

  // Plan item 5: open the in-scene text editor over a baked text, pre-filled
  // from its recipe. Pushes the stored value/height/font into the store
  // params (the editor's fields read from there, like the create flow), then
  // anchors the overlay at the recipe origin projected to client px. Enters
  // "edit existing group" mode (commit → updateSketchTextGroup).
  const openTextEditor = (
    text: TaggedTextHit,
    cam: Readonly<SketchCameraState>,
  ) => {
    if (!store) return;
    store.setSketchTextValue(text.value);
    store.setSketchTextHeight(text.height);
    store.setSketchTextFontId(text.fontId);
    const [u, v] = text.origin;
    const [px, py] = sketchToScreen(u, v, cam);
    setTextEditor({ left: px, top: py, u, v, group: text.group });
    requestAnimationFrame(() => {
      textInputRef.current?.focus();
      textInputRef.current?.select();
    });
  };

  // Plan item 4: open the corner radius / distance editor at a tagged
  // corner's anchor. Projects the sketch-plane anchor `(u, v)` to client
  // px (relative to the canvas) and pre-fills the current value; focus +
  // select on the next frame so the field is type-ready.
  const openCornerEditor = (
    corner: TaggedCornerHit,
    cam: Readonly<SketchCameraState>,
  ) => {
    const [u, v] = corner.anchor;
    const [px, py] = sketchToScreen(u, v, cam);
    setCornerEditor({
      kind: corner.kind,
      subCallId: corner.subCallId,
      u,
      v,
      left: px,
      top: py,
      value: corner.value.toFixed(2),
    });
    requestAnimationFrame(() => {
      cornerInputRef.current?.focus();
      cornerInputRef.current?.select();
    });
  };

  // Plan item 4: commit the typed radius / distance to the baked corner.
  // Blank / invalid / non-positive input just dismisses (the setter also
  // guards). Fillet → setFilletRadiusAt, chamfer → setChamferDistanceAt.
  const commitCornerEditor = () => {
    if (cornerEditor === null || !store) return;
    const value = parseField(cornerEditor.value);
    if (value !== null && value > 0) {
      if (cornerEditor.kind === "fillet") {
        store.setFilletRadiusAt(cornerEditor.subCallId, value);
      } else {
        store.setChamferDistanceAt(cornerEditor.subCallId, value);
      }
    }
    setCornerEditor(null);
    lastCamVersionRef.current = -1;
  };

  return (
    <>
      <canvas
        ref={canvasRef}
        data-testid="sketch-canvas-2d"
        className={
          "fd-sketch-canvas" +
          (toolActive
            ? " is-tool-active"
            : selectMode
              ? " is-select-mode"
              : "") +
          (queryPickActive ? " is-query-pick" : "")
        }
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerCancel}
        onPointerLeave={onPointerLeave}
        onContextMenu={(e) => e.preventDefault()}
      />
      {editor !== null && (
        <div
          className="fd-sketch-live-input"
          style={{ left: `${editor.left}px`, top: `${editor.top}px` }}
        >
          <label className="field">
            <input
              ref={lengthInputRef}
              data-testid="sketch-live-length"
              type="text"
              inputMode="decimal"
              value={editor.value}
              onChange={(e) => {
                setEditor((prev) =>
                  prev === null ? prev : { ...prev, value: e.target.value },
                );
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  commitEditor();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  setEditor(null);
                }
              }}
            />
            <span className="unit">{editor.unit}</span>
          </label>
        </div>
      )}
      {/* Plan item 4: in-scene re-edit of a baked fillet radius / chamfer
          setback. Reuses the line-length editor's `.fd-sketch-live-input`
          styling, anchored at the corner. Enter commits via the store's
          per-corner setter; Esc dismisses (window keydown effect). */}
      {cornerEditor !== null && store && (
        <div
          className="fd-sketch-live-input"
          style={{
            left: `${cornerEditor.left}px`,
            top: `${cornerEditor.top}px`,
          }}
        >
          <label className="field">
            <input
              ref={cornerInputRef}
              data-testid={
                cornerEditor.kind === "fillet"
                  ? "sketch-fillet-edit-radius"
                  : "sketch-chamfer-edit-distance"
              }
              type="text"
              inputMode="decimal"
              value={cornerEditor.value}
              onChange={(e) => {
                setCornerEditor((prev) =>
                  prev === null ? prev : { ...prev, value: e.target.value },
                );
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  commitCornerEditor();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  setCornerEditor(null);
                }
              }}
            />
            <span className="unit">mm</span>
          </label>
        </div>
      )}
      {/* Ticket pqoe: in-scene text editor — the input field and its param
          popover (height / font / upload) live HERE at the click location,
          not in the toolbar, and are visible ONLY while a text is being
          edited. The data-testids match the old toolbar fields so the
          existing e2e selectors keep resolving. Enter / blur commits the
          placement; Esc dismisses (handled in the window keydown effect). */}
      {textEditor !== null && store && (
        <div
          className="fd-sketch-text-editor"
          style={{ left: `${textEditor.left}px`, top: `${textEditor.top}px` }}
        >
          <input
            ref={textInputRef}
            className="text-value"
            data-testid="sketch-text-value"
            type="text"
            value={textParams.value}
            placeholder="Text"
            onChange={(e) => store.setSketchTextValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commitTextEditor();
              } else if (e.key === "Escape") {
                e.preventDefault();
                setTextEditor(null);
              }
            }}
          />
          <div className="text-params" data-testid="sketch-text-params">
            <label className="field" title="Text em height (mm)">
              H
              <input
                type="number"
                min="0"
                step="0.5"
                data-testid="sketch-text-height"
                value={textParams.height}
                onChange={(e) =>
                  store.setSketchTextHeight(Number(e.currentTarget.value))
                }
              />
            </label>
            <label className="field" title="Text font">
              F
              <select
                data-testid="sketch-text-font"
                value={textParams.fontId}
                onChange={(e) =>
                  store.setSketchTextFontId(e.currentTarget.value)
                }
              >
                {BUNDLED_FONTS.map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.label}
                  </option>
                ))}
                {textParams.fontId.startsWith(UPLOADED_FONT_PREFIX) && (
                  <option value={textParams.fontId}>
                    {textParams.fontId.slice(UPLOADED_FONT_PREFIX.length)}
                  </option>
                )}
              </select>
            </label>
            {/* Ticket lycy: the in-scene font file-upload control was
                removed — only the dropdown of already-available fonts +
                the height input remain. The store's upload capability
                (loadUploadedTextFont) is untouched. */}
            {!textParams.fontReady && (
              <span className="hint" data-testid="sketch-text-font-loading">
                Loading font…
              </span>
            )}
          </div>
        </div>
      )}
      {/* Ticket pqoe: in-scene tool-param input for the corner-op tools
          (fillet / chamfer / offset). The single numeric parameter that used
          to sit in the toolbar now floats in the canvas corner, visible ONLY
          while its tool is active. It's bound to the SAME store state/setters
          (setSketchFilletRadius etc.), so the two-click tool flow is
          unchanged — only the field moved. The data-testid matches the old
          toolbar field so the existing e2e selectors keep resolving. */}
      {store &&
        (() => {
          const field = toolParamField(session.selectedTool, toolParams);
          if (!field) return null;
          return (
            <div
              className="fd-sketch-tool-param"
              data-testid="sketch-tool-param"
            >
              <label className="field" title={field.title}>
                {field.label}
                <input
                  type="number"
                  min="0"
                  step="0.5"
                  data-testid={field.testId}
                  value={field.value}
                  onChange={(e) =>
                    field.set(store, Number(e.currentTarget.value))
                  }
                />
              </label>
            </div>
          );
        })()}
      {/* Ticket pqoe (plan item 1): in-scene tool-param overlay for the
          regular-polygon tool. Its side count + inscribed/circumscribed flag
          live in the tool DRAFT (session.toolDraft.sides / .inscribed) and
          must be set BEFORE the radius drag — there was never a toolbar field
          for them, so this is a pure addition. Pinned to the same canvas
          corner as the corner-op tool-param (offset down so the two never
          overlap, though only one tool is ever active). Bound to the SAME
          store call the gesture already uses, store.setSketchToolDraft. */}
      {store && session.selectedTool === "sketch_regular_polygon" && (
        <div
          className="fd-sketch-tool-param is-ngon"
          data-testid="sketch-ngon-param"
        >
          <label className="field" title="Number of polygon sides">
            N
            <input
              type="number"
              min="3"
              step="1"
              data-testid="sketch-ngon-sides"
              value={Number(session.toolDraft?.sides) || 6}
              onChange={(e) =>
                store.setSketchToolDraft({
                  sides: Math.max(3, Math.round(Number(e.currentTarget.value))),
                })
              }
            />
          </label>
          <label
            className="field toggle"
            title="Radius is to the corners (inscribed) or to the edge midpoints (circumscribed)"
          >
            <input
              type="checkbox"
              data-testid="sketch-ngon-inscribed"
              checked={session.toolDraft?.inscribed !== false}
              onChange={(e) =>
                store.setSketchToolDraft({ inscribed: e.currentTarget.checked })
              }
            />
            Inscribed
          </label>
        </div>
      )}
      {/* Ticket pqoe (plan items 2 + 3): in-scene selection-transform panel.
          The Pattern (count / spacing / angle + Linear X / Linear Y /
          Circular) and Rotate (angle + Rotate) controls used to live in the
          toolbar; they now float in the canvas corner, shown ONLY in Select
          mode with a non-empty selection (no active drawing tool) — both
          operations act on the current selection. Pinned below the tool-param
          slot so the two never overlap. Bound to the SAME store state/actions
          (setSketchPattern* / setSketchRotateAngle + linearPatternSketch /
          circularPatternSketch / rotateSketchSelection), so behaviour is
          unchanged — only the controls moved. The data-testids match the old
          toolbar fields so the existing e2e selectors keep resolving. */}
      {store && selectMode && session.selection.length > 0 && (
        <div
          className="fd-sketch-transform-panel"
          data-testid="sketch-transform-panel"
        >
          <div className="row">
            <label className="field" title="Pattern instance count">
              N
              <input
                type="number"
                min="1"
                step="1"
                data-testid="sketch-pattern-count"
                value={transformParams.patternCount}
                onChange={(e) =>
                  store.setSketchPatternCount(Number(e.currentTarget.value))
                }
              />
            </label>
            <label className="field" title="Linear pattern spacing (mm)">
              S
              <input
                type="number"
                step="1"
                data-testid="sketch-pattern-spacing"
                value={transformParams.patternSpacing}
                onChange={(e) =>
                  store.setSketchPatternSpacing(Number(e.currentTarget.value))
                }
              />
            </label>
            <label className="field" title="Circular pattern total angle (deg)">
              A
              <input
                type="number"
                step="15"
                data-testid="sketch-pattern-angle"
                value={transformParams.patternAngle}
                onChange={(e) =>
                  store.setSketchPatternAngle(Number(e.currentTarget.value))
                }
              />
            </label>
          </div>
          <div className="row">
            <button
              type="button"
              className="action"
              title="Linear pattern along X"
              data-testid="sketch-pattern-linear-x"
              onClick={() => store.linearPatternSketch("x")}
            >
              Linear X
            </button>
            <button
              type="button"
              className="action"
              title="Linear pattern along Y"
              data-testid="sketch-pattern-linear-y"
              onClick={() => store.linearPatternSketch("y")}
            >
              Linear Y
            </button>
            <button
              type="button"
              className="action"
              title="Circular pattern about the origin"
              data-testid="sketch-pattern-circular"
              onClick={() => store.circularPatternSketch()}
            >
              Circular
            </button>
          </div>
          <div className="row">
            <label className="field" title="Rotation angle (deg)">
              ∠
              <input
                type="number"
                step="15"
                data-testid="sketch-rotate-angle"
                value={transformParams.rotateAngle}
                onChange={(e) =>
                  store.setSketchRotateAngle(Number(e.currentTarget.value))
                }
              />
            </label>
            <button
              type="button"
              className="action"
              title="Rotate selection about the origin"
              data-testid="sketch-rotate-apply"
              onClick={() =>
                store.rotateSketchSelection(transformParams.rotateAngle, false)
              }
            >
              Rotate
            </button>
          </div>
        </div>
      )}
    </>
  );
}

// ─── helpers ────────────────────────────────────────────────────

function toXy(value: unknown): [number, number] | null {
  if (!Array.isArray(value) || value.length !== 2) return null;
  const a = Number(value[0]);
  const b = Number(value[1]);
  if (!Number.isFinite(a) || !Number.isFinite(b)) return null;
  return [a, b];
}

function screenToSketchFromEvent(
  e: React.PointerEvent<HTMLCanvasElement>,
  cam: Readonly<SketchCameraState>,
): [number, number] {
  const rect = (e.currentTarget as HTMLCanvasElement).getBoundingClientRect();
  const px = e.clientX - rect.left;
  const py = e.clientY - rect.top;
  return screenToSketch(px, py, cam);
}

/**
 * P0.3: the grip under the pointer (within `GRIP_PICK_PX`), or null.
 * Compared in screen pixels against each grip of the current
 * selection so the pick tolerance is zoom-independent.
 */
function pickGrip(
  e: React.PointerEvent<HTMLCanvasElement>,
  cam: Readonly<SketchCameraState>,
  session: SketchSessionState,
): Grip | null {
  const grips = gripsForSelection(session.subCalls, session.selection);
  if (grips.length === 0) return null;
  const rect = (e.currentTarget as HTMLCanvasElement).getBoundingClientRect();
  const ppx = e.clientX - rect.left;
  const ppy = e.clientY - rect.top;
  let best: Grip | null = null;
  let bestD = GRIP_PICK_PX;
  for (const g of grips) {
    const [gx, gy] = sketchToScreen(g.pos[0], g.pos[1], cam);
    const d = Math.hypot(ppx - gx, ppy - gy);
    if (d <= bestD) {
      bestD = d;
      best = g;
    }
  }
  return best;
}
