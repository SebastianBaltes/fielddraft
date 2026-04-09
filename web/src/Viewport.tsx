/**
 * Main 3D viewport.
 *
 * React Three Fiber + WebGPU renderer, populated from the kernel's
 * zero-copy packed feature buffer.
 *
 * **M2 rendering note:** the renderer still draws each feature as an
 * individual mesh (one boxGeometry / sphereGeometry / cylinderGeometry
 * per feature) and tints it by its boolean op (Union = solid blue,
 * Difference = translucent red, Intersect = translucent yellow). This is
 * a *preview* — it makes the feature spine visually legible (you can
 * see what each feature is and what it's supposed to do) without
 * actually solving the composite SDF on the GPU. The proper SDF
 * raymarching shader, which would render the *result* of the booleans
 * directly off the kernel's packed buffer, is the immediate follow-up
 * inside M2 once TSL raymarching is dialed in. The kernel evaluation in
 * `sdf_features` is already correct and exhaustively tested; only the
 * realtime visualization is stubbed.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import { Canvas, type ThreeElements } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import { BoolOpTag, FEATURE_STRIDE, PrimitiveKind } from "./kernel";
import { useKernelStore } from "./store";
import type { FeatureSummary } from "./kernel";

interface RenderedFeature {
  id: number;
  op: number; // 0 union, 1 difference, 2 intersect
  kind: number; // 0 box, 1 sphere, 2 cylinder
  center: [number, number, number];
  params: [number, number, number];
}

function readFeatures(view: Float32Array, summaries: FeatureSummary[]): RenderedFeature[] {
  const out: RenderedFeature[] = [];
  for (let i = 0; i < summaries.length; i++) {
    const base = i * FEATURE_STRIDE;
    out.push({
      id: summaries[i].id,
      kind: view[base],
      center: [view[base + 1], view[base + 2], view[base + 3]],
      params: [view[base + 4], view[base + 5], view[base + 6]],
      op: view[base + 7],
    });
  }
  return out;
}

interface MaterialDescriptor {
  color: string;
  transparent: boolean;
  opacity: number;
  wireframe: boolean;
}

function materialFor(op: number): MaterialDescriptor {
  switch (op) {
    case BoolOpTag.Difference:
      return { color: "#e25555", transparent: true, opacity: 0.35, wireframe: false };
    case BoolOpTag.Intersect:
      return { color: "#e6c14f", transparent: true, opacity: 0.5, wireframe: false };
    case BoolOpTag.Union:
    default:
      return { color: "#9ac3ff", transparent: false, opacity: 1, wireframe: false };
  }
}

interface PrimitiveMeshProps {
  feature: RenderedFeature;
  selected: boolean;
}

function PrimitiveMesh({ feature, selected }: PrimitiveMeshProps) {
  const mat = materialFor(feature.op);
  const meshProps: ThreeElements["mesh"] = {
    position: feature.center,
  };
  const matProps = {
    color: selected ? "#ffffff" : mat.color,
    transparent: mat.transparent,
    opacity: mat.opacity,
    metalness: 0.05,
    roughness: 0.45,
    emissive: selected ? mat.color : "#000000",
    emissiveIntensity: selected ? 0.4 : 0,
  };

  if (feature.kind === PrimitiveKind.Box) {
    const [hx, hy, hz] = feature.params;
    return (
      <mesh {...meshProps}>
        <boxGeometry args={[hx * 2, hy * 2, hz * 2]} />
        <meshStandardMaterial {...matProps} />
      </mesh>
    );
  }
  if (feature.kind === PrimitiveKind.Sphere) {
    const r = feature.params[0];
    return (
      <mesh {...meshProps}>
        <sphereGeometry args={[r, 48, 32]} />
        <meshStandardMaterial {...matProps} />
      </mesh>
    );
  }
  if (feature.kind === PrimitiveKind.Cylinder) {
    const r = feature.params[0];
    const hh = feature.params[1];
    return (
      <mesh {...meshProps}>
        <cylinderGeometry args={[r, r, hh * 2, 48]} />
        <meshStandardMaterial {...matProps} />
      </mesh>
    );
  }
  return null;
}

function Scene() {
  const { store, state } = useKernelStore();

  if (!store || !state) return null;

  const view = store.primitivesView();
  const features = readFeatures(view, state.features);

  return (
    <>
      <ambientLight intensity={0.4} />
      <directionalLight position={[50, 80, 40]} intensity={1.2} castShadow />
      <directionalLight position={[-60, 30, -20]} intensity={0.5} />

      <gridHelper args={[500, 50, "#3a3a46", "#2a2a32"]} />

      {features.map((f) => (
        <PrimitiveMesh
          key={f.id}
          feature={f}
          selected={state.selection.has(f.id)}
        />
      ))}
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
