/**
 * Bridge between the live 3D camera and the sketch overlay (tickets
 * yx6a/bxoo, Phase 3 of docs/SKETCH-IN-3D-PLAN.md).
 *
 * The sketch session no longer owns a separate top-down camera: the
 * normal viewport camera stays active, and the overlay maps sketch
 * (u, v) coordinates through the resolved sketch-plane frame into
 * world space and projects them with the REAL camera
 * (`pickCameraBridge` carries the camera reference). Input goes the
 * other way: a screen point becomes a camera ray intersected with
 * the sketch plane.
 *
 * `zoom` is the EFFECTIVE px-per-mm on the sketch plane measured at
 * the frame origin — consumers keep using it for pick tolerances,
 * grid spacing and marker radii exactly as before. Under a
 * perspective camera it varies across the plane; treating it as
 * constant is an accepted approximation for tolerances.
 *
 * A small R3F component inside the Canvas (`CameraBridgePublisher`)
 * calls `publish` on every frame; the overlay reads from the same
 * object in its own `requestAnimationFrame` loop. Avoids plumbing
 * camera state through React props, which would miss frames.
 */

import * as THREE from "three";
import { getPublishedPickCamera } from "./pickCameraBridge";
import { planeToWorld, type PlaneFrame } from "./sketchPlaneFrame";

export interface SketchCameraState {
  /** Viewport size in CSS pixels. */
  width: number;
  height: number;
  /** Effective px-per-mm on the sketch plane at the frame origin. */
  zoom: number;
  /** Device pixel ratio at publish time — the overlay canvas
   *  needs this for its own backing-store size. */
  devicePixelRatio: number;
  /** Monotonic counter bumped on every `publish` call. Lets
   *  the overlay's RAF loop detect "nothing moved" and skip a
   *  redraw without doing deep equality on the numerics. */
  version: number;
}

const XY_FRAME: PlaneFrame = {
  origin: [0, 0, 0],
  xAxis: [1, 0, 0],
  yAxis: [0, 1, 0],
  normal: [0, 0, 1],
};

/** The active session's resolved plane frame. Defaults to the XY
 *  datum so the bridge stays usable before the first resolve. */
let activeFrame: PlaneFrame = XY_FRAME;

export function setSketchPlaneFrame(frame: PlaneFrame | null): void {
  activeFrame = frame ?? XY_FRAME;
  bridge.version += 1;
}

export function getSketchPlaneFrame(): Readonly<PlaneFrame> {
  return activeFrame;
}

const bridge: SketchCameraState = {
  width: 0,
  height: 0,
  zoom: 1,
  devicePixelRatio: 1,
  version: 0,
};

const projV = new THREE.Vector3();

/** px-per-mm on the plane at the frame origin under the current
 *  camera — the screen distance of one xAxis millimetre. */
function computeZoom(width: number, height: number): number {
  const camera = getPublishedPickCamera();
  if (!camera || width <= 0 || height <= 0) return bridge.zoom;
  const o = activeFrame.origin;
  const x = activeFrame.xAxis;
  projV.set(o[0], o[1], o[2]).project(camera);
  const ox = (projV.x * 0.5 + 0.5) * width;
  const oy = (0.5 - projV.y * 0.5) * height;
  projV.set(o[0] + x[0], o[1] + x[1], o[2] + x[2]).project(camera);
  const px = (projV.x * 0.5 + 0.5) * width;
  const py = (0.5 - projV.y * 0.5) * height;
  const z = Math.hypot(px - ox, py - oy);
  return Number.isFinite(z) && z > 1e-9 ? z : bridge.zoom;
}

export function publishSketchCamera(next: {
  width: number;
  height: number;
  devicePixelRatio: number;
}): void {
  bridge.width = next.width;
  bridge.height = next.height;
  bridge.devicePixelRatio = next.devicePixelRatio;
  bridge.zoom = computeZoom(next.width, next.height);
  bridge.version += 1;
}

export function readSketchCamera(): Readonly<SketchCameraState> {
  return bridge;
}

