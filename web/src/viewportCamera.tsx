import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { OnshapeControls, type RotateMode } from "./onshapeControls";
import { publishSketchCamera, getSketchPlaneFrame } from "./sketchCameraBridge";
import { requestOverlayRedraw } from "./sketchOverlayBridge";
import {
  consumeViewCommand,
  peekViewCommand,
  registerInvalidate,
  requestView,
  VIEW_PRESETS,
  type ViewPreset,
} from "./viewCommands";
import { publishPickCamera } from "./pickCameraBridge";
import { getCameraPose, saveCameraPose } from "./cameraPoseBridge";
import { getSceneBounds, GRID_BOUNDS } from "./sceneBoundsBridge";
import { peekStore } from "./store";
import {
  CONTROLS_PRESETS,
  ORBIT_ACTION_CODE,
  toOrbitMouseButtons,
  type ControlsPresetId,
} from "./controlsPreset";

/**
 * Adapt a perspective camera's near/far clip planes to the scene every frame so
 * bodies never clip when zooming out (ticket lbzq: a fixed `far = 4000` with a
 * ~3300 mm scene let bodies fall past the far plane once the camera receded).
 * The planes tightly bracket the scene's bounding sphere from the current
 * camera position, with a depth-precision floor on the near/far ratio. Empty /
 * unknown scene → leaves the camera's defaults. Returns true if it changed.
 */
export function adaptClipPlanes(camera: THREE.Camera): boolean {
  const persp = camera as THREE.PerspectiveCamera;
  const ortho = camera as THREE.OrthographicCamera;
  const isPersp = persp.isPerspectiveCamera === true;
  const isOrtho = ortho.isOrthographicCamera === true;
  if (!isPersp && !isOrtho) return false;
  // Bracket near/far around EVERY drawn thing, not just the bodies: the world
  // grid extends well past a small model and its foreground edge sits far
  // closer to the camera, so bracketing the bodies alone clipped it (xv9c).
  // Take the closest near and farthest far across the body + grid spheres.
  const body = getSceneBounds();
  const spheres =
    body && body.radius > 0 ? [body, GRID_BOUNDS] : [GRID_BOUNDS];
  let near = Infinity;
  let far = 0;
  for (const s of spheres) {
    const dx = camera.position.x - s.center[0];
    const dy = camera.position.y - s.center[1];
    const dz = camera.position.z - s.center[2];
    const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
    // Pad so geometry exactly on the sphere never lands on a clip plane.
    const pad = s.radius * 0.5 + 1;
    far = Math.max(far, d + s.radius + pad);
    near = Math.min(near, d - s.radius - pad);
  }
  // Perspective: never put near behind the camera, below a small floor, or
  // past a 1e-4 ratio to far (the f32 reciprocal-z precision limit). Ortho
  // depth is LINEAR, so it has no such ratio limit — only keep near a hair
  // ahead of the camera (it may still be small) and below far.
  near = isOrtho
    ? Math.max(near, 0.05)
    : Math.max(near, far * 1e-4, 0.05);
  // `near`/`far`/`updateProjectionMatrix` live on both camera subclasses.
  const clip = camera as THREE.OrthographicCamera;
  if (
    Math.abs(clip.near - near) > near * 1e-3 ||
    Math.abs(clip.far - far) > far * 1e-3
  ) {
    clip.near = near;
    clip.far = far;
    clip.updateProjectionMatrix();
    return true;
  }
  return false;
}

/**
 * Orthographic zoom that frames a sphere of `radius` like a perspective camera
 * would at distance `dist`: a perspective view sees a world height of
 * `2·tan(fov/2)·dist` at the target depth, and an ortho camera with our
 * pixel-frustum convention (`top−bottom = viewportHeight`) sees `height / zoom`,
 * so equating the two gives this zoom. Used both to derive an ortho zoom when
 * toggling from perspective and to zoom-to-fit in ortho.
 */
