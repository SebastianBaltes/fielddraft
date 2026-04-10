/**
 * Main 3D viewport.
 *
 * K1 onwards: the kernel owns the mesh. On every mutation it re-runs
 * the Dual Contouring pass and writes flat positions/normals/indices
 * into WASM linear memory. This component binds those buffers to a
 * `THREE.BufferGeometry` on the JS side via zero-copy `Float32Array`
 * and `Uint32Array` views — nothing is serialized across the bridge,
 * and the geometry is rebuilt only when the kernel's `mesh_version`
 * advances.
 *
 * The TSL raymarcher lives on in `raymarch.ts` but is no longer wired
 * into the scene — it's kept as a reference implementation for future
 * tests of the 3D picking ray (K2).
 */

import {
  Component,
  type ErrorInfo,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { createKernelMeshMaterial } from "./meshMaterial";
import type { PickResult } from "./kernel";

/** World-space size of the build-volume reference grid (mm). Matches
 *  the Onshape-style 300 mm cube budget, doubled for breathing room. */
const BUILD_VOLUME_SIZE = 600;

/**
 * Live mesh driven by the kernel.
 *
 * We allocate one `BufferGeometry` up front, then on every frame check
 * whether the kernel's `meshVersion()` has advanced since our last
 * upload. When it has, we grab fresh zero-copy views of positions,
 * normals and indices and rebind them as attributes. React Three Fiber
 * picks up the new attribute buffers automatically on the next render.
 */
function KernelMesh() {
  const { store } = useKernelStore();

  const geometry = useMemo(() => new THREE.BufferGeometry(), []);

  // Simple Lambert-ish shader over the kernel's smoothed normals.
  // Implemented in a sidecar module because the TSL API is loose-typed
  // and the rest of this file stays under strict TS.
  const material = useMemo(() => createKernelMeshMaterial(), []);

  const lastVersionRef = useRef<number>(-1);

  const upload = (): void => {
    if (!store) return;
    const k = store.kernel;
    const version = k.meshVersion();
    if (version === lastVersionRef.current) return;
    lastVersionRef.current = version;

    const vertexCount = k.meshVertexCount();
    const indexCount = k.meshIndexCount();

    if (vertexCount === 0 || indexCount === 0) {
      geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(0), 3));
      geometry.setAttribute("normal", new THREE.BufferAttribute(new Float32Array(0), 3));
      geometry.setIndex(new THREE.BufferAttribute(new Uint32Array(0), 1));
      geometry.boundingBox = null;
      geometry.boundingSphere = null;
      return;
    }

    // `.slice()` snapshots the WASM-aliased view into a JS-owned
    // buffer. Keeping an alias instead would be tempting but unsafe:
    // the next kernel mutation can reallocate the underlying `Vec`,
    // which would silently corrupt the geometry's attribute data on
    // the GPU. A per-rebuild copy is cheap (tens of kB for K1 meshes)
    // and buys us lifetime safety.
    const positions = k.meshPositionsView().slice();
    const normals = k.meshNormalsView().slice();
    const indices = k.meshIndicesView().slice();

    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(normals, 3));
    geometry.setIndex(new THREE.BufferAttribute(indices, 1));
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  };

  // One-shot upload on mount / store change so the very first paint
  // has the correct geometry.
  useEffect(() => {
    lastVersionRef.current = -1;
    upload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store]);

  // Pick up later mutations. We cannot subscribe directly to the
  // kernel's mesh_version, so we poll it once per frame — this is
  // O(1) work per frame and only triggers an upload when the version
  // actually changed.
  useFrame(() => {
    upload();
  });

  useEffect(
    () => () => {
      geometry.dispose();
      material.dispose();
    },
    [geometry, material],
  );

  return (
    <mesh geometry={geometry} material={material} frustumCulled={false} />
  );
}

/**
 * Hover-picking probe.
 *
 * Listens to the parent div's normalised pointer coordinates (fed in
 * via `ndc`), builds a world-space ray out of the current camera,
 * calls the kernel's sphere-tracing `pick()` and publishes the result
 * upward through `onPick`. The actual visual marker at the hit point
 * is a sibling `<mesh>` driven by the same result.
 *
 * Running this inside the Canvas lets us use R3F's `useThree` to grab
 * the live camera without plumbing a ref out of the renderer.
 */
