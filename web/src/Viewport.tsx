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
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import {
  OrbitControls,
  OrthographicCamera,
  TransformControls,
} from "@react-three/drei";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { createKernelMeshMaterial } from "./meshMaterial";
import type { PickResult } from "./kernel";
import { SketchCanvas2d } from "./SketchCanvas2d";
import { publishSketchCamera } from "./sketchCameraBridge";
import { consumeViewCommand, peekViewCommand, registerInvalidate, VIEW_PRESETS } from "./viewCommands";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";
import {
  publishPickCamera,
  getPublishedPickCamera,
} from "./pickCameraBridge";
import {
  createSdfBodyMaterial,
  disposeSdfBody,
  type SdfBodyHandle,
} from "./sdfRenderer";

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
  transformDelta: [number, number, number] | null;
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
  const invalidate = useThree((s) => s.invalidate);
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
        bodyEntriesRef.current.set(body.id, {
          geometry, material: mat, version: body.version,
          transformDelta: body.transformDelta,
        });
        idSetChanged = true;
      } else if (existing.version !== body.version) {
        if (body.transformDelta && !existing.transformDelta) {
          existing.transformDelta = body.transformDelta;
        } else {
          uploadBody(body, existing.geometry);
          existing.transformDelta = body.transformDelta;
        }
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

  // After the body set changes (add/remove), React re-renders to
  // mount/unmount <mesh> elements. With frameloop="demand" no frame
  // is auto-scheduled for the new scene graph, so we must invalidate.
  useEffect(() => {
    if (bodySetVersion > 0) invalidate();
  }, [bodySetVersion, invalidate]);

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

  useEffect(() => {
    return () => { wireMatRef.current?.dispose(); };
  }, []);

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
          <group
            key={id}
            name={`dc-body-${id}`}
            position={entry.transformDelta ?? undefined}
          >
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

interface SdfEntry {
  handle: SdfBodyHandle;
  version: number;
}

function setDcBodyVisible(scene: THREE.Scene, bodyId: number, visible: boolean) {
  const dcGroup = scene.getObjectByName(`dc-body-${bodyId}`);
  if (dcGroup) dcGroup.visible = visible;
}

const SDF_SETTLE_MS = 300;

function SdfOverlay() {
  const { store } = useKernelStore();
  const invalidate = useThree((s) => s.invalidate);
  const scene = useThree((s) => s.scene);
  const gl = useThree((s) => s.gl);
  const entriesRef = useRef<Map<number, SdfEntry>>(new Map());
  const debounceRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  // wgslFn requires the WebGPU backend — skip SDF rendering on WebGL fallback
  const isWebGPU = (gl as any).backend?.isWebGPUBackend === true;
  const loggedRef = useRef(false);
  if (!loggedRef.current) {
    loggedRef.current = true;
    console.log(
      isWebGPU
        ? "fielddraft: SdfOverlay — using SDF raymarching (WebGPU)"
        : "fielddraft: SdfOverlay — using DC meshes (WebGL fallback)",
    );
  }

  const rebuildSdf = useCallback(() => {
    if (!store) return;
    const kernel = store.kernel;
    const count = kernel.bodyCount();
    const seen = new Set<number>();

    for (let i = 0; i < count; i++) {
      const id = kernel.bodyId(i);
      const ver = kernel.bodyVersion(i);
      seen.add(id);

      const existing = entriesRef.current.get(id);
      if (existing && existing.version === ver) {
        existing.handle.mesh.visible = true;
        setDcBodyVisible(scene, id, false);
        continue;
      }

      const opcodes = kernel.bodySdfOpcodesView(i);
      const aabb = kernel.bodySdfAabb(i);
      if (opcodes.length === 0) continue;

      if (existing) {
        scene.remove(existing.handle.mesh);
        disposeSdfBody(existing.handle);
      }

      try {
        const handle = createSdfBodyMaterial(opcodes, aabb);
        scene.add(handle.mesh);
        entriesRef.current.set(id, { handle, version: ver });
        setDcBodyVisible(scene, id, false);
        console.log(`fielddraft: body ${id} → SDF raymarching (${opcodes.length} opcodes)`);
      } catch {
        setDcBodyVisible(scene, id, true);
      }
    }

    for (const [id, entry] of entriesRef.current) {
      if (!seen.has(id)) {
        scene.remove(entry.handle.mesh);
        disposeSdfBody(entry.handle);
        entriesRef.current.delete(id);
        setDcBodyVisible(scene, id, true);
      }
    }

    invalidate();
  }, [store, scene, invalidate]);

  useFrame(() => {
    if (!store || !isWebGPU) return;
    const kernel = store.kernel;
    const count = kernel.bodyCount();

    let anyDirty = false;
    const seen = new Set<number>();
    for (let i = 0; i < count; i++) {
      const id = kernel.bodyId(i);
      const ver = kernel.bodyVersion(i);
      seen.add(id);
      const existing = entriesRef.current.get(id);
      if (!existing || existing.version !== ver) {
        anyDirty = true;
        if (existing) existing.handle.mesh.visible = false;
        setDcBodyVisible(scene, id, true);
      }
    }
    for (const [id] of entriesRef.current) {
      if (!seen.has(id)) anyDirty = true;
    }

    if (!anyDirty) return;

    clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      rebuildSdf();
    }, SDF_SETTLE_MS);
  });

  useEffect(() => {
    return () => {
      clearTimeout(debounceRef.current);
      for (const [id, entry] of entriesRef.current.entries()) {
        scene.remove(entry.handle.mesh);
        disposeSdfBody(entry.handle);
        setDcBodyVisible(scene, id, true);
      }
      entriesRef.current.clear();
    };
  }, [scene]);

  return null;
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
      <SdfOverlay />
      <CameraBridgePublisher />
      <PickProbe ndc={ndc} onPick={onPick} />
      <HitMarker result={pickResult} />
      <SelectionTransformGizmo />
      <SketchOverlay3D />
      <Invalidator ndc={ndc} />
      {/* Build-volume floor reference. */}
      <gridHelper args={[BUILD_VOLUME_SIZE, 30, "#b8b8ae", "#d6d6ce"]} />
    </>
  );
}