function orthoZoomForWorldHeight(viewportHeightPx: number, worldHeight: number): number {
  if (worldHeight <= 1e-9 || viewportHeightPx <= 0) return 1;
  return viewportHeightPx / worldHeight;
}

/**
 * Lay an OrthographicCamera's frustum over the canvas in PIXEL units
 * (`left/right/top/bottom` = ±half the canvas size) so `zoom` reads as
 * pixels-per-world-unit and `1/zoom` is the world span of one device pixel —
 * the same quantity the pan/dolly math wants. Re-applied on every resize.
 */
export function setOrthoFrustum(
  cam: THREE.OrthographicCamera,
  widthPx: number,
  heightPx: number,
): void {
  cam.left = -widthPx / 2;
  cam.right = widthPx / 2;
  cam.top = heightPx / 2;
  cam.bottom = -heightPx / 2;
  cam.updateProjectionMatrix();
}

/**
 * Owns the viewport camera and swaps its PROJECTION between orthographic
 * (default — parallel, isometric-style, no foreshortening) and perspective
 * (fov 45) when the `perspective` setting flips, WITHOUT remounting the Canvas
 * (a WebGPU remount re-compiles every SDF shader). Both cameras are created
 * once; on a flip we copy the live pose (position / orientation / up) onto the
 * incoming camera, match its framing to the outgoing one, and register it as
 * R3F's default via `set({ camera })`. CameraRig keys its OnshapeControls on the
 * R3F camera identity, so the controls automatically rebind to the new camera.
 */
export function ViewportCamera({ perspective }: { perspective: boolean }) {
  const set = useThree((s) => s.set);
  const get = useThree((s) => s.get);
  const size = useThree((s) => s.size);
  // The projection-swap effect below needs the CURRENT viewport size for its
  // framing math, but must NOT re-run on resize (that would copy the inactive
  // camera's default pose onto the active one — ticket flzq). Read the size
  // from a live ref so the swap stays keyed on `perspective` alone.
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const camerasRef = useRef<{
    persp: THREE.PerspectiveCamera;
    ortho: THREE.OrthographicCamera;
  } | null>(null);

  if (!camerasRef.current) {
    const aspect = size.width > 0 && size.height > 0 ? size.width / size.height : 1;
    const persp = new THREE.PerspectiveCamera(45, aspect, 1, 4000);
    persp.position.set(350, 280, 350);
    persp.lookAt(0, 0, 0);
    const ortho = new THREE.OrthographicCamera(-1, 1, 1, -1, 1, 4000);
    setOrthoFrustum(ortho, size.width || 1, size.height || 1);
    // Frame the same default view as the perspective camera: at the default
    // 350·√3 ≈ 575 mm distance with fov 45, the world height is ~476 mm.
    const dist = persp.position.length();
    const worldH = 2 * Math.tan((45 * Math.PI) / 360) * dist;
    ortho.zoom = orthoZoomForWorldHeight(size.height || 1, worldH);
    ortho.position.copy(persp.position);
    ortho.quaternion.copy(persp.quaternion);
    ortho.up.copy(persp.up);
    ortho.updateProjectionMatrix();
    camerasRef.current = { persp, ortho };
  }

  // Keep BOTH cameras' aspect/frustum current on resize, regardless of which
  // is active — so a toggle never shows a stretched frame for a frame.
  useEffect(() => {
    const cams = camerasRef.current;
    if (!cams || size.width <= 0 || size.height <= 0) return;
    cams.persp.aspect = size.width / size.height;
    cams.persp.updateProjectionMatrix();
    setOrthoFrustum(cams.ortho, size.width, size.height);
  }, [size.width, size.height]);

  // Swap the active camera when the projection setting flips. Copy the live
  // pose off the OUTGOING camera so the view doesn't jump, and match the
  // incoming camera's framing (ortho zoom ⇄ perspective distance) about the
  // current pivot so the model keeps the same on-screen size. Keyed on
  // `perspective` ONLY — a resize must never trigger this pose copy (it would
  // stamp the untouched inactive camera's default pose onto the active one,
  // resetting the view; ticket flzq). The current size comes from `sizeRef`.
  useEffect(() => {
    const cams = camerasRef.current;
    if (!cams) return;
    const size = sizeRef.current;
    const from = perspective ? cams.ortho : cams.persp;
    const to = perspective ? cams.persp : cams.ortho;
    to.position.copy(from.position);
    to.quaternion.copy(from.quaternion);
    to.up.copy(from.up);

    // Match framing about the CURRENT view pivot so the toggle is a pure
    // projection change, not a jump. `dist` = camera→pivot distance. The pivot
    // must be what the camera actually looks at — the controls' orbit target
    // (ticket q3bb: the framing used the scene-AABB centre instead, which
    // differs from the target after a boolean carve or an ohtn cursor-pivot, so
    // the model lurched on toggle). Fall back to the scene centre / origin
    // before the controls exist.
    const controls = get().controls as unknown as OnshapeControls | null;
    const b = getSceneBounds();
    const pivot =
      controls && controls.target
        ? controls.target.clone()
        : new THREE.Vector3(...(b && b.radius > 0 ? b.center : [0, 0, 0]));
    const dist = Math.max(from.position.distanceTo(pivot), 1e-3);
    if (perspective) {
      // ortho→persp: choose distance so the perspective world-height at the
      // pivot equals the ortho world-height the user currently sees.
      const orthoWorldH = (size.height || 1) / (cams.ortho.zoom || 1);
      const halfFov = ((cams.persp.fov || 45) * Math.PI) / 360;
      const newDist = orthoWorldH / (2 * Math.tan(halfFov));
      const dir = from.position.clone().sub(pivot);
      if (dir.lengthSq() > 1e-9) {
        dir.normalize();
        to.position.copy(pivot).addScaledVector(dir, Math.max(newDist, 1e-3));
      }
      to.updateProjectionMatrix();
    } else {
      // persp→ortho: set the zoom so the ortho world-height equals what the
      // perspective camera frames at the pivot distance.
      const halfFov = ((cams.persp.fov || 45) * Math.PI) / 360;
      const perspWorldH = 2 * Math.tan(halfFov) * dist;
      cams.ortho.zoom = orthoZoomForWorldHeight(size.height || 1, perspWorldH);
      setOrthoFrustum(cams.ortho, size.width || 1, size.height || 1);
    }
    to.updateMatrixWorld();
    set({ camera: to });
  }, [perspective, set, get]);

  return null;
}

