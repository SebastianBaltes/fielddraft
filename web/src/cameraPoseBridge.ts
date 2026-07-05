/**
 * Camera-pose bridge.
 *
 * Switching the render shader (sdf-vm / sdf-deferred / mesh / brep ⇄ webgl)
 * crosses the WebGPU⇄WebGL boundary, which mounts a *different* `<Canvas>` with
 * a fresh camera — resetting the view to the default `[350, 280, 350]`. This
 * module-scoped singleton carries the last camera pose across that remount:
 * `CameraPoseSync` (viewportCamera.tsx) keeps it current every rendered frame
 * and restores it once on the next canvas's mount, so the shader switch keeps
 * the exact viewpoint.
 *
 * In-memory only (per session) — a page reload starts from the default view.
 */

import * as THREE from "three";

export interface CameraPose {
  position: [number, number, number];
  quaternion: [number, number, number, number];
  up: [number, number, number];
  /** Orthographic zoom (1 for a perspective camera). */
  zoom: number;
  /** Orbit pivot (controls target). */
  target: [number, number, number];
}

let pose: CameraPose | null = null;

export function saveCameraPose(camera: THREE.Camera, target: THREE.Vector3): void {
  const proj = camera as THREE.OrthographicCamera & THREE.PerspectiveCamera;
  pose = {
    position: camera.position.toArray() as [number, number, number],
    quaternion: [
      camera.quaternion.x,
      camera.quaternion.y,
      camera.quaternion.z,
      camera.quaternion.w,
    ],
    up: camera.up.toArray() as [number, number, number],
    zoom: proj.zoom ?? 1,
    target: target.toArray() as [number, number, number],
  };
}

export function getCameraPose(): CameraPose | null {
  return pose;
}
