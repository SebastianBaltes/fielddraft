import { useEffect, useMemo, useRef, useState } from "react";
import { useThree } from "@react-three/fiber";
import { TransformControls as ThreeTransformControls } from "three/examples/jsm/controls/TransformControls.js";
import * as THREE from "three";
import { useKernelStore, type StoreState } from "./store";
import type { FeatureSummary, PickResult } from "./kernel";
import { PerfProbe } from "./perfMonitor";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import { resolveMaterials, type MaterialProps } from "./materialBridge";
import { BODY_COLORS } from "./sdfDeferredRenderer";

export function findSingleSelectedFeature(state: StoreState | null | undefined): FeatureSummary | undefined {
  if (!state || state.selection.size !== 1) return undefined;
  const first = state.selection.values().next().value;
  if (first === undefined) return undefined;
  return state.features.find((f) => f.id === first);
}

type SketchPlane = {
  u: [number, number, number];
  v: [number, number, number];
};

function parseSketchPlane(query: string): SketchPlane {
  if (query.includes("origin_plane_xz")) {
    return { u: [1, 0, 0], v: [0, 0, 1] };
  }
  if (query.includes("origin_plane_yz")) {
    return { u: [0, 1, 0], v: [0, 0, 1] };
  }
  return { u: [1, 0, 0], v: [0, 1, 0] };
}

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
      default:
        break;
    }
  }
  return out;
}

export function SketchOverlay3D() {
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
        // Skip invalid sketches
      }
    }
    return out;
  }, [store, visibleSketches, state?.features]);

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

const NO_GIZMO_KINDS: ReadonlySet<string> = new Set([
  "boolean", "sketch", "fillet_edge",
]);

export function SelectionTransformGizmo() {
  const { store, state } = useKernelStore();
  const meshRef = useRef<THREE.Mesh>(null);
  const tcRef = useRef<ThreeTransformControls | null>(null);
  const syncedTranslateRef = useRef<[number, number, number] | null>(null);
  const gizmoThrottleRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  const gizmoPendingRef = useRef(false);

  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const scene = useThree((s) => s.scene);
  const defaultControls = useThree((s) => s.controls);
  const invalidate = useThree((s) => s.invalidate);

  const selectedFeature = useMemo(() => findSingleSelectedFeature(state), [state]);

  const gizmoMode = state?.gizmoMode ?? "translate";

  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature) {
      syncedTranslateRef.current = null;
      return;
    }
    const [tx, ty, tz] = selectedFeature.translate;
    const prev = syncedTranslateRef.current;
    if (!prev || prev[0] !== tx || prev[1] !== ty || prev[2] !== tz) {
      mesh.position.set(tx, ty, tz);
      syncedTranslateRef.current = [tx, ty, tz];
    }
    const [rx, ry, rz, rw] = selectedFeature.rotation;
    mesh.quaternion.set(rx, ry, rz, rw);
    const [scx, scy, scz] = selectedFeature.scale;
    mesh.scale.set(scx, scy, scz);
  }, [selectedFeature]);

  const commitRef = useRef<() => void>(() => {});
  commitRef.current = () => {
    const mesh = meshRef.current;
    if (!mesh || !store || !selectedFeature) return;
    gizmoPendingRef.current = false;
    const p = mesh.position;
    const q = mesh.quaternion;
    let sx = mesh.scale.x, sy = mesh.scale.y, sz = mesh.scale.z;
    if (gizmoMode === "translate") {
      syncedTranslateRef.current = [p.x, p.y, p.z];
    }
    if (gizmoMode === "scale" && selectedFeature.kind === "sphere") {
      const u = Math.max(Math.abs(sx), Math.abs(sy), Math.abs(sz));
      sx = u; sy = u; sz = u;
      mesh.scale.set(u, u, u);
    }
    if (gizmoMode === "translate") {
      store.applyGizmoTranslate(selectedFeature.id, p.x, p.y, p.z);
    } else if (gizmoMode === "rotate") {
      store.applyGizmoRotate(selectedFeature.id, q.x, q.y, q.z, q.w);
    } else {
      store.applyGizmoScale(selectedFeature.id, sx, sy, sz);
    }
  };

  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || !selectedFeature || !store) {
      if (tcRef.current) {
        tcRef.current.detach();
        scene.remove(tcRef.current.getHelper() as THREE.Object3D);
        tcRef.current.dispose();
        tcRef.current = null;
      }
      return;
    }

    const tc = new ThreeTransformControls(camera, gl.domElement);
    const helper = tc.getHelper() as THREE.Object3D;
    tc.attach(mesh);
    tc.setMode(gizmoMode);
    tc.setSize(0.5);
    helper.traverse((child: THREE.Object3D) => child.layers.enable(OVERLAY_LAYER));

    const onDragging = (e: { value: boolean }) => {
      if (defaultControls) (defaultControls as any).enabled = !e.value;
    };
    const onChange = () => invalidate();
    const onObjectChange = () => {
      gizmoPendingRef.current = true;
      if (gizmoThrottleRef.current !== undefined) return;
      commitRef.current();
      gizmoThrottleRef.current = setTimeout(() => {
        gizmoThrottleRef.current = undefined;
        if (gizmoPendingRef.current) commitRef.current();
      }, 50);
    };

    tc.addEventListener("dragging-changed", onDragging as any);
    tc.addEventListener("change", onChange);
    tc.addEventListener("objectChange", onObjectChange);
    scene.add(helper);
    tcRef.current = tc;
    invalidate();

    return () => {
      clearTimeout(gizmoThrottleRef.current);
      gizmoThrottleRef.current = undefined;
      tc.removeEventListener("dragging-changed", onDragging as any);
      tc.removeEventListener("change", onChange);
      tc.removeEventListener("objectChange", onObjectChange);
      tc.detach();
      scene.remove(helper);
      tc.dispose();
      tcRef.current = null;
    };
  }, [selectedFeature?.id, gizmoMode, camera, gl, scene, defaultControls, invalidate, store]);

  useEffect(() => () => clearTimeout(gizmoThrottleRef.current), []);

  if (!selectedFeature || !store || NO_GIZMO_KINDS.has(selectedFeature.kind)) return null;

  return (
    <mesh ref={meshRef} visible={false}>
      <boxGeometry args={[1, 1, 1]} />
    </mesh>
  );
}

