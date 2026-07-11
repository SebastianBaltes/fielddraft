/**
 * Single-click editing-mode controllers (architecture review, Befund 2):
 * Trim, Split, Extend, Offset and Project. None of them is a kernel drawing
 * tool — a click picks a target curve/edge and applies the edit through the
 * store (sketchTrim/Split/Extend/Offset) or appends a projection sub-call.
 * 1:1 translations of the old `SketchCanvas2d` branches.
 */

import { nearestTrimTarget } from "../sketchTrim";
import { nearestProjectedEdge } from "../sketchProject";
import { pickTolMm } from "./toolTypes";
import { nearestLineSubCall } from "./pickHelpers";
import { buildDraftSketchState } from "../sketchSession";
import { hitTestSketchEntity } from "../sketchHitTest";
import { setHoveredSketchEntity } from "../sketchEntityHoverBridge";
import type { SketchToolController, ToolContext, ToolPointerEvent } from "./toolTypes";

/** Ticket 1efd4bcb: the single-click edit modes highlight the entity the
 *  click would edit (amber, like select-mode hover) instead of showing
 *  the snap-magnetised hover point — the immediate "this piece" feedback
 *  the ticket asks for. */
function hoverEditTarget(ctx: ToolContext, ev: ToolPointerEvent): void {
  const hit = hitTestSketchEntity(
    buildDraftSketchState(ctx.session.subCalls),
    ev.raw,
    pickTolMm(ev.cam),
  );
  setHoveredSketchEntity(hit);
}

// Trim: not a drawing tool — a single click cuts the curve under the
// cursor at its nearest intersections and drops the picked piece. No
// drag, no snap, no grid; the raw click decides which segment dies.
export const trimTool: SketchToolController = {
  noHoverPoint: true,
  onDown(ctx, ev) {
    ctx.store.trimSketchAt(ev.raw, pickTolMm(ev.cam));
    ctx.requestRedraw();
  },
  onHover: hoverEditTarget,
  onDeactivate() {
    setHoveredSketchEntity(null);
  },
};

// Split: keep-both sibling of Trim — also a single-click editing mode
// (not a kernel tool). The click picks the nearest curve under the
// cursor and cuts it into two pieces meeting at the click point; both
// halves survive (see store.splitSketchAt / sketchSplit.ts).
export const splitTool: SketchToolController = {
  noHoverPoint: true,
  onDown(ctx, ev) {
    const tolMm = pickTolMm(ev.cam);
    const pick = nearestTrimTarget(ctx.session.subCalls, ev.raw, tolMm);
    if (pick !== null) ctx.store.splitSketchAt(pick, ev.raw, tolMm);
    ctx.requestRedraw();
  },
  onHover: hoverEditTarget,
  onDeactivate() {
    setHoveredSketchEntity(null);
  },
};

// Extend: complement of Trim, also a single-click editing mode (not a
// kernel tool). The click picks the nearest LINE under the cursor and
// lengthens its grabbed end (the one nearer the click) toward the
// nearest other curve (see store.extendSketchAt / sketchExtend.ts).
export const extendTool: SketchToolController = {
  onDown(ctx, ev) {
    const tolMm = pickTolMm(ev.cam);
    const pick = nearestLineSubCall(ctx.session.subCalls, ev.raw, tolMm);
    if (pick !== null) ctx.store.extendSketchAt(pick, ev.raw, tolMm);
    ctx.requestRedraw();
  },
};

// F15: Offset is an interactive editing mode, not a kernel tool. A
// single click picks the nearest curve and appends a parallel copy at
// the toolbar distance toward the clicked side — the click point
// doubles as the side selector (a line offsets to the click's side; a
// circle/arc grows if the click is outside its rim, shrinks if inside).
// See store.offsetSketchAt / sketchOffset.ts.
export const offsetTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const tolMm = pickTolMm(ev.cam);
    const pick = nearestTrimTarget(ctx.session.subCalls, ev.gridded, tolMm);
    if (pick !== null) {
      ctx.store.offsetSketchAt(
        pick,
        ev.gridded,
        ctx.store.getState().sketchToolParams.offsetDistance,
      );
    }
    ctx.requestRedraw();
  },
};

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
export const projectTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    const tolMm = pickTolMm(ev.cam);
    const idx = nearestProjectedEdge(ctx.projectedEdges, ev.gridded, tolMm);
    if (idx !== null) {
      const edge = ctx.projectedEdges[idx];
      ctx.store.pushSketchSubCalls([
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
      ctx.scheduleSolverReplay();
    }
    ctx.requestRedraw();
  },
};
