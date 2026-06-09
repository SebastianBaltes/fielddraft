import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { TransformControls as ThreeTransformControls } from "three/examples/jsm/controls/TransformControls.js";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { Line2NodeMaterial } from "three/webgpu";
import * as THREE from "three";
import { useKernelStore, type StoreState } from "./store";
import {
  FACE_INDEX_SENTINEL,
  localToWorld,
  type BRepEdge,
  type OctreeCells,
  type BRepFace,
  type BRepVertex,
  type FeatureSummary,
  type Placement,
  type PickResult,
} from "./kernel";
import type { BRepEdgeMode } from "./SettingsMenu";
import { PerfProbe } from "./perfMonitor";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import { resolveMaterials, type MaterialProps } from "./materialBridge";
import { BODY_COLORS } from "./sdfDeferredRenderer";
import { setHoveredElementOfKind } from "./brepPickBridge";
import { filterAllows } from "./elementQuery";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";

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
  /** Stable decimal id_hash of this edge, or `undefined` until the kernel
   *  wasm exporting it has been regenerated (defensive — Brocken B-K). */
  idHash?: string;
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

/** NDC pick band for edges — the cursor snaps to an edge within this radius
 *  (in normalised device coords, like {@link VERTEX_PICK_NDC_RADIUS}). A touch
 *  tighter than the vertex radius so a corner still wins over its edges. */
const EDGE_PICK_NDC_RADIUS = 0.022;

/** Shortest distance, in NDC, from point `p` to the segment `a`–`b`. All three
 *  are 2-component {x,y}. Used to snap the cursor to the nearest B-Rep edge. */
function ndcPointSegmentDistance(
  px: number,
  py: number,
  ax: number,
  ay: number,
  bx: number,
  by: number,
): number {
  const dx = bx - ax;
  const dy = by - ay;
  const len2 = dx * dx + dy * dy;
  let t = len2 > 0 ? ((px - ax) * dx + (py - ay) * dy) / len2 : 0;
  if (t < 0) t = 0;
  else if (t > 1) t = 1;
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}

/**
 * Draws the analytic B-Rep edges (from the woven, cached kernel B-Rep — *not*
 * the mesh) as a dark wireframe overlay, and highlights the edge the cursor is
 * over. The hovered edge is found by projecting every analytic polyline to the
 * screen and snapping the cursor to the nearest one within
 * {@link EDGE_PICK_NDC_RADIUS} — the same screen-space approach
 * {@link BRepSelectionOverlay} uses for faces/vertices. (The kernel sphere-trace
 * pick almost always reports the face *behind* the thin edge, so it can't drive
 * edge hover on its own.) The matched edge carries the stable `id_hash` that
 * STEP export emits, so the lit edge is exactly the one a `qEdge` query targets.
 *
 * Rendered on both the default layer (mesh render mode) and the overlay layer
 * (composited over the SDF render pass), so edges are visible in every mode.
 */
