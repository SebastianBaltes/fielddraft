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
import { Canvas, useFrame, useThree, type ThreeEvent } from "@react-three/fiber";
import { OrbitControls, OrthographicCamera } from "@react-three/drei";
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
      <SketchDrawingPlane />
      <SketchOverlay />
      <PickProbe ndc={ndc} onPick={onPick} />
      <HitMarker result={pickResult} />
      {/* Build-volume floor reference. */}
      <gridHelper args={[BUILD_VOLUME_SIZE, 30, "#b8b8ae", "#d6d6ce"]} />
    </>
  );
}

/** Y lift for the sketch overlay so it sits just above the grid
 *  helper without z-fighting. */
const SKETCH_PLANE_Y = 0.5;

/** Map a world-space hit on the XZ ground plane to 2D sketch
 *  coordinates. Mirrors the `(x, y) → (x, SKETCH_PLANE_Y, y)`
 *  lift applied by `SketchOverlay`. */
function worldHitToSketchXy(hit: THREE.Vector3): [number, number] {
  return [hit.x, hit.z];
}

/**
 * K6b.2 follow-up: visual preview of the active sketch session's
 * applied sub-spine. Each `sketch_point` becomes a dot on the
 * sketch plane, each `sketch_line` becomes a fat line segment
 * (drei `Line` with `LineMaterial`, so the stroke is actually
 * visible — vanilla `lineBasicMaterial.linewidth` is capped at
 * 1 px in every real browser). Hidden entirely when the session
 * is inactive.
 */
function SketchOverlay() {
  const { state } = useKernelStore();
  const session = state?.sketchSession;

  if (!session?.active) return null;

  const points: Array<{ key: number; pos: [number, number, number] }> = [];
  const lines: Array<{
    key: number;
    a: [number, number, number];
    b: [number, number, number];
  }> = [];

  session.subCalls.forEach((call, i) => {
    if (call.type === "sketch_point") {
      const at = call.params.at as unknown;
      if (Array.isArray(at) && at.length === 2) {
        points.push({
          key: i,
          pos: [Number(at[0]), SKETCH_PLANE_Y, Number(at[1])],
        });
      }
    } else if (call.type === "sketch_line") {
      const start = call.params.start as unknown;
      const end = call.params.end as unknown;
      if (
        Array.isArray(start) &&
        start.length === 2 &&
        Array.isArray(end) &&
        end.length === 2
      ) {
        lines.push({
          key: i,
          a: [Number(start[0]), SKETCH_PLANE_Y, Number(start[1])],
          b: [Number(end[0]), SKETCH_PLANE_Y, Number(end[1])],
        });
      }
    }
  });

  return (
    <group>
      {points.map(({ key, pos }) => (
        <mesh key={`p-${key}`} position={pos}>
          <sphereGeometry args={[1.2, 12, 12]} />
          <meshBasicMaterial
            color="#2563eb"
            depthTest={false}
            depthWrite={false}
          />
        </mesh>
      ))}
      {lines.map(({ key, a, b }) => (
        <SketchLineBox key={`l-${key}`} a={a} b={b} />
      ))}
    </group>
  );
}

/**
 * Line segment rendered as a stretched thin box. Avoids
 * drei / three-stdlib `LineMaterial`, which isn't compatible
 * with the WebGPURenderer's NodeMaterial pipeline in this
 * project. The box-proxy approach works with any renderer and
 * gives a visible stroke at small sketch scales where the
 * WebGL/WebGPU 1-pixel line cap would otherwise hide the line.
 */
function SketchLineBox({
  a,
  b,
}: {
  a: [number, number, number];
  b: [number, number, number];
}) {
  const { position, quaternion, length } = useMemo(() => {
    const start = new THREE.Vector3(...a);
    const end = new THREE.Vector3(...b);
    const mid = start.clone().add(end).multiplyScalar(0.5);
    const dir = end.clone().sub(start);
    const len = dir.length();
    const q = new THREE.Quaternion();
    if (len > 1e-6) {
      // The box geometry's default long axis is Y; rotate (0,1,0)
      // onto the line direction so the stretch matches.
      q.setFromUnitVectors(new THREE.Vector3(0, 1, 0), dir.clone().normalize());
    }
    return { position: mid, quaternion: q, length: len };
  }, [a, b]);

  if (length <= 0) return null;
  return (
    <mesh position={position} quaternion={quaternion}>
      <boxGeometry args={[0.8, length, 0.8]} />
      <meshBasicMaterial
        color="#2563eb"
        depthTest={false}
        depthWrite={false}
      />
    </mesh>
  );
}

/**
 * K6b.2 follow-up: invisible click target on the ground plane that
 * intercepts pointer events while a sketch tool is active. Click
 * once on the plane to place a point or start a line; click again
 * to finish the line. Orbit-drag is preserved because R3F only
 * fires `onClick` after a non-dragging pointer-up, so orbit
 * controls still own the drag path.
 */
function SketchDrawingPlane() {
  const { store, state } = useKernelStore();
  const session = state?.sketchSession;
  const [pendingStart, setPendingStart] = useState<
    [number, number] | null
  >(null);

  // Reset the line-start buffer whenever the user switches tools or
  // exits the session — otherwise a half-drawn line from a previous
  // tool would carry over.
  useEffect(() => {
    setPendingStart(null);
  }, [session?.selectedTool, session?.active]);

  if (!store || !session?.active || !session.selectedTool) return null;

  const handleClick = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    const at = worldHitToSketchXy(e.point);
    if (session.selectedTool === "sketch_point") {
      store.setSketchToolDraft({ at });
      store.applySketchToolDraft();
    } else if (session.selectedTool === "sketch_line") {
      if (!pendingStart) {
        setPendingStart(at);
      } else {
        store.setSketchToolDraft({ start: pendingStart, end: at });
        store.applySketchToolDraft();
        setPendingStart(null);
      }
    }
  };

  return (
    <mesh rotation={[-Math.PI / 2, 0, 0]} onClick={handleClick}>
      <planeGeometry args={[BUILD_VOLUME_SIZE, BUILD_VOLUME_SIZE]} />
      <meshBasicMaterial
        color="#ffffff"
        transparent
        opacity={0}
        depthWrite={false}
      />
    </mesh>
  );
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
      className="fd-viewport"
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
          <CameraRig />
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
