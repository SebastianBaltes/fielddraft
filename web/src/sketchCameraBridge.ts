/**
 * Module-level bridge between the R3F camera (owned by the
 * WebGPU Canvas) and the 2D Canvas sketch overlay that lives
 * next to the R3F Canvas in the DOM.
 *
 * The bridge is a plain mutable object: a small R3F component
 * inside the Canvas (`CameraBridgePublisher`) calls `publish`
 * on every frame with the current orthographic camera state;
 * the overlay reads from the same object in its own
 * `requestAnimationFrame` loop. Avoids plumbing camera state
 * through React props / context, which would either miss frames
 * or force re-renders every frame.
 *
 * Tilted sketch planes (K6b.3+) will extend this with a full
 * `sketchPlaneBasis` matrix so the overlay's `sketchToScreen`
 * function can project arbitrary planar sketches; K6b.3 opening
 * slice hardcodes the top-down-ortho case where the sketch
 * plane is the world XZ plane.
 */

export interface SketchCameraState {
  /** Viewport size in CSS pixels. */
  width: number;
  height: number;
  /** Orthographic zoom factor (`THREE.OrthographicCamera.zoom`).
   *  Larger = more zoomed in, 1 CSS px == 1/zoom world unit on
   *  the sketch plane. */
  zoom: number;
  /** Camera world-space position projected onto the sketch
   *  plane. For the top-down-ortho case this is
   *  `(camera.position.x, camera.position.z)` — the sketch-2D
   *  coordinates of the camera centre. OrbitControls pan
   *  translates this. */
  centerU: number;
  centerV: number;
  /** Device pixel ratio at publish time — the overlay canvas
   *  needs this for its own backing-store size. */
  devicePixelRatio: number;
  /** Monotonic counter bumped on every `publish` call. Lets
   *  the overlay's RAF loop detect "nothing moved" and skip a
   *  redraw without doing deep equality on the numerics. */
  version: number;
}

const bridge: SketchCameraState = {
  width: 0,
  height: 0,
  zoom: 1,
  centerU: 0,
  centerV: 0,
  devicePixelRatio: 1,
  version: 0,
};

export function publishSketchCamera(next: Omit<SketchCameraState, "version">): void {
  bridge.width = next.width;
  bridge.height = next.height;
  bridge.zoom = next.zoom;
  bridge.centerU = next.centerU;
  bridge.centerV = next.centerV;
  bridge.devicePixelRatio = next.devicePixelRatio;
  bridge.version += 1;
}

export function readSketchCamera(): Readonly<SketchCameraState> {
  return bridge;
}

// Dev-mode window hook so Playwright tests can peek the
// current camera state without having to set up a
// side-channel export.
if (typeof window !== "undefined" && import.meta.env?.DEV) {
  (
    window as unknown as { __fdSketchCamera?: () => Readonly<SketchCameraState> }
  ).__fdSketchCamera = () => bridge;
}

/**
 * Project a 2D sketch-plane coordinate to CSS-pixel coordinates
 * of the overlay canvas. Assumes the sketch plane is the world
 * XZ plane (top-down orthographic view), so sketch-U maps to
 * world-X and sketch-V maps to world-Z. The camera's up vector
 * is `(0, 0, -1)`, so screen-up corresponds to `-V` in sketch
 * space: +V draws downward, matching the 3D layer underneath
 * (ticket fsmf).
 *
 * K8.0 reliability note: `cam.width / cam.height` here are
 * authoritative — callers that know their own container
 * dimensions (the 2D overlay measures its own
 * `getBoundingClientRect()` via `ResizeObserver`) should
 * override the bridge's values with a shallow-spread cam
 * object before calling. The R3F camera bridge sometimes
 * publishes stale dimensions while the WebGL canvas is still
 * resizing to its container.
 *
 * Returns `[px, py]` with origin at the top-left of the overlay
 * canvas (CSS pixels).
 */
export function sketchToScreen(
  u: number,
  v: number,
  cam: Readonly<SketchCameraState>,
): [number, number] {
  const du = u - cam.centerU;
  const dv = v - cam.centerV;
  const px = cam.width / 2 + du * cam.zoom;
  // Screen-up is world −Z (camera up `(0,0,-1)`), so +V draws DOWNWARD —
  // the same orientation the 3D viewport rasterises underneath. A −dv here
  // mirrored the whole 2D overlay vertically against the 3D layer, so the
  // two layers panned in OPPOSITE directions (ticket fsmf).
  const py = cam.height / 2 + dv * cam.zoom;
  return [px, py];
}

/** Inverse of [`sketchToScreen`]. */
export function screenToSketch(
  px: number,
  py: number,
  cam: Readonly<SketchCameraState>,
): [number, number] {
  const du = (px - cam.width / 2) / cam.zoom;
  const dv = (py - cam.height / 2) / cam.zoom;
  return [cam.centerU + du, cam.centerV + dv];
}
