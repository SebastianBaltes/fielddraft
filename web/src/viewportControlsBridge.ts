/**
 * Camera-controls bridge.
 *
 * `CameraRig` lives INSIDE the R3F canvas (it can read `useThree`), while the
 * viewport's DOM pointer handlers live on the host `<div>` OUTSIDE it. The
 * host needs the live controls for exactly one thing: cancelling an in-flight
 * touch gesture when a pointer-down turns out to be a box-select rather than a
 * camera orbit (ticket 2ca6e21c). A module singleton — same pattern as the
 * other viewport bridges — keeps that out of React state.
 */

import type { OnshapeControls } from "./onshapeControls";

let controls: OnshapeControls | null = null;

/** Publish the live controls (CameraRig, on create) or clear them (`null`). */
export function setViewportControls(next: OnshapeControls | null): void {
  controls = next;
}

/** The live camera controls, or `null` before the canvas mounts. */
export function getViewportControls(): OnshapeControls | null {
  return controls;
}
