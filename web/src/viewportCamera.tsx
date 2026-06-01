import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { OrbitControls, OrthographicCamera } from "@react-three/drei";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { publishSketchCamera } from "./sketchCameraBridge";
import {
  consumeViewCommand,
  peekViewCommand,
  registerInvalidate,
  requestView,
  VIEW_PRESETS,
  type ViewPreset,
} from "./viewCommands";
import { publishPickCamera } from "./pickCameraBridge";

export function CameraBridgePublisher() {
  const { camera, size, gl } = useThree();

  useEffect(() => {
    publishPickCamera(camera);
    return () => {
      publishPickCamera(null);
    };
  }, [camera]);

  useFrame(() => {
    const cam = camera as THREE.OrthographicCamera;
    const centerU = cam.position?.x ?? 0;
    const centerV = cam.position?.z ?? 0;
    publishSketchCamera({
      width: size.width,
      height: size.height,
      zoom: (cam.zoom ?? 1) as number,
      centerU,
      centerV,
      devicePixelRatio: (gl as unknown as { getPixelRatio?: () => number }).getPixelRatio?.() ?? window.devicePixelRatio ?? 1,
    });
  });
  return null;
}

export function CameraRig() {
  const { state } = useKernelStore();
  const sketchActive = state?.sketchSession.active ?? false;

  if (sketchActive) {
    return (
      <>
        <OrthographicCamera
          makeDefault
          position={[0, 500, 0]}
          up={[0, 0, -1]}
          near={1}
          far={4000}
          zoom={1.2}
        />
        <OrbitControls
          makeDefault
          enableDamping={false}
          enableRotate={false}
          enablePan
          enableZoom
          panSpeed={1.2}
          zoomSpeed={1.6}
          target={[0, 0, 0]}
        />
      </>
    );
  }

  return (
    <OrbitControls
      makeDefault
      enableDamping={false}
      rotateSpeed={1.4}
      zoomSpeed={1.6}
      panSpeed={1.2}
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

export function CameraQuaternionBridge() {
  useFrame(({ camera }) => {
    const q = camera.quaternion;
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
  });
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
      const bg = getComputedStyle(document.documentElement)
        .getPropertyValue("--color-bg")
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
