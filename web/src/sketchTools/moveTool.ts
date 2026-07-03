/**
 * Move / Copy controller (architecture review, Befund 2 / F17 + ticket
 * 3rz2): a two-click transform of the current selection (base → target
 * click translates by the delta), or a direct click-drag on an entity that
 * selects it (if needed) and translates on release. Move translates in
 * place, Copy appends translated copies. One controller serves both tool
 * names — `copy` is derived from the active tool at pointer-down.
 */

import { buildDraftSketchState } from "../sketchSession";
import { hitTestSketchEntity } from "../sketchHitTest";
import { pickTolMm } from "./toolTypes";
import type { SketchToolController } from "./toolTypes";

type Pt = [number, number];

interface MoveDrag {
  start: Pt;
  current: Pt;
  copy: boolean;
  moved: boolean;
}

/** The pending base point of the two-click base→target flow (tool-state
 *  slot; cleared on tool change by the canvas). */
type MoveBase = Pt;

export const moveTool: SketchToolController = {
  selectionGesture: true,
  onDown(ctx, ev) {
    const copy = ctx.session.selectedTool === "sketch_copy";
    const [u, v] = ev.raw;
    // Ticket 3rz2: if a base point is already pending (Onshape base→target
    // path), the second click applies the delta. Otherwise, a press ON an
    // entity selects it (if needed) and arms a direct click-drag; a press on
    // empty space records the base point for the two-click path.
    const base = ctx.toolState<MoveBase>();
    if (base !== null) {
      ctx.store.moveSketchSelection(u - base[0], v - base[1], copy);
      ctx.setToolState(null);
      ctx.requestRedraw();
      return;
    }
    const tolMm = pickTolMm(ev.cam);
    const state = buildDraftSketchState(ctx.session.subCalls);
    const hit = hitTestSketchEntity(state, [u, v], tolMm);
    if (hit !== null) {
      if (!ctx.session.selection.includes(hit)) {
        ctx.store.setSketchSelection([hit]);
      }
      ctx.capturePointer(ev);
      ctx.setDrag(moveTool, {
        start: [u, v],
        current: [u, v],
        copy,
        moved: false,
      } satisfies MoveDrag);
    } else {
      // Empty space → start the two-click base→target transform.
      ctx.setToolState([u, v] satisfies MoveBase);
    }
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    // Ticket 3rz2: direct click-drag of the Move/Copy selection. Track the
    // cursor; the actual translate lands on release (onUp) so the
    // constraint solver runs once, not per frame. The `moved` flag
    // separates a real drag from a plain select-click.
    const drag = ctx.dragOf<MoveDrag>(moveTool);
    if (!drag) return false;
    const [u, v] = ev.raw;
    const movedPx =
      Math.hypot(u - drag.start[0], v - drag.start[1]) *
      Math.max(ev.cam.zoom, 1e-6);
    ctx.setDrag(moveTool, {
      ...drag,
      current: [u, v],
      moved: drag.moved || movedPx > 4,
    });
    ctx.requestRedraw();
    return true;
  },
  onUp(ctx, ev) {
    // Ticket 3rz2: commit a direct Move/Copy click-drag. A real drag
    // translates the selection by (current − start); a release without drag
    // was just a select-click (the press already selected the entity).
    const drag = ctx.dragOf<MoveDrag>(moveTool);
    if (!drag) return;
    ctx.releasePointer(ev);
    ctx.clearDrag();
    if (drag.moved) {
      ctx.store.moveSketchSelection(
        drag.current[0] - drag.start[0],
        drag.current[1] - drag.start[1],
        drag.copy,
      );
    }
    ctx.requestRedraw();
  },
  initialHint(tool) {
    return tool === "sketch_copy"
      ? "Kopieren: Basispunkt anklicken, dann Zielpunkt."
      : "Verschieben: selektierte Geometrie ziehen, oder Basis- dann Zielpunkt anklicken.";
  },
};
