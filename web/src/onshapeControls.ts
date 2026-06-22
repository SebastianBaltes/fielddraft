/**
 * OnshapeControls — camera navigation modelled on Onshape, replacing drei's
 * `OrbitControls`.
 *
 * The brief (and why OrbitControls is the wrong base): in Onshape the camera is
 * conceptually fixed and the whole scene tumbles **unbounded** around a pivot —
 * you can roll past the poles and spin freely in every direction. OrbitControls
 * can't express that: its `Spherical.makeSafe()` hard-clamps the polar angle to
 * `(ε, π−ε)`, so the view locks when you try to go over the top. We instead
 * accumulate rotation as a quaternion, which has no pole and no gimbal lock.
 *
 * Two rotation modes, exactly like Onshape (resource-center "Rotation with an
 * upright vertical axis"):
 *   · `free`    — default. Trackball tumble about the camera-local right/up
 *                 axes; unbounded, the model may end up rolled.
 *   · `upright` — held while Alt is down. Yaw is about the WORLD up axis so the
 *                 model never rolls (its vertical axis stays vertical).
 *
 * Pivot (the "sticky bounding sphere"): on a rotate-start we re-centre the pivot
 * on the bodies'/sketches' bounding-sphere centre **while that sphere still
 * fits the viewport**. Once you've zoomed or panned so the sphere no longer
 * fits, the pivot stops re-centring and stays at its last fitting position —
 * so rotation never suddenly swings around an off-screen centre.
 *
 * Mouse-button mapping reuses the same `THREE.MOUSE` codes (ROTATE=0, DOLLY=1,
 * PAN=2) as OrbitControls, so the controls-preset wiring and the Shift/Ctrl
 * button remaps in `CameraRig` keep working unchanged. A `null` button is free
 * — the drag is ignored and the click falls through to selection.
 */
import * as THREE from "three";
import { getSceneBounds } from "./sceneBoundsBridge";

export type RotateMode = "free" | "upright";

type Action = "rotate" | "dolly" | "pan" | null;

/** Map a `THREE.MOUSE` code to an action. `undefined`/`null` button → free. */
function actionFromCode(code: number | null | undefined): Action {
  if (code === THREE.MOUSE.ROTATE) return "rotate";
  if (code === THREE.MOUSE.DOLLY) return "dolly";
  if (code === THREE.MOUSE.PAN) return "pan";
  return null;
}

const WORLD_UP = /*@__PURE__*/ new THREE.Vector3(0, 1, 0);

// Reused scratch — these are never live across an `await`/callback boundary.
const _offset = new THREE.Vector3();
const _right = new THREE.Vector3();
const _up = new THREE.Vector3();
const _c = new THREE.Vector3();
const _ndc = new THREE.Vector3();
const _viewDir = new THREE.Vector3();
const _rayDir = new THREE.Vector3();
const _focus = new THREE.Vector3();
const _q = new THREE.Quaternion();
const _qa = new THREE.Quaternion();
const _qb = new THREE.Quaternion();

/**
 * Does the bounding sphere (centre `center`, `radius`) sit fully inside the
 * perspective camera's viewport? True only when the sphere is both far enough
 * to fit the narrower field of view (angular size) AND its centre projects
 * inside the frame (so panning the model off to one side also counts as "no
 * longer fits"). The camera's world/projection matrices must be current.
 */
export function sphereFitsViewport(
  camera: THREE.PerspectiveCamera,
  center: THREE.Vector3,
  radius: number,
): boolean {
  if (radius <= 0) return false;
  const dist = camera.position.distanceTo(center);
  const vFov = (camera.fov * Math.PI) / 180;
  const aspect = camera.aspect || 1;
  const hFov = 2 * Math.atan(Math.tan(vFov / 2) * aspect);
  const half = Math.min(vFov, hFov) / 2;
  // Distance at which the sphere exactly touches the frame edges.
  const fitDist = radius / Math.sin(half);
  if (dist < fitDist) return false;
  _ndc.copy(center).project(camera);
  if (_ndc.z > 1) return false; // behind the camera / past the far plane
  return Math.abs(_ndc.x) <= 1 && Math.abs(_ndc.y) <= 1;
}

/**
 * Incremental rotation quaternion for a drag. `yawAngle`/`pitchAngle` are
 * radians (already scaled by speed); `right`/`up` are the camera's current
 * world axes. `free` yaws about the camera-local up (full tumble); `upright`
 * yaws about world-up so the model keeps its vertical axis and never rolls.
 */
export function rotationDelta(
  mode: RotateMode,
  yawAngle: number,
  pitchAngle: number,
  right: THREE.Vector3,
  up: THREE.Vector3,
  out: THREE.Quaternion,
): THREE.Quaternion {
  const yawAxis = mode === "upright" ? WORLD_UP : up;
  _qa.setFromAxisAngle(yawAxis, -yawAngle);
  _qb.setFromAxisAngle(right, -pitchAngle);
  return out.multiplyQuaternions(_qa, _qb);
}

