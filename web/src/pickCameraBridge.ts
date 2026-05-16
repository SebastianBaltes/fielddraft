/**
 * Pick-camera bridge.
 *
 * In `frameloop="demand"` mode, the `PickProbe` component's
 * `useFrame` callback only runs when R3F actually draws a frame
 * — which is *after* the click event has already fired. That
 * leaves `pickResult` (React state) holding stale data from the
 * last hover at click time, and the viewport's click handler
 * either misses the selection entirely or selects whichever
 * body the last hover happened to land on.
 *
 * The fix: do the raycast synchronously inside the click handler
 * using the kernel's `pick` API. To do that outside the Canvas
 * subtree we need access to the current perspective camera,
 * which this module-level singleton carries. A small
 * `CameraBridgePublisher` component inside the Canvas writes
 * the camera ref here on every frame; the click handler reads
 * it when it needs to fire a fresh pick.
 *
 * This bridge is separate from `sketchCameraBridge` because it
 * carries the full perspective camera (not just a 2D zoom +
 * centre), and runs even while the sketch session is inactive.
 */

import type * as THREE from "three";

let publishedCamera: THREE.Camera | null = null;

export function publishPickCamera(camera: THREE.Camera | null): void {
  publishedCamera = camera;
}

export function getPublishedPickCamera(): THREE.Camera | null {
  return publishedCamera;
}
