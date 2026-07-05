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
import { ndcZVisible } from "./projectionSpace";

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
 * camera's viewport? For a PERSPECTIVE camera the sphere must be far enough to
 * fit the narrower field of view (angular size); for an ORTHOGRAPHIC camera the
 * sphere's world diameter must fit the (zoom-scaled) frustum extents. In both
 * cases the centre must also project inside the frame, so panning the model off
 * to one side also counts as "no longer fits". The camera's world/projection
 * matrices must be current.
 */
export function sphereFitsViewport(
  camera: THREE.Camera,
  center: THREE.Vector3,
  radius: number,
): boolean {
  if (radius <= 0) return false;
  const ortho = camera as THREE.OrthographicCamera;
  if (ortho.isOrthographicCamera) {
    // Visible world half-extents = half the frustum span divided by zoom.
    const zoom = ortho.zoom || 1;
    const halfW = (ortho.right - ortho.left) / 2 / zoom;
    const halfH = (ortho.top - ortho.bottom) / 2 / zoom;
    if (radius > Math.min(halfW, halfH)) return false;
  } else {
    const persp = camera as THREE.PerspectiveCamera;
    const dist = persp.position.distanceTo(center);
    const vFov = (persp.fov * Math.PI) / 180;
    const aspect = persp.aspect || 1;
    const hFov = 2 * Math.atan(Math.tan(vFov / 2) * aspect);
    const half = Math.min(vFov, hFov) / 2;
    // Distance at which the sphere exactly touches the frame edges.
    const fitDist = radius / Math.sin(half);
    if (dist < fitDist) return false;
  }
  _ndc.copy(center).project(camera);
  // Behind the camera / past the far plane — the valid range depends on the
  // clip-space convention (WebGPU [0,1] vs WebGL [-1,1]), see projectionSpace.
  if (!ndcZVisible(camera, _ndc.z)) return false;
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

/** Finger travel (px) before a one-finger touch becomes a rotate (so a tap
 *  still falls through to selection). */
const TOUCH_ROTATE_THRESHOLD = 6;
/** Pinch distance change (px) → dolly `amount` scale (≈ a wheel notch is 100). */
const PINCH_DOLLY_SCALE = 2;

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

  // ── Touch state ──────────────────────────────────────────────────
  // Active touch pointers by id → last client position. One finger rotates
  // (after a small move so a tap can still select), two fingers pinch-zoom +
  // pan. Kept separate from the mouse-button mapping (touch reports button 0,
  // which is `LEFT` = usually unbound in the Onshape preset).
  private touches = new Map<number, { x: number; y: number }>();
  private touchMode: "none" | "pending" | "rotate" | "pinch" = "none";
  private touchStart = { x: 0, y: 0 };
  private pinchPrev: { dist: number; cx: number; cy: number } | null = null;

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

  /** Whether the active camera is orthographic — pan/dolly differ from the
   *  perspective path (parallel projection: dolly = zoom, not distance). */
  private get isOrtho(): boolean {
    return (this.object as THREE.OrthographicCamera).isOrthographicCamera === true;
  }

  private onContextMenu = (e: Event) => {
    // A right-drag rotate must not pop the browser menu on release. Only
    // swallow it when the right button is actually bound to a camera action.
    if (actionFromCode(this.mouseButtons.RIGHT) !== null) e.preventDefault();
  };

  private onPointerDown = (e: PointerEvent) => {
    if (!this.enabled) return;
    if (e.pointerType === "touch") {
      this.onTouchDown(e);
      return;
    }
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
    this.startWindowTracking();
    e.stopPropagation();
  };

  private onPointerMove = (e: PointerEvent) => {
    if (!this.enabled) return;
    if (this.touches.has(e.pointerId)) {
      this.onTouchMove(e);
      return;
    }
    if (this.action === null) return;
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
    if (this.touches.has(e.pointerId)) {
      this.onTouchUp(e);
      return;
    }
    this.action = null;
    (this.domElement as HTMLElement | null)?.releasePointerCapture?.(e.pointerId);
    this.stopWindowTrackingIfIdle();
  };

  // ── Touch gestures ───────────────────────────────────────────────

  private startWindowTracking(): void {
    // Same-reference listeners → the browser dedups repeated adds.
    window.addEventListener("pointermove", this.onPointerMove);
    window.addEventListener("pointerup", this.onPointerUp);
  }

  private stopWindowTrackingIfIdle(): void {
    if (this.action === null && this.touches.size === 0) {
      window.removeEventListener("pointermove", this.onPointerMove);
      window.removeEventListener("pointerup", this.onPointerUp);
    }
  }

  /** Distance + midpoint of the first two active touches. */
  private pinchState(): { dist: number; cx: number; cy: number } {
    const it = this.touches.values();
    const a = it.next().value as { x: number; y: number };
    const b = it.next().value as { x: number; y: number };
    return {
      dist: Math.hypot(b.x - a.x, b.y - a.y),
      cx: 0.5 * (a.x + b.x),
      cy: 0.5 * (a.y + b.y),
    };
  }

  private onTouchDown(e: PointerEvent): void {
    this.touches.set(e.pointerId, { x: e.clientX, y: e.clientY });
    (this.domElement as HTMLElement | null)?.setPointerCapture?.(e.pointerId);
    this.startWindowTracking();
    if (this.touches.size === 1) {
      this.touchMode = "pending"; // wait for movement so a tap can select
      this.touchStart = { x: e.clientX, y: e.clientY };
    } else if (this.touches.size === 2) {
      this.touchMode = "pinch";
      this.pinchPrev = this.pinchState();
      e.stopPropagation(); // a two-finger gesture is never a tap
    }
  }

  private onTouchMove(e: PointerEvent): void {
    const prev = this.touches.get(e.pointerId);
    if (!prev) return;
    const cur = { x: e.clientX, y: e.clientY };
    this.touches.set(e.pointerId, cur);

    if (this.touchMode === "pending") {
      const moved = Math.hypot(
        e.clientX - this.touchStart.x,
        e.clientY - this.touchStart.y,
      );
      if (moved <= TOUCH_ROTATE_THRESHOLD) return; // still a potential tap
      this.touchMode = "rotate";
      this.recentrePivot();
    }

    if (this.touchMode === "rotate" && this.touches.size === 1) {
      this.rotate(cur.x - prev.x, cur.y - prev.y);
      e.stopPropagation();
    } else if (this.touchMode === "pinch" && this.touches.size >= 2) {
      const st = this.pinchState();
      if (this.pinchPrev) {
        const ddist = st.dist - this.pinchPrev.dist;
        // Spreading fingers (dist grows) zooms IN → negative dolly amount.
        if (ddist !== 0) this.dolly(-ddist * PINCH_DOLLY_SCALE, st.cx, st.cy);
        this.pan(st.cx - this.pinchPrev.cx, st.cy - this.pinchPrev.cy);
      }
      this.pinchPrev = st;
      e.stopPropagation();
    }
  }

  private onTouchUp(e: PointerEvent): void {
    this.touches.delete(e.pointerId);
    (this.domElement as HTMLElement | null)?.releasePointerCapture?.(e.pointerId);
    if (this.touches.size === 1) {
      // Dropped from a pinch to one finger → resume rotating cleanly.
      this.touchMode = "rotate";
      this.pinchPrev = null;
      this.recentrePivot();
    } else if (this.touches.size === 0) {
      this.touchMode = "none";
      this.pinchPrev = null;
    }
    this.stopWindowTrackingIfIdle();
  }

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
    // Ticket f88u: re-centre on the *visible* solid's tight AABB centre
    // (`pivot`), never the conservative `center` — after a boolean carve that
    // still sits on the phantom original primitive (a half-cut sphere would
    // spin about the vanished sphere centre otherwise).
    const c = b.pivot ?? b.center;
    _c.set(c[0], c[1], c[2]);
    // Sticky-bounding-sphere rule: only re-centre while the part still frames
    // in the viewport; once zoomed in so it no longer fits, keep the pivot at
    // its last fitting position so a rotate doesn't lurch to an off-screen
    // centre.
    if (sphereFitsViewport(cam, _c, b.radius)) {
      this.target.copy(_c);
    }
    // else: sticky — leave the pivot at its last fitting position.
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
    // World units spanned by one device pixel. Perspective: scales with the
    // target depth and the field of view. Orthographic: depth-independent —
    // with our pixel-frustum convention (`top−bottom = canvasHeightPx`), one
    // device pixel maps to exactly `1 / zoom` world units.
    let perPixel: number;
    if (this.isOrtho) {
      const ortho = this.object as THREE.OrthographicCamera;
      const frustumH = (ortho.top - ortho.bottom) / (ortho.zoom || 1);
      perPixel = (frustumH / h) * this.panSpeed;
    } else {
      const dist = cam.position.distanceTo(this.target);
      const vFov = (cam.fov * Math.PI) / 180;
      perPixel = ((2 * Math.tan(vFov / 2) * dist) / h) * this.panSpeed;
    }
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
    const factorRaw = Math.pow(0.95, (-amount / 100) * this.zoomSpeed);

    // Orthographic: there is no perspective foreshortening, so a "dolly" must
    // change the camera's ZOOM, not its distance — moving the camera in/out
    // leaves an ortho image identical. `factorRaw < 1` means zoom IN (objects
    // grow), so the zoom scalar is its reciprocal. Cursor-zoom is preserved by
    // panning camera+target so the world point under the pointer keeps its
    // pixel: in ortho, scaling zoom by `s` magnifies the cursor point's screen
    // offset from centre by `s`, so we slide the view by that point's view-plane
    // offset times `(1 − 1/s)` to cancel it.
    if (this.isOrtho) {
      const ortho = this.object as THREE.OrthographicCamera;
      const s = 1 / factorRaw;
      ortho.zoom = Math.max(ortho.zoom * s, 1e-4);
      const rect = (this.domElement as HTMLElement | null)?.getBoundingClientRect();
      if (rect && rect.width > 0 && rect.height > 0) {
        // World point under the cursor, on the plane through the target ⊥ view.
        const ndcX = ((clientX - rect.left) / rect.width) * 2 - 1;
        const ndcY = -((clientY - rect.top) / rect.height) * 2 + 1;
        _focus.set(ndcX, ndcY, 0).unproject(cam);
        // Project that point's offset-from-target onto the camera view plane
        // (drop the view-direction component — irrelevant under parallel proj).
        _viewDir.setFromMatrixColumn(cam.matrixWorld, 2).normalize();
        _offset.copy(_focus).sub(this.target);
        _offset.addScaledVector(_viewDir, -_offset.dot(_viewDir));
        _offset.multiplyScalar(1 - 1 / s);
        cam.position.add(_offset);
        this.target.add(_offset);
      }
      ortho.updateProjectionMatrix();
      cam.updateMatrixWorld();
      this.onChange?.();
      return;
    }

    const dist = cam.position.distanceTo(this.target);
    let factor = factorRaw;
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