/**
 * Focus point for cursor-zoom: where the cursor ray (`rayDir`, a unit vector
 * from `camPos`) crosses the plane through `target` perpendicular to the view
 * direction. Scaling both camera and target about this point keeps it pinned
 * under the cursor — a uniform scale leaves the view direction unchanged, so
 * the focus projects to the same pixel. Returns `null` when the ray grazes the
 * plane (≈parallel) or the camera sits on the target.
 */
export function cursorZoomFocus(
  camPos: THREE.Vector3,
  target: THREE.Vector3,
  rayDir: THREE.Vector3,
  out: THREE.Vector3,
): THREE.Vector3 | null {
  _viewDir.copy(target).sub(camPos);
  const dist = _viewDir.length();
  if (dist < 1e-9) return null;
  _viewDir.multiplyScalar(1 / dist);
  const denom = rayDir.dot(_viewDir);
  if (Math.abs(denom) < 1e-6) return null;
  return out.copy(camPos).addScaledVector(rayDir, dist / denom);
}

export interface OnshapeMouseButtons {
  LEFT: number | null;
  MIDDLE: number | null;
  RIGHT: number | null;
}

export class OnshapeControls extends THREE.Controls {
  /** Pivot of orbit; pan moves it, the sticky-sphere logic re-centres it. */
  readonly target = new THREE.Vector3();
  /** `THREE.MOUSE` codes per button — same shape OrbitControls exposes. */
  mouseButtons: OnshapeMouseButtons = {
    LEFT: null,
    MIDDLE: THREE.MOUSE.PAN,
    RIGHT: THREE.MOUSE.ROTATE,
  };
  rotateSpeed = 1;
  panSpeed = 1;
  zoomSpeed = 1;
  minDistance = 0.05;
  maxDistance = Infinity;
  rotateMode: RotateMode = "free";
  /** Invoked after every camera change so a demand-mode root can invalidate. */
  onChange: (() => void) | null = null;

  private action: Action = null;
  private px = 0;
  private py = 0;

  constructor(object: THREE.Camera, domElement: HTMLElement | null = null) {
    super(object, domElement);
    if (domElement) this.connect(domElement);
  }

  connect(element: HTMLElement): void {
    super.connect(element);
    element.addEventListener("pointerdown", this.onPointerDown);
    element.addEventListener("wheel", this.onWheel, { passive: false });
    element.addEventListener("contextmenu", this.onContextMenu);
  }

  disconnect(): void {
    const el = this.domElement as HTMLElement | null;
    if (el) {
      el.removeEventListener("pointerdown", this.onPointerDown);
      el.removeEventListener("wheel", this.onWheel);
      el.removeEventListener("contextmenu", this.onContextMenu);
    }
    window.removeEventListener("pointermove", this.onPointerMove);
    window.removeEventListener("pointerup", this.onPointerUp);
  }

  dispose(): void {
    this.disconnect();
  }

  update(): void {
    // No deferred state: every interaction is applied immediately. Kept so the
    // R3F default-controls contract (view commands call `controls.update()`)
    // stays satisfied.
  }

  private get cam(): THREE.PerspectiveCamera {
    return this.object as THREE.PerspectiveCamera;
  }

  private onContextMenu = (e: Event) => {
    // A right-drag rotate must not pop the browser menu on release. Only
    // swallow it when the right button is actually bound to a camera action.
    if (actionFromCode(this.mouseButtons.RIGHT) !== null) e.preventDefault();
  };

  private onPointerDown = (e: PointerEvent) => {
    if (!this.enabled) return;
    const code =
      e.button === 0
        ? this.mouseButtons.LEFT
        : e.button === 1
          ? this.mouseButtons.MIDDLE
          : e.button === 2
            ? this.mouseButtons.RIGHT
            : null;
    const action = actionFromCode(code);
    if (action === null) return; // free button → let the click select
    this.action = action;
    this.px = e.clientX;
    this.py = e.clientY;
    if (action === "rotate") this.recentrePivot();
    (this.domElement as HTMLElement | null)?.setPointerCapture?.(e.pointerId);
    window.addEventListener("pointermove", this.onPointerMove);
    window.addEventListener("pointerup", this.onPointerUp);
    e.stopPropagation();
  };

  private onPointerMove = (e: PointerEvent) => {
    if (!this.enabled || this.action === null) return;
    const dx = e.clientX - this.px;
    const dy = e.clientY - this.py;
    this.px = e.clientX;
    this.py = e.clientY;
    if (this.action === "rotate") this.rotate(dx, dy);
    else if (this.action === "pan") this.pan(dx, dy);
    else this.dolly(dy, e.clientX, e.clientY);
    e.stopPropagation();
  };

