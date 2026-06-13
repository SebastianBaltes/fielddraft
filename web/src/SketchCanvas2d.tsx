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
import type { KernelStore, SketchSessionState } from "./store";
import {
  getSketchPlaneFrame,
  readSketchCamera,
  screenToSketch,
  setSketchPlaneFrame,
  sketchToScreen,
  type SketchCameraState,
} from "./sketchCameraBridge";
import { resolvePlaneFrame } from "./sketchPlaneFrame";
import {
  buildDraftSketchState,
  DRAFT_DRAG_START_ID,
  predictNextLineIds,
  regularPolygonVertices,
} from "./sketchSession";
import { measuredLength, parseField } from "./sketchLiveInput";
import { entitiesInBox, hitTestSketchEntity } from "./sketchHitTest";
import { gripsForSelection, type Grip } from "./sketchGrips";
import {
  angleAbout,
  buildArcSubCall,
  sampleArc,
  unwrapSweep,
} from "./sketchArc";
import {
  circleThrough3,
  rectCenterCorners,
  rect3pCorners,
} from "./sketchPrimitiveVariants";
import { slotOutline } from "./sketchSlot";
import { setOverlayRedraw } from "./sketchOverlayBridge";
import { bsplinePoints, catmullRomPoints, type Vec2 } from "./sketchSpline";
import { curveOfSubCall, distToCurve, nearestTrimTarget } from "./sketchTrim";
import { nearestProjectedEdge } from "./sketchProject";
import {
  inferCandidates,
  type InferenceCandidate,
} from "./solver/inferenceEngine";
import { solveAndReplace } from "./solver/replaySolve";
import { DragController } from "./solver/dragController";
import { displayDegreesOfFreedom } from "./solver/dofPreflight";
import type { SketchState } from "./solver/types";

interface Props {
  store: KernelStore | null;
  session: SketchSessionState;
}

/** Visual tokens. Keep in sync with `styles/tokens.css` if we
 *  ever add sketch colours there. */
const STROKE_PX = 1.5;
const POINT_PX = 3;
// K6b.6: 3-color DOF status.
// Blue = underconstrained (default), Green = fully constrained,
// Red = overconstrained.
const COLOR_UNDER = "#2563eb";
const COLOR_FULL = "#16a34a";
const COLOR_OVER = "#dc2626";
const COLOR_PREVIEW = "#93c5fd";

function dofColor(dof: number): string {
  if (dof > 0) return COLOR_UNDER;
  if (dof === 0) return COLOR_FULL;
  return COLOR_OVER;
}

const COLOR_SNAP = "#f59e0b";
const LABEL_FONT = "600 14px system-ui, -apple-system, sans-serif";
const LABEL_BG = "rgba(246, 246, 244, 0.92)";
/** Dimension readouts: unobtrusive plain text in the accent blue —
 *  no box, so the numbers never compete with the geometry. */
const LABEL_DIM = "#3b82f6";
/** Label offset in CSS pixels, along the line's left-normal. */
const LABEL_OFFSET_PX = 18;
/** Click radius (CSS px) around the dimension readout that opens the
 *  exact-length editor on the most recently placed line. */
const DIM_LABEL_HIT_PX = 24;
/** Snap radius in sketch-plane millimetres. Generous enough to
 *  catch ordinary cursor jitter but tight enough that the user
 *  can still place free points near a committed entity without
 *  unwanted magnetism. Can be tuned later. */
const SNAP_RADIUS_MM = 10;
/** Snap indicator radius in CSS pixels, drawn as an open ring
 *  around the snap anchor. */
const SNAP_RING_PX = 8;

const COLOR_EDGE_GUIDE = "rgba(100, 116, 139, 0.25)";
/** F23-full: dashed control-polygon guide for a control-point
 *  B-spline (lines between consecutive control points). */
const COLOR_GUIDE = "rgba(100, 116, 139, 0.6)";
/** Highlight colour for entities picked in select mode. */
const COLOR_SELECTED = "#f97316";
/** F4 marquee: enclosing ("window", left→right) = solid blue border;
 *  crossing (right→left) = dashed green border. Translucent fills. */
const COLOR_MARQUEE_ENCLOSING = "#2563eb";
const COLOR_MARQUEE_CROSSING = "#16a34a";
const FILL_MARQUEE_ENCLOSING = "rgba(37, 99, 235, 0.12)";
const FILL_MARQUEE_CROSSING = "rgba(22, 163, 74, 0.12)";
const MARQUEE_CROSSING_DASH = [6, 4];
/** Pick radius for select-mode hit-testing, in CSS pixels. Divided
 *  by the camera zoom to get the sketch-plane tolerance. */
const SELECT_PICK_PX = 6;

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

/** Grip handle half-size (drawn as a filled square) in CSS pixels. */
const GRIP_PX = 4;
/** Grip pick radius in CSS pixels — generous so the small handles
 *  are easy to grab. Beats the entity hit-test, so grabbing a grip
 *  starts a drag rather than re-toggling the selection. */
const GRIP_PICK_PX = 9;

/**
 * Find the WebGPU `<canvas>` sibling of the overlay so camera
 * gestures (wheel zoom, right-drag pan) can be forwarded to the
 * R3F OrbitControls underneath while the overlay owns left-click
 * selection in select mode.
 */
function viewportCanvas(overlay: HTMLCanvasElement): HTMLCanvasElement | null {
  const parent = overlay.parentElement;
  if (!parent) return null;
  for (const c of parent.querySelectorAll("canvas")) {
    if (c !== overlay) return c as HTMLCanvasElement;
  }
  return null;
}

/** Re-dispatch a pointer event onto the WebGPU canvas so the
 *  OrbitControls pan gesture (right / middle drag) works while the
 *  overlay is capturing events in select mode. OrbitControls'
 *  pointerdown handler takes a pointer-capture on its own element,
 *  so forwarding just the initial down is enough for the drag to
 *  continue there. */
function forwardPointerToViewport(
  e: React.PointerEvent<HTMLCanvasElement>,
): void {
  const target = viewportCanvas(e.currentTarget as HTMLCanvasElement);
  if (!target) return;
  target.dispatchEvent(
    new PointerEvent(e.nativeEvent.type, {
      pointerId: e.pointerId,
      pointerType: e.pointerType,
      button: e.button,
      buttons: e.buttons,
      clientX: e.clientX,
      clientY: e.clientY,
      bubbles: true,
      cancelable: true,
    }),
  );
}

/**
 * Touch pinch sensitivity: how many wheel `deltaY` units one CSS pixel of
 * finger-spread synthesises. Spreading the fingers zooms in (negative
 * deltaY by browser convention); the gain just keeps the dolly pacing the
 * gesture. Tuned to feel like the mouse wheel, not a hard requirement.
 */
const PINCH_ZOOM_GAIN = 2.5;

/** Euclidean distance between the first two tracked touch points (CSS px). */
function touchSpan(pts: Map<number, { x: number; y: number }>): {
  dist: number;
  midX: number;
  midY: number;
} {
  const it = pts.values();
  const a = it.next().value;
  const b = it.next().value;
  if (!a || !b) return { dist: 0, midX: 0, midY: 0 };
  return {
    dist: Math.hypot(a.x - b.x, a.y - b.y),
    midX: (a.x + b.x) / 2,
    midY: (a.y + b.y) / 2,
  };
}

/** Synthesize a wheel event on the WebGPU canvas so the existing
 *  OrbitControls dolly path handles the zoom — same route the mouse
 *  wheel already takes. Used by the two-finger pinch gesture. */