export function Invalidator({ ndc }: { ndc: { x: number; y: number; active: boolean } }) {
  const invalidate = useThree((s) => s.invalidate);
  const { state } = useKernelStore();
  const meshedBodies = state?.meshedBodies;
  const sketchActive = state?.sketchSession.active ?? false;
  const selection = state?.selection;
  const features = state?.features;
  const visibleSketches = state?.visibleSketches;

  useEffect(() => {
    invalidate();
  }, [
    invalidate,
    meshedBodies,
    sketchActive,
    selection,
    features,
    visibleSketches,
    ndc.x,
    ndc.y,
    ndc.active,
  ]);

  return null;
}

/** One analytic B-Rep edge: a world-space line-segment chain plus its
 *  canonical colliding-branch signature, used to match the hovered edge. */
interface AnalyticEdge {
  a: number;
  b: number;
  /** Flat `LineSegments` positions (pairs of points) for this edge. */
  positions: Float32Array;
}

/** Build the per-edge `LineSegments` positions from a body-relative polyline. */
function polylineToSegments(points: number[]): Float32Array {
  const n = points.length / 3;
  if (n < 2) return new Float32Array(0);
  const seg = new Float32Array((n - 1) * 6);
  for (let p = 0; p < n - 1; p++) {
    seg[p * 6] = points[p * 3];
    seg[p * 6 + 1] = points[p * 3 + 1];
    seg[p * 6 + 2] = points[p * 3 + 2];
    seg[p * 6 + 3] = points[(p + 1) * 3];
    seg[p * 6 + 4] = points[(p + 1) * 3 + 1];
    seg[p * 6 + 5] = points[(p + 1) * 3 + 2];
  }
  return seg;
}

/**
 * Draws the analytic B-Rep edges (from the woven, cached kernel B-Rep — *not*
 * the mesh) as a dark wireframe overlay, and highlights the edge the cursor is
 * over. The hovered edge is matched by its colliding-branch signature against
 * the `contributors` the kernel `pick` reports, so the same analytic edge that
 * STEP export emits is the one that lights up.
 *
 * Rendered on both the default layer (mesh render mode) and the overlay layer
 * (composited over the SDF render pass), so edges are visible in every mode.
 */