/**
 * Render every sketch whose id is in `state.visibleSketches` as
 * a set of world-space line overlays on its sketch plane. Sketches
 * are invisible in the 3D viewport by default (the 2D editor is
 * the canonical authoring surface); turning on the eye icon in
 * the outliner flips membership in the store's visibleSketches
 * list, and this component renders the geometry by reading the
 * sketch's committed `sub_calls` directly from the kernel.
 *
 * Supported sub-call primitives in K7.22:
 *   - `sketch_line` — single segment
 *   - `sketch_rectangle` — 4-segment axis-aligned rectangle
 *   - `sketch_circle` — discretised circle (64 segments)
 *   - `sketch_polygon` — n-gon
 *   - `sketch_arc` — discretised arc
 * Points, constraints, and modifier tools (trim/mirror/offset)
 * are intentionally skipped — they don't contribute geometry.
 */
function SketchOverlay3D() {
  const { store, state } = useKernelStore();
  const visibleSketches = state?.visibleSketches;

  const geometries = useMemo(() => {
    if (!store || !visibleSketches || visibleSketches.length === 0) return [];
    const out: {
      id: number;
      geometry: THREE.BufferGeometry;
    }[] = [];
    for (const sketchId of visibleSketches) {
      try {
        const params = store.kernel.getFeatureCallParams(sketchId) as {
          plane?: string;
          sub_calls?: Array<{ type: string; params: Record<string, unknown> }>;
        };
        const plane = parseSketchPlane(params.plane ?? "");
        const points = collectSketchLineSegments(params.sub_calls ?? [], plane);
        if (points.length === 0) continue;
        const geometry = new THREE.BufferGeometry();
        geometry.setAttribute(
          "position",
          new THREE.Float32BufferAttribute(points, 3),
        );
        out.push({ id: sketchId, geometry });
      } catch {
        // Kernel rejected the id (not a sketch, deleted, etc.) —
        // silently skip; the outliner will clear its eye toggle
        // on the next commit if the sketch is gone.
      }
    }
    return out;
  }, [store, visibleSketches, state?.features]);

  // Dispose old geometries when the list changes.
  useEffect(() => {
    return () => {
      for (const g of geometries) g.geometry.dispose();
    };
  }, [geometries]);

  if (geometries.length === 0) return null;

  return (
    <group>
      {geometries.map(({ id, geometry }) => (
        <lineSegments key={id} geometry={geometry}>
          <lineBasicMaterial color="#2563eb" linewidth={2} />
        </lineSegments>
      ))}
    </group>
  );
}

/** Sketch plane descriptor — two orthonormal axes in world space
 *  that the sketch's 2D (u, v) coordinates map onto. */
type SketchPlane = {
  u: [number, number, number];
  v: [number, number, number];
};

