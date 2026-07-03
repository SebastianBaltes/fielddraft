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
import { buildDraftSketchState } from "./sketchSession";
import { parseField } from "./sketchLiveInput";
import { isConstraintTool } from "./sketchConstraints";
import { useWorkspaceUnits, decimalPlaces } from "./settings";
import {
  getSketchLengthUnit,
  setSketchLengthUnit,
  sketchDisplayLength,
  sketchLengthToMm,
} from "./sketchUnitsBridge";
import type { TaggedCornerHit, TaggedTextHit } from "./sketchHitTest";
import { setOverlayRedraw } from "./sketchOverlayBridge";
import { setToolHint } from "./toolHintBridge";
import { setHoveredSketchEntity } from "./sketchEntityHoverBridge";
import { clearSketchLabels } from "./sketchLabelBridge";
import { solveAndReplace } from "./solver/replaySolve";
import { DragController } from "./solver/dragController";
import {
  projectEdgesToSketchPlane,
  resolveSnap,
  snapRadiusMmFor,
  type ProjectedEdge,
  type SnapResolution,
} from "./sketchSnapResolve";
import {
  DIM_LABEL_HIT_PX,
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
  gridSnapped,
  type ActiveToolDrag,
  type SketchToolController,
  type ToolContext,
  type ToolPointerEvent,
} from "./sketchTools/toolTypes";
import { controllerForTool } from "./sketchTools/registry";
import { constraintPickTool } from "./sketchTools/constraintPickTool";
import { selectTool } from "./sketchTools/selectTool";

