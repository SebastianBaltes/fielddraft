/**
 * Render-camera bridge — the "pick what you see" invariant (ticket pzvt).
 *
 * During the minute-long SDF shader compiles (gyroid scenes, o6ia class) the
 * SDF pass early-returns and the VISIBLE frame stays frozen, while hover/pick
 * math keeps reading the LIVE useThree camera. Any camera change inside that
 * window (project-view restore, small orbits) desyncs interaction from the
 * pixels on screen: the 8px edge band misses first, large faces appear to
 * keep working — exactly the pzvt symptom.
 *
 * The SDF passes therefore publish the camera of every frame they actually
 * PRESENT; all screen-space picks (edge/vertex projections, face raycasts,
 * occlusion probes) project against this snapshot instead of the live
 * camera. While no frame is being presented, the snapshot stays at the last
 * drawn frame — picks keep matching the frozen pixels.
 *
 * In mesh/WebGL mode (no SDF pass mounted) nothing publishes and
 * {@link getPickCamera} falls back to the live camera, which IS the rendered
 * camera there. A module-scoped singleton, exactly like `brepPickBridge`.
 */

import * as THREE from "three";

let snap: THREE.Camera | null = null;

/** Publish the camera state of the frame just presented. Copies the matrices
 *  `.project()` / `Raycaster.setFromCamera()` read, so later live-camera
 *  mutations don't leak into the snapshot. */
export function publishRenderCamera(camera: THREE.Camera): void {
  if (!snap || snap.type !== camera.type) {
    snap = camera.clone();
    // The snapshot is fed matrices directly — nothing may recompute them.
    snap.matrixAutoUpdate = false;
    snap.matrixWorldAutoUpdate = false;
  }
  snap.matrixWorld.copy(camera.matrixWorld);
  snap.matrixWorldInverse.copy(camera.matrixWorldInverse);
  snap.projectionMatrix.copy(camera.projectionMatrix);
  snap.projectionMatrixInverse.copy(camera.projectionMatrixInverse);
  snap.position.setFromMatrixPosition(camera.matrixWorld);
  const src = camera as Partial<THREE.OrthographicCamera>;
  const dst = snap as Partial<THREE.OrthographicCamera>;
  if (typeof src.zoom === "number") dst.zoom = src.zoom;
  if (typeof src.near === "number") dst.near = src.near;
  if (typeof src.far === "number") dst.far = src.far;
}

/** Drop the snapshot (SDF pass unmount / render-mode switch) so a stale
 *  camera can't shadow the live one in other render modes. */
export function clearRenderCamera(): void {
  snap = null;
}

/** The camera all screen-space picks must project against: the last
 *  PRESENTED frame's camera when an SDF pass publishes, else the live one. */
export function getPickCamera(live: THREE.Camera): THREE.Camera {
  return snap ?? live;
}