function parseSketchPlane(query: string): SketchPlane {
  // K7.22: only the three origin planes are supported here. Any
  // other query (e.g. `qFaceOf(...)`) falls back to XY so the
  // sketch at least renders instead of disappearing silently.
  if (query.includes("origin_plane_xz")) {
    return { u: [1, 0, 0], v: [0, 0, 1] };
  }
  if (query.includes("origin_plane_yz")) {
    return { u: [0, 1, 0], v: [0, 0, 1] };
  }
  // Default: xy plane (z = 0).
  return { u: [1, 0, 0], v: [0, 1, 0] };
}

/** Walk a sketch's committed sub_calls and emit a flat Float32
 *  array of line segment endpoints (pairs of xyz). Each sub-call
 *  that produces closed geometry (rectangle, circle, polygon,
 *  arc) is expanded into its own segment list; lines are a
 *  single segment. Unknown or modifier sub-calls are skipped. */
function collectSketchLineSegments(
  subCalls: Array<{ type: string; params: Record<string, unknown> }>,
  plane: SketchPlane,
): number[] {
  const out: number[] = [];
  const emit = (a: [number, number], b: [number, number]) => {
    const ax = plane.u[0] * a[0] + plane.v[0] * a[1];
    const ay = plane.u[1] * a[0] + plane.v[1] * a[1];
    const az = plane.u[2] * a[0] + plane.v[2] * a[1];
    const bx = plane.u[0] * b[0] + plane.v[0] * b[1];
    const by = plane.u[1] * b[0] + plane.v[1] * b[1];
    const bz = plane.u[2] * b[0] + plane.v[2] * b[1];
    out.push(ax, ay, az, bx, by, bz);
  };

  const getVec2 = (v: unknown): [number, number] | null => {
    if (Array.isArray(v) && v.length >= 2 && typeof v[0] === "number" && typeof v[1] === "number") {
      return [v[0], v[1]];
    }
    return null;
  };
  const getNum = (v: unknown): number | null =>
    typeof v === "number" ? v : null;

  for (const call of subCalls) {
    const p = call.params;
    switch (call.type) {
      case "sketch_line": {
        const a = getVec2(p.start);
        const b = getVec2(p.end);
        if (a && b) emit(a, b);
        break;
      }
      case "sketch_rectangle": {
        const a = getVec2(p.corner_a);
        const b = getVec2(p.corner_b);
        if (a && b) {
          const x0 = Math.min(a[0], b[0]);
          const x1 = Math.max(a[0], b[0]);
          const y0 = Math.min(a[1], b[1]);
          const y1 = Math.max(a[1], b[1]);
          emit([x0, y0], [x1, y0]);
          emit([x1, y0], [x1, y1]);
          emit([x1, y1], [x0, y1]);
          emit([x0, y1], [x0, y0]);
        }
        break;
      }
      case "sketch_circle": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        if (c && r !== null && r > 0) {
          const steps = 64;
          let prev: [number, number] = [c[0] + r, c[1]];
          for (let i = 1; i <= steps; i++) {
            const t = (i / steps) * Math.PI * 2;
            const next: [number, number] = [
              c[0] + r * Math.cos(t),
              c[1] + r * Math.sin(t),
            ];
            emit(prev, next);
            prev = next;
          }
        }
        break;
      }
      case "sketch_polygon": {
        const verts = p.vertices;
        if (Array.isArray(verts) && verts.length >= 4) {
          const points: [number, number][] = [];
          for (let i = 0; i + 1 < verts.length; i += 2) {
            const x = Number(verts[i]);
            const y = Number(verts[i + 1]);
            if (Number.isFinite(x) && Number.isFinite(y)) {
              points.push([x, y]);
            }
          }
          for (let i = 0; i < points.length; i++) {
            const a = points[i];
            const b = points[(i + 1) % points.length];
            emit(a, b);
          }
        }
        break;
      }
      case "sketch_arc": {
        const c = getVec2(p.center);
        const r = getNum(p.radius);
        const a0 = getNum(p.start_angle);
        const sweep = getNum(p.sweep_angle);
        if (c && r !== null && r > 0 && a0 !== null && sweep !== null) {
          const a1 = a0 + sweep;
          const steps = Math.max(8, Math.ceil((Math.abs(a1 - a0) / Math.PI) * 32));
          let prev: [number, number] = [
            c[0] + r * Math.cos(a0),
            c[1] + r * Math.sin(a0),
          ];
          for (let i = 1; i <= steps; i++) {
            const t = a0 + ((a1 - a0) * i) / steps;
            const next: [number, number] = [
              c[0] + r * Math.cos(t),
              c[1] + r * Math.sin(t),
            ];
            emit(prev, next);
            prev = next;
          }
        }
        break;
      }
      // sketch_point / sketch_trim / sketch_mirror / sketch_offset /
      // sketch_constraint_* — no line geometry, skip.
      default:
        break;
    }
  }
  return out;
}

