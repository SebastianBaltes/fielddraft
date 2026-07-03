/**
 * Text-tool controller (architecture review, Befund 2 / ticket pqoe): not a
 * drawing tool — a click opens the in-scene input field + param popover at
 * the click point so the user types / tweaks the text right where it lands.
 * Enter / blur commits the placement via store.placeSketchText with the
 * baseline-left at this sketch-plane location (see the canvas JSX and
 * sketchText.ts). A second click while the editor is open commits the
 * current text first, then re-opens at the new location (Onshape-style).
 */

import type { SketchToolController } from "./toolTypes";

export const textTool: SketchToolController = {
  onDown(ctx, ev) {
    if (ctx.ui.textEditorOpen()) {
      ctx.ui.commitTextEditor();
    }
    // Ticket lycy: every fresh create-mode open starts with an EMPTY
    // value — the persisted store value must not leak the previously
    // typed text into a new drop (clicking elsewhere would otherwise
    // re-place the last string). Edit mode (openTextEditorEdit) still
    // seeds the clicked text's value, unaffected.
    ctx.store.setSketchTextValue("");
    ctx.ui.openTextEditorCreate(ev.clickPx, ev.raw);
    ctx.requestRedraw();
  },
};
