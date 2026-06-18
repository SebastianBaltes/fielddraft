import { useEffect, useRef, type ReactElement } from "react";

export interface ContextMenuItem {
  label: string;
  onSelect: () => void;
  danger?: boolean;
  testId?: string;
}

interface ContextMenuProps {
  /** Cursor position in client (viewport) pixels. */
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
  testId?: string;
}

/**
 * Fixed-position right-click popover. The owner renders it at a cursor
 * position and clears it via `onClose`; the menu itself handles dismissal
 * on outside-click, Escape and scroll/wheel. Styling lives in
 * `styles/components/contextMenu.css` (shared `.fd-context-menu`).
 */
export function ContextMenu({
  x,
  y,
  items,
  onClose,
  testId,
}: ContextMenuProps): ReactElement {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("wheel", onClose, { passive: true });
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("wheel", onClose);
    };
  }, [onClose]);

  // Keep the menu on-screen: clamp against the viewport using a rough
  // per-item height so the last item never spills below the fold.
  const clampedX = Math.min(x, window.innerWidth - 220);
  const clampedY = Math.min(y, window.innerHeight - (items.length * 40 + 16));

  return (
    <div
      ref={ref}
      className="fd-context-menu"
      style={{ left: `${clampedX}px`, top: `${clampedY}px` }}
      data-testid={testId}
    >
      {items.map((item, i) => (
        <button
          key={i}
          type="button"
          className={"menu-item" + (item.danger ? " is-danger" : "")}
          data-testid={item.testId}
          onClick={() => {
            item.onSelect();
            onClose();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
