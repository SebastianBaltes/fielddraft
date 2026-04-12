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

import { useEffect, useLayoutEffect, useRef, type ReactElement } from "react";
import type { KernelStore, SketchSessionState } from "./store";
import {
  readSketchCamera,
  screenToSketch,
  sketchToScreen,
  type SketchCameraState,
} from "./sketchCameraBridge";
import {
  buildDraftSketchState,
  DRAFT_DRAG_START_ID,
  predictNextLineIds,
} from "./sketchSession";
import {
  inferCandidates,
  type InferenceCandidate,
} from "./solver/inferenceEngine";
import { solveSketch } from "./solver/planegcs";
import { countDegreesOfFreedom } from "./solver/dofPreflight";
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

// Legacy alias — default to underconstrained.
const COLOR_COMMITTED = COLOR_UNDER;
const COLOR_SNAP = "#f59e0b";
const LABEL_FONT = "600 14px system-ui, -apple-system, sans-serif";
const LABEL_BG = "rgba(246, 246, 244, 0.92)";
const LABEL_FG = "#1e3a8a";
const LABEL_PADDING = 6;
/** Label offset in CSS pixels, along the line's left-normal
 *  (≈ 2 rem given the default 16 px root font size). */
const LABEL_OFFSET_PX = 32;
/** Snap radius in sketch-plane millimetres. Generous enough to
 *  catch ordinary cursor jitter but tight enough that the user
 *  can still place free points near a committed entity without
 *  unwanted magnetism. Can be tuned later. */
const SNAP_RADIUS_MM = 10;
/** Snap indicator radius in CSS pixels, drawn as an open ring
 *  around the snap anchor. */