// Dev-mode window hooks so Playwright tests can peek the current camera
// state and, crucially, project a sketch (u, v) to overlay pixels through
// the SAME real-camera + plane-frame path the overlay hit-test uses.
// (Reimplementing the projection test-side drifts: the old top-down
// centerU/centerV model is long gone, so a hand-rolled ortho formula
// yields NaN clicks.) The projector hook takes optional width/height so a
// caller that measured its own canvas box can override the bridge's
// possibly-stale dims — see the note on `sketchToScreen`.
if (typeof window !== "undefined" && import.meta.env?.DEV) {
  const w = window as unknown as {
    __fdSketchCamera?: () => Readonly<SketchCameraState>;
    __fdSketchToScreen?: (
      u: number,
      v: number,
      width?: number,
      height?: number,
    ) => [number, number];
  };
  w.__fdSketchCamera = () => bridge;
  w.__fdSketchToScreen = (u, v, width, height) =>
    sketchToScreen(
      u,
      v,
      width && height ? { ...bridge, width, height } : bridge,
    );
}

/**
 * Project a sketch-plane (u, v) coordinate to CSS-pixel coordinates of
 * the overlay canvas: lift it into world space through the active plane
 * frame, project with the live camera. Straight lines stay straight
 * under projection, so callers may connect projected endpoints.
 *
 * K8.0 reliability note: `cam.width / cam.height` are authoritative —
 * callers that know their own container dimensions (the 2D overlay
 * measures its own `getBoundingClientRect()` via `ResizeObserver`)
 * should override the bridge's values with a shallow-spread cam object
 * before calling.
 *
 * Headless fallback (no published camera, e.g. unit tests): a plain
 * orthographic XY mapping centred on the canvas, +v up.
 */
export function sketchToScreen(
  u: number,
  v: number,
  cam: Readonly<SketchCameraState>,
): [number, number] {
  const camera = getPublishedPickCamera();
  if (!camera) {
    return [cam.width / 2 + u * cam.zoom, cam.height / 2 - v * cam.zoom];
  }
  const w = planeToWorld(u, v, activeFrame);
  projV.set(w[0], w[1], w[2]).project(camera);
  return [
    (projV.x * 0.5 + 0.5) * cam.width,
    (0.5 - projV.y * 0.5) * cam.height,
  ];
}

const rayCaster = new THREE.Raycaster();
const ndcV = new THREE.Vector2();
let lastUv: [number, number] = [0, 0];

/**
 * Inverse of [`sketchToScreen`]: cast the camera ray through the screen
 * point and intersect the sketch plane. Unlike `rayToPlaneUv` this does
 * NOT reject hits behind the ray origin — the overlay needs a stable
 * mapping for every cursor position; for a (near-)parallel ray it
 * returns the last good value.
 */
export function screenToSketch(
  px: number,
  py: number,
  cam: Readonly<SketchCameraState>,
): [number, number] {
  const camera = getPublishedPickCamera();
  if (!camera) {
    return [(px - cam.width / 2) / cam.zoom, (cam.height / 2 - py) / cam.zoom];
  }
  ndcV.set((px / cam.width) * 2 - 1, -((py / cam.height) * 2 - 1));
  rayCaster.setFromCamera(ndcV, camera);
  const o = rayCaster.ray.origin;
  const d = rayCaster.ray.direction;
  const f = activeFrame;
  const n = f.normal;
  const denom = d.x * n[0] + d.y * n[1] + d.z * n[2];
  if (Math.abs(denom) < 1e-9) return lastUv;
  const t =
    ((f.origin[0] - o.x) * n[0] +
      (f.origin[1] - o.y) * n[1] +
      (f.origin[2] - o.z) * n[2]) /
    denom;
  const hx = o.x + t * d.x - f.origin[0];
  const hy = o.y + t * d.y - f.origin[1];
  const hz = o.z + t * d.z - f.origin[2];
  lastUv = [
    hx * f.xAxis[0] + hy * f.xAxis[1] + hz * f.xAxis[2],
    hx * f.yAxis[0] + hy * f.yAxis[1] + hz * f.yAxis[2],
  ];
  return lastUv;
}