export function BRepEdgeOverlay({
  ndc,
  mode,
}: {
  ndc: { x: number; y: number; active: boolean };
  mode: BRepEdgeMode;
}) {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const invalidate = useThree((s) => s.invalidate);
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
  // Edge hover paints in the element-selection colour (gold-orange), 1px wide:
  // a `LineBasicMaterial` always rasterises 1px on WebGPU/WebGL (`linewidth` is
  // a no-op there), which is exactly the width the ticket asked for. The bolder
  // 2px fat line stays reserved for the *selected* edge so hover and selection
  // read the same hue but differ in weight.
  const hoverMaterialRef = useRef(
    new THREE.LineBasicMaterial({
      color: ELEMENT_SELECTION_COLOR,
      depthTest: false,
      depthWrite: false,
    }),
  );

  // "visible" mode hides edges occluded by faces, so the line materials must
  // depth-test. "xray" (default) and "none" keep depthTest off. Depth comes
  // from the meshes in mesh render mode; the SDF overlays additionally write
  // depth and skip the depth-clear in "visible" mode (see renderOverlayLayer).
  useEffect(() => {
    const depthTest = mode === "visible";
    for (const m of [baseMaterialRef.current, hoverMaterialRef.current]) {
      if (m.depthTest !== depthTest) {
        m.depthTest = depthTest;
        m.needsUpdate = true;
      }
    }
  }, [mode]);

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
      let bodyEdges: BRepEdge[];
      try {
        bodyEdges = store.kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      for (const e of bodyEdges) {
        const positions = polylineToSegments(e.points);
        if (positions.length > 0) out.push({ a: e.a, b: e.b, idHash: e.idHash, positions });
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

  // Screen-space edge hover. The kernel sphere-trace pick reports the face
  // *behind* the thin edge (1 contributor → "face"), so it can never light up
  // an edge on its own. Instead we project every analytic polyline to NDC each
  // frame and snap the cursor to the nearest segment within
  // EDGE_PICK_NDC_RADIUS — front-most wins on near-ties. Mirrors how
  // BRepSelectionOverlay screen-space-picks vertices.
  const [hoveredIdx, setHoveredIdx] = useState(-1);
  const projA = useRef(new THREE.Vector3());
  const projB = useRef(new THREE.Vector3());
  useFrame(() => {
    if (!ndc.active || edges.length === 0) {
      if (hoveredIdx !== -1) {
        setHoveredIdx(-1);
        invalidate();
      }
      return;
    }
    const a = projA.current;
    const b = projB.current;
    let best = -1;
    let bestDist = EDGE_PICK_NDC_RADIUS;
    let bestDepth = Infinity;
    for (let ei = 0; ei < edges.length; ei++) {
      const pos = edges[ei].positions;
      for (let i = 0; i + 5 < pos.length; i += 6) {
        a.set(pos[i], pos[i + 1], pos[i + 2]).project(camera);
        b.set(pos[i + 3], pos[i + 4], pos[i + 5]).project(camera);
        // Skip segments that cross or sit behind the near plane — projecting
        // those wraps the NDC coords and yields bogus distances.
        if (a.z < -1 || a.z > 1 || b.z < -1 || b.z > 1) continue;
        const d = ndcPointSegmentDistance(ndc.x, ndc.y, a.x, a.y, b.x, b.y);
        if (d > EDGE_PICK_NDC_RADIUS) continue;
        const depth = Math.min(a.z, b.z);
        if (d < bestDist - 1e-4 || (d <= bestDist + 1e-4 && depth < bestDepth)) {
          best = ei;
          bestDist = d;
          bestDepth = depth;
        }
      }
    }
    if (best !== hoveredIdx) {
      setHoveredIdx(best);
      invalidate();
    }
  });

  // Publish the hovered edge's stable id_hash to the pick bridge so a click
  // in query-picker mode can emit `qEdge(<idHash>)`. A missing idHash (kernel
  // wasm not yet regenerated) clears it — the edge isn't selectable yet.
  useEffect(() => {
    const e = hoveredIdx >= 0 ? edges[hoveredIdx] : undefined;
    if (e?.idHash !== undefined) {
      setHoveredElementOfKind("edge", { kind: "edge", idHash: e.idHash });
      return () => setHoveredElementOfKind("edge", null);
    }
    return undefined;
  }, [hoveredIdx, edges]);

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

  if (mode === "none" || !ready) return null;

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

/** Fingerprint that bumps on any body geometry edit or placement move — count
 *  plus per-body `id.version`. Shared by the two debug overlays below so their
 *  kernel reads re-run exactly when (and only when) the bodies change. Mirrors
 *  {@link BRepEdgeOverlay}'s own `bodyFingerprint`. */
function useBodyFingerprint(): string {
  const { store, state } = useKernelStore();
  return useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);
}

/**
 * Debug overlay: renders every B-Rep edge's **structural** points — the few
 * control points that actually define the curve, NOT the dense tessellation.
 * Three layers per edge:
 *   - **on-curve nodes** (red, large): the span joins that lie *on* the curve,
 *     at every `degree`-th control point. Count them to see how few points a
 *     NURBS edge is built from (e.g. an exact circle = a 4-span rational
 *     quadratic → 5 nodes).
 *   - **off-curve handles** (amber, small): the interior Bézier control points
 *     that bend each span; absent on straight/polyline edges.
 *   - **control polygon** (faint grey lines): connects consecutive control
 *     points, making the per-span quadratic/cubic structure visible.
 *
 * Reads the same cached analytic B-Rep (`controlPoints`/`degree`, world space)
 * as {@link BRepEdgeOverlay}, independent of mesh resolution and render mode.
 * Toggle via Settings → "Edge sample points".
 */
const EDGE_NODE_COLOR = 0xdc2626;
const EDGE_HANDLE_COLOR = 0xf59e0b;
const EDGE_POLYGON_COLOR = 0x94a3b8;

export function BRepEdgePointsOverlay() {
  const { store } = useKernelStore();
  const fingerprint = useBodyFingerprint();
  const nodeGeomRef = useRef(new THREE.BufferGeometry());
  const handleGeomRef = useRef(new THREE.BufferGeometry());
  const polygonGeomRef = useRef(new THREE.BufferGeometry());
  const nodeMatRef = useRef(
    new THREE.PointsMaterial({
      color: EDGE_NODE_COLOR, size: 9, sizeAttenuation: false,
      depthTest: false, depthWrite: false, transparent: true,
    }),
  );
  const handleMatRef = useRef(
    new THREE.PointsMaterial({
      color: EDGE_HANDLE_COLOR, size: 6, sizeAttenuation: false,
      depthTest: false, depthWrite: false, transparent: true,
    }),
  );
  const polygonMatRef = useRef(
    new THREE.LineBasicMaterial({
      color: EDGE_POLYGON_COLOR, depthTest: false, depthWrite: false,
      transparent: true, opacity: 0.6,
    }),
  );

  const buffers = useMemo(() => {
    if (!store) return null;
    const count = store.kernel.bodyCount();
    const nodes: number[] = [];
    const handles: number[] = [];
    const polygon: number[] = [];
    for (let i = 0; i < count; i++) {
      let bodyEdges: BRepEdge[];
      try {
        bodyEdges = store.kernel.bodyBrepEdges(i);
      } catch {
        continue;
      }
      for (const e of bodyEdges) {
        const cps = e.controlPoints;
        const d = Math.max(1, e.degree);
        const nCps = Math.floor(cps.length / 3);
        for (let p = 0; p < nCps; p++) {
          const x = cps[p * 3], y = cps[p * 3 + 1], z = cps[p * 3 + 2];
          // On-curve nodes sit at every `degree`-th control point; the rest
          // are off-curve Bézier handles.
          if (p % d === 0) nodes.push(x, y, z);
          else handles.push(x, y, z);
          // Control polygon: a segment from this control point to the next.
          if (p + 1 < nCps) {
            polygon.push(x, y, z, cps[(p + 1) * 3], cps[(p + 1) * 3 + 1], cps[(p + 1) * 3 + 2]);
          }
        }
      }
    }
    return {
      nodes: new Float32Array(nodes),
      handles: new Float32Array(handles),
      polygon: new Float32Array(polygon),
    };
  }, [store, fingerprint]);

  const [ready, setReady] = useState(false);
  useEffect(() => {
    if (!buffers || buffers.nodes.length === 0) {
      setReady(false);
      return;
    }
    nodeGeomRef.current.setAttribute("position", new THREE.BufferAttribute(buffers.nodes, 3));
    nodeGeomRef.current.computeBoundingSphere();
    handleGeomRef.current.setAttribute("position", new THREE.BufferAttribute(buffers.handles, 3));
    handleGeomRef.current.computeBoundingSphere();
    polygonGeomRef.current.setAttribute("position", new THREE.BufferAttribute(buffers.polygon, 3));
    polygonGeomRef.current.computeBoundingSphere();
    setReady(true);
  }, [buffers]);

  useEffect(() => {
    return () => {
      nodeGeomRef.current.dispose();
      handleGeomRef.current.dispose();
      polygonGeomRef.current.dispose();
      nodeMatRef.current.dispose();
      handleMatRef.current.dispose();
      polygonMatRef.current.dispose();
    };
  }, []);

  if (!ready || !buffers) return null;
  const enableOverlay = (obj: THREE.Object3D | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); };
  return (
    <>
      <lineSegments
        ref={enableOverlay}
        geometry={polygonGeomRef.current}
        material={polygonMatRef.current}
        frustumCulled={false}
        renderOrder={999}
      />
      {buffers.handles.length > 0 && (
        <points
          ref={enableOverlay}
          geometry={handleGeomRef.current}
          material={handleMatRef.current}
          frustumCulled={false}
          renderOrder={1000}
        />
      )}
      <points
        ref={enableOverlay}
        geometry={nodeGeomRef.current}
        material={nodeMatRef.current}
        frustumCulled={false}
        renderOrder={1001}
      />
    </>
  );
}

/** Per-class wireframe colour for the octree-cell overlay, indexed by the
 *  kernel's `class` byte: 0=Face (blue), 1=Edge (amber), 2=Vertex (red).
 *  RGB in 0..1 for a vertex-colour `LineBasicMaterial`. */
const OCTREE_CLASS_COLORS: [number, number, number][] = [
  [0.23, 0.51, 0.96],
  [0.96, 0.62, 0.04],
  [0.86, 0.15, 0.15],
];

/** Push the 12 wireframe edges of the axis-aligned box `[min,max]` into the
 *  `pos`/`col` arrays (24 vertices, each carrying `rgb`). */
function pushBoxWireframe(
  pos: number[],
  col: number[],
  min: [number, number, number],
  max: [number, number, number],
  rgb: [number, number, number],
) {
  const c: [number, number, number][] = [
    [min[0], min[1], min[2]], [max[0], min[1], min[2]],
    [max[0], max[1], min[2]], [min[0], max[1], min[2]],
    [min[0], min[1], max[2]], [max[0], min[1], max[2]],
    [max[0], max[1], max[2]], [min[0], max[1], max[2]],
  ];
  const edges: [number, number][] = [
    [0, 1], [1, 2], [2, 3], [3, 0], // bottom face
    [4, 5], [5, 6], [6, 7], [7, 4], // top face
    [0, 4], [1, 5], [2, 6], [3, 7], // verticals
  ];
  for (const [a, b] of edges) {
    pos.push(c[a][0], c[a][1], c[a][2], c[b][0], c[b][1], c[b][2]);
    col.push(rgb[0], rgb[1], rgb[2], rgb[0], rgb[1], rgb[2]);
  }
}

/**
 * Debug overlay: draws the topology **octree's leaf cells** as a wireframe
 * grid, colour-coded by class (blue = face, amber = edge, red = vertex). The
 * octree is **feature-adaptive on topology**: `edge`/`vertex` cells refine to
 * the fine `min_cell_size` (≈ largest-extent / 32) to pin the feature, while
 * `face` cells stop at the coarser `face_cell_size` (FACE_CELL_FACTOR× larger)
 * because a single-branch patch carries no topology to seed and feeds no weave
 * consumer. So flat/curved face interiors show large blue cells and the grid
 * tightens only along the amber edges and red corners. Cells arrive in the
 * body-local frame plus the body placement, which is applied as a group
 * matrix. Toggle via Settings → "Octree cells".
 */
export function OctreeCellOverlay() {
  const { store } = useKernelStore();
  const fingerprint = useBodyFingerprint();
  const materialRef = useRef(
    new THREE.LineBasicMaterial({
      vertexColors: true,
      depthTest: false,
      depthWrite: false,
      transparent: true,
      opacity: 0.5,
    }),
  );

  const bodies = useMemo(() => {
    if (!store) return [];
    const count = store.kernel.bodyCount();
    const out: { id: string; geometry: THREE.BufferGeometry; placement: OctreeCells }[] = [];
    for (let i = 0; i < count; i++) {
      let dump: OctreeCells;
      try {
        dump = store.kernel.bodyOctreeCells(i);
      } catch {
        continue;
      }
      if (dump.cells.length === 0) continue;
      const pos: number[] = [];
      const col: number[] = [];
      for (const cell of dump.cells) {
        const rgb = OCTREE_CLASS_COLORS[cell.class] ?? OCTREE_CLASS_COLORS[0];
        pushBoxWireframe(pos, col, cell.min, cell.max, rgb);
      }
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(pos), 3));
      geometry.setAttribute("color", new THREE.BufferAttribute(new Float32Array(col), 3));
      out.push({ id: `${store.kernel.bodyId(i)}`, geometry, placement: dump });
    }
    return out;
  }, [store, fingerprint]);

  // Dispose the previous frame's geometries when `bodies` is rebuilt.
  useEffect(() => {
    return () => {
      for (const b of bodies) b.geometry.dispose();
    };
  }, [bodies]);

  useEffect(() => {
    return () => materialRef.current.dispose();
  }, []);

  if (bodies.length === 0) return null;
  return (
    <>
      {bodies.map((b) => (
        <group
          key={b.id}
          position={[b.placement.translate[0], b.placement.translate[1], b.placement.translate[2]]}
          quaternion={[
            b.placement.rotation[0],
            b.placement.rotation[1],
            b.placement.rotation[2],
            b.placement.rotation[3],
          ]}
          scale={b.placement.scale}
        >
          <lineSegments
            ref={(obj: THREE.LineSegments | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
            geometry={b.geometry}
            material={materialRef.current}
            frustumCulled={false}
            renderOrder={997}
          />
        </group>
      ))}
    </>
  );
}

// ── B-Rep face / vertex selection overlay (Brocken B-W) ──────────────
//
// Token-driven highlight colours. THREE material colours can't live in CSS
// (they feed the WebGPU pipeline), so — like BRepEdgeOverlay's inline hex —
// they're module constants here. They mirror tokens.css:
//   FACE_HOVER  ← --color-accent        (#2f5fa8)
//   VERTEX      ← --color-warning        (#b07820)
//   VERTEX_HOVER← --color-accent-hover   (#3d72be)
const FACE_HOVER_COLOR = 0x2f5fa8;
const VERTEX_COLOR = 0xb07820;
const VERTEX_HOVER_COLOR = 0x3d72be;
/** Persistent element-selection colour: a saturated gold-orange (Onshape-like
 *  selection highlight), distinct from the lighter amber edge-hover (#f59e0b)
 *  and the blue picker-highlight. */
const ELEMENT_SELECTION_COLOR = 0xff7a00;
const VERTEX_MARKER_RADIUS = 2.2;
/** Screen-space radius (in NDC units) within which a vertex marker counts as
 *  hovered. ~3.5% of the viewport half-extent. */
const VERTEX_PICK_NDC_RADIUS = 0.035;

/** One body's selectable B-Rep data, resolved to world space for picking. */
interface BodyPickData {
  /** World-space triangle mesh (positions + indices), for face raycasts. */
  geometry: THREE.BufferGeometry;
  /** Persistent raycast target wrapping {@link geometry} (reused per frame). */
  pickMesh: THREE.Mesh;
  /** Per-triangle B-Rep face arena index (= `bodyFaceIndexView`). */
  faceIndexView: Uint32Array;
  faces: BRepFace[];
  /** Vertices with world-space positions folded through the placement. */
  vertices: { idHash: string; world: [number, number, number] }[];
}

function buildBodyPickData(
  kernel: {
    bodyCount(): number;
    bodyVertexCount(i: number): number;
    bodyPositionsView(i: number): Float32Array;
    bodyIndicesView(i: number): Uint32Array;
    bodyFaceIndexView(i: number): Uint32Array;
    bodyBrepFaces(i: number): BRepFace[];
    bodyBrepVertices(i: number): BRepVertex[];
    bodyPlacement(i: number): Placement;
  },
): BodyPickData[] {
  const out: BodyPickData[] = [];
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    if (kernel.bodyVertexCount(i) === 0) continue;
    let faces: BRepFace[];
    let verts: BRepVertex[];
    let placement: Placement;
    let faceIndexView: Uint32Array;
    let positions: Float32Array;
    let indices: Uint32Array;
    try {
      faces = kernel.bodyBrepFaces(i);
      verts = kernel.bodyBrepVertices(i);
      placement = kernel.bodyPlacement(i);
      // Copy out of WASM linear memory — the views alias buffers that any
      // later mutating kernel call could relocate.
      faceIndexView = kernel.bodyFaceIndexView(i).slice();
      positions = kernel.bodyPositionsView(i).slice();
      indices = kernel.bodyIndicesView(i).slice();
    } catch {
      continue;
    }
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.setIndex(new THREE.BufferAttribute(indices, 1));
    geometry.computeBoundingSphere();
    const vertices = verts.map((v) => ({
      idHash: v.idHash,
      world: localToWorld(v.positionLocal, placement),
    }));
    out.push({ geometry, pickMesh: new THREE.Mesh(geometry), faceIndexView, faces, vertices });
  }
  return out;
}

