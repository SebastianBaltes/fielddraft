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

// ── Change detection (dirty tracking) ────────────────────────────────
// `saveCameraPose` runs every rendered frame; the project autosave must only
// be marked dirty when the pose ACTUALLY changed (an orbit/pan/zoom), not per
// frame. Listeners are registered by the store bootstrap.

const changeListeners = new Set<() => void>();

/** Subscribe to real camera-pose changes (fires once per changed frame,
 *  never for an identical pose). Used to mark the project dirty. */
export function subscribeCameraPoseChanged(listener: () => void): () => void {
  changeListeners.add(listener);
  return () => {
    changeListeners.delete(listener);
  };
}

function samePose(a: CameraPose, b: CameraPose): boolean {
  const eq3 = (x: number[], y: number[]) =>
    x[0] === y[0] && x[1] === y[1] && x[2] === y[2];
  return (
    eq3(a.position, b.position) &&
    a.quaternion[0] === b.quaternion[0] &&
    a.quaternion[1] === b.quaternion[1] &&
    a.quaternion[2] === b.quaternion[2] &&
    a.quaternion[3] === b.quaternion[3] &&
    eq3(a.up, b.up) &&
    a.zoom === b.zoom &&
    eq3(a.target, b.target)
  );
}

export function saveCameraPose(camera: THREE.Camera, target: THREE.Vector3): void {
  const proj = camera as THREE.OrthographicCamera & THREE.PerspectiveCamera;
  const next: CameraPose = {
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
  const changed = pose === null || !samePose(pose, next);
  pose = next;
  if (changed) for (const l of changeListeners) l();
}

export function getCameraPose(): CameraPose | null {
  return pose;
}

/**
 * Forget the current pose (ticket #422). Called when switching to a project
 * that has NO saved view: the module-scoped `pose` singleton otherwise still
 * holds the PREVIOUS project's viewpoint, which the persistence funnel
 * (`buildPersistableProject` reads `getCameraPose()`) would then autosave into
 * the freshly opened project. Clearing it makes the next save write no view;
 * `saveCameraPose` repopulates the singleton from the live camera on the next
 * rendered frame (after the default-view fit has run), so the project ends up
 * with ITS OWN framing, never the one it was opened from.
 */
export function clearCameraPose(): void {
  pose = null;
}