const SNAP_RING_PX = 8;

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
      kind: "rectangle";
      start: [number, number];
      current: [number, number];
    }
  | {
      kind: "circle";
      center: [number, number];
      current: [number, number];
    }
  | {
      kind: "polygon";
      vertices: [number, number][];
      current: [number, number];
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

function resolveSnap(
  cursor: [number, number],
  session: SketchSessionState,
  dragStart: [number, number] | null,
  suppress = false,
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
      if (
        os &&
        oe &&
        os.kind === "point" &&
        oe.kind === "point"
      ) {
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
    if (!store || !session.active || !session.selectedTool) return;
    if (e.button !== 0) return; // left button only
    e.preventDefault();
    (e.currentTarget as HTMLCanvasElement).setPointerCapture(e.pointerId);
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    const [u, v] = screenToSketchFromEvent(e, cam);
    // Resolve snap at pointerdown so the placed point/line-start
    // respects endpoint / origin magnetism even on a dead-stop
    // click (no prior move events).
    const snap = resolveSnap([u, v], session, null, e.shiftKey);
    snapRef.current = snap;
    const [su, sv] = snap.snapped;
    if (session.selectedTool === "sketch_point") {
      store.setSketchToolDraft({ at: [su, sv] });
      store.applySketchToolDraft();
      return;
    }
    if (session.selectedTool === "sketch_line") {
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
    if (session.selectedTool === "sketch_polygon") {
      // If the previous polygon was applied (draft cleared),
      // start fresh rather than appending to the old one.
      const draftVerts = session.toolDraft?.vertices;
      const draftEmpty = !draftVerts || (Array.isArray(draftVerts) && draftVerts.length === 0);
      const existing =
        dragRef.current.kind === "polygon" && !draftEmpty
          ? dragRef.current.vertices
          : [];

      // Close-on-first-vertex: if we have ≥ 3 vertices and
      // the click is within 8 px of the first vertex, auto-
      // close and commit instead of adding another vertex.
      if (existing.length >= 3) {
        const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
        const [fpx, fpy] = sketchToScreen(
          existing[0][0],
          existing[0][1],
          cam,
        );
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
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!session.active || !session.selectedTool) return;
    const cam = readCamForCanvas(e.currentTarget as HTMLCanvasElement);
    const [u, v] = screenToSketchFromEvent(e, cam);
    if (dragRef.current.kind === "line") {
      const snap = resolveSnap(
        [u, v],
        session,
        dragRef.current.start,
        e.shiftKey,
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
      const snap = resolveSnap([u, v], session, null, e.shiftKey);
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
      );
    }
  };

  const scheduleSolverReplay = () => {
    if (!store) return;
    void (async () => {
      const before = store.getState().sketchSession;
      if (!before.active) return;
      const snapshot = before.subCalls;
      try {
        const state = buildDraftSketchState(snapshot);
        const result = await solveSketch(state);
        const rebuilt = rebuildSubCallsFromSolved(snapshot, result.state);
        const after = store.getState().sketchSession;
        if (!after.active || after.subCalls !== snapshot) return;
        store.replaceSketchSubCalls(rebuilt);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error("fielddraft: solveSketch failed", err);
      }
    })();
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
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
    );
    snapRef.current = snap;
    const { start, startAnchorId } = dragRef.current;
    dragRef.current = { kind: "idle" };
    const [su, sv] = snap.snapped;
    // Reject zero-length lines — a click without drag on an
    // already-placed point would otherwise land as a degenerate
    // line.
    const dx = su - start[0];
    const dy = sv - start[1];
    if (dx * dx + dy * dy < 1e-6) {
      const canvas = canvasRef.current;
      if (canvas) {
        drawOverlay(
          canvas,
          session,
          cam,
          dragRef.current,
          hoverRef.current,
          snapRef.current,
        );
      }
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
    const newCalls: Array<{ type: string; params: Record<string, unknown> }> =
      [lineCall];

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
      } else if (
        c.kind === "coincident" &&
        snap.coincidentTargetId !== null
      ) {
        newCalls.push({
          type: "sketch_constraint_coincident",
          params: { a: predictedIds.end, b: snap.coincidentTargetId },
        });
      }
    }

    store.pushSketchSubCalls(newCalls);
    scheduleSolverReplay();
  };

  const onPointerCancel = () => {
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

  if (!session.active) return null;

  const toolActive = session.selectedTool !== null;
  return (
    <canvas
      ref={canvasRef}
      data-testid="sketch-canvas-2d"
      className={"fd-sketch-canvas" + (toolActive ? " is-tool-active" : "")}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onPointerLeave={onPointerLeave}
      onContextMenu={(e) => e.preventDefault()}
    />
  );
}

// ─── K6b.3d: commit-time solver result apply ────────────────────

type SubCall = SketchSessionState["subCalls"][number];

/**
 * Walk a sub_calls list alongside a solved `SketchState` and
 * return a new sub_calls list where every `sketch_point` and
 * `sketch_line` carries the solved coordinates. The mapping
 * replicates the kernel's deterministic entity allocation
 * order: entity id 0 is the origin, then ids 1+ are allocated
 * one per `add_point` / `add_line` call in sub_calls order.
 * Constraint calls are left untouched — their entity-id
 * references don't change under a pure coordinate solve.
 */
function rebuildSubCallsFromSolved(
  original: ReadonlyArray<SubCall>,
  solved: SketchState,
): SubCall[] {
  const pointAt = (id: number): [number, number] | null => {
    const e = solved.entities.find((x) => x.id === id);
    if (!e) return null;
    if (e.kind === "point") return [e.at.x, e.at.y];
    if (e.kind === "origin") return [0, 0];
    return null;
  };
  let nextId = 1; // origin is at id 0
  return original.map((call): SubCall => {
    if (call.type === "sketch_point") {
      const at = pointAt(nextId++);
      if (!at) return call;
      return { ...call, params: { ...call.params, at } };
    }
    if (call.type === "sketch_line") {
      const start = pointAt(nextId++);
      const end = pointAt(nextId++);
      nextId += 1; // line entity itself
      if (!start || !end) return call;
      return {
        ...call,
        params: { ...call.params, start, end },
      };
    }
    if (call.type === "sketch_circle") {
      const center = pointAt(nextId++);
      const circleId = nextId++;
      if (!center) return call;
      const circle = solved.entities.find((e) => e.id === circleId);
      const radius =
        circle && circle.kind === "circle"
          ? circle.radius
          : Number(call.params.radius);
      return {
        ...call,
        params: { ...call.params, center, radius },
      };
    }
    if (call.type === "sketch_rectangle") {
      // Match the bl/br/tr/tl allocation order — corner_a maps
      // to bl, corner_b maps to tr.
      const bl = pointAt(nextId++);
      nextId += 1; // br
      const tr = pointAt(nextId++);
      nextId += 1; // tl
      nextId += 4; // bottom, right, top, left
      if (!bl || !tr) return call;
      return {
        ...call,
        params: { ...call.params, corner_a: bl, corner_b: tr },
      };
    }
    // Constraint calls don't allocate entities; leave them
    // verbatim.
    return call;
  });
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

  // Grid: adaptive spacing that snaps to 1/2/5/10/… mm steps
  // and stays visually stable across zoom levels.
  {
    const minPxGap = 20;
    let step = 1;
    while (step * cam.zoom < minPxGap) step *= step < 5 ? 5 : 2;

    const left = cam.centerU - cam.width / (2 * cam.zoom);
    const right = cam.centerU + cam.width / (2 * cam.zoom);
    const top = cam.centerV + cam.height / (2 * cam.zoom);
    const bottom = cam.centerV - cam.height / (2 * cam.zoom);

    ctx.strokeStyle = "rgba(0,0,0,0.08)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    const gridLeft = Math.floor(left / step) * step;
    const gridBottom = Math.floor(bottom / step) * step;
    for (let u = gridLeft; u <= right; u += step) {
      const [px] = sketchToScreen(u, 0, cam);
      ctx.moveTo(px, 0);
      ctx.lineTo(px, cam.height);
    }
    for (let v = gridBottom; v <= top; v += step) {
      const [, py] = sketchToScreen(0, v, cam);
      ctx.moveTo(0, py);
      ctx.lineTo(cam.width, py);
    }
    ctx.stroke();

    // Origin axes (thicker, darker).
    ctx.strokeStyle = "rgba(0,0,0,0.25)";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    const [ox] = sketchToScreen(0, 0, cam);
    const [, oy] = sketchToScreen(0, 0, cam);
    ctx.moveTo(ox, 0);
    ctx.lineTo(ox, cam.height);
    ctx.moveTo(0, oy);
    ctx.lineTo(cam.width, oy);
    ctx.stroke();
  }

  // K6b.6: compute DOF to pick the 3-color status.
  let committedColor = COLOR_COMMITTED;
  try {
    const skState = buildDraftSketchState(session.subCalls);
    const dof = countDegreesOfFreedom(skState);
    committedColor = dofColor(dof);
  } catch {
    // Solver not available or sub-calls can't build a state.
  }

  // Draw committed geometry first (darker), then the drag preview
  // on top (lighter) so the preview always reads as "unfinished".
  for (const call of session.subCalls) {
    if (call.type === "sketch_line") {
      const start = toXy(call.params.start);
      const end = toXy(call.params.end);
      if (start && end) {
        drawLine(ctx, start, end, cam, committedColor, STROKE_PX);
        drawPoint(ctx, start, cam, committedColor);
        drawPoint(ctx, end, cam, committedColor);
      }
    } else if (call.type === "sketch_point") {
      const at = toXy(call.params.at);
      if (at) drawPoint(ctx, at, cam, committedColor);
    } else if (call.type === "sketch_rectangle") {
      const a = toXy(call.params.corner_a);
      const b = toXy(call.params.corner_b);
      if (a && b) drawRectangle(ctx, a, b, cam, committedColor);
    } else if (call.type === "sketch_circle") {
      const center = toXy(call.params.center);
      const radius = Number(call.params.radius);
      if (center && Number.isFinite(radius) && radius > 0) {
        drawCircle(ctx, center, radius, cam, committedColor);
        drawPoint(ctx, center, cam, committedColor);
      }
    }
  }

  // K6b.6: draw constraint glyphs at entity midpoints.
  for (const call of session.subCalls) {
    if (!call.type.startsWith("sketch_constraint")) continue;
    // Find the constrained line's midpoint from its entity_a
    // index. Walk earlier sub-calls to find the line with
    // matching entity allocation order.
    const entityA = Number(call.params.entity_a ?? call.params.line_a ?? -1);
    if (entityA < 0) continue;
    // Heuristic: find the line sub-call whose first endpoint
    // entity index is near entityA (each line allocates 3 ids:
    // start_point, end_point, line).
    const lineCalls = session.subCalls.filter((c) => c.type === "sketch_line");
    let lineIdx = Math.floor(entityA / 3) - 1; // rough mapping
    if (lineIdx < 0) lineIdx = 0;
    if (lineIdx >= lineCalls.length) continue;
    const lc = lineCalls[lineIdx];
    const start = toXy(lc?.params?.start);
    const end = toXy(lc?.params?.end);
    if (!start || !end) continue;
    const mid: [number, number] = [
      (start[0] + end[0]) / 2,
      (start[1] + end[1]) / 2,
    ];
    const [px, py] = sketchToScreen(mid[0], mid[1], cam);
    ctx.font = "bold 11px sans-serif";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    const label = call.type.includes("horizontal")
      ? "H"
      : call.type.includes("vertical")
        ? "V"
        : call.type.includes("coincident")
          ? "⊙"
          : "C";
    ctx.fillStyle = committedColor;
    ctx.fillText(label, px, py - 10);
  }

  if (drag.kind === "line") {
    const { start, current } = drag;
    drawLine(ctx, start, current, cam, COLOR_PREVIEW, STROKE_PX);
    drawPoint(ctx, start, cam, COLOR_PREVIEW);
    drawPoint(ctx, current, cam, COLOR_PREVIEW);
    drawDistanceLabel(ctx, start, current, cam);
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
  const [cx, cy] = sketchToScreen(center[0], center[1], cam);
  const radiusPx = radiusMm * cam.zoom;
  if (radiusPx < 0.5) return;
  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = STROKE_PX;
  ctx.beginPath();
  ctx.arc(cx, cy, radiusPx, 0, Math.PI * 2);
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
  const anchorX = cx + LABEL_OFFSET_PX / 2;
  const anchorY = cy - LABEL_OFFSET_PX / 2;
  const text = `R ${radiusMm.toFixed(2)} mm`;
  ctx.save();
  ctx.font = LABEL_FONT;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";
  const metrics = ctx.measureText(text);
  const textW = metrics.width;
  const textH = 16;
  ctx.fillStyle = LABEL_BG;
  ctx.strokeStyle = COLOR_COMMITTED;
  ctx.lineWidth = 1;
  roundRect(
    ctx,
    anchorX - textW / 2 - LABEL_PADDING,
    anchorY - textH / 2 - LABEL_PADDING / 2,
    textW + LABEL_PADDING * 2,
    textH + LABEL_PADDING,
    4,
  );
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = LABEL_FG;
  ctx.fillText(text, anchorX, anchorY);
  ctx.restore();
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
  const text = `${w.toFixed(2)} × ${h.toFixed(2)} mm`;
  ctx.save();
  ctx.font = LABEL_FONT;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";
  const metrics = ctx.measureText(text);
  const textW = metrics.width;
  const textH = 16;
  const boxX = px - textW / 2 - LABEL_PADDING;
  const boxY = py - textH / 2 - LABEL_PADDING / 2;
  ctx.fillStyle = LABEL_BG;
  ctx.strokeStyle = COLOR_COMMITTED;
  ctx.lineWidth = 1;
  roundRect(ctx, boxX, boxY, textW + LABEL_PADDING * 2, textH + LABEL_PADDING, 4);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = LABEL_FG;
  ctx.fillText(text, px, py);
  ctx.restore();
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

function drawDistanceLabel(
  ctx: CanvasRenderingContext2D,
  start: [number, number],
  end: [number, number],
  cam: Readonly<SketchCameraState>,
): void {
  const [sx, sy] = sketchToScreen(start[0], start[1], cam);
  const [ex, ey] = sketchToScreen(end[0], end[1], cam);
  const dxScreen = ex - sx;
  const dyScreen = ey - sy;
  const lenScreen = Math.hypot(dxScreen, dyScreen);
  if (lenScreen < 1) return;

  // Distance is measured in sketch-plane units (mm).
  const dxSketch = end[0] - start[0];
  const dySketch = end[1] - start[1];
  const distMm = Math.hypot(dxSketch, dySketch);
  const text = `${distMm.toFixed(2)} mm`;

  // Left-normal in screen space: rotate the direction 90° CCW.
  // Canvas has screen-Y pointing *down*, so "left as you face
  // from start to end" maps to rotating the direction vector
  // by -90° (CW) in canvas coordinates. `left = (-dy, dx)` is
  // the mathematical CCW, which is visually "right" on a
  // Y-down canvas — so we negate to get visual left.
  const nx = dyScreen / lenScreen;
  const ny = -dxScreen / lenScreen;

  const midX = (sx + ex) / 2;
  const midY = (sy + ey) / 2;
  const anchorX = midX + nx * LABEL_OFFSET_PX;
  const anchorY = midY + ny * LABEL_OFFSET_PX;

  ctx.save();
  ctx.font = LABEL_FONT;
  ctx.textBaseline = "middle";
  ctx.textAlign = "center";
  const metrics = ctx.measureText(text);
  const textW = metrics.width;
  const textH = 16;
  const boxX = anchorX - textW / 2 - LABEL_PADDING;
  const boxY = anchorY - textH / 2 - LABEL_PADDING / 2;
  const boxW = textW + LABEL_PADDING * 2;
  const boxH = textH + LABEL_PADDING;
  ctx.fillStyle = LABEL_BG;
  ctx.strokeStyle = COLOR_COMMITTED;
  ctx.lineWidth = 1;
  roundRect(ctx, boxX, boxY, boxW, boxH, 4);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = LABEL_FG;
  ctx.fillText(text, anchorX, anchorY);
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
