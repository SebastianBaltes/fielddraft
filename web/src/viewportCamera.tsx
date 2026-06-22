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
import { getSceneBounds, GRID_BOUNDS } from "./sceneBoundsBridge";
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
  if (!persp.isPerspectiveCamera) return false;
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
  // Never put near behind the camera, below a small floor, or past a 1e-4
  // ratio to far (the f32 depth-buffer limit).
  near = Math.max(near, far * 1e-4, 0.05);
  if (
    Math.abs(persp.near - near) > near * 1e-3 ||
    Math.abs(persp.far - far) > far * 1e-3
  ) {
    persp.near = near;
    persp.far = far;
    persp.updateProjectionMatrix();
    return true;
  }
  return false;
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
      camera.position.set(...preset.position);
      camera.up.set(...preset.up);
      camera.lookAt(0, 0, 0);
      target.set(0, 0, 0);
    } else if (cmd.kind === "fit") {
      const b = getSceneBounds() ?? { center: [0, 0, 0] as [number, number, number], radius: 100 };
      const center = new THREE.Vector3(...b.center);
      const radius = Math.max(b.radius, 1);
      // Keep the current view direction; only dolly to frame the bounds.
      const dir = camera.position.clone().sub(target);
      if (dir.lengthSq() < 1e-9) dir.set(0, 0, 1);
      dir.normalize();
      const dist = fitDistance(camera as THREE.PerspectiveCamera, radius);
      camera.position.copy(center).addScaledVector(dir, dist);
      camera.lookAt(center);
      target.copy(center);
    } else if (cmd.kind === "normal") {
      const frame = getSketchPlaneFrame();
      if (!frame) return;
      const origin = new THREE.Vector3(...frame.origin);
      let normal = new THREE.Vector3(...frame.normal);
      if (normal.lengthSq() < 1e-9) return;
      normal.normalize();
      // Stay on whichever side of the plane the camera is already on so the
      // view doesn't flip 180° when looking "normal to" the sketch.
      if (camera.position.clone().sub(origin).dot(normal) < 0) normal.negate();
      const b = getSceneBounds();
      const dist = b && b.radius > 0
        ? fitDistance(camera as THREE.PerspectiveCamera, b.radius)
        : Math.max(camera.position.distanceTo(origin), 1);
      // Respect the camera's CURRENT up as much as possible (ticket s8l0):
      // project the live view-up onto the plane ⊥ normal so we only rotate to
      // face the sketch, without rolling to the plane's own Y axis. Fall back
      // to the plane yAxis only when the current up is parallel to the normal
      // (degenerate projection — nothing sensible to preserve).
      const up = new THREE.Vector3(0, 1, 0)
        .applyQuaternion(camera.quaternion)
        .projectOnPlane(normal);
      if (up.lengthSq() < 1e-6) up.set(...frame.yAxis).projectOnPlane(normal);
      camera.up.copy(up.normalize());
      camera.position.copy(origin).addScaledVector(normal, dist);
      camera.lookAt(origin);
      target.copy(origin);
    }

    camera.updateProjectionMatrix();
    if (ctrl?.update) {
      ctrl.update();
    }
    invalidate();
  });
  return null;
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