/**
 * Carries the camera pose across a render-shader switch. Switching between a
 * WebGPU shader and the WebGL fallback remounts the `<Canvas>` with a fresh
 * camera at the default view; this keeps the pose in `cameraPoseBridge` current
 * every frame and restores it ONCE on the new canvas's mount, so the viewpoint
 * survives the switch. Mounted in both canvas trees, next to `CameraRig`.
 */
export function CameraPoseSync() {
  const camera = useThree((s) => s.camera);
  const controls = useThree(
    (s) => s.controls,
  ) as unknown as OnshapeControls | null;
  const invalidate = useThree((s) => s.invalidate);
  const didRestore = useRef(false);
  const ready = useRef(false);

  // Restore exactly once, when the controls first exist on this canvas. Keyed
  // so it does NOT re-run on the perspective⇄ortho swap (which changes the
  // camera identity but does its own pose-preserving framing) — that would undo
  // the framing and stamp a stale zoom.
  useEffect(() => {
    if (!controls) return;
    if (!didRestore.current) {
      const p = getCameraPose();
      if (p) {
        camera.position.fromArray(p.position);
        camera.quaternion.fromArray(p.quaternion);
        camera.up.fromArray(p.up);
        const proj = camera as THREE.OrthographicCamera;
        if (proj.isOrthographicCamera) {
          proj.zoom = p.zoom;
          proj.updateProjectionMatrix();
        }
        controls.target.fromArray(p.target);
        camera.updateMatrixWorld();
        invalidate();
      }
      didRestore.current = true;
    }
    ready.current = true; // gate saving until the restore has run (no clobber)
  }, [camera, controls, invalidate]);

  // Keep the bridge current with the live camera on every rendered frame, so
  // the next canvas restores exactly where we left off.
  useFrame(() => {
    if (controls && ready.current) saveCameraPose(camera, controls.target);
  });
  return null;
}

