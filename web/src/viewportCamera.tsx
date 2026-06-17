import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import * as THREE from "three";
import { publishSketchCamera } from "./sketchCameraBridge";
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
import { getSceneBounds } from "./sceneBoundsBridge";
import {
  CONTROLS_PRESETS,
  ORBIT_ACTION_CODE,
  toOrbitMouseButtons,
  type ControlsPresetId,
  type OrbitMouseButtons,
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
  const b = getSceneBounds();
  if (!b || b.radius <= 0) return false;
  const dx = camera.position.x - b.center[0];
  const dy = camera.position.y - b.center[1];
  const dz = camera.position.z - b.center[2];
  const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
  // Pad so geometry exactly on the sphere never lands on a clip plane.
  const pad = b.radius * 0.5 + 1;
  const far = d + b.radius + pad;
  // Push near out as far as the scene front allows (best depth precision), but
  // never behind the camera, below a small floor, or past a 1e-4 ratio (the
  // f32 depth-buffer limit).
  const near = Math.max(d - b.radius - pad, far * 1e-4, 0.05);
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
  const controlsRef = useRef<{ mouseButtons: OrbitMouseButtons } | null>(null);

  // Fusion idiom: MMB pans, orbit only while Shift is held. OrbitControls
  // reads `mouseButtons` at pointerdown, so swapping the MIDDLE action on
  // Shift down/up is enough — no re-render needed.
  useEffect(() => {
    if (!camera.shiftMiddleOrbit) return;
    const setShift = (down: boolean) => {
      const ctrl = controlsRef.current;
      if (ctrl) {
        ctrl.mouseButtons.MIDDLE = down
          ? ORBIT_ACTION_CODE.rotate
          : ORBIT_ACTION_CODE.pan;
      }
    };
    const onDown = (e: KeyboardEvent) => { if (e.key === "Shift") setShift(true); };
    const onUp = (e: KeyboardEvent) => { if (e.key === "Shift") setShift(false); };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  }, [camera.shiftMiddleOrbit]);

  // Ticket bxoo: no separate sketch camera — the normal camera and
  // controls stay live during a sketch session; the overlay projects
  // through them (sketchCameraBridge).
  const mouseButtons = toOrbitMouseButtons(camera);

  return (
    <OrbitControls
      ref={controlsRef as never}
      makeDefault
      enableDamping={false}
      mouseButtons={mouseButtons}
      rotateSpeed={camera.rotateSpeed}
      zoomSpeed={camera.zoomSpeed}
      panSpeed={camera.panSpeed}
    />
  );
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
    const ctrl = controlsRef.current as unknown as {
      target?: THREE.Vector3;
      object?: THREE.Camera;
      update?: () => void;
    } | null;
    if (!ctrl) return;
    const cmd = consumeViewCommand()!;
    const preset = VIEW_PRESETS[cmd];
    camera.position.set(...preset.position);
    camera.up.set(...preset.up);
    camera.lookAt(0, 0, 0);
    camera.updateProjectionMatrix();
    if (ctrl.target) {
      ctrl.target.set(0, 0, 0);
    }
    if (ctrl.update) {
      ctrl.update();
    }
  });
  return null;
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
