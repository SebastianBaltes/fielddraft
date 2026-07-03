import * as THREE from "three";

/**
 * Is a projected NDC z inside the visible depth range of THIS camera?
 *
 * The valid range depends on which clip-space convention the renderer built
 * the camera's projection matrix for: WebGPU maps near→0 / far→1, WebGL maps
 * near→-1 / far→1 (`camera.coordinateSystem`, set by the renderer that drives
 * the camera). Hard-coding the WebGL range under a WebGPU camera lets points
 * BEHIND the camera pass the check — in ortho (the default projection) a
 * behind-camera point projects to a plain negative z that `z < -1` never
 * rejects. Same clip-space bug class as ticket yl0p; use this helper for
 * every `project()`-then-cull instead of literal bounds.
 */
export function ndcZVisible(camera: THREE.Camera, z: number): boolean {
  const min =
    camera.coordinateSystem === THREE.WebGPUCoordinateSystem ? 0 : -1;
  return z >= min && z <= 1;
}