/**
 * Drag-to-translate gizmo for the currently selected body.
 *
 * Mounts a drei `<TransformControls>` attached to an invisible
 * placeholder mesh positioned at the selected feature's translate.
 * While the user drags the gizmo handle, the placeholder moves
 * freely on the GPU side; on `mouseUp` we push the final world-
 * space position back into the kernel via `setFeatureTranslate`,
 * which triggers a re-mesh of the affected body. Rotation and
 * scale handles are disabled in this first cut — the user's
 * request only asked for translate, and the kernel's transform
 * snap semantics for rotation on non-primitive features still
 * need a separate slice to land cleanly.
 *
 * Gizmo vs. OrbitControls interference is handled automatically
 * by drei's `<TransformControls>`: while a gizmo handle is being
 * dragged, drei disables the default OrbitControls instance so
 * the camera doesn't move with the drag.
 */
function SelectionTransformGizmo() {
  const { store, state } = useKernelStore();
  const meshRef = useRef<THREE.Mesh>(null);
  // The latest *committed* translate for the selected feature.
  // We only re-sync the placeholder mesh's position when this
  // tuple changes — synchronising on every store state update
  // would race with the TransformControls' own writes mid-drag.
  const syncedTranslateRef = useRef<[number, number, number] | null>(null);

  // Resolve the single selected feature. The gizmo only shows
  // for single-selection; multi-select and no-selection both
  // hide it.
  const selectedFeature = useMemo(() => {
    if (!state || state.selection.size !== 1) return undefined;
    const first = state.selection.values().next().value;
    if (first === undefined) return undefined;
    return state.features.find((f) => f.id === first);
  }, [state]);

  // Sync the placeholder mesh with the feature's translate when:
  //   (a) a different feature is selected (id change)
  //   (b) the translate changed externally — via undo/redo, an
  //       arrow-key nudge, or the feature dialog — rather than
  //       from the gizmo itself.
  // We compare the incoming translate against `syncedTranslateRef`
  // so the onChange path below (which writes the placeholder
  // position and also commits to the kernel, resulting in a new
  // store state) doesn't clobber its own work during a drag.
  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature) {
      syncedTranslateRef.current = null;
      return;
    }
    const [tx, ty, tz] = selectedFeature.translate;
    const prev = syncedTranslateRef.current;
    // Compare the committed translate against the last one we
    // ourselves committed. If they match, the state change came
    // *from* us (the gizmo) and we've already moved the mesh —
    // no need to re-write it. If they differ, the source is
    // external (undo, nudge, dialog) and we snap the placeholder.
    if (!prev || prev[0] !== tx || prev[1] !== ty || prev[2] !== tz) {
      mesh.position.set(tx, ty, tz);
      syncedTranslateRef.current = [tx, ty, tz];
    }
  }, [selectedFeature]);

  if (!selectedFeature || !store) return null;

  // Live commit on every gizmo drag tick. drei's `onChange` fires
  // continuously while the user drags a handle, so the kernel
  // feature translate follows the mesh in real time and the
  // worker re-meshes each step — the user sees the body move
  // under the gizmo instead of snapping at mouseUp. The
  // `syncedTranslateRef` bookkeeping above keeps the feedback
  // loop stable: when our own commit bubbles back as a new
  // `selectedFeature` object, the useEffect recognises the
  // translate as self-originated and skips the mesh reset.
  const gizmoMode = state?.gizmoMode ?? "translate";

  const handleGizmoChange = () => {
    const mesh = meshRef.current;
    if (!mesh) return;

    if (gizmoMode === "translate") {
      const next: [number, number, number] = [
        mesh.position.x,
        mesh.position.y,
        mesh.position.z,
      ];
      syncedTranslateRef.current = next;
      store.setFeatureTranslate(selectedFeature.id, next[0], next[1], next[2]);
    } else if (gizmoMode === "rotate") {
      const q = mesh.quaternion;
      store.setFeatureRotation(selectedFeature.id, q.x, q.y, q.z, q.w);
    } else if (gizmoMode === "scale") {
      store.setFeatureScale(selectedFeature.id, mesh.scale.x);
    }
  };

  return (
    <>
      <mesh ref={meshRef} visible={false}>
        <boxGeometry args={[1, 1, 1]} />
      </mesh>
      {meshRef.current ? (
        <TransformControls
          object={meshRef.current}
          mode={gizmoMode}
          size={0.75}
          onObjectChange={handleGizmoChange}
        />
      ) : null}
    </>
  );
}