interface PickProbeProps {
  ndc: { x: number; y: number; active: boolean };
  onPick: (result: PickResult | null) => void;
}
function PickProbe({ ndc, onPick }: PickProbeProps) {
  const { store } = useKernelStore();
  const { camera } = useThree();
  const ndcRef = useRef(ndc);
  ndcRef.current = ndc;

  // Throttle: only pick when NDC has moved by more than a pixel-scale
  // amount OR the mesh version has changed. Keeps the kernel call
  // count bounded as the user drags the cursor across the canvas.
  const lastNdcRef = useRef<{ x: number; y: number }>({ x: NaN, y: NaN });
  const lastMeshVersionRef = useRef<number>(-1);
  const raycaster = useMemo(() => new THREE.Raycaster(), []);

  useFrame(() => {
    if (!store) return;
    const k = store.kernel;
    if (k.meshVertexCount() === 0) {
      if (lastMeshVersionRef.current !== -2) {
        onPick(null);
        lastMeshVersionRef.current = -2;
      }
      return;
    }

    const cur = ndcRef.current;
    if (!cur.active) {
      if (lastMeshVersionRef.current !== -3) {
        onPick(null);
        lastMeshVersionRef.current = -3;
      }
      return;
    }

    const version = k.meshVersion();
    const movedEnough =
      Math.abs(cur.x - lastNdcRef.current.x) > 0.001 ||
      Math.abs(cur.y - lastNdcRef.current.y) > 0.001;
    if (!movedEnough && version === lastMeshVersionRef.current) return;
    lastNdcRef.current = { x: cur.x, y: cur.y };
    lastMeshVersionRef.current = version;

    raycaster.setFromCamera(new THREE.Vector2(cur.x, cur.y), camera);
    const origin = raycaster.ray.origin;
    const dir = raycaster.ray.direction;
    const result = k.pick([origin.x, origin.y, origin.z], [dir.x, dir.y, dir.z]);
    onPick(result);
  });

  return null;
}

/**
 * Small translucent sphere rendered at the current pick hit point.
 * `null` result hides it; any hit shows it with a colour keyed to the
 * proxy element (face → blue, edge → orange, vertex → red).
 */
function HitMarker({ result }: { result: PickResult | null }) {
  if (!result || !result.hit) return null;
  const color =
    result.element === "edge"
      ? "#e88a28"
      : result.element === "vertex"
      ? "#d23a3a"
      : "#2b6cb0";
  return (
    <mesh position={[result.point[0], result.point[1], result.point[2]]}>
      <sphereGeometry args={[2.5, 16, 12]} />
      <meshBasicMaterial color={color} />
    </mesh>
  );
}

interface SceneProps {
  ndc: { x: number; y: number; active: boolean };
  onPick: (result: PickResult | null) => void;
  pickResult: PickResult | null;
}
function Scene({ ndc, onPick, pickResult }: SceneProps) {
  const { store } = useKernelStore();
  if (!store) return null;
  return (
    <>
      <KernelMesh />
      <PickProbe ndc={ndc} onPick={onPick} />
      <HitMarker result={pickResult} />
      {/* Build-volume floor reference. */}
      <gridHelper args={[BUILD_VOLUME_SIZE, 30, "#b8b8ae", "#d6d6ce"]} />
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

  const hostRef = useRef<HTMLDivElement | null>(null);
  const [ndc, setNdc] = useState<{ x: number; y: number; active: boolean }>({
    x: 0,
    y: 0,
    active: false,
  });
  const [pickResult, setPickResult] = useState<PickResult | null>(null);

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>): void => {
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    const y = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
    setNdc({ x, y, active: true });
  };

  const handlePointerLeave = (): void => {
    setNdc((prev) => ({ ...prev, active: false }));
    setPickResult(null);
  };

  return (
    <div
      ref={hostRef}
      className="fielddraft-canvas-host"
      data-testid="fielddraft-canvas"
      onPointerMove={handlePointerMove}
      onPointerLeave={handlePointerLeave}
    >
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
          <Scene ndc={ndc} onPick={setPickResult} pickResult={pickResult} />
          <OrbitControls
            makeDefault
            enableDamping={false}
            rotateSpeed={1.4}
            zoomSpeed={1.6}
            panSpeed={1.2}
          />
        </Canvas>
      </CanvasErrorBoundary>
      <PickHud result={pickResult} />
    </div>
  );
}

/**
 * Tiny overlay in the corner of the viewport that shows the current
 * hover pick result. Serves double duty as a visible smoke signal that
 * the provenance pipeline (kernel raycast → contributor classification
 * → JS HUD) is alive, and as a debugging read-out during K2 work.
 */
function PickHud({ result }: { result: PickResult | null }) {
  if (!result || !result.hit) {
    return (
      <div
        className="fielddraft-pick-hud"
        data-testid="fielddraft-pick-hud"
        data-pick-hit="false"
      >
        no hover
      </div>
    );
  }
  const contributors = result.contributors.join(", ");
  return (
    <div
      className="fielddraft-pick-hud"
      data-testid="fielddraft-pick-hud"
      data-pick-hit="true"
      data-pick-element={result.element}
      data-pick-contributors={contributors}
    >
      <span className="fielddraft-pick-hud__kind">{result.element}</span>
      <span className="fielddraft-pick-hud__ids">{contributors}</span>
    </div>
  );
}
