/**
 * Sketch-canvas event plumbing to the WebGPU viewport — extracted verbatim
 * from `SketchCanvas2d.tsx` (architecture review, Befund 2):
 *
 *  - forwarding camera gestures (non-left pointer buttons, wheel zoom) to
 *    the WebGPU `<canvas>` sibling so OrbitControls keeps working while the
 *    overlay owns left-click sketching, and
 *  - the two-finger touch pinch-to-zoom with its lockout, layered ON TOP of
 *    the single-finger sketch interaction.
 */

/**
 * Find the WebGPU `<canvas>` sibling of the overlay so camera
 * gestures (wheel zoom, right-drag pan) can be forwarded to the
 * R3F OrbitControls underneath while the overlay owns left-click
 * selection in select mode.
 */
export function viewportCanvas(
  overlay: HTMLCanvasElement,
): HTMLCanvasElement | null {
  const parent = overlay.parentElement;
  if (!parent) return null;
  for (const c of parent.querySelectorAll("canvas")) {
    if (c !== overlay) return c as HTMLCanvasElement;
  }
  return null;
}

/** Re-dispatch a pointer event onto the WebGPU canvas so the
 *  OrbitControls pan gesture (right / middle drag) works while the
 *  overlay is capturing events in select mode. OrbitControls'
 *  pointerdown handler takes a pointer-capture on its own element,
 *  so forwarding just the initial down is enough for the drag to
 *  continue there. */
export function forwardPointerToViewport(
  e: React.PointerEvent<HTMLCanvasElement>,
): void {
  const target = viewportCanvas(e.currentTarget as HTMLCanvasElement);
  if (!target) return;
  target.dispatchEvent(
    new PointerEvent(e.nativeEvent.type, {
      pointerId: e.pointerId,
      pointerType: e.pointerType,
      button: e.button,
      buttons: e.buttons,
      clientX: e.clientX,
      clientY: e.clientY,
      bubbles: true,
      cancelable: true,
    }),
  );
}

/**
 * Touch pinch sensitivity: how many wheel `deltaY` units one CSS pixel of
 * finger-spread synthesises. Spreading the fingers zooms in (negative
 * deltaY by browser convention); the gain just keeps the dolly pacing the
 * gesture. Tuned to feel like the mouse wheel, not a hard requirement.
 */
const PINCH_ZOOM_GAIN = 2.5;

/** Euclidean distance between the first two tracked touch points (CSS px). */
function touchSpan(pts: Map<number, { x: number; y: number }>): {
  dist: number;
  midX: number;
  midY: number;
} {
  const it = pts.values();
  const a = it.next().value;
  const b = it.next().value;
  if (!a || !b) return { dist: 0, midX: 0, midY: 0 };
  return {
    dist: Math.hypot(a.x - b.x, a.y - b.y),
    midX: (a.x + b.x) / 2,
    midY: (a.y + b.y) / 2,
  };
}

/** Synthesize a wheel event on the WebGPU canvas so the existing
 *  OrbitControls dolly path handles the zoom — same route the mouse
 *  wheel already takes. Used by the two-finger pinch gesture. */
export function dispatchZoomWheel(
  overlay: HTMLCanvasElement,
  clientX: number,
  clientY: number,
  deltaY: number,
): void {
  const target = viewportCanvas(overlay);
  if (!target) return;
  target.dispatchEvent(
    new WheelEvent("wheel", {
      deltaY,
      deltaMode: 0,
      clientX,
      clientY,
      bubbles: true,
      cancelable: true,
    }),
  );
}

/**
 * Touch pinch-to-zoom — a two-finger gesture layered ON TOP of the
 * single-finger sketch interaction. We track live touch pointers; once a
 * second finger lands we abort any in-progress single-finger draft and
 * hold a lockout (`active`) until EVERY finger lifts, so the leftover
 * finger can't resume a stray sketch action mid-gesture. Zoom is driven by
 * synthesising wheel events onto the WebGPU canvas, reusing the
 * OrbitControls dolly path the mouse wheel already uses. Mouse pointers
 * never enter any of this; single-finger sketching is unchanged. Two-finger
 * PAN is a deliberate follow-up (see ARCHITECTURE.md).
 *
 * Each `on*` handler returns `true` when it consumed the event (the pinch
 * owns the gesture) — the caller must then return without any further
 * sketch handling, mirroring the original inline early-returns.
 */
export class SketchTouchPinch {
  private pts = new Map<number, { x: number; y: number }>();
  private pinch = { active: false, lastDist: 0 };

  /** Touch pointerdown: track the pointer; a second finger turns the
   *  gesture into a camera pinch. `abortDraft` is called once when the
   *  pinch takes over so the canvas can drop an in-progress single-finger
   *  draft; pointer captures on all tracked touches are released. */
  onPointerDown(
    e: React.PointerEvent<HTMLCanvasElement>,
    abortDraft: () => void,
  ): boolean {
    if (e.pointerType !== "touch") return false;
    this.pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (this.pts.size < 2) return false;
    abortDraft();
    const canvas = e.currentTarget as HTMLCanvasElement;
    for (const id of this.pts.keys()) {
      try {
        canvas.releasePointerCapture(id);
      } catch {
        /* this pointer wasn't captured — fine */
      }
    }
    this.pinch = { active: true, lastDist: touchSpan(this.pts).dist };
    e.preventDefault();
    return true;
  }

  /** Pinch gesture owns the move while it's live. With both fingers down we
   *  dolly by the change in finger spread; with one finger left (mid-lift)
   *  we stay inert until it too lifts, so no stray sketch action leaks out. */
  onPointerMove(e: React.PointerEvent<HTMLCanvasElement>): boolean {
    if (!this.pinch.active) return false;
    if (e.pointerType === "touch" && this.pts.has(e.pointerId)) {
      this.pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
    }
    if (this.pts.size >= 2) {
      const span = touchSpan(this.pts);
      const delta = span.dist - this.pinch.lastDist;
      if (delta !== 0) {
        dispatchZoomWheel(
          e.currentTarget as HTMLCanvasElement,
          span.midX,
          span.midY,
          -delta * PINCH_ZOOM_GAIN,
        );
        this.pinch.lastDist = span.dist;
      }
    }
    e.preventDefault();
    return true;
  }

  /** Touch pointerup: consume every touch-up for the duration of the pinch
   *  lockout; the gesture only fully ends (and sketching resumes) once all
   *  fingers are off the glass. */
  onPointerUp(e: React.PointerEvent<HTMLCanvasElement>): boolean {
    if (e.pointerType !== "touch") return false;
    this.pts.delete(e.pointerId);
    if (!this.pinch.active) return false;
    if (this.pts.size === 0) {
      this.pinch = { active: false, lastDist: 0 };
    }
    e.preventDefault();
    return true;
  }

  /** Touch pointercancel bookkeeping (the caller still resets its own
   *  drag / hover state afterwards, pinch or no pinch). */
  onPointerCancel(e: React.PointerEvent<HTMLCanvasElement>): void {
    if (e.pointerType !== "touch") return;
    this.pts.delete(e.pointerId);
    if (this.pts.size === 0) {
      this.pinch = { active: false, lastDist: 0 };
    }
  }
}