function dispatchZoomWheel(
  overlay: HTMLCanvasElement,
  clientX: number,
  clientY: number,
  deltaY: number,
): void {
  const target = viewportCanvas(overlay);
  if (!target) return;
  target.dispatchEvent(
    new WheelEvent("wheel", {
      deltaY,
      deltaMode: 0,
      clientX,
      clientY,
      bubbles: true,
      cancelable: true,
    }),
  );
}

interface ProjectedEdge {
  p0: [number, number];
  p1: [number, number];
}

function projectEdgesToSketchPlane(
  edges: { p0: number[]; p1: number[] }[],
): ProjectedEdge[] {
  // Tickets yx6a/bxoo: drop each world point into the resolved plane
  // frame (orthogonal projection along the plane normal).
  const f = getSketchPlaneFrame();
  const uv = (p: number[]): [number, number] => {
    const dx = p[0] - f.origin[0];
    const dy = p[1] - f.origin[1];
    const dz = p[2] - f.origin[2];
    return [
      dx * f.xAxis[0] + dy * f.xAxis[1] + dz * f.xAxis[2],
      dx * f.yAxis[0] + dy * f.yAxis[1] + dz * f.yAxis[2],
    ];
  };
  return edges.map((e) => ({ p0: uv(e.p0), p1: uv(e.p1) }));
}

function nearestPointOnSegment(
  p: [number, number],
  a: [number, number],
  b: [number, number],
): [number, number] {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 1e-10) return a;
  const t = Math.max(
    0,
    Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / lenSq),
  );
  return [a[0] + t * dx, a[1] + t * dy];
}

function dist2d(a: [number, number], b: [number, number]): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

type DragState =
  | { kind: "idle" }
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
 * Inference result for the current cursor: the raw cursor
 * position, the snapped cursor position (possibly equal), and
 * the inference candidate that drove the snap, if any. Feeds
 * both the preview drawing and the `onPointerUp` commit path.
 */
interface SnapResolution {
  snapped: [number, number];
  candidate: InferenceCandidate | null;
  /** Anchor in sketch coordinates where the snap indicator
   *  should be drawn. For endpoint / origin snaps this is the
   *  snap target itself; for horizontal / vertical / parallel
   *  hints it's the snapped cursor. `null` skips the ring. */
  anchor: [number, number] | null;
  /** Real entity id the cursor snapped to, for coincident
   *  snaps. Used by the commit path to emit a matching
   *  constraint referencing the kernel-side entity. `null`
   *  when the snap is orientation-only (horizontal/vertical/
   *  parallel) or when there is no snap at all. */
  coincidentTargetId: number | null;
}

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

