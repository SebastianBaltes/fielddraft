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
import { OrbitControls, OrthographicCamera } from "@react-three/drei";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { createKernelMeshMaterial } from "./meshMaterial";
import type { PickResult } from "./kernel";
import { SketchCanvas2d } from "./SketchCanvas2d";
import { publishSketchCamera } from "./sketchCameraBridge";
import { consumeViewCommand, VIEW_PRESETS } from "./viewCommands";

/** World-space size of the build-volume reference grid (mm). Matches
 *  the Onshape-style 300 mm cube budget, doubled for breathing room. */
const BUILD_VOLUME_SIZE = 600;

interface BodyEntry {
  geometry: THREE.BufferGeometry;
  material: THREE.Material;
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
  const sketchActive = state?.sketchSession.active ?? false;

  // K6b.3 opening: ghost all body materials while a sketch
  // session is active.
  useEffect(() => {
    for (const entry of bodyEntriesRef.current.values()) {
      const anyMat = entry.material as unknown as {
        transparent: boolean;
        opacity: number;
        depthWrite: boolean;
        needsUpdate?: boolean;
      };
      anyMat.transparent = sketchActive;
      anyMat.opacity = sketchActive ? 0.35 : 1;
      anyMat.depthWrite = !sketchActive;
      anyMat.needsUpdate = true;
    }
  }, [sketchActive]);

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

