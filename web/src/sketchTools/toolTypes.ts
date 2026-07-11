/**
 * Sketch tool-controller registry types (architecture review, Befund 2).
 *
 * `SketchCanvas2d` used to be one giant component whose `onPointerDown` /
 * `onPointerMove` / `onPointerUp` were if-chains over ~25 tool names, with a
 * 16-variant `DragState` union and tool-specific refs. The gesture state
 * machines now live in one controller per tool (or per small tool family)
 * under `sketchTools/`; the canvas keeps the event plumbing (pointer capture,
 * camera forwarding, touch lockout, snap infrastructure, HUD redraw) and
 * dispatches to the active controller through this interface.
 *
 * Conventions:
 *  - Controllers are stateless singletons. All mutable state lives in the
 *    canvas-owned slots exposed through {@link ToolContext}: the drag slot
 *    (one in-flight gesture, owned by exactly one controller) and the
 *    tool-state slot (per-tool-activation scratch such as the mirror axis or
 *    the move base point, cleared on tool change).
 *  - Geometry/math helpers stay in the existing `sketch*.ts` modules and are
 *    imported by controllers directly; only component-coupled facilities
 *    (editors, redraw, solver replay) go through the context.
 */

import type { PointerEvent as ReactPointerEvent } from "react";
import type { KernelStore } from "../store";
import type { SketchSessionState } from "../sketchSession";
import type { SketchCameraState } from "../sketchCameraBridge";
import type { ProjectedEdge, SnapResolution } from "../sketchSnapResolve";
import type { SketchLabel } from "../sketchLabelBridge";
import type { TaggedCornerHit, TaggedTextHit } from "../sketchHitTest";
import type { DragController } from "../solver/dragController";

/** A sketch feature sub-call, the unit `store.pushSketchSubCalls` consumes. */
export type SketchSubCall = { type: string; params: Record<string, unknown> };

/** Pick radius for select-mode hit-testing, in CSS pixels. Divided
 *  by the camera zoom to get the sketch-plane tolerance. */
export const SELECT_PICK_PX = 6;

/** Grip pick radius in CSS pixels — generous so the small handles
 *  are easy to grab. Beats the entity hit-test, so grabbing a grip
 *  starts a drag rather than re-toggling the selection. */
export const GRIP_PICK_PX = 9;

/** The select-mode pick tolerance in sketch-plane mm for the current zoom.
 *  `factor` widens it for small targets (corners / axis lines use 2.5×). */
export function pickTolMm(cam: { zoom: number }, factor = 1): number {
  return (SELECT_PICK_PX * factor) / Math.max(cam.zoom, 1e-6);
}

/** K6b.8: Ctrl = snap to grid — round to the nearest adaptive grid step
 *  (the same step progression the drawn grid uses: 1, 5, 10, 50, …). */
export function gridSnapped(
  u: number,
  v: number,
  zoom: number,
): [number, number] {
  const minPxGap = 20;
  let step = 1;
  while (step * zoom < minPxGap) step *= step < 5 ? 5 : 2;
  return [Math.round(u / step) * step, Math.round(v / step) * step];
}

/** The canvas's in-flight pointer gesture: the owning controller plus its
 *  private drag state (each controller defines its own type and reads it
 *  back through {@link ToolContext.dragOf}). `null` in the canvas slot
 *  means idle. */
export interface ActiveToolDrag {
  owner: SketchToolController;
  state: unknown;
}

/** A pointer event as the canvas hands it to a controller: the original
 *  React event plus the cursor resolved into the coordinate spaces the old
 *  if-chains used at the equivalent point of their flow. */
