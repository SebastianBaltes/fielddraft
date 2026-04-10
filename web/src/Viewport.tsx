/**
 * Main 3D viewport.
 *
 * K3 part 3: the kernel partitions the document into independent
 * bodies. For every body it owns a separate positions/normals/indices
 * buffer in WASM linear memory plus a stable `bodyId` and a per-body
 * `bodyVersion` that bumps only when that body's mesh was actually
 * re-meshed. This component mirrors the body list on the JS side as
 * a `Map<body_id, BufferGeometry>` and, on every frame, rebinds
 * attributes only for the bodies whose version advanced — unchanged
 * bodies keep their BufferGeometry and their GPU buffers untouched.
 * Bodies that disappear from the kernel (e.g. because their seed
 * feature was deleted or they merged into a neighbour) have their
 * geometries disposed.
 *
 * The TSL raymarcher lives on in `raymarch.ts` but is no longer wired
 * into the scene — kept as reference for the K2 picking ray only.
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

interface BodyEntry {
  geometry: THREE.BufferGeometry;
  /** Last `bodyVersion(i)` we uploaded to the GPU. Stays put when
   *  other bodies mutate around this one; bumps only when the
   *  kernel actually re-meshed this body. */
  version: number;
}

/**
 * Live per-body mesh driven by the store's worker-populated cache.
 *
 * One `BufferGeometry` per body, keyed by the stable `bodyId` that
 * survives partition rebuilds. On every frame we walk the cache,
 * create a geometry for any new id, re-upload attributes for any
 * body whose `version` advanced, and dispose geometries whose body
 * disappeared from the cache (seed removed or merged into a
 * neighbour). Unchanged bodies touch neither CPU nor GPU — that's
 * the whole point of the K3 multi-body cut.
 *
 * The body payloads come from `store.state.meshedBodies`, which is
 * populated either by the mesher worker (normal path) or by a
 * fallback snapshot of the main-thread kernel (when the Worker
 * constructor couldn't spin up). Either way the TypedArrays are
 * main-thread-owned — no WASM-memory aliases, no `.slice()` needed
 * inside this component.
 */
function KernelMesh() {
  const { state } = useKernelStore();

  // Single material shared by every body mesh. Cheap — Three.js
  // re-binds the shader once and the bodies differ only in their
  // vertex attributes.
  const material = useMemo(() => createKernelMeshMaterial(), []);

  // Ref-based map so we can mutate inside `useFrame` without
  // triggering a React re-render on every poll. React re-renders
  // are forced explicitly via `bodySetVersion` whenever the set of
  // live body ids changes (add / remove), not on every version bump.
  const bodyEntriesRef = useRef<Map<number, BodyEntry>>(new Map());
  const [bodySetVersion, setBodySetVersion] = useState(0);

  const uploadBody = (
    body: NonNullable<typeof state>["meshedBodies"][number],
    geometry: THREE.BufferGeometry,
  ): void => {
    const vertexCount = body.positions.length / 3;
    const indexCount = body.indices.length;
    if (vertexCount === 0 || indexCount === 0) {
      geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(0), 3));
      geometry.setAttribute("normal", new THREE.BufferAttribute(new Float32Array(0), 3));
      geometry.setIndex(new THREE.BufferAttribute(new Uint32Array(0), 1));
      geometry.boundingBox = null;
      geometry.boundingSphere = null;
      return;
    }
    // Bodies from the worker are already main-thread-owned
    // TypedArrays (they arrived via the structured clone transfer
    // list), and fallback-mode bodies are already copies out of
    // WASM memory — either way, no extra slice() needed here.
    geometry.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
    geometry.setIndex(new THREE.BufferAttribute(body.indices, 1));
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  };

  // Poll the store's mesh cache once per frame. Steady state
  // (no mutation) is O(bodies) reads and no GPU traffic because
  // the per-body `version` gate rejects uploads for unchanged
  // bodies.
  useFrame(() => {
    if (!state) return;
    const bodies = state.meshedBodies;
    const seen = new Set<number>();
    let idSetChanged = false;

    for (const body of bodies) {
      seen.add(body.id);
      const existing = bodyEntriesRef.current.get(body.id);
      if (!existing) {
        const geometry = new THREE.BufferGeometry();
        uploadBody(body, geometry);
        bodyEntriesRef.current.set(body.id, { geometry, version: body.version });
        idSetChanged = true;
      } else if (existing.version !== body.version) {
        uploadBody(body, existing.geometry);
        existing.version = body.version;
      }
    }

    // Drop bodies that no longer exist in the store cache.
    for (const [id, entry] of bodyEntriesRef.current) {
      if (!seen.has(id)) {
        entry.geometry.dispose();
        bodyEntriesRef.current.delete(id);
        idSetChanged = true;
      }
    }

    if (idSetChanged) {
      setBodySetVersion((v) => v + 1);
    }
  });

  // Dispose everything on unmount.
  useEffect(() => {
    return () => {
      for (const entry of bodyEntriesRef.current.values()) {
        entry.geometry.dispose();
      }
      bodyEntriesRef.current.clear();
      material.dispose();
    };
  }, [material]);

  // Render one `<mesh>` per body. Depending on `bodySetVersion`
  // forces React to re-read `bodyEntriesRef.current.keys()` whenever
  // the id set mutates; version-only bumps (same ids, different
  // meshes) don't trigger a React render — the BufferGeometry
  // mutation in `useFrame` is picked up by R3F on the next paint.
  void bodySetVersion;
  const ids = Array.from(bodyEntriesRef.current.keys());
  return (
    <>
      {ids.map((id) => {
        const entry = bodyEntriesRef.current.get(id);
        if (!entry) return null;
        return (
          <mesh
            key={id}
            geometry={entry.geometry}
            material={material}
            frustumCulled={false}
          />
        );
      })}
    </>
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
