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
} from "./sketchSession";
import {
  inferCandidates,
  type InferenceCandidate,
} from "./solver/inferenceEngine";
import type { SketchState } from "./solver/types";

interface Props {
  store: KernelStore | null;
  session: SketchSessionState;
}

/** Visual tokens. Keep in sync with `styles/tokens.css` if we
 *  ever add sketch colours there. */
const STROKE_PX = 1.5;
const POINT_PX = 3;
const COLOR_COMMITTED = "#2563eb";
const COLOR_PREVIEW = "#93c5fd";
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
  | { kind: "line"; start: [number, number]; current: [number, number] };

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
}

function resolveSnap(
  cursor: [number, number],
  session: SketchSessionState,
  dragStart: [number, number] | null,
): SnapResolution {
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
    return { snapped: cursor, candidate: null, anchor: null };
  }
  const top = cands[0];
  const c = top.constraint;
  // Endpoint / origin snap: pull the cursor onto the target
  // point's coordinates.
  if (c.kind === "coincident") {
    const target = state.entities.find((e) => e.id === c.b);
    if (target) {
      if (target.kind === "point") {
        return {
          snapped: [target.at.x, target.at.y],
          candidate: top,
          anchor: [target.at.x, target.at.y],
        };
      }
      if (target.kind === "origin") {
        return {
          snapped: [0, 0],
          candidate: top,
          anchor: [0, 0],
        };
      }
    }
  }
  // Horizontal/vertical: lock one axis to the drag-start axis.
  if (c.kind === "horizontal" && dragStart) {
    const snapped: [number, number] = [cursor[0], dragStart[1]];
    return { snapped, candidate: top, anchor: snapped };
  }
  if (c.kind === "vertical" && dragStart) {
    const snapped: [number, number] = [dragStart[0], cursor[1]];
    return { snapped, candidate: top, anchor: snapped };
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
          return { snapped, candidate: top, anchor: snapped };
        }
      }
    }
  }
  return { snapped: cursor, candidate: null, anchor: null };
}

export function SketchCanvas2d({ store, session }: Props): ReactElement | null {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const dragRef = useRef<DragState>({ kind: "idle" });
  const hoverRef = useRef<[number, number] | null>(null);
  const snapRef = useRef<SnapResolution>({
    snapped: [0, 0],
    candidate: null,
    anchor: null,
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
        const cam = readSketchCamera();
        if (
          cam.version !== lastCamVersionRef.current ||
          lastSessionRef.current !== session
        ) {
          lastCamVersionRef.current = cam.version;
          lastSessionRef.current = session;
          drawOverlay(
            canvas,
            session,
            cam,
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
    const cam = readSketchCamera();
    const [u, v] = screenToSketchFromEvent(e, cam);
    // Resolve snap at pointerdown so the placed point/line-start
    // respects endpoint / origin magnetism even on a dead-stop
    // click (no prior move events).
    const snap = resolveSnap([u, v], session, null);
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
      };
      lastCamVersionRef.current = -1;
    }
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!session.active || !session.selectedTool) return;
    const cam = readSketchCamera();
    const [u, v] = screenToSketchFromEvent(e, cam);
    if (dragRef.current.kind === "line") {
      const snap = resolveSnap([u, v], session, dragRef.current.start);
      snapRef.current = snap;
      dragRef.current = { ...dragRef.current, current: snap.snapped };
    } else {
      // Hover path: show a preview point with snap magnetism so
      // the user sees the same feedback a click-to-place would
      // land on.
      const snap = resolveSnap([u, v], session, null);
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

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (dragRef.current.kind !== "line") return;
    (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
    const cam = readSketchCamera();
    const [u, v] = screenToSketchFromEvent(e, cam);
    const snap = resolveSnap([u, v], session, dragRef.current.start);
    snapRef.current = snap;
    const { start } = dragRef.current;
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
    if (store) {
      store.setSketchToolDraft({ start, end: [su, sv] });
      store.applySketchToolDraft();
    }
  };

  const onPointerCancel = () => {
    dragRef.current = { kind: "idle" };
    hoverRef.current = null;
    snapRef.current = { snapped: [0, 0], candidate: null, anchor: null };
    lastCamVersionRef.current = -1;
  };

  const onPointerLeave = () => {
    hoverRef.current = null;
    snapRef.current = { snapped: [0, 0], candidate: null, anchor: null };
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

  // Draw committed geometry first (darker), then the drag preview
  // on top (lighter) so the preview always reads as "unfinished".
  for (const call of session.subCalls) {
    if (call.type === "sketch_line") {
      const start = toXy(call.params.start);
      const end = toXy(call.params.end);
      if (start && end) {
        drawLine(ctx, start, end, cam, COLOR_COMMITTED, STROKE_PX);
        drawPoint(ctx, start, cam, COLOR_COMMITTED);
        drawPoint(ctx, end, cam, COLOR_COMMITTED);
      }
    } else if (call.type === "sketch_point") {
      const at = toXy(call.params.at);
      if (at) drawPoint(ctx, at, cam, COLOR_COMMITTED);
    }
  }

  if (drag.kind === "line") {
    const { start, current } = drag;
    drawLine(ctx, start, current, cam, COLOR_PREVIEW, STROKE_PX);
    drawPoint(ctx, start, cam, COLOR_PREVIEW);
    drawPoint(ctx, current, cam, COLOR_PREVIEW);
    drawDistanceLabel(ctx, start, current, cam);
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
