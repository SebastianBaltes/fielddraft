/**
 * Mirror controller (architecture review, Befund 2 / F14 + tickets jq6i,
 * ps21, czqf, o6hx): the Onshape-style two-step flow. Step 1: the click
 * picks the mirror AXIS LINE (tool-state slot). Step 2: each click on a
 * part mirrors it LIVE across the axis (pick-to-apply), no Enter, no
 * selection collecting. Esc drops back to the pick-axis step, pressed again
 * it leaves Mirror for the default Select tool. The hover highlight shows
 * the axis candidate (step 1) or the entity to mirror (step 2).
 */

import { buildDraftSketchState } from "../sketchSession";
import { hitTestSketchEntity } from "../sketchHitTest";
import { curveOfSubCall } from "../sketchTrim";
import { setToolHint } from "../toolHintBridge";
import { setHoveredSketchEntity } from "../sketchEntityHoverBridge";
import { pickTolMm } from "./toolTypes";
import { nearestLineSubCall } from "./pickHelpers";
import type { SketchToolController } from "./toolTypes";

const HINT_PICK_AXIS = "Spiegeln: Spiegelachse anklicken.";
const HINT_PICK_PARTS = "Spiegeln: Teile anklicken zum Spiegeln · Esc beendet.";

/** The picked mirror-axis line's sub-call index (tool-state slot; `null` /
 *  cleared slot means "no axis yet" — step 1). */
type MirrorAxis = number;

export const mirrorTool: SketchToolController = {
  resolveSnapOnDown: true,
  onDown(ctx, ev) {
    // Ticket jq6i: Onshape-style two-step flow.
    // Step 1 (no axis yet): the click picks the mirror axis line.
    const axis = ctx.toolState<MirrorAxis>();
    if (axis === null) {
      const tolMm = pickTolMm(ev.cam, 2.5);
      const picked = nearestLineSubCall(ctx.session.subCalls, ev.gridded, tolMm);
      if (picked !== null) {
        ctx.setToolState(picked satisfies MirrorAxis);
        ctx.store.clearSketchSelection();
        setToolHint(HINT_PICK_PARTS);
      }
      ctx.requestRedraw();
      return;
    }
    // Step 2 (axis set): ticket ps21 — each click on a part mirrors it
    // LIVE across the axis (Onshape pick-to-apply), no Enter, no
    // selection collecting. Esc ends the tool (onKey below).
    const tolMm = pickTolMm(ev.cam);
    const state = buildDraftSketchState(ctx.session.subCalls);
    const hit = hitTestSketchEntity(state, ev.gridded, tolMm);
    if (hit !== null) ctx.store.mirrorSketchEntityAt(axis, hit);
    ctx.requestRedraw();
  },
  onKey(ctx, e) {
    // Ticket ps21: Esc ENDS the tool — with an axis set it drops back to
    // the pick-axis step; pressed again (no axis) it leaves Mirror for the
    // default Select tool. Switching to another toolbar button also ends it
    // (the canvas clears the tool-state slot on tool change).
    if (e.key !== "Escape") return;
    e.preventDefault();
    if (ctx.toolState<MirrorAxis>() !== null) {
      ctx.setToolState(null);
      ctx.store.clearSketchSelection();
      setHoveredSketchEntity(null); // o6hx: drop step-2 hover highlight
      setToolHint(HINT_PICK_AXIS);
    } else {
      ctx.store.selectSketchTool(null);
    }
  },
  onHover(ctx, ev) {
    // Ticket o6hx: the Mirror flow has two steps. STEP 1 (no axis yet) the
    // candidate is the AXIS LINE → highlight the nearest line (czqf). STEP 2
    // (axis set) the candidate is the OBJECT TO MIRROR (point / line /
    // polygon) → highlight the entity under the cursor (same hover as Select
    // mode), and drop the axis-line highlight.
    if (ctx.toolState<MirrorAxis>() !== null) {
      const tolMm = pickTolMm(ev.cam);
      const hovered = hitTestSketchEntity(
        buildDraftSketchState(ctx.session.subCalls),
        ev.gridded,
        tolMm,
      );
      setHoveredSketchEntity(hovered);
      ctx.setMirrorAxisHover(null);
      return;
    }
    const tolMm = pickTolMm(ev.cam, 2.5);
    const idx = nearestLineSubCall(ctx.session.subCalls, ev.gridded, tolMm);
    const cv = idx !== null ? curveOfSubCall(ctx.session.subCalls[idx]) : null;
    ctx.setMirrorAxisHover(cv && cv.kind === "line" ? [cv.a, cv.b] : null);
    setHoveredSketchEntity(null);
  },
  initialHint() {
    return HINT_PICK_AXIS;
  },
};
