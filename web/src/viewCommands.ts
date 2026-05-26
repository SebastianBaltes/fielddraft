/**
 * Module-level signal bridge between the toolbar (outside the
 * Canvas) and the camera rig (inside). The toolbar writes a
 * view preset name; a small R3F component (`ViewCommandConsumer`)
 * reads it per frame, snaps the camera, and clears the signal.
 */

export type ViewPreset = "front" | "back" | "top" | "bottom" | "right" | "left" | "iso" | null;

const state: { pending: ViewPreset; invalidate: (() => void) | null } = {
  pending: null,
  invalidate: null,
};

export function registerInvalidate(fn: () => void): void {
  state.invalidate = fn;
}

export function invalidateCanvas(): void {
  state.invalidate?.();
}

export function requestView(preset: ViewPreset): void {
  state.pending = preset;
  state.invalidate?.();
}

export function peekViewCommand(): ViewPreset {
  return state.pending;
}

export function consumeViewCommand(): ViewPreset {
  const cmd = state.pending;
  state.pending = null;
  return cmd;
}

/** Camera parameters for each standard view. `up` is the
 *  camera's up vector — Three.js OrbitControls need it to
 *  avoid gimbal-locking when the view axis is parallel to
 *  the world Y axis. */
export const VIEW_PRESETS: Record<
  Exclude<ViewPreset, null>,
  { position: [number, number, number]; up: [number, number, number] }
> = {
  front:  { position: [0, 0, 500],     up: [0, 1, 0] },
  back:   { position: [0, 0, -500],    up: [0, 1, 0] },
  top:    { position: [0, 500, 0],     up: [0, 0, -1] },
  bottom: { position: [0, -500, 0],    up: [0, 0, 1] },
  right:  { position: [500, 0, 0],     up: [0, 1, 0] },
  left:   { position: [-500, 0, 0],    up: [0, 1, 0] },
  iso:    { position: [350, 280, 350], up: [0, 1, 0] },
};