/**
 * On-demand frame-loop orchestrator.
 *
 * The Canvas runs with `frameloop="demand"` so the browser tab stays
 * idle when nothing changes. OrbitControls and other drei helpers call
 * `invalidate()` themselves on user interaction; we only need to
 * explicitly re-trigger a render when something *outside* the Canvas
 * changes — new body meshes from the worker, or a pointer-move that
 * updates the hover pick. Both come in through props/store and are
 * watched here via `useEffect`.
 */
function Invalidator({ ndc }: { ndc: { x: number; y: number; active: boolean } }) {
  const invalidate = useThree((s) => s.invalidate);
  const { state } = useKernelStore();
  const meshedBodies = state?.meshedBodies;
  const sketchActive = state?.sketchSession.active ?? false;
  const selection = state?.selection;
  // K7.22 follow-up: toggling the sketch overlay must redraw the
  // canvas on a demand-frameloop. Without this dep, the scene
  // graph was updated correctly (SketchOverlay3D removed its
  // <lineSegments>) but no frame was ever drawn to the GPU, so
  // the hide-sketch click appeared to do nothing until the next
  // unrelated interaction (hover, camera drag) redrew the canvas.
  const visibleSketches = state?.visibleSketches;

  useEffect(() => {
    invalidate();
  }, [
    invalidate,
    meshedBodies,
    sketchActive,
    selection,
    visibleSketches,
    ndc.x,
    ndc.y,
    ndc.active,
  ]);

  return null;
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

  // K7.22: publish the full camera ref for synchronous click-picks
  // (the viewport host's onClick reads it). Must be kept in sync
  // whenever R3F re-creates or swaps the camera (e.g. sketch-mode
  // ortho → perspective round-trips).
  useEffect(() => {
    publishPickCamera(camera);
    return () => {
      publishPickCamera(null);
    };
  }, [camera]);

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
/** K10.6: render B-Rep edges as thin dark LineSegments.
 *  Cached — only recomputes when the mesh version changes. */
function EdgeOverlay() {
  const { store, state } = useKernelStore();
  const geometryRef = useRef(new THREE.BufferGeometry());
  const materialRef = useRef(
    new THREE.LineBasicMaterial({ color: 0x222222, linewidth: 1 }),
  );
  const cachedVersionRef = useRef(-1);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!store || !state || state.meshedBodies.length === 0) {
      setReady(false);
      return;
    }
    const version = store.kernel.meshVersion();
    if (version === cachedVersionRef.current) return;
    cachedVersionRef.current = version;
    try {
      const edges = store.kernel.detectEdges();
      if (edges.length === 0) {
        setReady(false);
        return;
      }
      const positions = new Float32Array(edges.length * 6);
      for (let i = 0; i < edges.length; i++) {
        positions[i * 6] = edges[i].p0[0];
        positions[i * 6 + 1] = edges[i].p0[1];
        positions[i * 6 + 2] = edges[i].p0[2];
        positions[i * 6 + 3] = edges[i].p1[0];
        positions[i * 6 + 4] = edges[i].p1[1];
        positions[i * 6 + 5] = edges[i].p1[2];
      }
      geometryRef.current.setAttribute(
        "position",
        new THREE.BufferAttribute(positions, 3),
      );
      geometryRef.current.computeBoundingSphere();
      setReady(true);
    } catch {
      setReady(false);
    }
  }, [store, state?.meshedBodies]);

  useEffect(() => {
    return () => {
      geometryRef.current.dispose();
      materialRef.current.dispose();
    };
  }, []);

  if (!ready) return null;

  return (
    <lineSegments
      geometry={geometryRef.current}
      material={materialRef.current}
      frustumCulled={false}
    />
  );
}

/** K9.6: render mate connectors as small axis triads. */
/** P1: render assembly part instances as positioned mesh groups. */
function AssemblyPartBody({ body, isSelected }: {
  body: { positions: Float32Array; normals: Float32Array; indices: Uint32Array };
  isSelected: boolean;
}) {
  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(body.positions, 3));
    geom.setAttribute("normal", new THREE.BufferAttribute(body.normals, 3));
    geom.setIndex(new THREE.BufferAttribute(body.indices, 1));
    geom.computeBoundingSphere();
    return geom;
  }, [body.positions, body.normals, body.indices]);

  useEffect(() => {
    return () => geometry.dispose();
  }, [geometry]);

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color={isSelected ? "#6366f1" : "#94a3b8"}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

