/**
 * Select-mode controller (architecture review, Befund 2): the pseudo-tool
 * active when NO drawing tool is selected. Owns the entity hover highlight
 * (zadt), click selection with additive toggle, the grip drag with the live
 * solver (P0.3), the rigid entity/group translate (matf), the box-select
 * marquee (F4) and the in-scene editor hit-tests (corner radius / text /
 * dimension value). 1:1 translation of the old `SketchCanvas2d` select-mode
 * branches; resolved explicitly by the canvas (not in the name registry).
 */

import { sketchToScreen } from "../sketchCameraBridge";
import { buildDraftSketchState } from "../sketchSession";
import {
  entitiesInBox,
  hitTestSketchEntity,
  hitTestTaggedCorner,
  hitTestTaggedText,
} from "../sketchHitTest";
import {
  allGrips,
  entityIdForGrip,
  gripsForSelection,
  pickGripAt,
  type Grip,
} from "../sketchGrips";
import { planSelectDrag } from "../sketchSelectDrag";
import { setHoveredSketchEntity } from "../sketchEntityHoverBridge";
import { drawMarquee } from "../sketchCanvasHud";
import { GRIP_PICK_PX, pickTolMm } from "./toolTypes";
import type {
  SketchToolController,
  ToolHud,
  ToolPointerEvent,
} from "./toolTypes";

type Pt = [number, number];

type SelectDrag =
  | {
      /** P0.3: dragging a grip handle on committed geometry. The geometry
       *  follows the cursor live (continuation-model solver); the drop
       *  settles the last frame. */
      kind: "grip";
      grip: Grip;
    }
  | {
      /** Ticket matf: a plain Select-mode click-drag that rigidly translates
       *  geometry — a single entity body (line / circle / arc / region) or a
       *  whole multi-selection grabbed and dragged together. `entityIds` are
       *  the committed entities to translate; the translate lands on release
       *  by (current − start), then the solver settles constraints. A release
       *  without a real drag was a plain pick (the down already selected). */
      kind: "translate";
      entityIds: number[];
      /** Selection to apply once a real drag begins (so the moved group
       *  highlights). Applied lazily on the first move, not at arm time, to
       *  preserve a no-drag click's additive toggle. */
      selectIds: number[];
      start: Pt;
      current: Pt;
      moved: boolean;
    }
  | {
      /** F4: box-select / marquee. Drag direction picks the mode: left→right
       *  is an enclosing "window" select, right→left a "crossing" select.
       *  `startPx`/`currentPx` decide whether the gesture was a real drag or
       *  just a click. */
      kind: "marquee";
      start: Pt;
      current: Pt;
      startPx: Pt;
      currentPx: Pt;
    };

/**
 * P0.3: the grip under the pointer (within `GRIP_PICK_PX`), or null.
 * Compared in screen pixels against each grip of the current
 * selection so the pick tolerance is zoom-independent.
 */
function pickSelectionGrip(ev: ToolPointerEvent, grips: Grip[]): Grip | null {
  if (grips.length === 0) return null;
  let best: Grip | null = null;
  let bestD = GRIP_PICK_PX;
  for (const g of grips) {
    const [gx, gy] = sketchToScreen(g.pos[0], g.pos[1], ev.cam);
    const d = Math.hypot(ev.clickPx[0] - gx, ev.clickPx[1] - gy);
    if (d <= bestD) {
      bestD = d;
      best = g;
    }
  }
  return best;
}