export interface ToolPointerEvent {
  /** The raw React pointer event (buttons, modifier keys, `detail`, …). */
  e: ReactPointerEvent<HTMLCanvasElement>;
  /** Camera snapshot with the overlay's real canvas dimensions. */
  cam: Readonly<SketchCameraState>;
  /** Cursor in sketch-plane mm, unmodified. */
  raw: [number, number];
  /** Cursor in sketch-plane mm with Ctrl/Cmd grid snapping applied
   *  (equal to `raw` when no modifier is held). */
  gridded: [number, number];
  /** Cursor in canvas-relative CSS pixels (for label hit tests). */
  clickPx: [number, number];
  /** Snap resolution at `gridded`, when the canvas resolved it for this
   *  dispatch ({@link SketchToolController.resolveSnapOnDown}); else null. */
  snap: SnapResolution | null;
}

/** Transient HUD contribution of a controller's in-flight drag — preview
 *  primitives in sketch (u,v) space (rendered as 3D geometry by
 *  `SketchHud3D`), 3D billboard labels (dimension readouts), and an
 *  optional screen-space draw pass (the marquee box). */
export interface ToolHud {
  previewCalls?: SketchSubCall[];
  previewLines?: Array<[[number, number], [number, number]]>;
  previewPoints?: Array<[number, number]>;
  previewGuides?: Array<[[number, number], [number, number]]>;
  labels?: SketchLabel[];
  drawScreen?: (
    ctx2d: CanvasRenderingContext2D,
    cam: Readonly<SketchCameraState>,
  ) => void;
}

/**
 * The canvas facilities behind the in-scene DOM editors (dimension length,
 * text, corner radius/distance) and the recent-line readout. Implemented by
 * `SketchCanvas2d` over its React state; controllers drive them through this
 * facade so they stay plain modules.
 */
export interface SketchCanvasUi {
  /** The floating length/dimension value editor (F18). */
  dimEditorOpen(): boolean;
  dismissDimEditor(): void;
  /** Open the length editor on a committed line's dimension readout. */
  openLineLengthEditor(
    index: number,
    anchorPx: [number, number],
    lengthMm: number,
  ): void;
  /** Click an existing distance-dimension's value label to edit it as a
   *  CONSTRAINT. Returns true when it handled the click. */
  tryOpenDimensionEditor(
    clickPx: [number, number],
    cam: Readonly<SketchCameraState>,
  ): boolean;
  /** The most recently placed line's sub-call index (click-to-edit
   *  dimension readout target), or null. */
  recentLine(): number | null;
  setRecentLine(index: number | null): void;
  /** The committed geometry of the recent line (post-solve params). */
  recentLineGeometry(): {
    start: [number, number];
    end: [number, number];
  } | null;
  /** The in-scene text editor (ticket pqoe). */
  textEditorOpen(): boolean;
  commitTextEditor(): void;
  openTextEditorCreate(clickPx: [number, number], uv: [number, number]): void;
  openTextEditorEdit(
    hit: TaggedTextHit,
    cam: Readonly<SketchCameraState>,
  ): void;
  /** The in-scene fillet-radius / chamfer-setback editor (plan item 4). */
  cornerEditorOpen(): boolean;
  dismissCornerEditor(): void;
  openCornerEditor(
    hit: TaggedCornerHit,
    cam: Readonly<SketchCameraState>,
  ): void;
}

/**
 * Canvas-owned mutable slots and facilities, handed to every controller
 * callback. One context instance per canvas render (closes over the
 * component's refs), so controllers must not hold on to it across events.
 */
export interface ToolContext {
  readonly store: KernelStore;
  readonly session: SketchSessionState;
  /** Analytic B-Rep edges projected onto the sketch plane (snap targets /
   *  Project-tool guides). */
  readonly projectedEdges: ProjectedEdge[];

  /** The in-flight drag state, if it is owned by `owner`; else null. */
  dragOf<T>(owner: SketchToolController): T | null;
  /** Begin/replace the in-flight drag, owned by `owner`. */
  setDrag(owner: SketchToolController, state: unknown): void;
  clearDrag(): void;

  /** Per-tool-activation scratch state (mirror axis, move base point).
   *  Cleared by the canvas whenever the selected tool changes. */
  toolState<T>(): T | null;
  setToolState(state: unknown): void;