function AssemblyParts() {
  const { state } = useKernelStore();
  const assembly = state?.assembly;
  if (!assembly?.active || assembly.document.parts.length === 0) return null;

  return (
    <>
      {assembly.document.parts.map((part) => {
        const rot = part.transform.rotate.map((d: number) => (d * Math.PI) / 180) as [number, number, number];
        return (
          <group
            key={part.id}
            position={part.transform.translate}
            rotation={rot}
          >
            {part.meshBodies.map((body, i: number) => (
              <AssemblyPartBody
                key={`${part.id}-body-${i}`}
                body={body}
                isSelected={part.id === assembly.selectedPartId}
              />
            ))}
          </group>
        );
      })}
    </>
  );
}

function MateConnectorOverlay() {
  const { store, state } = useKernelStore();
  const groupRef = useRef<THREE.Group>(null);

  useEffect(() => {
    const group = groupRef.current;
    if (!group || !state) return;
    // Clear previous children — dispose GPU resources to prevent leaks.
    while (group.children.length > 0) {
      const child = group.children[0];
      group.remove(child);
      if (child instanceof THREE.LineSegments) {
        child.geometry.dispose();
        (child.material as THREE.Material).dispose();
      }
    }
    // Find mate_connector features.
    const connectors = state.features.filter(
      (f) => f.kind === "mate_connector",
    );
    if (connectors.length === 0 || !store) return;
    for (const c of connectors) {
      try {
        const params = store.kernel.getFeatureCallParams(c.id);
        const origin = (params as { origin?: number[] }).origin ?? [0, 0, 0];
        const normal = (params as { normal?: number[] }).normal ?? [0, 1, 0];
        const size = 15;

        // Z axis = normal (blue).
        const nLen = Math.hypot(normal[0], normal[1], normal[2]) || 1;
        const nz = [normal[0] / nLen, normal[1] / nLen, normal[2] / nLen];
        // X axis = cross(up, nz) or fallback.
        let up = [0, 1, 0];
        if (Math.abs(nz[1]) > 0.99) up = [1, 0, 0];
        const nx = [
          up[1] * nz[2] - up[2] * nz[1],
          up[2] * nz[0] - up[0] * nz[2],
          up[0] * nz[1] - up[1] * nz[0],
        ];
        const nxLen = Math.hypot(nx[0], nx[1], nx[2]) || 1;
        nx[0] /= nxLen; nx[1] /= nxLen; nx[2] /= nxLen;
        // Y axis = cross(nz, nx).
        const ny = [
          nz[1] * nx[2] - nz[2] * nx[1],
          nz[2] * nx[0] - nz[0] * nx[2],
          nz[0] * nx[1] - nz[1] * nx[0],
        ];

        const axes: [number[], number][] = [
          [nx, 0xe74c3c], // X = red
          [ny, 0x2ecc71], // Y = green
          [nz, 0x3498db], // Z = blue
        ];
        for (const [dir, color] of axes) {
          const positions = new Float32Array([
            origin[0], origin[1], origin[2],
            origin[0] + dir[0] * size,
            origin[1] + dir[1] * size,
            origin[2] + dir[2] * size,
          ]);
          const geo = new THREE.BufferGeometry();
          geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
          const mat = new THREE.LineBasicMaterial({ color });
          const line = new THREE.LineSegments(geo, mat);
          group.add(line);
        }
      } catch {
        // Skip if params can't be read.
      }
    }
    return () => {
      if (!group) return;
      while (group.children.length > 0) {
        const child = group.children[0];
        group.remove(child);
        if (child instanceof THREE.LineSegments) {
          child.geometry.dispose();
          (child.material as THREE.Material).dispose();
        }
      }
    };
  }, [store, state?.features]);

  return <group ref={groupRef} />;
}