export const selectTool: SketchToolController = {
  selectionGesture: true,
  onDown(ctx, ev) {
    const { session, store } = ctx;
    const [u, v] = ev.raw;

    // Ticket matf: resolve the precise HANDLE under the cursor (a free
    // point, a line endpoint, a polygon/spline vertex, a circle/arc
    // centre/rim) up-front. Whether grabbing it reshapes that one degree of
    // freedom (a lone handle) or rigid-translates a group (a handle inside a
    // multi-selection) is decided by `planSelectDrag` below, once the broad
    // entity hit is also known. Searches ALL committed entities so a direct
    // pointer-down grabs without a prior selecting click (ticket t11x).
    const directGrip =
      pickSelectionGrip(
        ev,
        gripsForSelection(session.subCalls, session.selection),
      ) ??
      pickGripAt(
        allGrips(session.subCalls),
        ev.clickPx,
        (pos) => sketchToScreen(pos[0], pos[1], ev.cam),
        GRIP_PICK_PX,
      );
    const handleEntityId =
      directGrip !== null
        ? entityIdForGrip(session.subCalls, directGrip)
        : null;

    const tolMm = pickTolMm(ev.cam);

    // Plan item 4: a click on a recipe-tagged corner (fillet arc /
    // chamfer bevel) opens the in-scene radius / distance editor instead
    // of selecting. The tagged-corner test reads sub-call geometry
    // directly (a fillet arc isn't a picked entity), so it must run
    // before the entity hit-test. A click elsewhere while the editor is
    // open just dismisses it.
    if (ctx.ui.cornerEditorOpen()) {
      ctx.ui.dismissCornerEditor();
      ctx.requestRedraw();
      return;
    }
    const corner = hitTestTaggedCorner(session.subCalls, [u, v], tolMm);
    if (corner !== null) {
      ctx.ui.openCornerEditor(corner, ev.cam);
      ctx.requestRedraw();
      return;
    }

    // Plan item 5: a click on a recipe-tagged baked text reopens the
    // in-scene text editor pre-filled with the stored value/height/font,
    // in "edit existing group" mode. Runs after the corner test (a tagged
    // corner is a small precise target) but before the entity hit-test
    // (clicking a glyph fill should edit the text, not pick a region). A
    // click while the editor is already open just commits + dismisses
    // (mirrors the corner editor), rather than falling through to select.
    if (ctx.ui.textEditorOpen()) {
      ctx.ui.commitTextEditor();
      ctx.requestRedraw();
      return;
    }
    const text = hitTestTaggedText(session.subCalls, [u, v], tolMm);
    if (text !== null) {
      ctx.ui.openTextEditorEdit(text, ev.cam);
      ctx.requestRedraw();
      return;
    }

    // A click on an existing dimension's value label re-edits it (constraint
    // value), before the entity hit-test so it wins over selecting the line.
    // A click while the editor is open just dismisses it.
    if (ctx.ui.dimEditorOpen()) {
      ctx.ui.dismissDimEditor();
      ctx.requestRedraw();
      return;
    }
    if (ctx.ui.tryOpenDimensionEditor(ev.clickPx, ev.cam)) {
      ctx.requestRedraw();
      return;
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
      ctx.capturePointer(ev);
      if (plan.selectId !== null) store.setSketchSelection([plan.selectId]);
      ctx.setDrag(selectTool, {
        kind: "grip",
        grip: directGrip,
      } satisfies SelectDrag);
      // Live-solve from here: every move re-solves so constraints stay
      // satisfied as the grip moves, and drop just settles the last frame.
      ctx.gripDrag.start(store, directGrip);
      ctx.requestRedraw();
      return;
    }

    if (plan.kind === "translate") {
      // Arm a rigid-translate of `entityIds`. The move lands on release
      // (current − start) so the solver runs once; a release WITHOUT a real
      // drag falls back to the historic additive toggle (handled in onUp),
      // so plain click-to-build-a-selection still works. The selection is
      // set lazily on the first real move (onMove), not here — arming must
      // not disturb a no-drag click's toggle.
      ctx.capturePointer(ev);
      ctx.setDrag(selectTool, {
        kind: "translate",
        entityIds: plan.entityIds,
        selectIds: plan.selectIds,
        start: [u, v],
        current: [u, v],
        moved: false,
      } satisfies SelectDrag);
      ctx.requestRedraw();
      return;
    }

    // Nothing under the cursor: begin a marquee. We DON'T clear the
    // selection yet — that happens on pointerup if the gesture turns out to
    // be a plain click (no drag).
    ctx.capturePointer(ev);
    ctx.setDrag(selectTool, {
      kind: "marquee",
      start: [u, v],
      current: [u, v],
      startPx: ev.clickPx,
      currentPx: ev.clickPx,
    } satisfies SelectDrag);
    ctx.requestRedraw();
  },
  onMove(ctx, ev) {
    const drag = ctx.dragOf<SelectDrag>(selectTool);
    if (!drag) {
      // Select mode without a drag: highlight the entity under the cursor
      // so the user sees what a click will pick (ticket zadt).
      // SketchSession3D draws the published id amber, mirroring the orange
      // click-selection.
      const tolMm = pickTolMm(ev.cam);
      const hit = hitTestSketchEntity(
        buildDraftSketchState(ctx.session.subCalls),
        ev.raw,
        tolMm,
      );
      setHoveredSketchEntity(hit);
      return true;
    }
    if (drag.kind === "grip") {
      // P0.3: the controller solves every frame (continuation model), so
      // constraints stay satisfied as the grip moves — the geometry the
      // user sees is already solved. Ctrl = grid snap (ev.gridded).
      void ctx.gripDrag.update(ev.gridded);
      ctx.requestRedraw();
      return true;
    }
    if (drag.kind === "translate") {
      // Ticket matf: track the cursor; the translate lands on release so
      // the solver runs once. The `moved` flag separates a real drag from a
      // plain select-click (which falls back to a toggle).
      const [u, v] = ev.raw;
      const movedPx =
        Math.hypot(u - drag.start[0], v - drag.start[1]) *
        Math.max(ev.cam.zoom, 1e-6);
      const nowMoved = drag.moved || movedPx > 4;
      // On the FIRST frame that becomes a real drag, set the selection to
      // the grabbed group so it highlights while moving (deferred from arm
      // time so a no-drag click keeps its additive toggle).
      if (nowMoved && !drag.moved) {
        ctx.store.setSketchSelection(drag.selectIds);
      }
      ctx.setDrag(selectTool, { ...drag, current: [u, v], moved: nowMoved });
      ctx.requestRedraw();
      return true;
    }
    // F4: marquee — track both the sketch-mm corner and the client-pixel
    // corner, then redraw immediately (the box is screen-space).
    ctx.setDrag(selectTool, {
      ...drag,
      current: ev.raw,
      currentPx: ev.clickPx,
    });
    ctx.requestRedraw();
    ctx.redrawNow();
    return true;
  },
  onUp(ctx, ev) {
    const drag = ctx.dragOf<SelectDrag>(selectTool);
    if (!drag) return;
    ctx.releasePointer(ev);
    ctx.clearDrag();
    ctx.requestRedraw();
    if (drag.kind === "grip") {
      // Settle the drag: one authoritative solve at the final cursor
      // (coalescing may have dropped the last live frame), then commit.
      void ctx.gripDrag.finish();
      return;
    }
    if (drag.kind === "translate") {
      // Ticket matf: a real drag moves the grabbed entity / group by
      // (current − start); a release without a real drag was just a
      // select-click — fall back to toggling the entity, so a plain click
      // still selects exactly one.
      if (drag.moved) {
        ctx.store.translateSketchEntities(
          drag.entityIds,
          drag.current[0] - drag.start[0],
          drag.current[1] - drag.start[1],
        );
      } else if (drag.entityIds.length === 1) {
        // A plain click without a drag keeps the historic additive toggle
        // (click entities one by one to build a multi-selection). The drag
        // arming did NOT touch the selection, so a straight toggle is right.
        ctx.store.toggleSketchSelection(drag.entityIds[0]);
      }
      return;
    }
    // F4 marquee drop: a gesture under a few pixels is a plain click → fall
    // back to the existing clear-selection behaviour.
    const movedPx = Math.hypot(
      drag.currentPx[0] - drag.startPx[0],
      drag.currentPx[1] - drag.startPx[1],
    );
    if (movedPx < 4) {
      ctx.store.clearSketchSelection();
      return;
    }
    // Drag direction picks the mode: left→right (startU ≤ endU) is an
    // enclosing "window" select; right→left is a crossing select.
    const mode = drag.start[0] <= drag.current[0] ? "enclosing" : "crossing";
    const min: Pt = [
      Math.min(drag.start[0], drag.current[0]),
      Math.min(drag.start[1], drag.current[1]),
    ];
    const max: Pt = [
      Math.max(drag.start[0], drag.current[0]),
      Math.max(drag.start[1], drag.current[1]),
    ];
    const state = buildDraftSketchState(ctx.session.subCalls);
    ctx.store.setSketchSelection(entitiesInBox(state, min, max, mode));
  },
  hud(drag) {
    const d = drag as SelectDrag;
    const hud: ToolHud = {};
    if (d.kind === "marquee") {
      hud.drawScreen = (ctx2d, cam) =>
        drawMarquee(ctx2d, d.start, d.current, cam);
    }
    return hud;
  },
};
