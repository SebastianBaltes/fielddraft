/**
 * Constraint-pick controller (architecture review, Befund 2 / zz1r): with a
 * constraint TOOL active (`sketch_constraint_*`, incl. the Dimension and
 * Coincident sentinels) committed-entity clicks become a running selection
 * that applies the constraint the moment it fits — the tool-first constrain
 * workflow. Same entity hit-test as select mode, but no grips / marquee /
 * in-scene editors (those are select-mode edits). A miss clears the running
 * selection. The hover highlight (7lxk) shows the nearest line as the pick
 * candidate. The canvas resolves this controller for every constraint tool
 * name — it is not in the name registry.
 */

import { buildDraftSketchState } from "../sketchSession";
import { hitTestSketchEntity } from "../sketchHitTest";
import { curveOfSubCall } from "../sketchTrim";
import { setHoveredSketchEntity } from "../sketchEntityHoverBridge";
import { pickTolMm } from "./toolTypes";
import { nearestLineSubCall } from "./pickHelpers";
import type { SketchToolController } from "./toolTypes";

export const constraintPickTool: SketchToolController = {
  onDown(ctx, ev) {
    // A click on an existing dimension's value label edits it (constraint),
    // rather than starting a new constraint pick. A click while the editor
    // is open just dismisses it.
    if (ctx.ui.dimEditorOpen()) {
      ctx.ui.dismissDimEditor();
      ctx.requestRedraw();
      return;
    }
    if (ctx.ui.tryOpenDimensionEditor(ev.clickPx, ev.cam)) {
      ctx.requestRedraw();
      return;
    }
    const tolMm = pickTolMm(ev.cam);
    const state = buildDraftSketchState(ctx.session.subCalls);
    const hit = hitTestSketchEntity(state, ev.raw, tolMm);
    if (hit === null) {
      ctx.store.clearSketchSelection();
    } else {
      ctx.store.pickSketchConstraintEntity(hit);
    }
    ctx.requestRedraw();
  },
  onHover(ctx, ev) {
    // Highlight the line under the cursor as a pick candidate while a
    // constraint tool — e.g. Dimension (7lxk) — is armed, so the user sees
    // what a click will target. (Point hover comes from the snap dot.)
    const tolMm = pickTolMm(ev.cam, 2.5);
    const idx = nearestLineSubCall(ctx.session.subCalls, ev.gridded, tolMm);
    const cv = idx !== null ? curveOfSubCall(ctx.session.subCalls[idx]) : null;
    ctx.setMirrorAxisHover(cv && cv.kind === "line" ? [cv.a, cv.b] : null);
    setHoveredSketchEntity(null);
  },
};