function ClearColorSync() {
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
    return () => { geometry.dispose(); };
  }, [geometry]);

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
  const controlsRef = useRef<THREE.EventDispatcher | null>(null);
  const invalidate = useThree((s) => s.invalidate);
  useEffect(() => {
    registerInvalidate(invalidate);
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

  const handleClick = (event: React.MouseEvent<HTMLDivElement>): void => {
    if (!store) return;

    // K7.22 follow-up: re-do the pick *synchronously* at the
    // exact click coordinates, using the camera the
    // CameraBridgePublisher has been posting to the pickCameraBridge
    // module. Relying on `pickResult` (React state, fed by the
    // async `PickProbe.useFrame`) is wrong under `frameloop="demand"`:
    // the last frame may not have run between the final pointer
    // move and the click, so `pickResult` is stale or null and the
    // click either picks the wrong body or selects nothing. A
    // synchronous `kernel.pick` at the true click NDC is
    // authoritative.
    const host = hostRef.current;
    const camera = getPublishedPickCamera();
    let click: PickResult | null = null;
    if (host && camera) {
      const rect = host.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        const cx = ((event.clientX - rect.left) / rect.width) * 2 - 1;
        const cy = -(((event.clientY - rect.top) / rect.height) * 2 - 1);
        const raycaster = new THREE.Raycaster();
        raycaster.setFromCamera(new THREE.Vector2(cx, cy), camera);
        const o = raycaster.ray.origin;
        const d = raycaster.ray.direction;
        click = store.kernel.pick(
          [o.x, o.y, o.z],
          [d.x, d.y, d.z],
        );
        // Mirror the fresh pick into the HUD so downstream readers
        // (measure-mode preview, debug overlay) stay in sync.
        setPickResult(click);
      }
    }

    // K7.22 UX-fix: query-picker has priority over every other
    // click mode. If a FeatureDialog query widget has armed itself
    // via `queryPickerBridge.register`, convert the current pick
    // result into a `qCreatedBy(feature_id)` expression and hand
    // it to the widget instead of mutating the selection.
    const picker = getActiveQueryPicker();
    if (picker && click?.hit && click.contributors.length > 0) {
      const contributor = click.contributors[0];
      if (contributor > 0) {
        const query = `qCreatedBy(${contributor})`;
        const consumed = picker.onPick(query);
        if (consumed) return;
      }
    }

    // K7.21e: face-pick mode — click a face to set the sketch plane.
    if (state?.facePickMode) {
      if (click?.hit && click.contributors.length > 0) {
        store.enterSketchFromFacePick(click.normal as [number, number, number]);
      } else {
        store.cancelFacePickMode();
        store.enterSketch("qCreatedBy(origin_plane_xy)");
      }
      return;
    }

    // P2: IK mode — click sets the IK target position.
    if (state?.ikMode && state?.assembly.active && click?.hit) {
      const revMates = state.assembly.document.mates.filter((m: { type: string }) => m.type === "revolute");
      if (revMates.length > 0) {
        const lastPart = state.assembly.document.parts[state.assembly.document.parts.length - 1];
        if (lastPart && lastPart.mateConnectors.length > 0) {
          store.runIK({
            position: click.point as [number, number, number],
            endEffectorPartId: lastPart.id,
            endEffectorConnectorId: lastPart.mateConnectors[0].featureId,
          });
        }
      }
      return;
    }

    // K9: mate mode — click a surface to place a mate connector.
    if (state?.mateMode && click?.hit) {
      store.addMateConnectorAtPick(
        click.point as [number, number, number],
        click.normal as [number, number, number],
      );
      return;
    }

    // Fillet mode: click on a surface → find nearest edge →
    // create fillet_edge feature.
    if (state?.filletMode) {
      if (!click?.hit) return;
      if (click.contributors.length < 2) {
        store.showTransientError("Fillet requires an edge between two features — click where two bodies meet.");
        return;
      }
      const a = click.contributors[0];
      const b = click.contributors[1];
      if (a > 0 && b > 0) {
        const pt = click.point;
        store.addFilletAtEdge(
          a,
          b,
          [pt[0] - 20, pt[1] - 20, pt[2] - 20],
          [pt[0] + 20, pt[1] + 20, pt[2] + 20],
          3,
        );
      }
      return;
    }

    // Measure mode: place points instead of selecting.
    if (state?.measureMode && click?.hit) {
      const pt: [number, number, number] = [
        click.point[0],
        click.point[1],
        click.point[2],
      ];
      if (!measurePoints || measurePoints.b) {
        setMeasurePoints({ a: pt });
      } else {
        setMeasurePoints({ ...measurePoints, b: pt });
      }
      return;
    }

    if (!click || !click.hit) {
      store.clearSelection();
      setMeasurePoints(null);
      return;
    }
    const featureId = click.contributors[0];
    if (featureId !== undefined && featureId > 0) {
      store.selectOnly(featureId);
    }
  };

  const handleDragOver = (e: React.DragEvent<HTMLDivElement>): void => {
    // Allow drop — the default is to disallow.
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  };

  const handleDrop = (e: React.DragEvent<HTMLDivElement>): void => {
    e.preventDefault();
    if (!store) return;
    const kind = e.dataTransfer.getData("fielddraft/primitive");
    if (kind) {
      type PrimName = "box" | "sphere" | "cylinder" | "torus" | "cone" | "capsule" | "hex_prism" | "ellipsoid";
      const valid: PrimName[] = ["box", "sphere", "cylinder", "torus", "cone", "capsule", "hex_prism", "ellipsoid"];
      // Place the new primitive where the user dropped the icon.
      // For drops on empty space, translate is undefined → origin.
      const translate: [number, number, number] | undefined = pickResult?.hit
        ? [pickResult.point[0], pickResult.point[1], pickResult.point[2]]
        : undefined;
      if (valid.includes(kind as PrimName)) {
        store.addPrimitive(kind as PrimName, undefined, translate);
      } else if (kind === "hole") {
        store.addHole();
      } else if (kind === "gyroid") {
        store.addGyroid();
      }
      return;
    }
    // K14: drag-sketch-on-face — when a sketch is dropped onto
    // a surface, start a sketch with the plane aligned to the
    // face normal at the last pick point.
    const sketchDrop = e.dataTransfer.getData("fielddraft/sketch");
    if (sketchDrop && pickResult?.hit) {
      const n = pickResult.normal;
      // Pick the closest standard plane based on the normal.
      const ax = Math.abs(n[0]);
      const ay = Math.abs(n[1]);
      const az = Math.abs(n[2]);
      let plane = "qCreatedBy(origin_plane_xy)";
      if (ax > ay && ax > az) {
        plane = "qCreatedBy(origin_plane_yz)";
      } else if (az > ay) {
        plane = "qCreatedBy(origin_plane_xz)";
      }
      store.enterSketch(plane);
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
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      <CanvasErrorBoundary>
        <Canvas
          shadows={false}
          frameloop="demand"
          camera={{ position: [350, 280, 350], fov: 45, near: 1, far: 4000 }}
          gl={async (props) => {
            const { WebGPURenderer } = await import("three/webgpu");
            const renderer = new WebGPURenderer({
              ...(props as object),
              antialias: true,
            });
            await renderer.init();
            const gpuBackend = (renderer as any).backend?.isWebGPUBackend === true;
            if (gpuBackend) {
              console.log(
                "fielddraft: WebGPU backend active — SDF raymarching enabled.",
              );
            } else {
              console.warn(
                "fielddraft: WebGPURenderer fell back to WebGL backend. " +
                "SDF raymarching is disabled — DC meshes will be used instead.",
              );
            }
            return renderer;
          }}
          onCreated={(rootState) => {
            rootState.invalidate();
          }}
        >
          <ClearColorSync />
          <Scene ndc={ndc} onPick={setPickResult} pickResult={pickResult} />
          <ReferenceMesh />
          <EdgeOverlay />
          <MateConnectorOverlay />
          <AssemblyParts />
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
      {state?.facePickMode && (
        <div className="face-pick-hud" data-testid="face-pick-hud">
          Click a face to set sketch plane — or press Esc for XY
        </div>
      )}
      {state?.mateMode && (
        <div className="face-pick-hud" data-testid="mate-mode-hud">
          Click a surface to place a mate connector
        </div>
      )}
      {pickResult?.hit && pickResult.element === "face" && pickResult.contributors.length === 1 && state?.selection.size === 1 && !state?.filletMode && !state?.mateMode && !state?.measureMode && !state?.sketchSession.active && (
        <button
          type="button"
          className="extrude-face-btn"
          data-testid="extrude-face-btn"
          onClick={() => {
            store?.extrudeFace(
              pickResult.point as [number, number, number],
              pickResult.normal as [number, number, number],
            );
          }}
        >
          ↥ Extrude Face
        </button>
      )}
      {state?.ikMode && (
        <div className="face-pick-hud" data-testid="ik-mode-hud">
          Click to set IK target position
        </div>
      )}
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
    // No visible HUD while idle — the overlay used to print "no hover"
    // as a debug readout, but that confused users into thinking it was
    // a button. The `data-testid` marker stays so smoke tests that
    // assert the HUD exists keep working.
    return (
      <div
        className="pick-hud is-idle"
        data-testid="fielddraft-pick-hud"
        data-pick-hit="false"
        aria-hidden="true"
      />
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