/**
 * Hoverable + selectable B-Rep **faces** and **vertices** in the viewport.
 *
 * Faces: an invisible per-body pick mesh (built straight from the kernel's
 * world-space body buffers, independent of render mode — same trick as
 * `BRepEdgeOverlay`) is raycast each frame. THREE reports
 * `intersection.faceIndex` = the **triangle** index; indexing
 * `bodyFaceIndexView(i)` with it gives the B-Rep **face arena index**, and
 * `bodyBrepFaces(i)[arena].idHash` the stable id. All triangles sharing that
 * arena index are lit as the hover highlight.
 *
 * Vertices: rendered as small token-coloured point markers (positions folded
 * body-LOCAL → world via `localToWorld`), nearest-to-cursor in screen space
 * is the hover.
 *
 * The hovered element is published to `brepPickBridge` so the Viewport click
 * handler can emit `qFace`/`qVertex`. Highlighting + markers only light up
 * when an element-level query picker is active and its `entityFilter` allows
 * that kind — so this never adds clutter during normal modelling.
 */
export function BRepSelectionOverlay({
  ndc,
}: {
  ndc: { x: number; y: number; active: boolean };
}) {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const invalidate = useThree((s) => s.invalidate);
  const raycaster = useRef(new THREE.Raycaster());

  const faceHighlightGeom = useRef(new THREE.BufferGeometry());
  const faceHighlightMat = useRef(
    new THREE.MeshBasicMaterial({
      color: FACE_HOVER_COLOR,
      transparent: true,
      opacity: 0.4,
      depthTest: false,
      depthWrite: false,
      side: THREE.DoubleSide,
    }),
  );

  // Same body fingerprint trick as BRepEdgeOverlay: rebuild the pick data only
  // when bodies actually change (count or per-body id.version).
  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  const bodies = useMemo<BodyPickData[]>(() => {
    if (!store) return [];
    return buildBodyPickData(store.kernel);
  }, [store, bodyFingerprint]);

  useEffect(() => {
    return () => {
      for (const b of bodies) b.geometry.dispose();
    };
  }, [bodies]);

  useEffect(() => {
    const geom = faceHighlightGeom.current;
    const mat = faceHighlightMat.current;
    return () => {
      geom.dispose();
      mat.dispose();
    };
  }, []);

  const [faceHover, setFaceHover] = useState(false);
  const [vertexHoverIdx, setVertexHoverIdx] = useState<[number, number] | null>(null);

  // Build the highlight triangle list for one arena face index.
  const highlightFace = (body: BodyPickData, arena: number): void => {
    const idx = body.geometry.getIndex();
    const pos = body.geometry.getAttribute("position");
    if (!idx || !pos) return;
    const tris: number[] = [];
    const triCount = body.faceIndexView.length;
    for (let t = 0; t < triCount; t++) {
      if (body.faceIndexView[t] !== arena) continue;
      for (let k = 0; k < 3; k++) {
        const vi = idx.getX(t * 3 + k);
        tris.push(pos.getX(vi), pos.getY(vi), pos.getZ(vi));
      }
    }
    faceHighlightGeom.current.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(tris, 3),
    );
    faceHighlightGeom.current.computeBoundingSphere();
  };

  useFrame(() => {
    const picker = getActiveQueryPicker();
    // Hover detection runs whenever the cursor is over the viewport — tool-free
    // element selection (chunk 3a) needs the hovered element published even
    // with no active picker. When a picker IS active we honour its
    // entityFilter; with no picker, all kinds are pickable.
    if (!ndc.active || bodies.length === 0) {
      if (faceHover) { setFaceHover(false); invalidate(); }
      if (vertexHoverIdx) { setVertexHoverIdx(null); invalidate(); }
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", null);
      return;
    }
    const filter = picker?.entityFilter;
    const wantFace = picker ? filterAllows("face", filter) : true;
    const wantVertex = picker ? filterAllows("vertex", filter) : true;

    raycaster.current.setFromCamera(new THREE.Vector2(ndc.x, ndc.y), camera);

    // ── Faces ──
    let resolvedFace: { body: BodyPickData; arena: number; idHash: string; dist: number } | null = null;
    if (wantFace) {
      for (const body of bodies) {
        const hits = raycaster.current.intersectObject(body.pickMesh, false);
        for (const h of hits) {
          if (h.faceIndex === undefined || h.faceIndex === null) continue;
          const arena = body.faceIndexView[h.faceIndex];
          if (arena === undefined || arena === FACE_INDEX_SENTINEL) continue;
          const face = body.faces.find((f) => f.faceIndex === arena);
          if (!face) continue;
          if (!resolvedFace || h.distance < resolvedFace.dist) {
            resolvedFace = { body, arena, idHash: face.idHash, dist: h.distance };
          }
          break;
        }
      }
    }

    // ── Vertices (nearest in screen space, only if no nearer face hit) ──
    let resolvedVertex:
      | { bi: number; vi: number; idHash: string; ndcDist: number }
      | null = null;
    if (wantVertex && !resolvedFace) {
      const v = new THREE.Vector3();
      for (let bi = 0; bi < bodies.length; bi++) {
        const verts = bodies[bi].vertices;
        for (let vi = 0; vi < verts.length; vi++) {
          v.set(verts[vi].world[0], verts[vi].world[1], verts[vi].world[2]);
          v.project(camera);
          if (v.z < -1 || v.z > 1) continue;
          const d = Math.hypot(v.x - ndc.x, v.y - ndc.y);
          if (d > VERTEX_PICK_NDC_RADIUS) continue;
          if (!resolvedVertex || d < resolvedVertex.ndcDist) {
            resolvedVertex = { bi, vi, idHash: verts[vi].idHash, ndcDist: d };
          }
        }
      }
    }

    if (resolvedFace) {
      highlightFace(resolvedFace.body, resolvedFace.arena);
      if (!faceHover) { setFaceHover(true); invalidate(); }
      if (vertexHoverIdx) { setVertexHoverIdx(null); invalidate(); }
      setHoveredElementOfKind("face", { kind: "face", idHash: resolvedFace.idHash });
      setHoveredElementOfKind("vertex", null);
    } else if (resolvedVertex) {
      if (faceHover) { setFaceHover(false); invalidate(); }
      const next: [number, number] = [resolvedVertex.bi, resolvedVertex.vi];
      if (!vertexHoverIdx || vertexHoverIdx[0] !== next[0] || vertexHoverIdx[1] !== next[1]) {
        setVertexHoverIdx(next);
        invalidate();
      }
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", { kind: "vertex", idHash: resolvedVertex.idHash });
    } else {
      if (faceHover) { setFaceHover(false); invalidate(); }
      if (vertexHoverIdx) { setVertexHoverIdx(null); invalidate(); }
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", null);
    }
  });

  const picker = getActiveQueryPicker();
  const showVertexMarkers =
    picker !== null && filterAllows("vertex", picker.entityFilter);

  return (
    <>
      {faceHover && (
        <mesh
          ref={(obj: THREE.Mesh | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
          geometry={faceHighlightGeom.current}
          material={faceHighlightMat.current}
          frustumCulled={false}
          renderOrder={997}
        />
      )}
      {showVertexMarkers &&
        bodies.map((b, bi) =>
          b.vertices.map((v, vi) => {
            const hovered =
              vertexHoverIdx !== null && vertexHoverIdx[0] === bi && vertexHoverIdx[1] === vi;
            return (
              <mesh
                key={`v-${bi}-${vi}`}
                position={v.world}
                ref={(obj: THREE.Mesh | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
                renderOrder={1000}
              >
                <sphereGeometry args={[hovered ? VERTEX_MARKER_RADIUS * 1.6 : VERTEX_MARKER_RADIUS, 12, 12]} />
                <meshBasicMaterial
                  color={hovered ? VERTEX_HOVER_COLOR : VERTEX_COLOR}
                  depthTest={false}
                  transparent
                  opacity={hovered ? 1 : 0.85}
                />
              </mesh>
            );
          }),
        )}
    </>
  );
}

/**
 * Persistent B-Rep element-selection highlight (chunk 3a).
 *
 * Renders `state.selectedElement` — a single face, edge, or vertex picked
 * tool-free in the viewport — in the distinct teal selection colour. Unlike
 * the hover highlights in {@link BRepSelectionOverlay}/{@link BRepEdgeOverlay}
 * this is NOT picker-gated and persists after the pointer leaves; it clears
 * only when the store's `selectedElement` is reset.
 *
 * The highlight geometry is resolved from the kernel by `idHash`:
 *   - face → all triangles whose face-arena index matches, in world space;
 *   - edge → the matching analytic polyline, body-local → world;
 *   - vertex → a sphere marker at the world position.
 */
export function BRepSelectedOverlay() {
  const { store, state } = useKernelStore();
  const selected = state?.selectedElement ?? null;
  // Full multi-select set: a face primary highlights every selected face
  // (the coplanar extrude gather); edges/vertices stay single-primary.
  const selectedList = state?.selectedElements ?? [];
  const selectedKey = selectedList.map((e) => `${e.kind}:${e.idHash}`).join(",");

  const faceGeomRef = useRef(new THREE.BufferGeometry());
  const faceMatRef = useRef(
    new THREE.MeshBasicMaterial({
      color: ELEMENT_SELECTION_COLOR,
      transparent: true,
      opacity: 0.5,
      depthTest: false,
      depthWrite: false,
      side: THREE.DoubleSide,
    }),
  );
  // Selected edges use a *fat* line (LineSegments2 + Line2NodeMaterial). A
  // plain LineBasicMaterial silently ignores `linewidth` on WebGPU/WebGL — it
  // always rasterises 1px — so the 2px the ticket asked for only materialises
  // through the instanced screen-space line shader. The WebGPU variant derives
  // its pixel scale from the built-in viewport/DPR nodes, so unlike the WebGL
  // LineMaterial there is no `resolution` uniform to keep in sync.
  const edgeMatRef = useRef(
    new Line2NodeMaterial({
      color: ELEMENT_SELECTION_COLOR,
      linewidth: 2,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  // The selected edge gets a BRAND-NEW LineSegments2 + geometry per selection,
  // built in the edge branch of `resolved` below — never an in-place mutation
  // of one persistent geometry. The WebGPU renderer caches a
  // LineSegmentsGeometry's instanced GPU buffers and does NOT re-upload them
  // after `setPositions` overwrites them, so reusing one geometry froze the
  // highlight on the first edge ever selected (ticket twxw: "immer dieselbe
  // Kante"). Handing the renderer a fresh object forces fresh buffers. The ref
  // only tracks the current object so the resolver can dispose the previous.
  const edgeObjRef = useRef<LineSegments2 | null>(null);

  useEffect(() => {
    const fg = faceGeomRef.current;
    const fm = faceMatRef.current;
    const em = edgeMatRef.current;
    return () => {
      fg.dispose();
      fm.dispose();
      edgeObjRef.current?.geometry.dispose();
      em.dispose();
    };
  }, []);

  // Body fingerprint: re-resolve the highlight when bodies actually change
  // (same trick as the hover overlays), since a regen invalidates idHashes.
  const bodyFingerprint = useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++) fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);

  // Resolve the selected element to renderable data. Returns the kind plus the
  // payload the JSX below needs (face/edge geometry are written into the shared
  // refs; vertex resolves to a world position).
  const resolved = useMemo<
    | { kind: "face" }
    | { kind: "edge" }
    | { kind: "vertex"; world: [number, number, number] }
    | null
  >(() => {
    if (!store || !selected) return null;
    const k = store.kernel;
    const count = k.bodyCount();

    if (selected.kind === "face") {
      // Highlight EVERY selected face (multi-select gather), accumulating their
      // triangles across all bodies into the shared face geometry.
      const faceHashes = new Set(
        selectedList.filter((e) => e.kind === "face").map((e) => e.idHash),
      );
      const tris: number[] = [];
      for (let i = 0; i < count; i++) {
        let faces: BRepFace[];
        let placement: Placement;
        let faceIndexView: Uint32Array;
        let positions: Float32Array;
        let indices: Uint32Array;
        try {
          faces = k.bodyBrepFaces(i);
          placement = k.bodyPlacement(i);
          faceIndexView = k.bodyFaceIndexView(i).slice();
          positions = k.bodyPositionsView(i).slice();
          indices = k.bodyIndicesView(i).slice();
        } catch {
          continue;
        }
        const wanted = new Set(
          faces.filter((f) => faceHashes.has(f.idHash)).map((f) => f.faceIndex),
        );
        if (wanted.size === 0) continue;
        for (let t = 0; t < faceIndexView.length; t++) {
          if (!wanted.has(faceIndexView[t])) continue;
          for (let c = 0; c < 3; c++) {
            const vi = indices[t * 3 + c];
            const local: [number, number, number] = [
              positions[vi * 3],
              positions[vi * 3 + 1],
              positions[vi * 3 + 2],
            ];
            const w = localToWorld(local, placement);
            tris.push(w[0], w[1], w[2]);
          }
        }
      }
      if (tris.length === 0) return null;
      faceGeomRef.current.setAttribute(
        "position",
        new THREE.Float32BufferAttribute(tris, 3),
      );
      faceGeomRef.current.computeBoundingSphere();
      return { kind: "face" };
    }

    if (selected.kind === "edge") {
      for (let i = 0; i < count; i++) {
        let edges: BRepEdge[];
        try {
          edges = k.bodyBrepEdges(i);
        } catch {
          continue;
        }
        const edge = edges.find((e) => e.idHash === selected.idHash);
        if (!edge) continue;
        // BRepEdge.points are already in world space (kernel.ts) — unlike faces
        // and vertices, which are body-local. Folding them through the body
        // placement again would double-transform and offset the highlight; the
        // hover overlay (BRepEdgeOverlay) consumes the same points raw.
        const segs = polylineToSegments(edge.points);
        if (segs.length === 0) continue;
        // LineSegmentsGeometry wants the flat 6-per-segment layout that
        // polylineToSegments already produces (start xyz, end xyz, …). Build a
        // fresh geometry + object every time (see the ref comment above):
        // mutating one persistent geometry in place doesn't re-upload on the
        // WebGPU backend, which froze the highlight on the first edge (twxw).
        const geom = new LineSegmentsGeometry();
        geom.setPositions(segs);
        const obj = new LineSegments2(geom, edgeMatRef.current);
        obj.frustumCulled = false;
        obj.renderOrder = 1002;
        obj.layers.enable(OVERLAY_LAYER);
        edgeObjRef.current?.geometry.dispose();
        edgeObjRef.current = obj;
        return { kind: "edge" };
      }
      return null;
    }

    // vertex
    for (let i = 0; i < count; i++) {
      let verts: BRepVertex[];
      let placement: Placement;
      try {
        verts = k.bodyBrepVertices(i);
        placement = k.bodyPlacement(i);
      } catch {
        continue;
      }
      const v = verts.find((vx) => vx.idHash === selected.idHash);
      if (!v) continue;
      return { kind: "vertex", world: localToWorld(v.positionLocal, placement) };
    }
    return null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, selected, selectedKey, bodyFingerprint]);

  if (!resolved) return null;

  if (resolved.kind === "face") {
    return (
      <mesh
        ref={(obj: THREE.Mesh | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
        geometry={faceGeomRef.current}
        material={faceMatRef.current}
        frustumCulled={false}
        renderOrder={1001}
      />
    );
  }
  if (resolved.kind === "edge") {
    // `dispose={null}`: the object identity changes per selection, so R3F
    // remounts this primitive each time. Without it R3F would auto-dispose the
    // outgoing object on unmount — including the *shared* material — breaking
    // every later highlight. We dispose the previous geometry ourselves above.
    // The memo always sets `edgeObjRef.current` before returning kind "edge".
    return <primitive object={edgeObjRef.current!} dispose={null} />;
  }
  return (
    <mesh
      position={resolved.world}
      ref={(obj: THREE.Mesh | null) => { if (obj) obj.layers.enable(OVERLAY_LAYER); }}
      renderOrder={1003}
    >
      <sphereGeometry args={[VERTEX_MARKER_RADIUS * 1.7, 16, 16]} />
      <meshBasicMaterial color={ELEMENT_SELECTION_COLOR} depthTest={false} />
    </mesh>
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
    if (!store) return;
    // C2: the kernel resolves each connector's CSYS from its face query
    // during regeneration; we just render the triads it hands back.
    const connectors = store.kernel.resolvedMateConnectors();
    if (connectors.length === 0) return;
    for (const c of connectors) {
      try {
        const origin = c.origin;
        const size = 15;

        // CSYS: Z = face normal, X = the kernel's in-plane basis vector,
        // Y = Z × X (right-handed).
        const nLen = Math.hypot(c.normal[0], c.normal[1], c.normal[2]) || 1;
        const nz = [c.normal[0] / nLen, c.normal[1] / nLen, c.normal[2] / nLen];
        const xLen = Math.hypot(c.x_axis[0], c.x_axis[1], c.x_axis[2]) || 1;
        const nx = [c.x_axis[0] / xLen, c.x_axis[1] / xLen, c.x_axis[2] / xLen];
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
        // Skip if a connector can't be rendered.
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