    for (let bi = 0; bi < bodies.length; bi++) {
      const body = bodies[bi];
      seen.add(body.id);
      const existing = bodyEntriesRef.current.get(body.id);
      if (!existing) {
        const geometry = new THREE.BufferGeometry();
        uploadBody(body, geometry);
        const mat = createKernelMeshMaterial(bi);
        bodyEntriesRef.current.set(body.id, { geometry, material: mat, version: body.version });
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
        entry.material.dispose();
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
        entry.material.dispose();
      }
      bodyEntriesRef.current.clear();
    };
  }, []);

  // Selection-highlight wireframe material (shared, lazy-init).
  const wireMatRef = useRef<THREE.MeshBasicMaterial | null>(null);
  if (!wireMatRef.current) {
    wireMatRef.current = new THREE.MeshBasicMaterial({
      wireframe: true,
      color: 0x3388ff,
      transparent: true,
      opacity: 0.35,
      depthTest: true,
      depthWrite: false,
    });
  }

  // Active body id for selection highlight.
  const { store: kernelStore } = useKernelStore();
  const activeBodyId = kernelStore?.kernel.activeBodyId() ?? 0;

  void bodySetVersion;
  const ids = Array.from(bodyEntriesRef.current.keys());
  return (
    <>
      {ids.map((id) => {
        const entry = bodyEntriesRef.current.get(id);
        if (!entry) return null;
        const isActive = id === activeBodyId && activeBodyId !== 0;
        return (
          <group key={id}>
            <mesh
              geometry={entry.geometry}
              material={entry.material}
              frustumCulled={false}
            />
            {isActive && wireMatRef.current ? (
              <mesh
                geometry={entry.geometry}
                material={wireMatRef.current}
                frustumCulled={false}
              />
            ) : null}
          </group>
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
      <CameraBridgePublisher />
      <PickProbe ndc={ndc} onPick={onPick} />
      <HitMarker result={pickResult} />
      {/* Build-volume floor reference. */}
      <gridHelper args={[BUILD_VOLUME_SIZE, 30, "#b8b8ae", "#d6d6ce"]} />
    </>
  );
}

/**
 * R3F component that lives inside the Canvas and republishes the
 * active orthographic camera's state to the module-level
 * `sketchCameraBridge` every frame. The 2D sketch canvas
 * overlay (a sibling DOM node of the `<Canvas>`) reads from the
 * bridge in its own RAF loop, so pan/zoom on the OrbitControls
 * stays perfectly in sync with the 2D geometry without any React
 * prop churn.
 *
 * When the sketch session is not active we still publish, but
 * the overlay never mounts, so the cost is a few numeric copies
 * per frame.
 */
function CameraBridgePublisher() {
  const { camera, size, gl } = useThree();
  useFrame(() => {
    const cam = camera as THREE.OrthographicCamera;
    // K6b.3 opening: plane is world XZ, so sketch coords map to
    // world (X, Z). Camera pan translates `position.x` and
    // `position.z` — those are our sketch-plane centre.
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

/**
 * Camera rig that swaps between the perspective orbit view (normal
 * document editing) and a top-down orthographic camera (sketch
 * mode). Only one `<OrbitControls>` is mounted at a time, so drei
 * never has to reconcile two "default" controls against each
 * other. When the sketch session exits, the perspective camera
 * comes back with its own orbit controls and the user picks up
 * where they left off.
 *
 * Orthographic mode: camera sits straight above the origin looking
 * down the -Y axis, so the world XZ plane maps 1:1 to the screen.
 * Orbit rotation is disabled — only pan and zoom are allowed —
 * which matches Onshape's sketch-mode expectation that you can't
 * tumble the camera while a sketch is active.
 */
/** Simple XZ ground plane grid using THREE.GridHelper (WebGPU safe). */
function WorldGrid() {
  const grid = useMemo(() => {
    const g = new THREE.GridHelper(600, 60, 0x999999, 0xdddddd);
    g.position.y = -0.1;
    (g.material as THREE.Material).transparent = true;
    (g.material as THREE.Material).opacity = 0.5;
    return g;
  }, []);
  return <primitive object={grid} />;
}

function ReferenceMesh() {
  const { state } = useKernelStore();
  const ref = useRef<THREE.Mesh>(null);
  const geometry = useMemo(() => new THREE.BufferGeometry(), []);

  useEffect(() => {
    const mesh = state?.referenceMesh;
    if (!mesh || mesh.positions.length === 0) {
      geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(0), 3));
      geometry.setIndex(null);
      return;
    }
    geometry.setAttribute("position", new THREE.BufferAttribute(mesh.positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(mesh.normals, 3));
    geometry.setIndex(new THREE.BufferAttribute(mesh.indices, 1));
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  }, [geometry, state?.referenceMesh]);

  if (!state?.referenceMesh) return null;

  return (
    <mesh ref={ref} geometry={geometry}>
      <meshStandardMaterial
        color="#8888cc"
        transparent
        opacity={0.4}
        depthWrite={false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

function ViewCommandConsumer() {
  const { camera, controls } = useThree();
  useFrame(() => {
    const cmd = consumeViewCommand();
    if (!cmd) return;
    const preset = VIEW_PRESETS[cmd];
    camera.position.set(...preset.position);
    camera.up.set(...preset.up);
    camera.lookAt(0, 0, 0);
    camera.updateProjectionMatrix();
    const orbit = controls as { target?: THREE.Vector3; update?: () => void } | null;
    if (orbit?.target) {
      orbit.target.set(0, 0, 0);
      orbit.update?.();
    }
  });
  return null;
}

function CameraRig() {
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
        <div className="error">
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

  const { store, state } = useKernelStore();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [ndc, setNdc] = useState<{ x: number; y: number; active: boolean }>({
    x: 0,
    y: 0,
    active: false,
  });
  const [pickResult, setPickResult] = useState<PickResult | null>(null);
  const [measurePoints, setMeasurePoints] = useState<
    { a: [number, number, number]; b?: [number, number, number] } | null
  >(null);

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

  const handleClick = (): void => {
    if (!store) return;

    // Measure mode: place points instead of selecting.
    if (state?.measureMode && pickResult?.hit) {
      const pt: [number, number, number] = [
        pickResult.point[0],
        pickResult.point[1],
        pickResult.point[2],
      ];
      if (!measurePoints || measurePoints.b) {
        // Start new measurement.
        setMeasurePoints({ a: pt });
      } else {
        // Complete the measurement.
        setMeasurePoints({ ...measurePoints, b: pt });
      }
      return;
    }

    if (!pickResult || !pickResult.hit) {
      store.clearSelection();
      setMeasurePoints(null);
      return;
    }
    const featureId = pickResult.contributors[0];
    if (featureId !== undefined && featureId > 0) {
      store.selectOnly(featureId);
    }
  };

  return (
    <div
      ref={hostRef}
      className="fd-viewport"
      data-testid="fielddraft-canvas"
      onPointerMove={handlePointerMove}
      onPointerLeave={handlePointerLeave}
      onClick={handleClick}
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
          <ReferenceMesh />
          <WorldGrid />
          <CameraRig />
          <ViewCommandConsumer />
        </Canvas>
      </CanvasErrorBoundary>
      {state?.sketchSession.active ? (
        <SketchCanvas2d store={store} session={state.sketchSession} />
      ) : null}
      <PickHud result={pickResult} />
      {measurePoints ? (
        <div className="measure-hud" data-testid="measure-hud">
          {measurePoints.b ? (
            <>
              {Math.hypot(
                measurePoints.b[0] - measurePoints.a[0],
                measurePoints.b[1] - measurePoints.a[1],
                measurePoints.b[2] - measurePoints.a[2],
              ).toFixed(2)}{" "}
              mm
            </>
          ) : (
            "Click second point…"
          )}
        </div>
      ) : null}
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
        className="pick-hud"
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
      className="pick-hud"
      data-testid="fielddraft-pick-hud"
      data-pick-hit="true"
      data-pick-element={result.element}
      data-pick-contributors={contributors}
    >
      <span className="kind">{result.element}</span>
      <span className="ids">{contributors}</span>
    </div>
  );
}