  /** Resolve the snap at `cursor` the way the old inline flow did: the
   *  session's committed sub-calls, the pixel-true snap radius for
   *  `ev.cam`, Shift suppression from `ev.e`, and the projected edges. */
  resolveSnapAt(
    ev: ToolPointerEvent,
    cursor: [number, number],
    dragStart: [number, number] | null,
  ): SnapResolution;
  /** Publish the snap resolution the HUD indicator ring / label draws. */
  setSnap(snap: SnapResolution): void;
  /** Publish the hover preview point (idle-cursor snap magnetism). */
  setHover(p: [number, number] | null): void;
  /** Highlight a line as the mirror-axis / constraint pick candidate. */
  setMirrorAxisHover(
    seg: [[number, number], [number, number]] | null,
  ): void;

  capturePointer(ev: ToolPointerEvent): void;
  releasePointer(ev: ToolPointerEvent): void;

  /** Invalidate the overlay — redraw on the next animation frame. */
  requestRedraw(): void;
  /** Redraw the overlay synchronously (degenerate-commit cleanup). */
  redrawNow(): void;
  /** Re-run the constraint solver over the committed sub-calls. */
  scheduleSolverReplay(): void;

  /** Live grip-drag solver (continuation model) — one stable instance for
   *  the canvas lifetime, used by the select controller. */
  readonly gripDrag: DragController;

  readonly ui: SketchCanvasUi;
}

/**
 * One sketch tool's gesture state machine. Registered under its
 * `selectedTool` name in `registry.ts`; the pseudo-tools "select mode"
 * (no tool) and "constraint tool" (any `sketch_constraint_*`) have
 * dedicated controllers the canvas resolves explicitly.
 */
export interface SketchToolController {
  /** When true the canvas applies Ctrl-grid snapping and resolves + stores
   *  the snap BEFORE `onDown` (the old flow position of the shared grid /
   *  `resolveSnap` block). Tools listed before that block (trim, text,
   *  split, extend, fillet, chamfer, move/copy) leave it unset. */
  resolveSnapOnDown?: boolean;
  /** Marks the select-mode gesture controllers (select, move/copy) whose
   *  in-flight drags bypass the tool-mode move postlude (hover fallback,
   *  tool hover highlight, synchronous overlay redraw). */
  selectionGesture?: boolean;
  /** Suppress the shared hover snap point (the little dot with snap
   *  magnetism). The single-click edit modes (trim / split / extend)
   *  highlight their target entity instead — a jumpy point that snaps
   *  onto curves reads as noise there (ticket 1efd4bcb). */
  noHoverPoint?: boolean;

  onDown?(ctx: ToolContext, ev: ToolPointerEvent): void;
  /** Return true when the move was consumed (an owned drag was updated);
   *  anything else lets the canvas run the shared hover-snap fallback. */
  onMove?(ctx: ToolContext, ev: ToolPointerEvent): boolean | void;
  onUp?(ctx: ToolContext, ev: ToolPointerEvent): void;
  /** Window-level keydown routed to the active tool (spline finish,
   *  mirror escape, pending-line cancel). */
  onKey?(ctx: ToolContext, e: KeyboardEvent): void;
  /** Tool-specific hover highlight, run after `onMove` / the hover
   *  fallback on every tool-mode move. When absent the canvas clears the
   *  mirror-axis highlight (the old else-branch). */
  onHover?(ctx: ToolContext, ev: ToolPointerEvent): void;
  /** HUD primitives + labels for this controller's in-flight drag. */
  hud?(
    drag: unknown,
    session: SketchSessionState,
    cam: Readonly<SketchCameraState>,
  ): ToolHud;
  /** First-step instruction for the floating ToolHint pill. */
  initialHint?(tool: string): string | null;
  /** The tool is being left (tool change / Esc-to-select): commit or drop
   *  an in-flight gesture. The canvas clears the tool-state slot after. */
  onDeactivate?(ctx: ToolContext): void;
}
