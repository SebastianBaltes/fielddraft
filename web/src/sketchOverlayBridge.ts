/**
 * Drives the 2D sketch overlay's redraw FROM the R3F render frame so it
 * stays in lockstep with the WebGPU render.
 *
 * The overlay (`SketchCanvas2d`) draws the transient HUD that is not (yet)
 * 3D scene geometry — hover/snap markers, selection highlight + grips,
 * drag/rubber-band previews, the marquee and the dimension/constraint text.
 * Its own `requestAnimationFrame` loop gated the redraw on
 * `sketchCameraBridge.version`, which is bumped by `CameraBridgePublisher`
 * INSIDE the R3F frame — so whenever the overlay's rAF tick ran before that
 * frame, it skipped the redraw and painted the previous camera pose, a frame
 * behind the WebGPU canvas ("ruckelt nach").
 *
 * `CameraBridgePublisher` now calls {@link requestOverlayRedraw} every frame
 * right after publishing the camera; the overlay redraws with the current
 * camera in the SAME browser frame as the WebGPU render, so the two canvases
 * composite together and nothing on the overlay lags during camera motion.
 */

let redrawFn: (() => void) | null = null;

/** Register (or clear with `null`) the active sketch overlay's redraw. */
export function setOverlayRedraw(fn: (() => void) | null): void {
  redrawFn = fn;
}

/** Redraw the sketch overlay now, if one is mounted. No-op otherwise. */
export function requestOverlayRedraw(): void {
  redrawFn?.();
}
