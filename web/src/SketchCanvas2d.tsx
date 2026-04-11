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
const LABEL_FONT = "600 14px system-ui, -apple-system, sans-serif";
const LABEL_BG = "rgba(246, 246, 244, 0.92)";
const LABEL_FG = "#1e3a8a";
const LABEL_PADDING = 6;
/** Label offset in CSS pixels, along the line's left-normal
 *  (≈ 2 rem given the default 16 px root font size). */
const LABEL_OFFSET_PX = 32;

type DragState =
  | { kind: "idle" }
  | { kind: "line"; start: [number, number]; current: [number, number] };

export function SketchCanvas2d({ store, session }: Props): ReactElement | null {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const dragRef = useRef<DragState>({ kind: "idle" });
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
          drawOverlay(canvas, session, cam, dragRef.current);
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
    if (session.selectedTool === "sketch_point") {
      store.setSketchToolDraft({ at: [u, v] });
      store.applySketchToolDraft();
      return;
    }
    if (session.selectedTool === "sketch_line") {
      dragRef.current = { kind: "line", start: [u, v], current: [u, v] };
      // Force redraw so the preview appears at zero length.
      lastCamVersionRef.current = -1;
    }
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (dragRef.current.kind !== "line") return;
    const cam = readSketchCamera();
    const [u, v] = screenToSketchFromEvent(e, cam);
    dragRef.current = { ...dragRef.current, current: [u, v] };
    // Redraw every move — the drag path is hot, but the overlay
    // canvas is cheap.
    const canvas = canvasRef.current;
    if (canvas) drawOverlay(canvas, session, cam, dragRef.current);
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (dragRef.current.kind !== "line") return;
    (e.currentTarget as HTMLCanvasElement).releasePointerCapture(e.pointerId);
    const cam = readSketchCamera();
    const [u, v] = screenToSketchFromEvent(e, cam);
    const { start } = dragRef.current;
    dragRef.current = { kind: "idle" };
    // Reject zero-length lines — a click without drag on an
    // already-placed point would otherwise land as a degenerate
    // line.
    const dx = u - start[0];
    const dy = v - start[1];
    if (dx * dx + dy * dy < 1e-6) {
      const canvas = canvasRef.current;
      if (canvas) drawOverlay(canvas, session, cam, dragRef.current);
      return;
    }
    if (store) {
      store.setSketchToolDraft({ start, end: [u, v] });
      store.applySketchToolDraft();
    }
  };

  const onPointerCancel = () => {
    dragRef.current = { kind: "idle" };
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
  }
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