  private onPointerUp = (e: PointerEvent) => {
    this.action = null;
    (this.domElement as HTMLElement | null)?.releasePointerCapture?.(e.pointerId);
    window.removeEventListener("pointermove", this.onPointerMove);
    window.removeEventListener("pointerup", this.onPointerUp);
  };

  private onWheel = (e: WheelEvent) => {
    if (!this.enabled) return;
    e.preventDefault();
    this.dolly(e.deltaY, e.clientX, e.clientY);
  };

  /** Decide the rotate pivot via the sticky-bounding-sphere rule. */
  private recentrePivot(): void {
    const cam = this.cam;
    cam.updateMatrixWorld();
    const b = getSceneBounds();
    if (!b || b.radius <= 0) return; // nothing drawn → keep the current pivot
    _c.set(b.center[0], b.center[1], b.center[2]);
    if (sphereFitsViewport(cam, _c, b.radius)) {
      this.target.copy(_c); // sphere still frames → re-centre on the model
    }
    // else: sticky — leave the pivot at its last fitting position
  }

  private rotate(dx: number, dy: number): void {
    const cam = this.cam;
    const h = (this.domElement as HTMLElement | null)?.clientHeight || 1;
    // A full screen-height drag sweeps ≈π of rotation, scaled by the speed.
    const yaw = (dx / h) * Math.PI * this.rotateSpeed;
    const pitch = (dy / h) * Math.PI * this.rotateSpeed;
    cam.updateMatrixWorld();
    _right.setFromMatrixColumn(cam.matrixWorld, 0).normalize();
    _up.setFromMatrixColumn(cam.matrixWorld, 1).normalize();
    rotationDelta(this.rotateMode, yaw, pitch, _right, _up, _q);
    // Orbit the camera about the (fixed) target and carry its orientation with
    // it. Setting the quaternion directly — never lookAt — is what keeps the
    // accumulated free-tumble roll instead of snapping back to an up vector.
    _offset.copy(cam.position).sub(this.target).applyQuaternion(_q);
    cam.position.copy(this.target).add(_offset);
    cam.quaternion.premultiply(_q);
    cam.up.applyQuaternion(_q);
    cam.updateMatrixWorld();
    this.onChange?.();
  }

  private pan(dx: number, dy: number): void {
    const cam = this.cam;
    const h = (this.domElement as HTMLElement | null)?.clientHeight || 1;
    cam.updateMatrixWorld();
    const dist = cam.position.distanceTo(this.target);
    const vFov = (cam.fov * Math.PI) / 180;
    // World units spanned by one device pixel at the target's depth.
    const perPixel = ((2 * Math.tan(vFov / 2) * dist) / h) * this.panSpeed;
    _right.setFromMatrixColumn(cam.matrixWorld, 0).normalize();
    _up.setFromMatrixColumn(cam.matrixWorld, 1).normalize();
    _offset
      .copy(_right)
      .multiplyScalar(-dx * perPixel)
      .addScaledVector(_up, dy * perPixel);
    cam.position.add(_offset);
    this.target.add(_offset);
    cam.updateMatrixWorld();
    this.onChange?.();
  }

  /** `amount > 0` (scroll/drag down) zooms OUT; `< 0` zooms IN. Zooms toward
   *  the cursor: the world point under the pointer stays put on screen. */
  private dolly(amount: number, clientX: number, clientY: number): void {
    const cam = this.cam;
    cam.updateMatrixWorld();
    const dist = cam.position.distanceTo(this.target);
    let factor = Math.pow(0.95, (-amount / 100) * this.zoomSpeed);
    // Clamp the factor so the orbit distance stays within [min, max].
    const clamped = Math.min(
      Math.max(dist * factor, this.minDistance),
      this.maxDistance,
    );
    factor = dist > 1e-9 ? clamped / dist : 1;

    const rect = (this.domElement as HTMLElement | null)?.getBoundingClientRect();
    if (rect && rect.width > 0 && rect.height > 0) {
      const ndcX = ((clientX - rect.left) / rect.width) * 2 - 1;
      const ndcY = -((clientY - rect.top) / rect.height) * 2 + 1;
      _rayDir.set(ndcX, ndcY, 0.5).unproject(cam).sub(cam.position).normalize();
      if (cursorZoomFocus(cam.position, this.target, _rayDir, _focus)) {
        // Scale camera + target uniformly about the cursor focus point.
        cam.position.sub(_focus).multiplyScalar(factor).add(_focus);
        this.target.sub(_focus).multiplyScalar(factor).add(_focus);
        cam.updateMatrixWorld();
        this.onChange?.();
        return;
      }
    }
    // Fallback: zoom toward the target (centre) when no canvas rect / grazing.
    _offset.copy(cam.position).sub(this.target).multiplyScalar(factor);
    cam.position.copy(this.target).add(_offset);
    cam.updateMatrixWorld();
    this.onChange?.();
  }
}
