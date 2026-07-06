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
 * Ticket pxff: the pose is also serialised into the project file (`Project.view`)
 * so re-opening a project restores the exact viewpoint — `requestCameraRestore`
 * seeds this bridge from a loaded project and notifies `CameraPoseSync` to apply
 * it to the live camera.
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
const restoreListeners = new Set<() => void>();

/** Seed the bridge from a loaded project's saved view and ask the live
 *  `CameraPoseSync` to apply it now (ticket pxff). No-op for `null`. */
export function requestCameraRestore(next: CameraPose | null): void {
  if (!next) return;
  pose = next;
  for (const l of restoreListeners) l();
}

/** Subscribe to camera-restore requests (project load). */
export function subscribeCameraRestore(listener: () => void): () => void {
  restoreListeners.add(listener);
  return () => {
    restoreListeners.delete(listener);
  };
}

/** Validate an untrusted object as a `CameraPose` (loading a project file). */
export function parseCameraPose(raw: unknown): CameraPose | null {
  if (typeof raw !== "object" || raw === null) return null;
  const p = raw as Record<string, unknown>;
  const num3 = (v: unknown): v is [number, number, number] =>
    Array.isArray(v) && v.length === 3 && v.every((n) => typeof n === "number" && Number.isFinite(n));
  const num4 = (v: unknown): v is [number, number, number, number] =>
    Array.isArray(v) && v.length === 4 && v.every((n) => typeof n === "number" && Number.isFinite(n));
  if (!num3(p.position) || !num4(p.quaternion) || !num3(p.up) || !num3(p.target)) return null;
  if (typeof p.zoom !== "number" || !Number.isFinite(p.zoom)) return null;
  return {
    position: p.position,
    quaternion: p.quaternion,
    up: p.up,
    zoom: p.zoom,
    target: p.target,
  };
}

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
