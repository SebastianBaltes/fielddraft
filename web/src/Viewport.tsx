/**
 * Main 3D viewport.
 *
 * React Three Fiber + WebGPU renderer, populated from the kernel's
 * zero-copy packed primitive buffer. At M1 the primitives are drawn as
 * straight Three.js meshes (one per entry) — exactly the same parameters
 * the SDF uses, just rendered the classical way.
 *
 * The raymarching fragment shader that will eventually replace mesh
 * rendering (so the viewport hits the composite distance field directly)
 * is intentionally deferred to a follow-up iteration within M1, because
 * Three.js' WebGPU/TSL path is still evolving and we want M1 shippable
 * with both WebGPU and the zero-copy bridge demonstrably working first.
 * See the inline TODO near `<PrimitiveMesh>`.
 */

import { Component, type ErrorInfo, type ReactNode, useMemo } from "react";
import { Canvas } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import { PRIMITIVE_STRIDE, PrimitiveKind } from "./kernel";
import { useKernelStore } from "./store";

interface PrimitiveMeshProps {
  slot: Float32Array; // 8-float view into the packed buffer
  index: number;
}

function PrimitiveMesh({ slot, index }: PrimitiveMeshProps) {
  const kind = slot[0];
  const center: [number, number, number] = [slot[1], slot[2], slot[3]];

  // TODO(M1-raymarch): replace this mesh-based rendering with a single
  // fullscreen raymarching pass that reads the whole packed buffer in one
  // shader invocation. For M1's foundation we ship traditional meshes so
  // the WebGPU renderer, zero-copy bridge, and autosave path can be
  // verified end-to-end without also debugging a new TSL shader.
  if (kind === PrimitiveKind.Box) {
    const hx = slot[4];
    const hy = slot[5];
    const hz = slot[6];
    return (
      <mesh position={center} key={index}>
        <boxGeometry args={[hx * 2, hy * 2, hz * 2]} />
        <meshStandardMaterial color="#9ac3ff" metalness={0.1} roughness={0.4} />
      </mesh>
    );
  }
  if (kind === PrimitiveKind.Sphere) {
    const r = slot[4];
    return (
      <mesh position={center} key={index}>
        <sphereGeometry args={[r, 48, 32]} />
        <meshStandardMaterial color="#9ac3ff" metalness={0.1} roughness={0.4} />
      </mesh>
    );
  }
  if (kind === PrimitiveKind.Cylinder) {
    const r = slot[4];
    const hh = slot[5];
    return (
      <mesh position={center} key={index}>
        <cylinderGeometry args={[r, r, hh * 2, 48]} />
        <meshStandardMaterial color="#9ac3ff" metalness={0.1} roughness={0.4} />
      </mesh>
    );
  }
  return null;
}

function Scene() {
  const { store, error } = useKernelStore();

  const slices = useMemo(() => {
    if (!store) return [] as Float32Array[];
    const view = store.primitives();
    const count = store.kernel.primitiveCount();
    const out: Float32Array[] = [];
    for (let i = 0; i < count; i++) {
      out.push(view.subarray(i * PRIMITIVE_STRIDE, (i + 1) * PRIMITIVE_STRIDE));
    }
    return out;
  }, [store, store?.version]);

  if (error) {
    return (
      <mesh>
        <boxGeometry args={[5, 5, 5]} />
        <meshStandardMaterial color="#ff5050" />
      </mesh>
    );
  }

  return (
    <>
      <ambientLight intensity={0.4} />
      <directionalLight position={[50, 80, 40]} intensity={1.2} castShadow />
      <directionalLight position={[-60, 30, -20]} intensity={0.5} />

      {/* GridHelper is a line-based primitive — WebGPU-compatible as of
          three r183. drei's <Grid> will replace this once its ShaderMaterial
          is ported to NodeMaterial. */}
      <gridHelper args={[500, 50, "#3a3a46", "#2a2a32"]} />

      {slices.map((slot, i) => (
        <PrimitiveMesh slot={slot} index={i} key={i} />
      ))}
    </>
  );
}

/**
 * Small error boundary scoped to the 3D canvas. Keeps the app shell
 * (header, primitive counter, future panels) alive if R3F / WebGPU /
 * drei throws during scene construction or render.
 */
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
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            width: "100%",
            height: "100%",
            color: "#ff6b6b",
            fontFamily: "monospace",
            padding: "16px",
            textAlign: "center",
          }}
        >
          viewport error: {this.state.error.message}
        </div>
      );
    }
    return this.props.children;
  }
}

export function Viewport() {
  // WebGPU renderer via R3F 9's async `gl` factory. `three/webgpu` exports
  // `WebGPURenderer`, which we initialize asynchronously (required by the
  // WebGPU adapter handshake) before R3F mounts the scene.
  if (typeof navigator !== "undefined" && !("gpu" in navigator)) {
    console.warn(
      "fielddraft: navigator.gpu not available. WebGPU is a hard requirement — the viewport will fail to initialize on this browser.",
    );
  }

  return (
    <div data-testid="fielddraft-canvas" style={{ width: "100%", height: "100%" }}>
      <CanvasErrorBoundary>
        <Canvas
          shadows
          camera={{ position: [60, 50, 60], fov: 45, near: 0.1, far: 2000 }}
          gl={async (props) => {
            const { WebGPURenderer } = await import("three/webgpu");
            const renderer = new WebGPURenderer({
              ...(props as object),
              antialias: true,
            });
            await renderer.init();
            return renderer;
          }}
        >
          <Scene />
          <OrbitControls makeDefault />
          {/*
           * NOTE: drei's GizmoViewcube / GizmoViewport still call
           * WebGLRenderer-only APIs (`getMaxAnisotropy`), so the view cube
           * is deliberately deferred until drei's WebGPU compatibility
           * catches up. Tracked as a small follow-up inside M1.
           */}
        </Canvas>
      </CanvasErrorBoundary>
    </div>
  );
}