export function BRepEdgeOverlay({ pick }: { pick: PickResult | null }) {
  const { store, state } = useKernelStore();
  const baseGeometryRef = useRef(new THREE.BufferGeometry());
  const hoverGeometryRef = useRef(new THREE.BufferGeometry());
  const baseMaterialRef = useRef(
    new THREE.LineBasicMaterial({
      color: 0x1e293b,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  const hoverMaterialRef = useRef(
    new THREE.LineBasicMaterial({
      color: 0xf59e0b,
      depthTest: false,
      depthWrite: false,
    }),
  );

  // Body fingerprint (count + per-body id.version). Bumps on any geometry edit
  // and on a placement move (the body version increments), so the edge cache
  // re-reads exactly when — and only when — the bodies actually change.
  //
  // Deliberately NOT driven by `meshedBodies`: DC meshing is enabled only in
  // mesh render mode (App gates `setDcMeshingEnabled` on it), so in the default
  // SDF modes `meshedBodies` is empty. The analytic B-Rep lives on the
  // main-thread kernel regardless of render mode, so we read it straight from
  // `bodyCount()` — that is what makes the edges appear in SDF modes too.
  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  const edges = useMemo<AnalyticEdge[]>(() => {
    if (!store) return [];
    const count = store.kernel.bodyCount();
    const out: AnalyticEdge[] = [];
    for (let i = 0; i < count; i++) {
      let bodyEdges: { a: number; b: number; points: number[] }[];
      try {
        bodyEdges = store.kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      for (const e of bodyEdges) {
        const positions = polylineToSegments(e.points);
        if (positions.length > 0) out.push({ a: e.a, b: e.b, positions });
      }
    }
    return out;
  }, [store, bodyFingerprint]);

  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (edges.length === 0) {
      setReady(false);
      return;
    }
    let total = 0;
    for (const e of edges) total += e.positions.length;
    const merged = new Float32Array(total);
    let off = 0;
    for (const e of edges) {
      merged.set(e.positions, off);
      off += e.positions.length;
    }
    baseGeometryRef.current.setAttribute("position", new THREE.BufferAttribute(merged, 3));
    baseGeometryRef.current.computeBoundingSphere();
    setReady(true);
  }, [edges]);

  // Match the hovered edge by signature: pick reports the colliding faces as
  // packed `branches`; an edge lights up when both of its signature branches
  // are among them. (Selection uses the flat `contributors`; the edge's a/b
  // are packed per-face ids, so they must be matched against `branches`.)
  const hoveredIdx = useMemo(() => {
    if (!pick || !pick.hit || pick.element !== "edge") return -1;
    const b = pick.branches;
    return edges.findIndex((e) => b.includes(e.a) && b.includes(e.b));
  }, [pick, edges]);

  const [hoverReady, setHoverReady] = useState(false);
  useEffect(() => {
    if (hoveredIdx < 0 || hoveredIdx >= edges.length) {
      setHoverReady(false);
      return;
    }
    hoverGeometryRef.current.setAttribute(
      "position",
      new THREE.BufferAttribute(edges[hoveredIdx].positions, 3),
    );
    hoverGeometryRef.current.computeBoundingSphere();
    setHoverReady(true);
  }, [hoveredIdx, edges]);

  useEffect(() => {
    return () => {
      baseGeometryRef.current.dispose();
      hoverGeometryRef.current.dispose();
      baseMaterialRef.current.dispose();
      hoverMaterialRef.current.dispose();
    };
  }, []);

  if (!ready) return null;

  return (
    <>
      <lineSegments
        ref={(obj: THREE.LineSegments | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
        geometry={baseGeometryRef.current}
        material={baseMaterialRef.current}
        frustumCulled={false}
        renderOrder={998}
      />
      {hoverReady && (
        <lineSegments
          ref={(obj: THREE.LineSegments | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
          geometry={hoverGeometryRef.current}
          material={hoverMaterialRef.current}
          frustumCulled={false}
          renderOrder={999}
        />
      )}
    </>
  );
}

function MeshPreviewBody({ body, mat, bodyIndex }: {
  body: { positions: Float32Array; normals: Float32Array; indices: Uint32Array };
  mat: MaterialProps | undefined;
  bodyIndex: number;
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

  const fallback = BODY_COLORS[bodyIndex % BODY_COLORS.length];
  const color = mat
    ? new THREE.Color(mat.color[0], mat.color[1], mat.color[2])
    : new THREE.Color(fallback[0], fallback[1], fallback[2]);
  const emissive = mat
    ? new THREE.Color(mat.emissive[0], mat.emissive[1], mat.emissive[2])
    : undefined;

  return (
    <mesh geometry={geometry}>
      <meshStandardMaterial
        color={color}
        roughness={mat?.roughness ?? 0.6}
        metalness={mat?.metalness ?? 0.0}
        emissive={emissive}
        opacity={mat?.opacity ?? 1}
        transparent={mat != null && mat.opacity < 1}
        wireframe={mat?.wireframe ?? false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

export function MeshPreview() {
  const { store, state } = useKernelStore();
  const bodies = state?.meshedBodies;
  const features = state?.features;

  const materials = useMemo(() => {
    if (!store || !features) return new Map<number, MaterialProps>();
    return resolveMaterials(store.kernel, features);
  }, [store, features]);

  if (!bodies || bodies.length === 0) return null;

  return (
    <>
      <ambientLight intensity={0.5} />
      <directionalLight position={[200, 400, 300]} intensity={0.8} />
      <directionalLight position={[-100, 200, -200]} intensity={0.3} />
      {bodies.map((body, i) => (
        <MeshPreviewBody key={body.id} body={body} mat={materials.get(i)} bodyIndex={i} />
      ))}
    </>
  );
}

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

export function AssemblyParts() {
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

export function MateConnectorOverlay() {
  const { store, state } = useKernelStore();
  const groupRef = useRef<THREE.Group>(null);

  useEffect(() => {
    const group = groupRef.current;
    if (!group || !state) return;
    while (group.children.length > 0) {
      const child = group.children[0];
      group.remove(child);
      if (child instanceof THREE.LineSegments) {
        child.geometry.dispose();
        (child.material as THREE.Material).dispose();
      }
    }
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

        const nLen = Math.hypot(normal[0], normal[1], normal[2]) || 1;
        const nz = [normal[0] / nLen, normal[1] / nLen, normal[2] / nLen];
        let up = [0, 1, 0];
        if (Math.abs(nz[1]) > 0.99) up = [1, 0, 0];
        const nx = [
          up[1] * nz[2] - up[2] * nz[1],
          up[2] * nz[0] - up[0] * nz[2],
          up[0] * nz[1] - up[1] * nz[0],
        ];
        const nxLen = Math.hypot(nx[0], nx[1], nx[2]) || 1;
        nx[0] /= nxLen; nx[1] /= nxLen; nx[2] /= nxLen;
        const ny = [
          nz[1] * nx[2] - nz[2] * nx[1],
          nz[2] * nx[0] - nz[0] * nx[2],
          nz[0] * nx[1] - nz[1] * nx[0],
        ];

        const axes: [number[], number][] = [
          [nx, 0xe74c3c],
          [ny, 0x2ecc71],
          [nz, 0x3498db],
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

const DATUM_PLANE_SIZE = 200;
const DATUM_PLANE_OPACITY = 0.08;
const DATUM_EDGE_OPACITY = 0.35;
const DATUM_COLOR = 0x3b82f6;

const DATUM_PLANES: {
  id: string;
  rotation: [number, number, number];
  label: string;
}[] = [
  { id: "front", rotation: [0, 0, 0],                     label: "Front" },
  { id: "top",   rotation: [-Math.PI / 2, 0, 0],          label: "Top" },
  { id: "right", rotation: [0, Math.PI / 2, 0],           label: "Right" },
];

const DATUM_LABEL_POS: [number, number, number] = [
  -DATUM_PLANE_SIZE / 2 + 22,
  DATUM_PLANE_SIZE / 2 - 10,
  0.5,
];

function makeLabelMaterial(text: string): THREE.MeshBasicMaterial {
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d")!;
  const font = "bold 64px sans-serif";
  ctx.font = font;
  const metrics = ctx.measureText(text);
  const w = Math.ceil(metrics.width) + 8;
  const h = 80;
  canvas.width = w;
  canvas.height = h;
  ctx.font = font;
  ctx.fillStyle = "#3b82f6";
  ctx.globalAlpha = 0.6;
  ctx.textBaseline = "top";
  ctx.fillText(text, 0, 4);
  const tex = new THREE.CanvasTexture(canvas);
  tex.minFilter = THREE.LinearFilter;
  tex.magFilter = THREE.LinearFilter;
  return new THREE.MeshBasicMaterial({ map: tex, transparent: true, depthTest: false, side: THREE.DoubleSide });
}

const datumLabelCache: Record<string, { material: THREE.MeshBasicMaterial; aspect: number }> = {};
function getDatumLabel(text: string) {
  if (!datumLabelCache[text]) {
    const mat = makeLabelMaterial(text);
    const aspect = (mat.map as THREE.CanvasTexture).image.width / (mat.map as THREE.CanvasTexture).image.height;
    datumLabelCache[text] = { material: mat, aspect };
  }
  return datumLabelCache[text];
}

const datumEdgesSource = new THREE.PlaneGeometry(DATUM_PLANE_SIZE, DATUM_PLANE_SIZE);
const datumEdgesGeometry = new THREE.EdgesGeometry(datumEdgesSource);
datumEdgesSource.dispose();

export function DatumOverlay() {
  const { state } = useKernelStore();
  const hidden = state?.hiddenDatums ?? new Set<string>();
  const invalidate = useThree((s) => s.invalidate);

  const originVisible = !hidden.has("origin");
  const visiblePlanes = DATUM_PLANES.filter((p) => !hidden.has(p.id));

  useEffect(() => { invalidate(); }, [hidden, invalidate]);

  if (!originVisible && visiblePlanes.length === 0) return null;

  return (
    <group>
      {originVisible && (
        <mesh>
          <sphereGeometry args={[1.5, 16, 16]} />
          <meshBasicMaterial color={DATUM_COLOR} transparent opacity={0.6} depthTest={false} />
        </mesh>
      )}
      {visiblePlanes.map((plane) => (
        <group key={plane.id} rotation={plane.rotation}>
          <mesh>
            <planeGeometry args={[DATUM_PLANE_SIZE, DATUM_PLANE_SIZE]} />
            <meshBasicMaterial
              color={DATUM_COLOR}
              transparent
              opacity={DATUM_PLANE_OPACITY}
              side={THREE.DoubleSide}
              depthWrite={false}
            />
          </mesh>
          <lineSegments geometry={datumEdgesGeometry}>
            <lineBasicMaterial color={DATUM_COLOR} transparent opacity={DATUM_EDGE_OPACITY} />
          </lineSegments>
          <mesh
            position={DATUM_LABEL_POS}
            material={getDatumLabel(plane.label).material}
            scale={[12 * getDatumLabel(plane.label).aspect, 12, 1]}
          >
            <planeGeometry args={[1, 1]} />
          </mesh>
        </group>
      ))}
    </group>
  );
}

export function WorldGrid() {
  const grid = useMemo(() => {
    const g = new THREE.GridHelper(600, 60, 0x999999, 0xdddddd);
    g.position.y = -0.1;
    (g.material as THREE.Material).transparent = true;
    (g.material as THREE.Material).opacity = 0.5;
    return g;
  }, []);
  return <primitive object={grid} />;
}

function MeshOverlayEntry({ data }: { data: { positions: Float32Array; normals: Float32Array; indices: Uint32Array } }) {
  const geometry = useMemo(() => new THREE.BufferGeometry(), []);

  useEffect(() => {
    return () => { geometry.dispose(); };
  }, [geometry]);

  useEffect(() => {
    geometry.setAttribute("position", new THREE.BufferAttribute(data.positions, 3));
    geometry.setAttribute("normal", new THREE.BufferAttribute(data.normals, 3));
    geometry.setIndex(new THREE.BufferAttribute(data.indices, 1));
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  }, [geometry, data]);

  return (
    <mesh geometry={geometry}>
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

export function ReferenceMesh() {
  const { store, state } = useKernelStore();

  const overlays = useMemo(() => {
    if (!store || !state) return [];
    return store.getMeshOverlayFeatures();
  }, [store, state?.features]);

  const refMesh = state?.referenceMesh;

  if (overlays.length === 0 && !refMesh) return null;

  return (
    <>
      {refMesh ? <MeshOverlayEntry data={refMesh} /> : null}
      {overlays.map((o) => (
        <MeshOverlayEntry key={o.id} data={o.data} />
      ))}
    </>
  );
}

// Invisible pick-state probe. The on-screen face/edge id readout was a
// debug overlay and is gone; this element stays in the DOM (visually
// hidden) only as a programmatic hook for the hover-pick e2e smoke test.
export function PickHud({ result }: { result: PickResult | null }) {
  return (
    <div
      className="pick-hud"
      data-testid="fielddraft-pick-hud"
      data-pick-hit={result?.hit ? "true" : "false"}
      aria-hidden="true"
    />
  );
}

export { PerfProbe };
