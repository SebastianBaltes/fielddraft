/**
 * Corner-op controllers (architecture review, Befund 2): Fillet and Chamfer.
 * Tickets tqi4/chfp — a single CLICK on a corner rounds/bevels it: the
 * corner under the cursor resolves to the two line edges meeting there
 * (exploding a Polygon / N-gon / Rectangle into lines first if needed) and
 * applies with the current radius / setback distance. A generous corner
 * tolerance (vertices are small targets). 1:1 translations of the old
 * `SketchCanvas2d` branches.
 */

import { pickTolMm } from "./toolTypes";
import type { SketchToolController } from "./toolTypes";

export const filletTool: SketchToolController = {
  onDown(ctx, ev) {
    const tolMm = pickTolMm(ev.cam, 2.5);
    ctx.store.filletCornerAt(
      ev.raw,
      ctx.store.getState().sketchToolParams.filletRadius,
      tolMm,
    );
    ctx.requestRedraw();
  },
};

export const chamferTool: SketchToolController = {
  onDown(ctx, ev) {
    const tolMm = pickTolMm(ev.cam, 2.5);
    ctx.store.chamferCornerAt(
      ev.raw,
      ctx.store.getState().sketchToolParams.chamferDistance,
      tolMm,
    );
    ctx.requestRedraw();
  },
};
