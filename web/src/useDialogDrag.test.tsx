import { describe, expect, it } from "vitest";
import { act, type PointerEvent as ReactPointerEvent } from "react";
import { createRoot } from "react-dom/client";
import { useDialogDrag, type DialogDragHandlers } from "./useDialogDrag";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;

/** Renders the hook via a throwaway component and hands the latest handlers
 *  back through `onReady` (called on every render). */
function Harness({ onReady }: { onReady: (h: DialogDragHandlers) => void }) {
  onReady(useDialogDrag());
  return null;
}

/** `ctx.handlers` must be re-read after every `act()` — destructuring it once
 *  up front would freeze on the pre-drag snapshot instead of tracking
 *  `positionStyle` across the re-renders that a drag triggers. */
function setup() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const ctx: { handlers: DialogDragHandlers } = { handlers: undefined as unknown as DialogDragHandlers };
  act(() => {
    root.render(<Harness onReady={(h) => { ctx.handlers = h; }} />);
  });
  return ctx;
}

/** A fake "dialog root > header" DOM pair — jsdom implements neither
 *  `getBoundingClientRect` nor pointer capture, so both are stubbed. */
function makeDialogHeader(rect: { left: number; top: number }) {
  const dialog = document.createElement("div");
  dialog.getBoundingClientRect = () => ({
    left: rect.left,
    top: rect.top,
    right: rect.left + 200,
    bottom: rect.top + 100,
    width: 200,
    height: 100,
    x: rect.left,
    y: rect.top,
    toJSON() {},
  });
  const header = document.createElement("div");
  header.setPointerCapture = () => {};
  header.releasePointerCapture = () => {};
  dialog.appendChild(header);
  return header;
}

describe("useDialogDrag", () => {
  it("has no position override before the first drag", () => {
    const ctx = setup();
    expect(ctx.handlers.positionStyle).toBeUndefined();
  });

  it("moves the position by the pointer delta on drag", () => {
    const ctx = setup();
    const header = makeDialogHeader({ left: 100, top: 50 });

    act(() => {
      ctx.handlers.onHeaderDown({
        target: header,
        currentTarget: header,
        clientX: 120,
        clientY: 70,
        pointerId: 1,
      } as unknown as ReactPointerEvent);
    });
    // Pointer-down grabs the dialog at its current rect.
    expect(ctx.handlers.positionStyle).toMatchObject({ left: 100, top: 50 });

    act(() => {
      ctx.handlers.onHeaderMove({
        target: header,
        currentTarget: header,
        clientX: 150,
        clientY: 90,
        pointerId: 1,
      } as unknown as ReactPointerEvent);
    });
    // Pointer moved by (+30, +20) — the dialog must follow by the same delta.
    expect(ctx.handlers.positionStyle).toMatchObject({ left: 130, top: 70 });
  });

  it("stops tracking after pointer-up", () => {
    const ctx = setup();
    const header = makeDialogHeader({ left: 100, top: 50 });

    act(() => {
      ctx.handlers.onHeaderDown({
        target: header,
        currentTarget: header,
        clientX: 120,
        clientY: 70,
        pointerId: 1,
      } as unknown as ReactPointerEvent);
    });
    act(() => {
      ctx.handlers.onHeaderUp({
        target: header,
        currentTarget: header,
        pointerId: 1,
      } as unknown as ReactPointerEvent);
    });
    act(() => {
      ctx.handlers.onHeaderMove({
        target: header,
        currentTarget: header,
        clientX: 500,
        clientY: 500,
        pointerId: 1,
      } as unknown as ReactPointerEvent);
    });
    // A move after release must not budge the dialog any further.
    expect(ctx.handlers.positionStyle).toMatchObject({ left: 100, top: 50 });
  });
});