interface Props {
  store: KernelStore | null;
  session: SketchSessionState;
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

export function SketchCanvas2d({ store, session }: Props): ReactElement | null {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // The in-flight pointer gesture: the owning tool controller + its private
  // drag state, or null when idle (Befund 2 — the old 16-variant DragState
  // union dissolved into the controllers).
  const dragRef = useRef<ActiveToolDrag | null>(null);
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
  // Registry migration: per-tool-activation scratch state for controllers
  // (mirror axis, move base). Cleared whenever the selected tool changes.
  const toolStateRef = useRef<unknown>(null);
  // The ToolContext handed to controllers — rebuilt every render (closes
  // over the current session / editor state), used only at event time.
  const ctxRef = useRef<ToolContext | null>(null);
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
    // A controller-owned gesture is committed or dropped by its owner
    // (spline finishes, multi-click drafts reset, a pending line is
    // cancelled).
    if (dragRef.current !== null && ctxRef.current) {
      dragRef.current.owner.onDeactivate?.(ctxRef.current);
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
    // Registry migration: drop the leaving controller's per-activation
    // scratch state (mirror axis jq6i, move base F17).
    toolStateRef.current = null;
    // o6hx: drop any mirror step-2 hover highlight when leaving the tool.
    setHoveredSketchEntity(null);
    // D1: surface the new tool's first-step instruction as the hint pill
    // (jq6i / 3rz2) — the active controller supplies it.
    const tool = session.selectedTool;
    const controller =
      tool === null
        ? null
        : isConstraintTool(tool)
          ? constraintPickTool
          : controllerForTool(tool);
    setToolHint(controller?.initialHint?.(tool!) ?? null);
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

  // Registry migration: resolve a pointer event into the ToolPointerEvent a
  // controller receives — the cursor in raw / Ctrl-grid-snapped sketch mm and
  // canvas-relative pixels. When `resolveSnapOnDown` is set the shared snap
  // resolution runs at the gridded cursor and lands in the snap slot, exactly
  // where the old inline flow resolved it before the late tool branches.
  const toolPointerEvent = (
    e: React.PointerEvent<HTMLCanvasElement>,
    cam: Readonly<SketchCameraState>,
    resolveSnapOnDown: boolean,
  ): ToolPointerEvent => {
    const rect = (e.currentTarget as HTMLCanvasElement).getBoundingClientRect();
    const clickPx: [number, number] = [
      e.clientX - rect.left,
      e.clientY - rect.top,
    ];
    const raw = screenToSketchFromEvent(e, cam);
    const gridded =
      e.ctrlKey || e.metaKey ? gridSnapped(raw[0], raw[1], cam.zoom) : raw;
    let snap: SnapResolution | null = null;
    if (resolveSnapOnDown) {
      snap = resolveSnap(
        gridded,
        sessionRef.current,
        null,
        snapRadiusMmFor(cam),
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
    }
    return { e, cam, raw, gridded, clickPx, snap };
  };

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
        if (dragRef.current !== null) {
          dragRef.current = null;
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
      if (ctxRef.current) {
        constraintPickTool.onDown?.(
          ctxRef.current,
          toolPointerEvent(e, cam, false),
        );
      }
      return;
    }
    if (store && session.active && !session.selectedTool) {
      if (e.button !== 0) {
        forwardPointerToViewport(e);
        return;
      }
      e.preventDefault();
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      if (ctxRef.current) {
        selectTool.onDown?.(ctxRef.current, toolPointerEvent(e, cam, false));
      }
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
    // Registry dispatch: the active tool's controller handles the gesture.
    const controller = controllerForTool(session.selectedTool);
    if (controller && ctxRef.current) {
      controller.onDown?.(
        ctxRef.current,
        toolPointerEvent(e, cam, controller.resolveSnapOnDown === true),
      );
      return;
    }
    // Unknown tool name (registry miss): mirror the old fall-through — the
    // shared snap still resolves at the (grid-snapped) cursor so the HUD
    // indicator behaves, but no gesture starts.
    toolPointerEvent(e, cam, true);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    // Pinch gesture owns the move while it's live. With both fingers down we
    // dolly by the change in finger spread; with one finger left (mid-lift)
    // we stay inert until it too lifts, so no stray sketch action leaks out.
    if (touchPinchRef.current.onPointerMove(e)) return;

    // Registry dispatch: a select-mode gesture controller (select, move/
    // copy) owns its drag end-to-end — no hover fallback, no tool-mode
    // postlude, mirroring the early-return legacy branches below.
    if (dragRef.current !== null && dragRef.current.owner.selectionGesture) {
      const d = dragRef.current;
      d.owner.onMove?.(
        ctxRef.current!,
        toolPointerEvent(
          e,
          readCamForCanvas(e.currentTarget as HTMLCanvasElement),
          false,
        ),
      );
      return;
    }

    // Select mode (no drawing/constraint tool active): the select controller
    // owns the entity hover highlight (ticket zadt) — the drag cases were
    // already dispatched above through the selection-gesture routing.
    if (session.active && !session.selectedTool) {
      if (ctxRef.current) {
        const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
        selectTool.onMove?.(ctxRef.current, toolPointerEvent(e, cam, false));
      }
      return;
    }
    if (!session.active || !session.selectedTool) return;
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    // The tool-mode pointer event: raw + Ctrl-grid-snapped cursor (K6b.8).
    const ev = toolPointerEvent(e, cam, false);
    const ownedMove =
      dragRef.current !== null
        ? dragRef.current.owner.onMove?.(ctxRef.current!, ev) === true
        : false;
    if (!ownedMove) {
      // Hover path: show a preview point with snap magnetism so
      // the user sees the same feedback a click-to-place would
      // land on.
      const snap = resolveSnap(
        ev.gridded,
        session,
        null,
        snapRadiusMmFor(cam),
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
      hoverRef.current = snap.snapped;
    }
    // Tool-specific hover highlight — the mirror axis / constraint pick
    // candidate (czqf / 7lxk / o6hx). Routed to the active controller;
    // without a hook the highlight clears (the old else-branch).
    {
      const hoverController = isConstraintTool(session.selectedTool)
        ? constraintPickTool
        : controllerForTool(session.selectedTool);
      if (hoverController?.onHover && ctxRef.current) {
        hoverController.onHover(ctxRef.current, ev);
      } else {
        mirrorAxisHoverRef.current = null;
      }
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

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    // Consume every touch-up for the duration of the pinch lockout; the
    // gesture only fully ends (and sketching resumes) once all fingers
    // are off the glass.
    if (touchPinchRef.current.onPointerUp(e)) return;
    // Registry dispatch: a controller-owned drag finishes in its owner.
    if (dragRef.current !== null) {
      const d = dragRef.current;
      d.owner.onUp?.(
        ctxRef.current!,
        toolPointerEvent(
          e,
          readCamForCanvas(e.currentTarget as HTMLCanvasElement),
          false,
        ),
      );
      return;
    }
  };

  const onPointerCancel = (e: React.PointerEvent<HTMLCanvasElement>) => {
    touchPinchRef.current.onPointerCancel(e);
    dragRef.current = null;
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

  // F18: Esc dismisses an open in-scene editor first (dimension value, text,
  // corner radius/distance) — window-level so it works even when the canvas
  // has focus; the editor inputs wire their own key handlers too. Everything
  // else routes to the active tool controller's onKey (pending-line cancel,
  // spline finish, …).
  useEffect(() => {
    if (!session.active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
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
      }
      // Registry dispatch: keys route to the active tool's controller.
      const controller = controllerForTool(sessionRef.current.selectedTool);
      if (controller?.onKey && ctxRef.current) {
        controller.onKey(ctxRef.current, e);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [session.active, editor, textEditor, cornerEditor]);

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

  // Registry migration: the ToolContext handed to controllers. Rebuilt on
  // every render so the ui facade closes over the current editor state; all
  // gesture state lives in refs, so event-time reads stay current.
  ctxRef.current =
    store === null
      ? null
      : {
          store,
          session,
          get projectedEdges() {
            return projectedEdgesRef.current;
          },
          dragOf<T>(owner: SketchToolController): T | null {
            const d = dragRef.current;
            return d !== null && d.owner === owner ? (d.state as T) : null;
          },
          setDrag(owner, state) {
            dragRef.current = { owner, state };
          },
          clearDrag() {
            dragRef.current = null;
          },
          toolState<T>(): T | null {
            return toolStateRef.current as T | null;
          },
          setToolState(state) {
            toolStateRef.current = state;
          },
          resolveSnapAt(ev, cursor, dragStart) {
            return resolveSnap(
              cursor,
              sessionRef.current,
              dragStart,
              snapRadiusMmFor(ev.cam),
              ev.e.shiftKey,
              projectedEdgesRef.current,
            );
          },
          setSnap(snap) {
            snapRef.current = snap;
          },
          setHover(p) {
            hoverRef.current = p;
          },
          setMirrorAxisHover(seg) {
            mirrorAxisHoverRef.current = seg;
          },
          capturePointer(ev) {
            (ev.e.currentTarget as HTMLCanvasElement).setPointerCapture(
              ev.e.pointerId,
            );
          },
          releasePointer(ev) {
            (ev.e.currentTarget as HTMLCanvasElement).releasePointerCapture(
              ev.e.pointerId,
            );
          },
          requestRedraw() {
            lastCamVersionRef.current = -1;
          },
          redrawNow() {
            redrawRef.current?.();
          },
          scheduleSolverReplay,
          gripDrag: dragControllerRef.current,
          ui: {
            dimEditorOpen: () => editor !== null,
            dismissDimEditor: () => setEditor(null),
            openLineLengthEditor: (index, anchorPx, lengthMm) => {
              const unit = getSketchLengthUnit();
              setEditor({
                kind: "line",
                index,
                left: anchorPx[0],
                top: anchorPx[1],
                value: sketchDisplayLength(lengthMm).toFixed(unit.decimals),
                unit: unit.unit,
              });
              requestAnimationFrame(() => {
                lengthInputRef.current?.focus();
                lengthInputRef.current?.select();
              });
            },
            tryOpenDimensionEditor,
            recentLine: () => recentLineRef.current,
            setRecentLine: (index) => {
              recentLineRef.current = index;
            },
            recentLineGeometry,
            textEditorOpen: () => textEditor !== null,
            commitTextEditor,
            openTextEditorCreate: (clickPx, uv) => {
              setTextEditor({
                left: clickPx[0],
                top: clickPx[1],
                u: uv[0],
                v: uv[1],
                group: null,
              });
              requestAnimationFrame(() => {
                textInputRef.current?.focus();
                textInputRef.current?.select();
              });
            },
            openTextEditorEdit: openTextEditor,
            cornerEditorOpen: () => cornerEditor !== null,
            dismissCornerEditor: () => setCornerEditor(null),
            openCornerEditor,
          },
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