export function CameraBridgePublisher() {
  const { camera, size, gl } = useThree();

  useEffect(() => {
    publishPickCamera(camera);
    return () => {
      publishPickCamera(null);
    };
  }, [camera]);

  useFrame(() => {
    // Keep the clip planes wrapped around the scene so zooming out never
    // pushes bodies past the (SDF-deferred) far plane (ticket lbzq).
    adaptClipPlanes(camera);
    // Bring the camera's world/inverse matrices up to date for THIS frame
    // before anything projects through it. In useFrame these still reflect
    // the previous render until the renderer recomputes them, so the
    // overlay's `camera.project()` would otherwise lag one frame behind.
    camera.updateMatrixWorld();
    camera.matrixWorldInverse.copy(camera.matrixWorld).invert();
    publishSketchCamera({
      width: size.width,
      height: size.height,
      devicePixelRatio: (gl as unknown as { getPixelRatio?: () => number }).getPixelRatio?.() ?? window.devicePixelRatio ?? 1,
    });
    // Redraw the 2D sketch overlay in lockstep with this frame's WebGPU
    // render so its HUD (hover/snap/grips/selection/dimensions) tracks the
    // camera without the one-frame lag of its own rAF loop.
    requestOverlayRedraw();
  });
  return null;
}

export function CameraRig({ preset }: { preset: ControlsPresetId }) {
  const { camera } = CONTROLS_PRESETS[preset];
  const cam = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const invalidate = useThree((s) => s.invalidate);
  const set = useThree((s) => s.set);
  const get = useThree((s) => s.get);
  const controlsRef = useRef<OnshapeControls | null>(null);

  // Create the OnshapeControls and register them as the R3F default `controls`
  // — view commands (ViewCube / zoom-to-fit / view-normal), the transform
  // gizmos and the B-Rep overlays all read `state.controls`. Recreated only
  // when the camera/canvas identity changes; preset config lives in its own
  // effect so switching presets doesn't tear the controls down.
  useEffect(() => {
    const controls = new OnshapeControls(cam, gl.domElement as HTMLElement);
    // Demand-frameloop roots only render when invalidated, so a drag that
    // moves the camera must request a frame (the WebGL root's own rAF loop
    // ticks regardless, but the extra invalidate is harmless there).
    controls.onChange = () => invalidate();
    // Onshape orbit-pivot heuristic (ticket ohtn): on rotate-start, sphere-trace
    // the cursor ray through the CSG and pivot on the hit point; a miss returns
    // null → the controller falls back to the scene centre. `kernel.pick` is
    // renderer-independent (works in every SDF / WebGL mode). `peekStore` avoids
    // subscribing this rig to store-state changes.
    const host = gl.domElement as HTMLElement;
    controls.pickPivot = (clientX, clientY) => {
      const st = peekStore();
      if (!st) return null;
      const rect = host.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return null;
      const ndcX = ((clientX - rect.left) / rect.width) * 2 - 1;
      const ndcY = -(((clientY - rect.top) / rect.height) * 2 - 1);
      const rc = new THREE.Raycaster();
      rc.setFromCamera(new THREE.Vector2(ndcX, ndcY), cam);
      const o = rc.ray.origin;
      const d = rc.ray.direction;
      const res = st.kernel.pick([o.x, o.y, o.z], [d.x, d.y, d.z]);
      return res.hit ? res.point : null;
    };
    // Inherit the last-known orbit target (ticket q3bb): the perspective toggle
    // swaps the active camera, which recreates these controls with a default
    // (origin) target — and CameraPoseSync only restores once per canvas mount,
    // not on the swap. Seed the target from the pose bridge so a rotate right
    // after a toggle still orbits the same point (and the toggle's framing,
    // which reads this target, stays correct across repeated toggles).
    const savedPose = getCameraPose();
    if (savedPose) controls.target.fromArray(savedPose.target);
    controlsRef.current = controls;
    const previous = get().controls;
    set({ controls: controls as unknown as THREE.Controls });
    return () => {
      set({ controls: previous });
      controls.dispose();
      controlsRef.current = null;
    };
  }, [cam, gl, invalidate, set, get]);

  // Apply the active preset's button mapping + speeds. Mutating the live
  // controls is enough — they read `mouseButtons` at pointerdown.
  useEffect(() => {
    const controls = controlsRef.current;
    if (!controls) return;
    const mb = toOrbitMouseButtons(camera);
    controls.mouseButtons = {
      LEFT: mb.LEFT ?? null,
      MIDDLE: mb.MIDDLE ?? null,
      RIGHT: mb.RIGHT ?? null,
    };
    controls.rotateSpeed = camera.rotateSpeed;
    controls.panSpeed = camera.panSpeed;
    controls.zoomSpeed = camera.zoomSpeed;
  }, [camera]);

  // Onshape's two rotation modes (resource-center: "Rotation with an upright
  // vertical axis"): free tumble by default, upright (model never rolls) while
  // Alt is held. Flip the mode on the modifier; `blur` resets a stuck Alt.
  useEffect(() => {
    const setMode = (m: RotateMode) => {
      if (controlsRef.current) controlsRef.current.rotateMode = m;
    };
    const onDown = (e: KeyboardEvent) => {
      if (e.key === "Alt") setMode("upright");
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.key === "Alt") setMode("free");
    };
    const onBlur = () => setMode("free");
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  // Shift/Ctrl-held button remaps. The controls read `mouseButtons` at
  // pointerdown, so mutating the live object on Shift/Ctrl down/up is enough —
  // no re-render needed.
  //  · Fusion: MMB pans normally, orbits while Shift is held.
  //  · Blender: MMB orbits normally, pans while Shift is held (the inverse).
  //  · SolidWorks: MMB orbits normally, pans while Ctrl is held.
  //  · Three.js: LMB orbits normally, but Shift FREES it so Shift+LMB-drag
  //    falls through to the viewport's box select instead of orbiting.
  useEffect(() => {
    if (
      !camera.shiftMiddleOrbit &&
      !camera.shiftMiddlePan &&
      !camera.ctrlMiddlePan &&
      !camera.shiftLeftBoxSelect
    )
      return;
    const setMod = (key: "Shift" | "Control", down: boolean) => {
      const ctrl = controlsRef.current;
      if (!ctrl) return;
      if (key === "Shift" && camera.shiftMiddleOrbit) {
        ctrl.mouseButtons.MIDDLE = down
          ? ORBIT_ACTION_CODE.rotate
          : ORBIT_ACTION_CODE.pan;
      }
      if (key === "Shift" && camera.shiftMiddlePan) {
        ctrl.mouseButtons.MIDDLE = down
          ? ORBIT_ACTION_CODE.pan
          : ORBIT_ACTION_CODE.rotate;
      }
      if (key === "Control" && camera.ctrlMiddlePan) {
        ctrl.mouseButtons.MIDDLE = down
          ? ORBIT_ACTION_CODE.pan
          : ORBIT_ACTION_CODE.rotate;
      }
      if (key === "Shift" && camera.shiftLeftBoxSelect) {
        ctrl.mouseButtons.LEFT = down ? null : ORBIT_ACTION_CODE.rotate;
      }
    };
    const onDown = (e: KeyboardEvent) => {
      if (e.key === "Shift") setMod("Shift", true);
      if (e.key === "Control") setMod("Control", true);
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.key === "Shift") setMod("Shift", false);
      if (e.key === "Control") setMod("Control", false);
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  }, [
    camera.shiftMiddleOrbit,
    camera.shiftMiddlePan,
    camera.ctrlMiddlePan,
    camera.shiftLeftBoxSelect,
  ]);

  // Ticket bxoo: no separate sketch camera — the normal camera and controls
  // stay live during a sketch session; the overlay projects through them
  // (sketchCameraBridge). The controls are imperative (set as the R3F default
  // above), so the rig renders nothing.
  return null;
}

export function ViewCommandConsumer() {
  const controlsRef = useRef<THREE.EventDispatcher | null>(null);
  const invalidate = useThree((s) => s.invalidate);
  useEffect(() => {
    registerInvalidate(invalidate);
    if (import.meta.env.DEV) {
      (window as unknown as { __fdInvalidate?: () => void }).__fdInvalidate = invalidate;
    }
    return () => registerInvalidate(() => {});
  }, [invalidate]);
  useFrame(({ camera, controls }) => {
    if (controls) controlsRef.current = controls as unknown as THREE.EventDispatcher;
    if (!peekViewCommand()) return;
    // Ticket xw51: apply the view command even when no orbit-controls object
    // is registered on the R3F state at this instant (it can be null on the
    // frame the command fires — and is entirely absent in a sketch session,
    // where the rig isn't the default controls). The camera is moved directly
    // below; `ctrl` only keeps an OrbitControls target/update in sync when one
    // exists. The old `if (!ctrl) return` left the ViewCube / zoom-to-fit dead.
    const ctrl = controlsRef.current as unknown as {
      target?: THREE.Vector3;
      object?: THREE.Camera;
      update?: () => void;
    } | null;
    const cmd = consumeViewCommand()!;
    const target = ctrl?.target ?? new THREE.Vector3(0, 0, 0);

    if (cmd.kind === "preset") {
      const preset = VIEW_PRESETS[cmd.preset];
      // Frame the scene's bounding sphere from the preset's direction, centred
      // on the sphere — NOT the world origin. The ViewCube always references
      // the model so a body sitting away from the origin stays framed.
      const b = getSceneBounds();
      const center = new THREE.Vector3(...(b && b.radius > 0 ? b.center : [0, 0, 0]));
      const radius = b && b.radius > 0 ? Math.max(b.radius, 1) : 100;
      const dir = new THREE.Vector3(...preset.position).normalize();
      camera.up.set(...preset.up);
      frameSphere(camera, center, radius, dir);
      target.copy(center);
    } else if (cmd.kind === "fit") {
      const b = getSceneBounds() ?? { center: [0, 0, 0] as [number, number, number], radius: 100 };
      const center = new THREE.Vector3(...b.center);
      const radius = Math.max(b.radius, 1);
      // Keep the current view direction; only dolly/zoom to frame the bounds.
      const dir = camera.position.clone().sub(target);
      if (dir.lengthSq() < 1e-9) dir.set(0, 0, 1);
      dir.normalize();
      frameSphere(camera, center, radius, dir);
      target.copy(center);
    } else if (cmd.kind === "normal") {
      const frame = getSketchPlaneFrame();
      if (!frame) return;
      if (!orientNormalTo(camera, frame.origin, frame.normal, frame.yAxis)) return;
      target.set(...frame.origin);
    } else if (cmd.kind === "normalToFace") {
      // Face analogue of "normal" (ticket z0vk): same orientation + fit math,
      // driven by the selected planar face's world normal/centroid instead of
      // the sketch plane frame. No in-plane yAxis to fall back to, so the only
      // fallback up is world Y (handled inside orientNormalTo).
      if (!orientNormalTo(camera, cmd.centroid, cmd.normal)) return;
      target.set(...cmd.centroid);
    }

    camera.updateProjectionMatrix();
    if (ctrl?.update) {
      ctrl.update();
    }
    invalidate();
  });
  return null;
}

/**
 * Orient `camera` to look straight down `rawNormal` at `origin`, then frame the
 * scene's bounding sphere from straight on (ortho zooms, perspective dollies).
 * Shared by the sketch-plane "normal" and face "normalToFace" view commands so
 * both follow identical camera math (ticket z0vk).
 *
 * - Stays on whichever side of the plane the camera is already on, so the view
 *   never flips 180°.
 * - Preserves the camera's current up as much as possible by projecting it onto
 *   the plane ⊥ normal (ticket s8l0); falls back to `fallbackUp` (the sketch
 *   plane's yAxis) and then world Y when the current up is parallel to normal.
 * - Empty scene: keeps the current standoff and just faces the plane.
 *
 * `origin`, `rawNormal`, `fallbackUp` are world-space tuples. Returns false
 * (leaving the camera untouched) when `rawNormal` is degenerate.
 */
export function orientNormalTo(
  camera: THREE.Camera,
  origin: [number, number, number],
  rawNormal: [number, number, number],
  fallbackUp: [number, number, number] = [0, 1, 0],
): boolean {
  const o = new THREE.Vector3(...origin);
  const normal = new THREE.Vector3(...rawNormal);
  if (normal.lengthSq() < 1e-9) return false;
  normal.normalize();
  if (camera.position.clone().sub(o).dot(normal) < 0) normal.negate();
  const up = new THREE.Vector3(0, 1, 0)
    .applyQuaternion(camera.quaternion)
    .projectOnPlane(normal);
  if (up.lengthSq() < 1e-6) up.set(...fallbackUp).projectOnPlane(normal);
  if (up.lengthSq() < 1e-6) up.set(0, 1, 0).projectOnPlane(normal);
  if (up.lengthSq() < 1e-6) up.set(1, 0, 0).projectOnPlane(normal);
  camera.up.copy(up.normalize());
  const b = getSceneBounds();
  if (b && b.radius > 0) {
    frameSphere(camera, o, Math.max(b.radius, 1), normal);
  } else {
    const dist = Math.max(camera.position.distanceTo(o), 1);
    camera.position.copy(o).addScaledVector(normal, dist);
    camera.lookAt(o);
  }
  return true;
}

/** Distance at which a perspective camera frames a sphere of `radius`,
 *  binding on whichever of the vertical/horizontal field-of-view is
 *  narrower, with a small margin so the geometry isn't flush to the edge. */
function fitDistance(camera: THREE.PerspectiveCamera, radius: number): number {
  const vFov = ((camera.isPerspectiveCamera ? camera.fov : 45) * Math.PI) / 180;
  const aspect = camera.isPerspectiveCamera ? camera.aspect || 1 : 1;
  const hFov = 2 * Math.atan(Math.tan(vFov / 2) * aspect);
  const half = Math.min(vFov, hFov) / 2;
  return (radius / Math.sin(half)) * 1.15;
}

/**
 * Place `camera` so it frames a sphere of `radius` centred at `center`, looking
 * from direction `dir` (a unit vector from centre toward the camera). Works for
 * both projections: a perspective camera dollies to {@link fitDistance}; an
 * orthographic camera keeps a fixed standoff (so the scene stays inside its
 * near/far) and instead sets `zoom` to fit the sphere into the narrower frustum
 * extent. Leaves `camera.up` untouched — callers set it before calling.
 */
function frameSphere(
  camera: THREE.Camera,
  center: THREE.Vector3,
  radius: number,
  dir: THREE.Vector3,
): void {
  const ortho = camera as THREE.OrthographicCamera;
  if (ortho.isOrthographicCamera) {
    // Stand off far enough that the whole sphere stays within near/far, then
    // zoom to fit. Half the smaller frustum span (in world units, before zoom)
    // is (extent/2); we want radius·1.15 to equal that ÷ zoom.
    const standoff = Math.max(radius * 4, 1);
    camera.position.copy(center).addScaledVector(dir, standoff);
    camera.lookAt(center);
    const halfExtent = Math.min(
      (ortho.right - ortho.left) / 2,
      (ortho.top - ortho.bottom) / 2,
    );
    if (halfExtent > 1e-6 && radius > 1e-9) {
      ortho.zoom = halfExtent / (radius * 1.15);
    }
    ortho.updateProjectionMatrix();
    return;
  }
  const dist = fitDistance(camera as THREE.PerspectiveCamera, radius);
  camera.position.copy(center).addScaledVector(dir, dist);
  camera.lookAt(center);
}

const cameraQuatState = { x: 0, y: 0, z: 0, w: 1, listeners: new Set<() => void>() };

function subscribeCameraQuat(cb: () => void) {
  cameraQuatState.listeners.add(cb);
  return () => { cameraQuatState.listeners.delete(cb); };
}

/** Publish the active camera's orientation to the ViewCube. Shared by the
 *  WebGPU bridge (useFrame) and the WebGL viewport's manual render loop, which
 *  bypasses useFrame (ticket r8b4 — the cube tracks whichever path is live). */
export function publishCameraQuat(q: { x: number; y: number; z: number; w: number }) {
  if (
    Math.abs(q.x - cameraQuatState.x) > 1e-4 ||
    Math.abs(q.y - cameraQuatState.y) > 1e-4 ||
    Math.abs(q.z - cameraQuatState.z) > 1e-4 ||
    Math.abs(q.w - cameraQuatState.w) > 1e-4
  ) {
    cameraQuatState.x = q.x;
    cameraQuatState.y = q.y;
    cameraQuatState.z = q.z;
    cameraQuatState.w = q.w;
    for (const cb of cameraQuatState.listeners) cb();
  }
}

export function CameraQuaternionBridge() {
  useFrame(({ camera }) => publishCameraQuat(camera.quaternion));
  return null;
}

export function ViewCube() {
  const cubeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    return subscribeCameraQuat(() => {
      if (!cubeRef.current) return;
      const { x, z, w } = cameraQuatState;
      const y = -cameraQuatState.y;
      cubeRef.current.style.transform =
        `matrix3d(${1-2*(y*y+z*z)},${2*(x*y+z*w)},${2*(x*z-y*w)},0,` +
        `${2*(x*y-z*w)},${1-2*(x*x+z*z)},${2*(y*z+x*w)},0,` +
        `${2*(x*z+y*w)},${2*(y*z-x*w)},${1-2*(x*x+y*y)},0,` +
        `0,0,0,1)`;
    });
  }, []);

  const faceClick = (e: React.MouseEvent, preset: Exclude<ViewPreset, null>) => {
    e.stopPropagation();
    requestView(preset);
  };

  return (
    <div className="viewcube-wrapper" onClick={(e) => e.stopPropagation()}>
      <div className="viewcube-scene">
        <div ref={cubeRef} className="viewcube">
          <div className="viewcube-face front" onClick={(e) => faceClick(e, "front")}>Front</div>
          <div className="viewcube-face back" onClick={(e) => faceClick(e, "back")}>Back</div>
          <div className="viewcube-face top" onClick={(e) => faceClick(e, "top")}>Top</div>
          <div className="viewcube-face bottom" onClick={(e) => faceClick(e, "bottom")}>Bottom</div>
          <div className="viewcube-face right" onClick={(e) => faceClick(e, "right")}>Right</div>
          <div className="viewcube-face left" onClick={(e) => faceClick(e, "left")}>Left</div>
        </div>
      </div>
    </div>
  );
}

export function ClearColorSync() {
  const gl = useThree((s) => s.gl);
  const invalidate = useThree((s) => s.invalidate);

  useEffect(() => {
    const apply = () => {
      // ticket xqp3: der 3D-Viewport wird weiß geleert (eigener Token),
      // während die UI-Shell (--color-bg) grau bleibt.
      const bg = getComputedStyle(document.documentElement)
        .getPropertyValue("--color-viewport-bg")
        .trim();
      if (bg) {
        gl.setClearColor(new THREE.Color(bg), 1);
        invalidate();
      }
    };
    apply();
    const obs = new MutationObserver(apply);
    obs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => obs.disconnect();
  }, [gl, invalidate]);

  return null;
}
