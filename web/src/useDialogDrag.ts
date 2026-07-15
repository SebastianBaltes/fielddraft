/**
 * Drag-to-reposition for a non-modal dialog header (#368).
 *
 * Pointer-down on the header (except on a button, so the close button still
 * works) grabs the dialog at its current on-screen rect; pointer-move
 * repositions it clamped so at least a corner stays reachable; pointer-up
 * releases capture. Shared by every draggable dialog (Settings, Voxel,
 * Mesher, Mesher PASM, Mesher: Lichen, SDF-Plane) — previously duplicated
 * verbatim in each of them.
 *
 * Usage: spread `positionStyle` onto the dialog root's `style` prop (falsy
 * until the first drag, so the dialog keeps its CSS default position/
 * centering until the user moves it) and wire the three handlers onto the
 * header element.
 */
import { useCallback, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

export interface DialogDragHandlers {
  /** `transform: "none"` clears any CSS centering transform (e.g. a dialog
   *  centered by default via `top/left: 50%; transform: translate(-50%,
   *  -50%)`) once the user has dragged it to an explicit pixel position;
   *  dialogs that don't rely on a transform for their default position are
   *  unaffected (their default transform is already `none`). */
  positionStyle: { left: number; top: number; right: "auto"; transform: "none" } | undefined;
  onHeaderDown: (e: ReactPointerEvent) => void;
  onHeaderMove: (e: ReactPointerEvent) => void;
  onHeaderUp: (e: ReactPointerEvent) => void;
}

export function useDialogDrag(): DialogDragHandlers {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const dragRef = useRef<{ dx: number; dy: number } | null>(null);

  const onHeaderDown = useCallback((e: ReactPointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    const dialog = e.currentTarget.parentElement as HTMLElement | null;
    if (!dialog) return;
    const r = dialog.getBoundingClientRect();
    dragRef.current = { dx: e.clientX - r.left, dy: e.clientY - r.top };
    setPos({ x: r.left, y: r.top });
    e.currentTarget.setPointerCapture(e.pointerId);
  }, []);

  const onHeaderMove = useCallback((e: ReactPointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    setPos({
      x: Math.max(0, Math.min(window.innerWidth - 80, e.clientX - d.dx)),
      y: Math.max(0, Math.min(window.innerHeight - 40, e.clientY - d.dy)),
    });
  }, []);

  const onHeaderUp = useCallback((e: ReactPointerEvent) => {
    dragRef.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
  }, []);

  return {
    positionStyle: pos
      ? { left: pos.x, top: pos.y, right: "auto", transform: "none" }
      : undefined,
    onHeaderDown,
    onHeaderMove,
    onHeaderUp,
  };
}
