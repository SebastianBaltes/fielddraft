/**
 * Module-level signal bridge between UI outside the Canvas (toolbar,
 * ViewCube, viewport context menu) and the camera rig inside it. A caller
 * requests a view command; a small R3F component (`ViewCommandConsumer`)
 * reads it per frame, moves the camera, and clears the signal.
 */

export type ViewPreset = "front" | "back" | "top" | "bottom" | "right" | "left" | "iso";

/** A camera move request. `preset` snaps to a standard view; `fit` frames
 *  the whole scene along the current view direction; `normal` looks straight
 *  at the active sketch plane; `normalToFace` looks straight at a selected
 *  planar face given its world-space normal + centroid. */
export type ViewCommand =
  | { kind: "preset"; preset: ViewPreset }
  | { kind: "fit" }
  | { kind: "normal" }
  | {
      kind: "normalToFace";
      normal: [number, number, number];
      centroid: [number, number, number];
    };

const state: { pending: ViewCommand | null; invalidate: (() => void) | null } = {
  pending: null,
  invalidate: null,
};

export function registerInvalidate(fn: () => void): void {
  state.invalidate = fn;
}

export function invalidateCanvas(): void {
  state.invalidate?.();
}

function request(cmd: ViewCommand): void {
  state.pending = cmd;
  state.invalidate?.();
}

export function requestView(preset: ViewPreset): void {
  request({ kind: "preset", preset });
}

/** Zoom/dolly the camera so the whole scene fits the viewport. */
export function requestViewFit(): void {
  request({ kind: "fit" });
}

/** Orient the camera to look straight down the active sketch plane's normal. */
export function requestViewNormal(): void {
  request({ kind: "normal" });
}

/** Orient the camera to look straight down a selected planar face's normal,
 *  framing the scene from straight on — the face analogue of
 *  {@link requestViewNormal} (ticket z0vk). `normal`/`centroid` are world space. */
export function requestViewNormalToFace(
  normal: [number, number, number],
  centroid: [number, number, number],
): void {
  request({ kind: "normalToFace", normal, centroid });
}

export function peekViewCommand(): ViewCommand | null {
  return state.pending;
}

export function consumeViewCommand(): ViewCommand | null {
  const cmd = state.pending;
  state.pending = null;
  return cmd;
}

/** Camera parameters for each standard view. `up` is the
 *  camera's up vector — Three.js OrbitControls need it to
 *  avoid gimbal-locking when the view axis is parallel to
 *  the world Y axis. */
export const VIEW_PRESETS: Record<
  ViewPreset,
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
