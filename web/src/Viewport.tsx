/**
 * Main 3D viewport.
 *
 * React Three Fiber + WebGPU + a TSL sphere-tracing raymarcher reading
 * the kernel's packed feature buffer directly. The raymarched scene is
 * the only visualization — there are no per-feature meshes anymore.
 *
 * The world-bounding mesh is a `BackSide` cube the size of the build
 * volume. Its NodeMaterial overrides `colorNode` to sphere-trace the
 * composite SDF every fragment; OrbitControls orbits the camera around
 * it just like a normal mesh in the scene.
 */

import { Component, type ErrorInfo, type ReactNode, useEffect, useMemo, useRef } from "react";
import { Canvas, useFrame } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import * as THREE from "three";
import { useKernelStore } from "./store";
import {
  RAYMARCH_VOLUME_SIZE,
  createRaymarchMaterial,
  uploadFeatures,
  type RaymarchHandle,
} from "./raymarch";

function RaymarchMesh() {
  const { store, state } = useKernelStore();
  const handleRef = useRef<RaymarchHandle | null>(null);

  const handle = useMemo<RaymarchHandle>(() => {
    const h = createRaymarchMaterial();
    handleRef.current = h;
    return h;
  }, []);

  // Push the latest features into the uniform array on every render.
  // We re-upload on `useFrame` rather than only on store mutations so
  // that any in-place mutation of the WASM-backed view is picked up.
  useFrame(() => {
    if (!store) return;
    const view = store.primitivesView();
    uploadFeatures(handle, view, store.kernel.featureCount());
  });

  // Also push immediately when the feature list changes — useFrame
  // gets us continuous freshness, this gets us a one-shot for the very
  // first frame after a mutation.
  useEffect(() => {
    if (!store || !state) return;
    const view = store.primitivesView();
    uploadFeatures(handle, view, store.kernel.featureCount());
  }, [store, state, handle]);

  return (
    <mesh frustumCulled={false} renderOrder={-1}>
      <boxGeometry
        args={[RAYMARCH_VOLUME_SIZE, RAYMARCH_VOLUME_SIZE, RAYMARCH_VOLUME_SIZE]}
      />
      <primitive object={handle.material} attach="material" />
    </mesh>
  );
}

function Scene() {
  const { store } = useKernelStore();
  if (!store) return null;
  return (
    <>
      <RaymarchMesh />
      {/* Build-volume floor reference. WebGPU-friendly LineSegments
          via three.js GridHelper. */}
      <gridHelper args={[RAYMARCH_VOLUME_SIZE, 30, "#b8b8ae", "#d6d6ce"]} />
    </>
  );
}

interface CanvasBoundaryProps {
  children: ReactNode;
}
interface CanvasBoundaryState {
  error: Error | null;
}
class CanvasErrorBoundary extends Component<CanvasBoundaryProps, CanvasBoundaryState> {
  state: CanvasBoundaryState = { error: null };
  static getDerivedStateFromError(error: Error): CanvasBoundaryState {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("fielddraft: viewport crashed", error, info);
  }
  render() {
    if (this.state.error) {
      return (
        <div className="fielddraft-viewport-error">
          viewport error: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

export function Viewport() {
  if (typeof navigator !== "undefined" && !("gpu" in navigator)) {
    console.warn(
      "fielddraft: navigator.gpu not available. WebGPU is a hard requirement — the viewport will fail to initialize on this browser.",
    );
  }

  return (
    <div className="fielddraft-canvas-host" data-testid="fielddraft-canvas">
      <CanvasErrorBoundary>
        <Canvas
          shadows={false}
          camera={{ position: [350, 280, 350], fov: 45, near: 1, far: 4000 }}
          gl={async (props) => {
            const { WebGPURenderer } = await import("three/webgpu");
            const renderer = new WebGPURenderer({
              ...(props as object),
              antialias: true,
            });
            await renderer.init();
            renderer.setClearColor(new THREE.Color("#f6f6f4"), 1);
            return renderer;
          }}
        >
          <Scene />
          <OrbitControls
            makeDefault
            enableDamping={false}
            rotateSpeed={1.4}
            zoomSpeed={1.6}
            panSpeed={1.2}
          />
        </Canvas>
      </CanvasErrorBoundary>
    </div>
  );
}