function resolveSnap(
  cursor: [number, number],
  session: SketchSessionState,
  dragStart: [number, number] | null,
  suppress = false,
  projectedEdges: ProjectedEdge[] = [],
): SnapResolution {
  if (suppress) {
    return {
      snapped: cursor,
      candidate: null,
      anchor: null,
      coincidentTargetId: null,
    };
  }
  const state: SketchState = buildDraftSketchState(
    session.subCalls,
    dragStart ?? undefined,
  );
  const cands = inferCandidates({
    state,
    cursor: { x: cursor[0], y: cursor[1] },
    drawingLineFrom: dragStart ? DRAFT_DRAG_START_ID : undefined,
    snapDistanceMm: SNAP_RADIUS_MM,
  });

  // K6b: check B-Rep edge snapping before falling through to no-snap.
  // Edge snap is lower priority than entity snap (coincident/H/V) but
  // higher priority than no snap at all.
  if (cands.length === 0 && projectedEdges.length > 0) {
    let bestDist = SNAP_RADIUS_MM;
    let bestPt: [number, number] | null = null;
    for (const edge of projectedEdges) {
      // Snap to edge endpoints (vertices)
      const d0 = dist2d(cursor, edge.p0);
      if (d0 < bestDist) {
        bestDist = d0;
        bestPt = edge.p0;
      }
      const d1 = dist2d(cursor, edge.p1);
      if (d1 < bestDist) {
        bestDist = d1;
        bestPt = edge.p1;
      }
      // Snap to nearest point on edge segment
      const np = nearestPointOnSegment(cursor, edge.p0, edge.p1);
      const dn = dist2d(cursor, np);
      if (dn < bestDist) {
        bestDist = dn;
        bestPt = np;
      }
    }
    if (bestPt) {
      return {
        snapped: bestPt,
        candidate: null,
        anchor: bestPt,
        coincidentTargetId: null,
      };
    }
  }

  if (cands.length === 0) {
    return {
      snapped: cursor,
      candidate: null,
      anchor: null,
      coincidentTargetId: null,
    };
  }
  const top = cands[0];
  const c = top.constraint;
  // Endpoint / origin snap: pull the cursor onto the target
  // point's coordinates.
  if (c.kind === "coincident") {
    const target = state.entities.find((e) => e.id === c.b);
    if (target && target.kind === "point") {
      return {
        snapped: [target.at.x, target.at.y],
        candidate: top,
        anchor: [target.at.x, target.at.y],
        coincidentTargetId: target.id,
      };
    }
    if (target && target.kind === "origin") {
      return {
        snapped: [0, 0],
        candidate: top,
        anchor: [0, 0],
        coincidentTargetId: target.id,
      };
    }
  }
  // Horizontal/vertical: lock one axis to the drag-start axis.
  if (c.kind === "horizontal" && dragStart) {
    const snapped: [number, number] = [cursor[0], dragStart[1]];
    return {
      snapped,
      candidate: top,
      anchor: snapped,
      coincidentTargetId: null,
    };
  }
  if (c.kind === "vertical" && dragStart) {
    const snapped: [number, number] = [dragStart[0], cursor[1]];
    return {
      snapped,
      candidate: top,
      anchor: snapped,
      coincidentTargetId: null,
    };
  }
  // Parallel to an existing line: project the cursor onto the
  // parallel ray through the drag start.
  if (c.kind === "parallel" && dragStart) {
    const other = state.entities.find((e) => e.id === c.b);
    if (other && other.kind === "line") {
      const os = state.entities.find((e) => e.id === other.start);
      const oe = state.entities.find((e) => e.id === other.end);
      if (os && oe && os.kind === "point" && oe.kind === "point") {
        const dx = oe.at.x - os.at.x;
        const dy = oe.at.y - os.at.y;
        const len = Math.hypot(dx, dy);
        if (len > 1e-6) {
          const ux = dx / len;
          const uy = dy / len;
          // Project (cursor - start) onto the unit direction.
          const cdx = cursor[0] - dragStart[0];
          const cdy = cursor[1] - dragStart[1];
          const t = cdx * ux + cdy * uy;
          const snapped: [number, number] = [
            dragStart[0] + ux * t,
            dragStart[1] + uy * t,
          ];
          return {
            snapped,
            candidate: top,
            anchor: snapped,
            coincidentTargetId: null,
          };
        }
      }
    }
  }
  return {
    snapped: cursor,
    candidate: null,
    anchor: null,
    coincidentTargetId: null,
  };
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

  // Touch pinch-to-zoom — a two-finger gesture layered ON TOP of the
  // single-finger sketch interaction. We track live touch pointers; once a
  // second finger lands we abort any in-progress single-finger draft and
  // hold a lockout (pinchRef.active) until EVERY finger lifts, so the
  // leftover finger can't resume a stray sketch action mid-gesture. Zoom is
  // driven by synthesising wheel events onto the WebGPU canvas, reusing the
  // OrbitControls dolly path the mouse wheel already uses. Mouse pointers
  // never enter any of this; single-finger sketching is unchanged. Two-finger
  // PAN is a deliberate follow-up (see ARCHITECTURE.md).
  const touchPtsRef = useRef<Map<number, { x: number; y: number }>>(new Map());
  const pinchRef = useRef<{ active: boolean; lastDist: number }>({
    active: false,
    lastDist: 0,
  });

  // F18 (reworked): click-to-edit dimension. While drawing, the canvas
  // shows only the unobtrusive numeric readout; once a line is placed,
  // its sub-call index lands in `recentLineRef` and the readout stays
  // visible at the committed line. Clicking the readout opens `editor`
  // — a small input at the label anchor; Enter applies the exact length
  // via store.setSketchLineLength, Esc / click-away dismisses.
  const recentLineRef = useRef<number | null>(null);
  const [editor, setEditor] = useState<{
    index: number;
    left: number;
    top: number;
    length: string;
  } | null>(null);
  const lengthInputRef = useRef<HTMLInputElement | null>(null);

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

  // K6b: project analytic B-Rep edges onto the sketch plane for snap targets.
  useEffect(() => {
    if (!store) {
      projectedEdgesRef.current = [];
      return;
    }
    try {
      const segs: { p0: number[]; p1: number[] }[] = [];
      const count = store.kernel.bodyCount();
      for (let i = 0; i < count; i++) {
        let bodyEdges: { a: number; b: number; points: number[] }[];
        try {
          bodyEdges = store.kernel.bodyBrepEdges(i);
        } catch {
          continue;
        }
        for (const e of bodyEdges) {
          const n = e.points.length / 3;
          for (let p = 0; p < n - 1; p++) {
            segs.push({
              p0: [e.points[p * 3], e.points[p * 3 + 1], e.points[p * 3 + 2]],
              p1: [
                e.points[(p + 1) * 3],
                e.points[(p + 1) * 3 + 1],
                e.points[(p + 1) * 3 + 2],
              ],
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
    const k = dragRef.current.kind;
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
    // F10: drop a pending first fillet pick when leaving the tool.
    filletFirstRef.current = null;
    // F17: drop a pending Move / Copy base point when leaving the tool.
    moveBaseRef.current = null;
    // F11: drop a pending first chamfer pick when leaving the tool.
    chamferFirstRef.current = null;
  }, [session.selectedTool]);

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
    if (e.pointerType === "touch") {
      touchPtsRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (touchPtsRef.current.size >= 2) {
        if (dragRef.current.kind !== "idle") {
          dragRef.current = { kind: "idle" };
          hoverRef.current = null;
        }
        const canvas = e.currentTarget as HTMLCanvasElement;
        for (const id of touchPtsRef.current.keys()) {
          try {
            canvas.releasePointerCapture(id);
          } catch {
            /* this pointer wasn't captured — fine */
          }
        }
        pinchRef.current = {
          active: true,
          lastDist: touchSpan(touchPtsRef.current).dist,
        };
        lastCamVersionRef.current = -1;
        e.preventDefault();
        return;
      }
    }

    // Select mode: no drawing tool active. Left-click hit-tests the
    // committed geometry and toggles it in the selection; clicking
    // empty space clears. Other buttons (pan) are forwarded to the
    // R3F canvas so OrbitControls keeps working under the overlay.
    if (store && session.active && !session.selectedTool) {
      if (e.button !== 0) {
        forwardPointerToViewport(e);
        return;
      }
      e.preventDefault();
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      const [u, v] = screenToSketchFromEvent(e, cam);

      // P0.3: grabbing a grip handle on the current selection starts a
      // drag-edit. Grips win over entity picking — they're a deliberate
      // edit gesture, and a re-toggle of the selection would be wrong.
      const grip = pickGrip(e, cam, session);
      if (grip) {
        (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
        dragRef.current = { kind: "grip", grip };
        // Live-solve from here: every move re-solves so constraints stay
        // satisfied as the grip moves, and drop just settles the last frame.
        dragControllerRef.current.start(store, grip);
        lastCamVersionRef.current = -1;
        return;
      }

      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const state = buildDraftSketchState(session.subCalls);
      const hit = hitTestSketchEntity(state, [u, v], tolMm);
      if (hit === null) {
        // Empty space: begin a marquee. We DON'T clear the selection
        // yet — that happens on pointerup if the gesture turns out to
        // be a plain click (no drag).
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
      } else {
        store.toggleSketchSelection(hit);
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
    // Text: not a drawing tool — a single click places the current text
    // value as single-stroke geometry with its lower-left at the click
    // point. Mirrors the trim-style single-click editing branch (see
    // store.placeSketchText / sketchText.ts).
    if (session.selectedTool === "sketch_text") {
      store.placeSketchText([u, v]);
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
    // Fillet: two-click flow. Each click picks the line sub-call nearest
    // the cursor (line-only — composites aren't filleted here). The first
    // pick is held pending in `filletFirstRef`; the second pick applies
    // the fillet with the current radius and clears the pending state.
    if (session.selectedTool === "sketch_fillet") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const pick = nearestLineSubCall(session.subCalls, [u, v], tolMm);
      if (pick !== null) {
        const first = filletFirstRef.current;
        if (first === null || first === pick) {
          filletFirstRef.current = pick;
        } else {
          store.filletSketchAt(
            first,
            pick,
            store.getState().sketchFilletRadius,
          );
          filletFirstRef.current = null;
        }
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // Chamfer: two-click flow. Each click picks the line sub-call nearest
    // the cursor (line-only — composites aren't chamfered here). The first
    // pick is held pending in `chamferFirstRef`; the second pick applies
    // the chamfer with the current setback and clears the pending state.
    if (session.selectedTool === "sketch_chamfer") {
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const pick = nearestLineSubCall(session.subCalls, [u, v], tolMm);
      if (pick !== null) {
        const first = chamferFirstRef.current;
        if (first === null || first === pick) {
          chamferFirstRef.current = pick;
        } else {
          store.chamferSketchAt(
            first,
            pick,
            store.getState().sketchChamferDistance,
          );
          chamferFirstRef.current = null;
        }
      }
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
      const base = moveBaseRef.current;
      if (base === null) {
        moveBaseRef.current = [u, v];
      } else {
        const copy = session.selectedTool === "sketch_copy";
        store.moveSketchSelection(u - base[0], v - base[1], copy);
        moveBaseRef.current = null;
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // K6b.8: Ctrl = snap to grid. Round to nearest grid step.
    if (e.ctrlKey || e.metaKey) {
      const minPxGap = 20;
      let step = 1;
      while (step * cam.zoom < minPxGap) step *= step < 5 ? 5 : 2;
      u = Math.round(u / step) * step;
      v = Math.round(v / step) * step;
    }
    // Resolve snap at pointerdown so the placed point/line-start
    // respects endpoint / origin magnetism even on a dead-stop
    // click (no prior move events).
    const snap = resolveSnap(
      [u, v],
      session,
      null,
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
          setEditor({
            index: recentLineRef.current!,
            left: anchor[0],
            top: anchor[1],
            length: measuredLength(recent.start, recent.end).toFixed(2),
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
          : store.getState().sketchSplineMode;
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
          store.pushSketchSubCalls([
            {
              type: "sketch_circle",
              params: { center: res.center, radius: res.radius },
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
      const tolMm = SELECT_PICK_PX / Math.max(cam.zoom, 1e-6);
      const axis = nearestLineSubCall(session.subCalls, [u, v], tolMm);
      if (axis !== null) store.mirrorSketchSelection(axis);
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
          store.getState().sketchOffsetDistance,
        );
      }
      lastCamVersionRef.current = -1;
      return;
    }
    // F21: Project / Use external geometry. The faint guides drawn from
    // existing bodies' B-Rep edges (projected onto the sketch plane in
    // `projectedEdgesRef`) are inert reference lines. A single click adopts
    // the nearest guide as a real CONSTRUCTION `sketch_line`, so the solver
    // can constrain against it. Construction matches the CAD "projected
    // reference" convention (visible + snappable, excluded from the profile).
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
          {
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
    if (pinchRef.current.active) {
      if (e.pointerType === "touch" && touchPtsRef.current.has(e.pointerId)) {
        touchPtsRef.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
      }
      if (touchPtsRef.current.size >= 2) {
        const span = touchSpan(touchPtsRef.current);
        const delta = span.dist - pinchRef.current.lastDist;
        if (delta !== 0) {
          dispatchZoomWheel(
            e.currentTarget as HTMLCanvasElement,
            span.midX,
            span.midY,
            -delta * PINCH_ZOOM_GAIN,
          );
          pinchRef.current.lastDist = span.dist;
        }
      }
      e.preventDefault();
      return;
    }

    // P0.3: grip drag lives in select mode (no tool active), so it
    // runs before the drawing-tool guard below. The controller solves
    // every frame (continuation model), so constraints stay satisfied
    // as the grip moves — the geometry the user sees is already solved.
    if (dragRef.current.kind === "grip") {
      const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
      let [u, v] = screenToSketchFromEvent(e, cam);
      if (e.ctrlKey || e.metaKey) {
        const minPxGap = 20;
        let step = 1;
        while (step * cam.zoom < minPxGap) step *= step < 5 ? 5 : 2;
        u = Math.round(u / step) * step;
        v = Math.round(v / step) * step;
      }
      void dragControllerRef.current.update([u, v]);
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
    if (!session.active || !session.selectedTool) return;
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    let [u, v] = screenToSketchFromEvent(e, cam);
    // K6b.8: Ctrl = snap to grid on move too.
    if (e.ctrlKey || e.metaKey) {
      const minPxGap = 20;
      let step = 1;
      while (step * cam.zoom < minPxGap) step *= step < 5 ? 5 : 2;
      u = Math.round(u / step) * step;
      v = Math.round(v / step) * step;
    }
    if (dragRef.current.kind === "line") {
      const snap = resolveSnap(
        [u, v],
        session,
        dragRef.current.start,
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
    } else {
      // Hover path: show a preview point with snap magnetism so
      // the user sees the same feedback a click-to-place would
      // land on.
      const snap = resolveSnap(
        [u, v],
        session,
        null,
        e.shiftKey,
        projectedEdgesRef.current,
      );
      snapRef.current = snap;
      hoverRef.current = snap.snapped;
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
  const recentLineGeometry = (): {
    start: [number, number];
    end: [number, number];
  } | null => {
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
    const calls: Array<{ type: string; params: Record<string, unknown> }> = [
      {
        type: "sketch_line",
        params: {
          start: [start[0], start[1]] as [number, number],
          end: [current[0], current[1]] as [number, number],
        },
      },
    ];
    if (startAnchorId !== null) {
      calls.push({
        type: "sketch_constraint_coincident",
        params: { a: ids.start, b: startAnchorId },
      });
    }
    recentLineRef.current = session.subCalls.length;
    store.pushSketchSubCalls(calls);
    cancelPendingLine();
    scheduleSolverReplay();
    return true;
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (e.pointerType === "touch") {
      touchPtsRef.current.delete(e.pointerId);
      if (pinchRef.current.active) {
        // Consume every touch-up for the duration of the pinch lockout; the
        // gesture only fully ends (and sketching resumes) once all fingers
        // are off the glass.
        if (touchPtsRef.current.size === 0) {
          pinchRef.current = { active: false, lastDist: 0 };
        }
        e.preventDefault();
        return;
      }
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

    // Start coincident: the line's start snapped to a pre-
    // existing entity at pointerdown. Emit a coincident
    // constraint referencing that entity's id.
    if (startAnchorId !== null) {
      newCalls.push({
        type: "sketch_constraint_coincident",
        params: { a: predictedIds.start, b: startAnchorId },
      });
    }

    // End snap: can be horizontal / vertical / coincident.
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
      }
    }

    // The placed line becomes the click-to-edit dimension target.
    recentLineRef.current = session.subCalls.length;
    store.pushSketchSubCalls(newCalls);
    scheduleSolverReplay();
  };

  const onPointerCancel = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (e.pointerType === "touch") {
      touchPtsRef.current.delete(e.pointerId);
      if (touchPtsRef.current.size === 0) {
        pinchRef.current = { active: false, lastDist: 0 };
      }
    }
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

  const onPointerLeave = () => {
    hoverRef.current = null;
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
      if (dragRef.current.kind === "pending_line") {
        e.preventDefault();
        cancelPendingLine();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [session.active, editor]);

  // F23: keyboard finish for the spline gesture — Enter commits the
  // in-progress spline (open, needs ≥ 2 control points); Escape cancels
  // it. Mirrors the click/double-click finish in onPointerDown.
  useEffect(() => {
    if (!store || session.selectedTool !== "sketch_spline") return;
    const onKey = (e: KeyboardEvent) => {
      const drag = dragRef.current;
      if (drag.kind !== "spline") return;
      if (e.key === "Enter" && drag.points.length >= 2) {
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

  // F18: apply the typed exact length from the dimension editor to the
  // committed line. Blank / invalid input just dismisses.
  const commitEditor = () => {
    if (editor === null || !store) return;
    const len = parseField(editor.length);
    if (len !== null && len > 0) {
      store.setSketchLineLength(editor.index, len);
    }
    setEditor(null);
    lastCamVersionRef.current = -1;
  };

  return (
    <>
      <canvas
        ref={canvasRef}
        data-testid="sketch-canvas-2d"
        className={
          "fd-sketch-canvas" +
          (toolActive ? " is-tool-active" : selectMode ? " is-select-mode" : "") +
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
              value={editor.length}
              onChange={(e) => {
                setEditor((prev) =>
                  prev === null ? prev : { ...prev, length: e.target.value },
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
            <span className="unit">mm</span>
          </label>
        </div>
      )}
    </>
  );
}

// ─── drawing ────────────────────────────────────────────────────

function resizeToContainer(canvas: HTMLCanvasElement): void {
  const rect = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(rect.width * dpr));
  const h = Math.max(1, Math.round(rect.height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

function drawOverlay(
  canvas: HTMLCanvasElement,
  session: SketchSessionState,
  cam: Readonly<SketchCameraState>,
  drag: DragState,
  hover: [number, number] | null,
  snap: SnapResolution,
  projectedEdges: ProjectedEdge[] | null = null,
  recentLine: {
    start: [number, number];
    end: [number, number];
  } | null = null,
): void {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = cam.devicePixelRatio || window.devicePixelRatio || 1;

  // Reset to identity and clear. The device-pixel-ratio scale
  // goes on _after_ the clear so we can work in CSS pixels for
  // everything below.
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.scale(dpr, dpr);

  // A non-positive zoom is a degenerate camera: every screen transform
  // divides by zoom and the adaptive-grid loop below would never terminate.
  // Bail with a cleared canvas rather than freezing the tab.
  if (!(cam.zoom > 0)) return;

  // K6b: draw projected B-Rep edges as faint guide lines.
  if (projectedEdges && projectedEdges.length > 0) {
    ctx.strokeStyle = COLOR_EDGE_GUIDE;
    ctx.lineWidth = 1.5;
    ctx.setLineDash([4, 4]);
    ctx.beginPath();
    for (const edge of projectedEdges) {
      const [x0, y0] = sketchToScreen(edge.p0[0], edge.p0[1], cam);
      const [x1, y1] = sketchToScreen(edge.p1[0], edge.p1[1], cam);
      ctx.moveTo(x0, y0);
      ctx.lineTo(x1, y1);
    }
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // skState (solved entities) feeds constraint-glyph placement and the
  // selection highlight below. Committed GEOMETRY now renders in 3D
  // (SketchSession3D); drawOverlay no longer strokes it here — only the
  // transient HUD (selection highlight, grips, constraint glyphs, drag
  // previews, snap/hover) remains on the overlay.
  const skState = buildDraftSketchState(session.subCalls);
  // DOF status colour, still used to tint the constraint glyphs.
  let committedColor = COLOR_UNDER;
  try {
    const over =
      session.diagnostics.conflicting.length > 0 ||
      session.diagnostics.malformed.length > 0;
    const dof = displayDegreesOfFreedom(skState, session.diagnostics);
    committedColor = over ? COLOR_OVER : dofColor(dof);
  } catch {
    // Solver not available — fall back to the underconstrained color.
  }

  // Select mode: highlight the picked entities on top of the
  // committed geometry so the constraint palette's target is
  // obvious. Resolved against skState (same id space as selection).
  if (session.selection.length > 0) {
    drawSelectionHighlight(ctx, skState, session.selection, cam);
    // P0.3: draggable grip handles on the selected entities.
    drawGrips(ctx, gripsForSelection(session.subCalls, session.selection), cam);
  }

  // K6b.6: draw constraint glyphs anchored to the constrained geometry.
  // Resolve entity ids against the already-built solved state (skState)
  // rather than guessing from sub-call ordering.
  {
    const entityPos = (entityId: number): [number, number] | null => {
      const e = skState.entities.find((x) => x.id === entityId);
      if (!e) return null;
      if (e.kind === "point") return [e.at.x, e.at.y];
      if (e.kind === "origin") return [0, 0];
      return null;
    };
    const lineMidpoint = (lineId: number): [number, number] | null => {
      const e = skState.entities.find((x) => x.id === lineId);
      if (!e || e.kind !== "line") return null;
      const s = entityPos(e.start);
      const en = entityPos(e.end);
      if (!s || !en) return null;
      return [(s[0] + en[0]) / 2, (s[1] + en[1]) / 2];
    };

    ctx.font = "bold 11px sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillStyle = committedColor;
    for (const c of skState.constraints) {
      let anchor: [number, number] | null = null;
      let label = "C";
      if (c.kind === "horizontal") {
        anchor = lineMidpoint(c.line);
        label = "H";
      } else if (c.kind === "vertical") {
        anchor = lineMidpoint(c.line);
        label = "V";
      } else if (c.kind === "coincident") {
        anchor = entityPos(c.a);
        label = "⊙";
      }
      if (!anchor) continue;
      const [px, py] = sketchToScreen(anchor[0], anchor[1], cam);
      ctx.fillText(label, px, py - 10);
    }
  }

  // F18: dimension readout on the most recently placed line — clicking
  // it opens the exact-length editor (hit-tested in onPointerDown).
  if (recentLine !== null && drag.kind === "idle") {
    drawDistanceLabel(ctx, recentLine.start, recentLine.end, cam);
  }

  if (drag.kind === "line" || drag.kind === "pending_line") {
    const { start, current } = drag;
    // Label first so the line draws over it — the readout stays behind
    // the geometry, unobtrusive by design.
    drawDistanceLabel(ctx, start, current, cam);
    drawLine(ctx, start, current, cam, COLOR_PREVIEW, STROKE_PX);
    drawPoint(ctx, start, cam, COLOR_PREVIEW);
    drawPoint(ctx, current, cam, COLOR_PREVIEW);
  } else if (drag.kind === "rectangle") {
    const { start, current } = drag;
    drawRectangle(ctx, start, current, cam, COLOR_PREVIEW);
    drawSizeLabel(ctx, start, current, cam);
  } else if (drag.kind === "circle") {
    const { center, current } = drag;
    const dx = current[0] - center[0];
    const dy = current[1] - center[1];
    const radius = Math.hypot(dx, dy);
    drawCircle(ctx, center, radius, cam, COLOR_PREVIEW);
    drawPoint(ctx, center, cam, COLOR_PREVIEW);
    drawRadiusLabel(ctx, center, radius, cam);
  } else if (drag.kind === "ellipse") {
    const { center, current } = drag;
    const radiusX = Math.abs(current[0] - center[0]);
    const radiusY = Math.abs(current[1] - center[1]);
    const rotation = Number(session.toolDraft?.rotation) || 0;
    drawEllipse(ctx, center, radiusX, radiusY, rotation, cam, COLOR_PREVIEW);
    drawPoint(ctx, center, cam, COLOR_PREVIEW);
  } else if (drag.kind === "slot") {
    const { center_a, current } = drag;
    const radius = Number(session.toolDraft?.radius) || 8;
    drawSlot(ctx, center_a, current, radius, cam, COLOR_PREVIEW);
    drawPoint(ctx, center_a, cam, COLOR_PREVIEW);
    drawPoint(ctx, current, cam, COLOR_PREVIEW);
  } else if (drag.kind === "rect_center") {
    // Mirror the cursor about the centre to ghost the axis-aligned box.
    const { center, current } = drag;
    const opp: [number, number] = [
      2 * center[0] - current[0],
      2 * center[1] - current[1],
    ];
    drawRectangle(ctx, opp, current, cam, COLOR_PREVIEW);
    drawPoint(ctx, center, cam, COLOR_PREVIEW);
    drawSizeLabel(ctx, opp, current, cam);
  } else if (drag.kind === "circle_3p") {
    const { p1, p2, current } = drag;
    drawPoint(ctx, p1, cam, COLOR_PREVIEW);
    if (p2 === null) {
      // Stage 1: rubber-band chord from p1 to the cursor.
      drawLine(ctx, p1, current, cam, COLOR_PREVIEW, STROKE_PX);
    } else {
      // Stage 2: ghost the circumcircle through the three current points.
      drawPoint(ctx, p2, cam, COLOR_PREVIEW);
      const res = circleThrough3(p1, p2, current);
      if (res) {
        drawCircle(ctx, res.center, res.radius, cam, COLOR_PREVIEW);
        drawRadiusLabel(ctx, res.center, res.radius, cam);
      }
      drawPoint(ctx, current, cam, COLOR_PREVIEW);
    }
  } else if (drag.kind === "rect_3p") {
    const { p1, p2, current } = drag;
    drawPoint(ctx, p1, cam, COLOR_PREVIEW);
    if (p2 === null) {
      // Stage 1: rubber-band the first edge.
      drawLine(ctx, p1, current, cam, COLOR_PREVIEW, STROKE_PX);
    } else {
      // Stage 2: ghost the rotated rectangle through the width point.
      drawPoint(ctx, p2, cam, COLOR_PREVIEW);
      const corners = rect3pCorners(p1, p2, current);
      if (corners) {
        for (let i = 0; i < 4; i++) {
          drawLine(
            ctx,
            corners[i],
            corners[(i + 1) % 4],
            cam,
            COLOR_PREVIEW,
            STROKE_PX,
          );
          drawPoint(ctx, corners[i], cam, COLOR_PREVIEW);
        }
      } else {
        drawLine(ctx, p1, p2, cam, COLOR_PREVIEW, STROKE_PX);
      }
    }
  } else if (drag.kind === "regular_polygon") {
    const { center, current } = drag;
    const radius = Math.hypot(current[0] - center[0], current[1] - center[1]);
    const verts = regularPolygonVertices(
      center,
      radius,
      Number(session.toolDraft?.sides) || 6,
      session.toolDraft?.inscribed !== false,
    );
    if (verts.length >= 3) {
      ctx.strokeStyle = COLOR_PREVIEW;
      ctx.lineWidth = 2;
      ctx.beginPath();
      const [px0, py0] = sketchToScreen(verts[0][0], verts[0][1], cam);
      ctx.moveTo(px0, py0);
      for (let i = 1; i < verts.length; i++) {
        const [px, py] = sketchToScreen(verts[i][0], verts[i][1], cam);
        ctx.lineTo(px, py);
      }
      ctx.closePath();
      ctx.stroke();
      for (const v of verts) {
        drawPoint(ctx, v, cam, COLOR_PREVIEW);
      }
      drawPoint(ctx, center, cam, COLOR_PREVIEW);
      drawRadiusLabel(ctx, center, radius, cam);
    }
  } else if (drag.kind === "polygon") {
    const { vertices, current } = drag;
    if (vertices.length > 0) {
      ctx.strokeStyle = COLOR_PREVIEW;
      ctx.lineWidth = 2;
      ctx.beginPath();
      const [px0, py0] = sketchToScreen(vertices[0][0], vertices[0][1], cam);
      ctx.moveTo(px0, py0);
      for (let i = 1; i < vertices.length; i++) {
        const [px, py] = sketchToScreen(vertices[i][0], vertices[i][1], cam);
        ctx.lineTo(px, py);
      }
      const [cx, cy] = sketchToScreen(current[0], current[1], cam);
      ctx.lineTo(cx, cy);
      // Close back to first vertex as a dashed ghost.
      ctx.stroke();
      ctx.setLineDash([4, 4]);
      ctx.lineTo(px0, py0);
      ctx.stroke();
      ctx.setLineDash([]);
      // Draw vertex dots.
      for (const v of vertices) {
        drawPoint(ctx, v, cam, COLOR_PREVIEW);
      }
    }
  } else if (drag.kind === "spline") {
    const { points, current, mode } = drag;
    if (points.length > 0) {
      // Live curve through the placed points plus the cursor. F23-full:
      // in control mode use de Boor (B-spline) + a dashed control
      // polygon guide; otherwise Catmull-Rom interpolation.
      const ctrl: Vec2[] = [...points, current];
      const isControl = mode === "control";
      if (isControl) {
        ctx.save();
        ctx.setLineDash([6, 4]);
        ctx.strokeStyle = COLOR_GUIDE;
        ctx.lineWidth = 1;
        ctx.beginPath();
        const [gx0, gy0] = sketchToScreen(ctrl[0][0], ctrl[0][1], cam);
        ctx.moveTo(gx0, gy0);
        for (let i = 1; i < ctrl.length; i++) {
          const [px, py] = sketchToScreen(ctrl[i][0], ctrl[i][1], cam);
          ctx.lineTo(px, py);
        }
        ctx.stroke();
        ctx.restore();
      }
      const sampled = isControl
        ? bsplinePoints(ctrl, 3, false, 48)
        : catmullRomPoints(ctrl, false, 16);
      if (sampled.length >= 2) {
        ctx.strokeStyle = COLOR_PREVIEW;
        ctx.lineWidth = 2;
        ctx.beginPath();
        const [sx0, sy0] = sketchToScreen(sampled[0][0], sampled[0][1], cam);
        ctx.moveTo(sx0, sy0);
        for (let i = 1; i < sampled.length; i++) {
          const [px, py] = sketchToScreen(sampled[i][0], sampled[i][1], cam);
          ctx.lineTo(px, py);
        }
        ctx.stroke();
      }
      // Control-point dots (the placed ones, not the cursor).
      for (const p of points) {
        drawPoint(ctx, p, cam, COLOR_PREVIEW);
      }
    }
  } else if (drag.kind === "arc") {
    const { center, start, current, startAngle, sweep } = drag;
    if (start === null) {
      // Stage 1: picking radius/start — ghost full circle + radius spoke.
      const r = Math.hypot(current[0] - center[0], current[1] - center[1]);
      drawCircle(ctx, center, r, cam, COLOR_PREVIEW);
      drawLine(ctx, center, current, cam, COLOR_PREVIEW, STROKE_PX);
      drawPoint(ctx, center, cam, COLOR_PREVIEW);
      drawRadiusLabel(ctx, center, r, cam);
    } else {
      // Stage 2: sweeping — the arc itself plus the start spoke.
      const r = Math.hypot(start[0] - center[0], start[1] - center[1]);
      drawArc(ctx, center, r, startAngle, sweep, cam, COLOR_PREVIEW, STROKE_PX);
      drawLine(ctx, center, start, cam, COLOR_PREVIEW, 1);
      drawPoint(ctx, center, cam, COLOR_PREVIEW);
      drawPoint(ctx, start, cam, COLOR_PREVIEW);
      drawRadiusLabel(ctx, center, r, cam);
    }
  } else if (drag.kind === "marquee") {
    drawMarquee(ctx, drag.start, drag.current, cam);
  } else if (hover) {
    // Hover preview outside of a drag — shows where a click
    // would land, which is crucial to see the snap magnetism
    // before committing.
    drawPoint(ctx, hover, cam, COLOR_PREVIEW);
  }

  // Snap indicator is drawn last so it sits on top of the
  // preview geometry. The anchor is `null` when no snap
  // candidate was selected.
  if (snap.anchor && snap.candidate) {
    drawSnapIndicator(ctx, snap.anchor, snap.candidate.label, cam);
  }
}

/**
 * Overdraw the selected entities in the highlight colour: points get
 * an open ring, lines a thicker stroke, circles/arcs a thicker rim.
 * Resolved against the rebuilt solver state so the id space matches
 * the session's `selection`.
 */
/**
 * P0.3: draw the draggable grip handles for the current selection as
 * small white-bordered squares in the highlight colour, so the user
 * sees exactly where the geometry can be grabbed and dragged.
 */
function drawGrips(
  ctx: CanvasRenderingContext2D,
  grips: ReadonlyArray<Grip>,
  cam: Readonly<SketchCameraState>,
): void {
  if (grips.length === 0) return;
  ctx.save();
  ctx.fillStyle = COLOR_SELECTED;
  ctx.strokeStyle = "#ffffff";
  ctx.lineWidth = 1;
  for (const g of grips) {
    const [px, py] = sketchToScreen(g.pos[0], g.pos[1], cam);
    ctx.beginPath();
    ctx.rect(px - GRIP_PX, py - GRIP_PX, GRIP_PX * 2, GRIP_PX * 2);
    ctx.fill();
    ctx.stroke();
  }
  ctx.restore();
}

function drawSelectionHighlight(
  ctx: CanvasRenderingContext2D,
  state: SketchState,
  selection: ReadonlyArray<number>,
  cam: Readonly<SketchCameraState>,
): void {
  const pointAt = (id: number): [number, number] | null => {
    const e = state.entities.find((x) => x.id === id);
    if (!e) return null;
    if (e.kind === "point") return [e.at.x, e.at.y];
    if (e.kind === "origin") return [0, 0];
    return null;
  };
  ctx.save();
  ctx.strokeStyle = COLOR_SELECTED;
  ctx.fillStyle = COLOR_SELECTED;
  for (const id of selection) {
    const e = state.entities.find((x) => x.id === id);
    if (!e) continue;
    if (e.kind === "point" || e.kind === "origin") {
      const at =
        e.kind === "origin"
          ? ([0, 0] as [number, number])
          : ([e.at.x, e.at.y] as [number, number]);
      const [px, py] = sketchToScreen(at[0], at[1], cam);
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(px, py, POINT_PX + 3, 0, Math.PI * 2);
      ctx.stroke();
    } else if (e.kind === "line") {
      const a = pointAt(e.start);
      const b = pointAt(e.end);
      if (a && b) drawLine(ctx, a, b, cam, COLOR_SELECTED, STROKE_PX + 2);
    } else if (e.kind === "circle" || e.kind === "arc") {
      const c = pointAt(e.center);
      if (c) {
        const [cx, cy] = sketchToScreen(c[0], c[1], cam);
        const rPx = e.radius * cam.zoom;
        if (rPx >= 0.5) {
          ctx.lineWidth = STROKE_PX + 2;
          ctx.beginPath();
          ctx.arc(cx, cy, rPx, 0, Math.PI * 2);
          ctx.stroke();
        }
      }
    }
  }
  ctx.restore();
}

function drawSnapIndicator(
  ctx: CanvasRenderingContext2D,
  anchor: [number, number],
  label: string,
  cam: Readonly<SketchCameraState>,
): void {
  const [px, py] = sketchToScreen(anchor[0], anchor[1], cam);
  ctx.save();
  ctx.strokeStyle = COLOR_SNAP;
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(px, py, SNAP_RING_PX, 0, Math.PI * 2);
  ctx.stroke();
  // Label — small pill just above and right of the anchor.
  ctx.font = LABEL_FONT;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  const textX = px + SNAP_RING_PX + 6;
  const textY = py - SNAP_RING_PX - 6;
  const metrics = ctx.measureText(label);
  const boxX = textX - 4;
  const boxY = textY - 10;
  const boxW = metrics.width + 8;
  const boxH = 20;
  ctx.fillStyle = LABEL_BG;
  ctx.strokeStyle = COLOR_SNAP;
  ctx.lineWidth = 1;
  roundRect(ctx, boxX, boxY, boxW, boxH, 4);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = COLOR_SNAP;
  ctx.fillText(label, textX, textY);
  ctx.restore();
}

function drawLine(
  ctx: CanvasRenderingContext2D,
  a: [number, number],
  b: [number, number],
  cam: Readonly<SketchCameraState>,
  color: string,
  widthPx: number,
): void {
  const [ax, ay] = sketchToScreen(a[0], a[1], cam);
  const [bx, by] = sketchToScreen(b[0], b[1], cam);
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = widthPx;
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(ax, ay);
  ctx.lineTo(bx, by);
  ctx.stroke();
  ctx.restore();
}

function drawCircle(
  ctx: CanvasRenderingContext2D,
  center: [number, number],
  radiusMm: number,
  cam: Readonly<SketchCameraState>,
  color: string,
): void {
  // Tickets yx6a/bxoo: sample in sketch space and project each point —
  // under a tilted camera the circle is an ellipse on screen, which
  // `ctx.arc` can't draw.
  if (radiusMm * cam.zoom < 0.5) return;
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = STROKE_PX;
  ctx.beginPath();
  const steps = 64;
  for (let i = 0; i <= steps; i++) {
    const t = (i / steps) * Math.PI * 2;
    const [px, py] = sketchToScreen(
      center[0] + radiusMm * Math.cos(t),
      center[1] + radiusMm * Math.sin(t),
      cam,
    );
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();
  ctx.restore();
}

/**
 * Draw an axis-aligned (optionally rotated) ellipse via `ctx.ellipse`
 * in SCREEN space. radius_x/radius_y are sketch-space lengths scaled by
 * the camera zoom. Sketch space is Y-up but the canvas is Y-down, so the
 * rotation is negated to keep the on-screen tilt matching the kernel's
 * CCW convention (no-op at rotation 0). Skips degenerate ellipses.
 */
function drawEllipse(
  ctx: CanvasRenderingContext2D,
  center: [number, number],
  radiusXMm: number,
  radiusYMm: number,
  rotation: number,
  cam: Readonly<SketchCameraState>,
  color: string,
): void {
  if (radiusXMm * cam.zoom < 0.5 || radiusYMm * cam.zoom < 0.5) return;
  // Tickets yx6a/bxoo: sample the rotated ellipse in sketch space and
  // project each point (see drawCircle).
  const cosR = Math.cos(rotation);
  const sinR = Math.sin(rotation);
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = STROKE_PX;
  ctx.beginPath();
  const steps = 64;
  for (let i = 0; i <= steps; i++) {
    const t = (i / steps) * Math.PI * 2;
    const ex = radiusXMm * Math.cos(t);
    const ey = radiusYMm * Math.sin(t);
    const [px, py] = sketchToScreen(
      center[0] + ex * cosR - ey * sinR,
      center[1] + ex * sinR + ey * cosR,
      cam,
    );
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();
  ctx.restore();
}

/**
 * Draw a centre-to-centre slot (stadium) outline: two straight edges
 * joined by two outward-bulging semicircle caps. The boundary geometry
 * comes from {@link slotOutline} (the same math the kernel lowers to),
 * and the caps are sampled in sketch space via {@link sampleArc} so the
 * ±π sweeps render correctly on the Y-down canvas (`ctx.arc` would
 * misinterpret the sketch-space angles). Skips degenerate slots.
 */
function drawSlot(
  ctx: CanvasRenderingContext2D,
  a: [number, number],
  b: [number, number],
  radiusMm: number,
  cam: Readonly<SketchCameraState>,
  color: string,
): void {
  const o = slotOutline(a, b, radiusMm);
  if (!o || radiusMm * cam.zoom < 0.5) return;
  // One continuous loop: A+r·n → B+r·n, cap B, B−r·n → A−r·n, cap A.
  const loop: [number, number][] = [o.aPlus, o.bPlus];
  loop.push(...sampleArc(o.centerB, o.radius, o.capBStart, -Math.PI));
  loop.push(o.aMinus);
  loop.push(...sampleArc(o.centerA, o.radius, o.capAStart, -Math.PI));
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = STROKE_PX;
  ctx.lineJoin = "round";
  ctx.beginPath();
  for (let i = 0; i < loop.length; i++) {
    const [px, py] = sketchToScreen(loop[i][0], loop[i][1], cam);
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.closePath();
  ctx.stroke();
  ctx.restore();
}

/**
 * Draw an arc by sampling it in sketch space and projecting each point
 * through `sketchToScreen`. Going via `ctx.arc` would feed sketch-space
 * angles into the Y-down canvas convention and mirror the arc relative to
 * its grips; the sampler keeps drawing, grips and the kernel in agreement.
 */
function drawArc(
  ctx: CanvasRenderingContext2D,
  center: [number, number],
  radiusMm: number,
  startAngle: number,
  sweep: number,
  cam: Readonly<SketchCameraState>,
  color: string,
  widthPx: number,
): void {
  if (radiusMm * cam.zoom < 0.5 || Math.abs(sweep) < 1e-4) return;
  const pts = sampleArc(center, radiusMm, startAngle, sweep);
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = widthPx;
  ctx.lineCap = "round";
  ctx.beginPath();
  for (let i = 0; i < pts.length; i++) {
    const [px, py] = sketchToScreen(pts[i][0], pts[i][1], cam);
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();
  ctx.restore();
}

function drawRadiusLabel(
  ctx: CanvasRenderingContext2D,
  center: [number, number],
  radiusMm: number,
  cam: Readonly<SketchCameraState>,
): void {
  if (radiusMm < 1e-4) return;
  const [cx, cy] = sketchToScreen(center[0], center[1], cam);
  // Anchor the label slightly above-right of the centre so it
  // doesn't clash with the centre-point indicator.
  drawDimensionText(
    ctx,
    `R ${radiusMm.toFixed(2)}`,
    cx + LABEL_OFFSET_PX,
    cy - LABEL_OFFSET_PX,
  );
}

function drawRectangle(
  ctx: CanvasRenderingContext2D,
  a: [number, number],
  b: [number, number],
  cam: Readonly<SketchCameraState>,
  color: string,
): void {
  const minX = Math.min(a[0], b[0]);
  const maxX = Math.max(a[0], b[0]);
  const minY = Math.min(a[1], b[1]);
  const maxY = Math.max(a[1], b[1]);
  const bl: [number, number] = [minX, minY];
  const br: [number, number] = [maxX, minY];
  const tr: [number, number] = [maxX, maxY];
  const tl: [number, number] = [minX, maxY];
  drawLine(ctx, bl, br, cam, color, STROKE_PX);
  drawLine(ctx, br, tr, cam, color, STROKE_PX);
  drawLine(ctx, tr, tl, cam, color, STROKE_PX);
  drawLine(ctx, tl, bl, cam, color, STROKE_PX);
  drawPoint(ctx, bl, cam, color);
  drawPoint(ctx, br, cam, color);
  drawPoint(ctx, tr, cam, color);
  drawPoint(ctx, tl, cam, color);
}

/**
 * F4: draw the box-select marquee. The drag direction decides the
 * style — left→right (start.x ≤ current.x) is an enclosing "window"
 * select drawn with a solid blue border, right→left a crossing select
 * drawn with a dashed green border. Both get a faint matching fill.
 */
function drawMarquee(
  ctx: CanvasRenderingContext2D,
  start: [number, number],
  current: [number, number],
  cam: Readonly<SketchCameraState>,
): void {
  const enclosing = start[0] <= current[0];
  const [sx, sy] = sketchToScreen(start[0], start[1], cam);
  const [cx, cy] = sketchToScreen(current[0], current[1], cam);
  const x = Math.min(sx, cx);
  const y = Math.min(sy, cy);
  const w = Math.abs(cx - sx);
  const h = Math.abs(cy - sy);
  ctx.save();
  ctx.fillStyle = enclosing ? FILL_MARQUEE_ENCLOSING : FILL_MARQUEE_CROSSING;
  ctx.fillRect(x, y, w, h);
  ctx.strokeStyle = enclosing
    ? COLOR_MARQUEE_ENCLOSING
    : COLOR_MARQUEE_CROSSING;
  ctx.lineWidth = 1.5;
  ctx.setLineDash(enclosing ? [] : MARQUEE_CROSSING_DASH);
  ctx.strokeRect(x, y, w, h);
  ctx.restore();
}

function drawSizeLabel(
  ctx: CanvasRenderingContext2D,
  a: [number, number],
  b: [number, number],
  cam: Readonly<SketchCameraState>,
): void {
  const w = Math.abs(b[0] - a[0]);
  const h = Math.abs(b[1] - a[1]);
  if (w < 1e-4 && h < 1e-4) return;
  const centre: [number, number] = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
  const [px, py] = sketchToScreen(centre[0], centre[1], cam);
  drawDimensionText(ctx, `${w.toFixed(2)} × ${h.toFixed(2)}`, px, py);
}

function drawPoint(
  ctx: CanvasRenderingContext2D,
  at: [number, number],
  cam: Readonly<SketchCameraState>,
  color: string,
): void {
  const [px, py] = sketchToScreen(at[0], at[1], cam);
  ctx.save();
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.arc(px, py, POINT_PX, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

/**
 * Anchor (canvas CSS px) of a line's dimension readout: the midpoint,
 * pushed `LABEL_OFFSET_PX` along the line's visual left-normal. Shared
 * by the label renderer and the click-to-edit hit test so the two can
 * never disagree. `null` for a (near) zero-length screen segment.
 */
function distanceLabelAnchorPx(
  start: [number, number],
  end: [number, number],
  cam: Readonly<SketchCameraState>,
): [number, number] | null {
  const [sx, sy] = sketchToScreen(start[0], start[1], cam);
  const [ex, ey] = sketchToScreen(end[0], end[1], cam);
  const dxScreen = ex - sx;
  const dyScreen = ey - sy;
  const lenScreen = Math.hypot(dxScreen, dyScreen);
  if (lenScreen < 1) return null;
  // Left-normal in screen space: rotate the direction 90° CCW.
  // Canvas has screen-Y pointing *down*, so "left as you face
  // from start to end" maps to rotating the direction vector
  // by -90° (CW) in canvas coordinates. `left = (-dy, dx)` is
  // the mathematical CCW, which is visually "right" on a
  // Y-down canvas — so we negate to get visual left.
  const nx = dyScreen / lenScreen;
  const ny = -dxScreen / lenScreen;
  return [
    (sx + ex) / 2 + nx * LABEL_OFFSET_PX,
    (sy + ey) / 2 + ny * LABEL_OFFSET_PX,
  ];
}

/**
 * Unobtrusive dimension readout beside a line: plain accent-coloured
 * number, no box, no unit — it must never compete with the geometry.
 * Used both for the in-flight preview and for the click-to-edit
 * readout on the most recently placed line.
 */
function drawDistanceLabel(
  ctx: CanvasRenderingContext2D,
  start: [number, number],
  end: [number, number],
  cam: Readonly<SketchCameraState>,
): void {
  const anchor = distanceLabelAnchorPx(start, end, cam);
  if (anchor === null) return;
  // Distance is measured in sketch-plane units (mm).
  const distMm = Math.hypot(end[0] - start[0], end[1] - start[1]);
  drawDimensionText(ctx, distMm.toFixed(2), anchor[0], anchor[1]);
}

/** Shared renderer for the subtle drag-preview dimension readouts. */
function drawDimensionText(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
): void {
  ctx.save();
  ctx.font = LABEL_FONT;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";
  // Thin halo so the number stays legible over the grid without
  // needing a boxed background.
  ctx.strokeStyle = LABEL_BG;
  ctx.lineWidth = 3;
  ctx.strokeText(text, x, y);
  ctx.fillStyle = LABEL_DIM;
  ctx.fillText(text, x, y);
  ctx.restore();
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.lineTo(x + w - rr, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + rr);
  ctx.lineTo(x + w, y + h - rr);
  ctx.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
  ctx.lineTo(x + rr, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - rr);
  ctx.lineTo(x, y + rr);
  ctx.quadraticCurveTo(x, y, x + rr, y);
  ctx.closePath();
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
