import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { Line2NodeMaterial } from "three/webgpu";
import * as THREE from "three";
import { ndcZVisible } from "./projectionSpace";
import { useKernelStore } from "./store";
import type { BRepEdge, OctreeCells } from "./kernel";
import type { BRepEdgeMode } from "./SettingsMenu";
import { OVERLAY_LAYER } from "./viewportLayers";
import { disposeAfterGpuIdle } from "./sdfPipelineCommon";
import { setHoveredElementOfKind } from "./brepPickBridge";
import { getPickCamera } from "./renderCameraBridge";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";
import { useBodyFingerprint } from "./bodyFingerprint";
import {
  polylineToSegments,
  ELEMENT_HOVER_COLOR,
  ELEMENT_HOVER_LINEWIDTH,
} from "./viewportOverlayShared";

// ── B-Rep edge overlays ──────────────────────────────────────────────
//
// The analytic B-Rep wireframe + edge hover/pick overlay, and the two
// debug overlays (edge control points, octree leaf cells).

/** One analytic B-Rep edge: a world-space line-segment chain plus its
 *  canonical colliding-branch signature, used to match the hovered edge. */
interface AnalyticEdge {
  a: number;
  b: number;
  /** Stable decimal id_hash of this edge, or `undefined` until the kernel
   *  wasm exporting it has been regenerated (defensive — Brocken B-K). */
  idHash?: string;
  /** Canonical fingerprint-carrying element query from the kernel. */
  query?: string;
  /** Flat `LineSegments` positions (pairs of points) for this edge. */
  positions: Float32Array;
}

/** Pixel pick band for edges — the cursor snaps to an edge within this many
 *  SCREEN pixels (converted to NDC per frame; a fixed NDC radius scales with
 *  the window and reached ~15px on full-HD — wide enough to blanket a narrow
 *  face from both silhouettes, so its middle could never face-hover, ticket
 *  7cv2). A touch tighter than the vertex band so a corner still wins over
 *  its edges. */
const EDGE_PICK_PX = 8;

/**
 * Draws the analytic B-Rep edges (from the woven, cached kernel B-Rep — *not*
 * the mesh) as a dark wireframe overlay, and highlights the edge the cursor is
 * over. The hovered edge is found by projecting every analytic polyline to the
 * screen and snapping the cursor to the nearest one within
 * {@link EDGE_PICK_PX} — the same screen-space approach
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
  depthBias = 2,
}: {
  ndc: { x: number; y: number; active: boolean };
  mode: BRepEdgeMode;
  /** Depth-Bias-Stärke im "visible"-Modus (Setting "Z-Offset"):
   *  polygonOffsetFactor = −bias, Units = −2·bias. 0 = kein Bias. */
  depthBias?: number;
}) {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const invalidate = useThree((s) => s.invalidate);
  const gl = useThree((s) => s.gl);
  const viewportSize = useThree((s) => s.size);
  // The fat-line (Line2NodeMaterial) path only renders on the WebGPU backend;
  // the classic THREE.WebGLRenderer fallback needs plain LineBasicMaterial
  // LineSegments, or B-Rep edges don't show at all there (ticket jnej).
  const isWebGPU =
    (gl as unknown as { isWebGPURenderer?: boolean }).isWebGPURenderer === true;
  const baseClassicRef = useRef(
    new THREE.LineBasicMaterial({
      color: 0x1e293b,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  const hoverClassicRef = useRef(
    new THREE.LineBasicMaterial({
      color: ELEMENT_HOVER_COLOR,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  // Base edges render as fat lines too (ticket 8f5z: plain LineBasicMaterial
  // rasterises 1px hairlines on WebGPU that read broken/washed-out up close —
  // "Kanten wirken nicht voll"). 2.5px through the instanced screen-space
  // line shader gives the solid Onshape-style wireframe; hover (3px) and
  // selection (4px) stay heavier.
  const baseMaterialRef = useRef(
    new Line2NodeMaterial({
      color: 0x1e293b,
      linewidth: 2.5,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  // Edge hover paints in a LIGHTER tint of the element-selection colour
  // (ticket m0yw: same hue as the selected edge, just brighter). It renders
  // through the same fat-line path as the *selected* edge (LineSegments2 +
  // Line2NodeMaterial) because that is the one line path proven to composite
  // over the SDF passes on the WebGPU backend — a plain LineSegments hover
  // overlay never showed up there. 3px wide (ticket bqvz: 1px drowned in
  // the base wireframe); the bolder 4px stays reserved for the
  // selection, so hover and selection share the hue but differ in brightness
  // and weight.
  const hoverMaterialRef = useRef(
    new Line2NodeMaterial({
      color: ELEMENT_HOVER_COLOR,
      linewidth: ELEMENT_HOVER_LINEWIDTH,
      worldUnits: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );

  // "visible" mode hides edges occluded by faces, so the line materials must
  // depth-test. "xray" (default) and "none" keep depthTest off. Depth comes
  // from the meshes in mesh render mode; the SDF overlays additionally write
  // depth and skip the depth-clear in "visible" mode (see renderOverlayLayer).
  //
  // The edges lie EXACTLY on the surface, so against the true surface depth
  // they z-fight (ragged/washed-out, ticket b9xz). The fix is polygonOffset
  // pulling the edge fragments slightly toward the camera — LineSegments2
  // rasterizes triangles, so the offset applies on WebGPU too (depthBias).
  // Biasing the edges forward is side-effect-free, unlike the previous
  // approach of pushing the whole SDF scene depth BACK, which skewed the
  // depth test against rasterized geometry (SDF-plane crescent, ticket iw6f).
  useEffect(() => {
    const depthTest = mode === "visible";
    const wantOffset = depthTest && depthBias > 0;
    for (const m of [
      baseMaterialRef.current,
      hoverMaterialRef.current,
      baseClassicRef.current,
      hoverClassicRef.current,
    ]) {
      if (m.depthTest !== depthTest) {
        m.depthTest = depthTest;
        m.needsUpdate = true;
      }
      // Bias Richtung Kamera, damit koplanare Kanten nicht z-fighten; Stärke
      // kommt aus dem Setting (direkte Eingabe neben "Visible only").
      if (
        m.polygonOffset !== wantOffset ||
        (wantOffset && m.polygonOffsetFactor !== -depthBias)
      ) {
        m.polygonOffset = wantOffset;
        m.polygonOffsetFactor = -depthBias;
        m.polygonOffsetUnits = -2 * depthBias;
        m.needsUpdate = true;
      }
    }
  }, [mode, depthBias]);

  // Body fingerprint (count + per-body id.version) — the edge cache re-reads
  // exactly when, and only when, the bodies actually change.
  //
  // Deliberately NOT driven by `meshedBodies`: DC meshing is enabled only in
  // mesh render mode (App gates `setDcMeshingEnabled` on it), so in the default
  // SDF modes `meshedBodies` is empty. The analytic B-Rep lives on the
  // main-thread kernel regardless of render mode, so we read it straight from
  // `bodyCount()` — that is what makes the edges appear in SDF modes too.
  const bodyFingerprint = useBodyFingerprint();

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
        if (positions.length > 0)
          out.push({ a: e.a, b: e.b, idHash: e.idHash, query: e.query, positions });
      }
    }
    return out;
  }, [store, bodyFingerprint]);

  // One fresh LineSegments2 for ALL base edges per geometry change — the
  // WebGPU renderer never re-uploads a LineSegmentsGeometry mutated in place
  // (ticket twxw), so the merged buffer gets a brand-new geometry + object.
  const [baseObj, setBaseObj] = useState<THREE.Object3D | null>(null);
  useEffect(() => {
    if (edges.length === 0) {
      setBaseObj(null);
      // The edge set going empty (a body removed, or a load that started with
      // none) still needs a frame so the old wireframe is cleared on screen.
      invalidate();
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
    let obj: THREE.Object3D;
    let geom: { dispose(): void };
    if (isWebGPU) {
      const g = new LineSegmentsGeometry();
      g.setPositions(merged);
      obj = new LineSegments2(g, baseMaterialRef.current);
      geom = g;
    } else {
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute(merged, 3));
      obj = new THREE.LineSegments(g, baseClassicRef.current);
      geom = g;
    }
    obj.frustumCulled = false;
    obj.renderOrder = 998;
    obj.layers.enable(OVERLAY_LAYER);
    setBaseObj(obj);
    // Request a repaint: the WebGPU root runs in DEMAND mode, so mounting this
    // fresh edge object into the scene does NOT itself trigger a frame. On the
    // EDIT path an unrelated invalidate (mesh rebuild / camera tick) happened to
    // paint it; on the LOAD path the async weaver imports the B-Rep AFTER the
    // first (surface-only) frame and bumps `brepVersion`, which re-runs this
    // effect with the freshly-woven edges — without this invalidate the new
    // wireframe sat in the scene unpainted until the next resize/orbit/edit, so
    // a loaded box-with-hole showed its faces but no B-Rep edges (tickets
    // 1do9/zhfy). One invalidate per edge-set change paints them immediately.
    invalidate();
    // Same fg9o race on a kernel rebuild while a submit is in flight — defer.
    return () => disposeAfterGpuIdle(gl, () => geom.dispose());
  }, [edges, isWebGPU, gl, invalidate]);

  // Screen-space edge hover. The kernel sphere-trace pick reports the face
  // *behind* the thin edge (1 contributor → "face"), so it can never light up
  // an edge on its own. Instead we project every analytic polyline to NDC each
  // frame and snap the cursor to the nearest segment within
  // the pixel pick band — front-most wins on near-ties. Mirrors how
  // BRepSelectionOverlay screen-space-picks vertices.
  const [hoveredIdx, setHoveredIdx] = useState(-1);
  const projA = useRef(new THREE.Vector3());
  const projB = useRef(new THREE.Vector3());

  // Occlusion probe (ticket twvd): a point is hidden when the kernel's
  // analytic raycast from the camera hits a surface clearly in FRONT of it.
  // The slack absorbs sphere-trace tolerance on the faces meeting AT the
  // probed edge — only a genuinely nearer surface rejects a candidate.
  const isOccludedPoint = (w: [number, number, number]): boolean => {
    if (!store) return false;
    // Ticket pzvt: probe from the PRESENTED frame's camera, not the live one.
    const o = getPickCamera(camera).position;
    const dx = w[0] - o.x;
    const dy = w[1] - o.y;
    const dz = w[2] - o.z;
    const dist = Math.hypot(dx, dy, dz);
    if (dist < 1e-6) return false;
    const hit = store.kernel.pick(
      [o.x, o.y, o.z],
      [dx / dist, dy / dist, dz / dist],
    );
    return hit.hit && hit.t < dist - Math.max(1.0, dist * 0.02);
  };

  useFrame(() => {
    // Ticket 94jk: while editing a sketch, only sketch geometry is interactive
    // — B-Rep edges must NOT hover/highlight (the 2D sketch canvas owns hover).
    // Exception: a query picker armed mid-session (e.g. re-picking the sketch
    // plane) still needs B-Rep hover, so don't suppress then.
    const sketchBlocksHover =
      state?.sketchSession?.active === true && getActiveQueryPicker() === null;
    if (!ndc.active || edges.length === 0 || sketchBlocksHover) {
      if (hoveredIdx !== -1) {
        setHoveredIdx(-1);
        invalidate();
      }
      return;
    }
    const a = projA.current;
    const b = projB.current;
    // Ticket pzvt "pick what you see": project against the camera of the
    // last PRESENTED frame — during shader compiles the visible frame is
    // frozen while the live camera may already have moved.
    const pickCam = getPickCamera(camera);
    // Collect the best segment per edge within the pick band; the winner is
    // resolved afterwards so non-x-ray modes can skip occluded candidates.
    const candidates: {
      ei: number;
      d: number;
      depth: number;
      world: [number, number, number];
    }[] = [];
    // Pixel → NDC (y spans 2 over the viewport height), per frame so window
    // resizes keep the band at a constant on-screen width.
    const pickNdc = (2 * EDGE_PICK_PX) / Math.max(1, viewportSize.height);
    for (let ei = 0; ei < edges.length; ei++) {
      const pos = edges[ei].positions;
      let bestD = pickNdc;
      let bestDepth = Infinity;
      let bestWorld: [number, number, number] | null = null;
      for (let i = 0; i + 5 < pos.length; i += 6) {
        a.set(pos[i], pos[i + 1], pos[i + 2]).project(pickCam);
        b.set(pos[i + 3], pos[i + 4], pos[i + 5]).project(pickCam);
        // Skip segments that cross or sit behind the near plane — projecting
        // those wraps the NDC coords and yields bogus distances.
        if (!ndcZVisible(pickCam, a.z) || !ndcZVisible(pickCam, b.z)) continue;
        // Closest point on the segment in NDC; `t` doubles as the lerp
        // parameter for the world-space occlusion probe point below (the
        // perspective error over one short tessellation segment is nil).
        const sx = b.x - a.x;
        const sy = b.y - a.y;
        const len2 = sx * sx + sy * sy;
        let t = len2 > 0 ? ((ndc.x - a.x) * sx + (ndc.y - a.y) * sy) / len2 : 0;
        t = Math.max(0, Math.min(1, t));
        const d = Math.hypot(ndc.x - (a.x + t * sx), ndc.y - (a.y + t * sy));
        if (d > pickNdc) continue;
        const depth = Math.min(a.z, b.z);
        if (d < bestD - 1e-4 || (d <= bestD + 1e-4 && depth < bestDepth)) {
          bestD = d;
          bestDepth = depth;
          bestWorld = [
            pos[i] + t * (pos[i + 3] - pos[i]),
            pos[i + 1] + t * (pos[i + 4] - pos[i + 1]),
            pos[i + 2] + t * (pos[i + 5] - pos[i + 2]),
          ];
        }
      }
      if (bestWorld)
        candidates.push({ ei, d: bestD, depth: bestDepth, world: bestWorld });
    }
    // Nearest cursor distance wins; near-ties resolve to the front-most edge.
    candidates.sort((p, q) =>
      Math.abs(p.d - q.d) > 1e-4 ? p.d - q.d : p.depth - q.depth,
    );
    let best = -1;
    if (mode === "xray") {
      best = candidates.length > 0 ? candidates[0].ei : -1;
    } else {
      // Outside x-ray mode, edges hidden behind geometry are neither hovered
      // nor selectable (ticket twvd): probe candidates front-to-back until one
      // is actually visible. Capped — rarely more than a couple under the
      // cursor — so the per-frame raycast cost stays bounded.
      const limit = Math.min(candidates.length, 8);
      for (let c = 0; c < limit; c++) {
        if (!isOccludedPoint(candidates[c].world)) {
          best = candidates[c].ei;
          break;
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
      setHoveredElementOfKind("edge", {
        kind: "edge",
        idHash: e.idHash,
        query: e.query,
      });
      return () => setHoveredElementOfKind("edge", null);
    }
    return undefined;
  }, [hoveredIdx, edges]);

  // The hovered edge gets a BRAND-NEW LineSegments2 + geometry per change —
  // the WebGPU renderer does not re-upload a line geometry mutated in place
  // (same caveat as the selected-edge fat line, ticket twxw), so reusing one
  // object would freeze the highlight on whatever edge it was first built for.
  const [hoverObj, setHoverObj] = useState<THREE.Object3D | null>(null);
  useEffect(() => {
    if (hoveredIdx < 0 || hoveredIdx >= edges.length) {
      setHoverObj(null);
      return;
    }
    const positions = edges[hoveredIdx].positions;
    let obj: THREE.Object3D;
    let geom: { dispose(): void };
    if (isWebGPU) {
      const g = new LineSegmentsGeometry();
      g.setPositions(positions);
      obj = new LineSegments2(g, hoverMaterialRef.current);
      geom = g;
    } else {
      const g = new THREE.BufferGeometry();
      g.setAttribute(
        "position",
        new THREE.Float32BufferAttribute(positions, 3),
      );
      obj = new THREE.LineSegments(g, hoverClassicRef.current);
      geom = g;
    }
    obj.frustumCulled = false;
    obj.renderOrder = 999;
    obj.layers.enable(OVERLAY_LAYER);
    setHoverObj(obj);
    // Ticket fg9o/o6hx: hovering sweeps `hoveredIdx` every frame, so this
    // fat-line geometry is torn down a frame later while an in-flight WebGPU
    // submit may still reference it. Destroying it synchronously trips
    // "[Buffer] used in submit while destroyed" (counting up until the device
    // wedges and the overlay/datum pass stops drawing). Defer until the GPU
    // queue drains (falls back to sync on the WebGL backend).
    return () => disposeAfterGpuIdle(gl, () => geom.dispose());
  }, [hoveredIdx, edges, isWebGPU, gl]);

  useEffect(() => {
    const mats = [
      baseMaterialRef.current,
      hoverMaterialRef.current,
      baseClassicRef.current,
      hoverClassicRef.current,
    ];
    return () => mats.forEach((m) => m.dispose());
  }, []);

  if (mode === "none" || !baseObj) return null;

  return (
    <>
      {/* dispose={null}: object identity changes per rebuild; the previous
          geometry is disposed by the effect above, the material is shared. */}
      <primitive object={baseObj} dispose={null} />
      {hoverObj && <primitive object={hoverObj} dispose={null} />}
    </>
  );
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
      color: EDGE_NODE_COLOR,
      size: 9,
      sizeAttenuation: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  const handleMatRef = useRef(
    new THREE.PointsMaterial({
      color: EDGE_HANDLE_COLOR,
      size: 6,
      sizeAttenuation: false,
      depthTest: false,
      depthWrite: false,
      transparent: true,
    }),
  );
  const polygonMatRef = useRef(
    new THREE.LineBasicMaterial({
      color: EDGE_POLYGON_COLOR,
      depthTest: false,
      depthWrite: false,
      transparent: true,
      opacity: 0.6,
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
          const x = cps[p * 3],
            y = cps[p * 3 + 1],
            z = cps[p * 3 + 2];
          // On-curve nodes sit at every `degree`-th control point; the rest
          // are off-curve Bézier handles.
          if (p % d === 0) nodes.push(x, y, z);
          else handles.push(x, y, z);
          // Control polygon: a segment from this control point to the next.
          if (p + 1 < nCps) {
            polygon.push(
              x,
              y,
              z,
              cps[(p + 1) * 3],
              cps[(p + 1) * 3 + 1],
              cps[(p + 1) * 3 + 2],
            );
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
    nodeGeomRef.current.setAttribute(
      "position",
      new THREE.BufferAttribute(buffers.nodes, 3),
    );
    nodeGeomRef.current.computeBoundingSphere();
    handleGeomRef.current.setAttribute(
      "position",
      new THREE.BufferAttribute(buffers.handles, 3),
    );
    handleGeomRef.current.computeBoundingSphere();
    polygonGeomRef.current.setAttribute(
      "position",
      new THREE.BufferAttribute(buffers.polygon, 3),
    );
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
  const enableOverlay = (obj: THREE.Object3D | null) => {
    if (obj) obj.layers.enable(OVERLAY_LAYER);
  };
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
    [min[0], min[1], min[2]],
    [max[0], min[1], min[2]],
    [max[0], max[1], min[2]],
    [min[0], max[1], min[2]],
    [min[0], min[1], max[2]],
    [max[0], min[1], max[2]],
    [max[0], max[1], max[2]],
    [min[0], max[1], max[2]],
  ];
  const edges: [number, number][] = [
    [0, 1],
    [1, 2],
    [2, 3],
    [3, 0], // bottom face
    [4, 5],
    [5, 6],
    [6, 7],
    [7, 4], // top face
    [0, 4],
    [1, 5],
    [2, 6],
    [3, 7], // verticals
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
    const out: {
      id: string;
      geometry: THREE.BufferGeometry;
      placement: OctreeCells;
    }[] = [];
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
      geometry.setAttribute(
        "position",
        new THREE.BufferAttribute(new Float32Array(pos), 3),
      );
      geometry.setAttribute(
        "color",
        new THREE.BufferAttribute(new Float32Array(col), 3),
      );
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
          position={[
            b.placement.translate[0],
            b.placement.translate[1],
            b.placement.translate[2],
          ]}
          quaternion={[
            b.placement.rotation[0],
            b.placement.rotation[1],
            b.placement.rotation[2],
            b.placement.rotation[3],
          ]}
          scale={b.placement.scale}
        >
          <lineSegments
            ref={(obj: THREE.LineSegments | null) => {
              if (obj) obj.layers.enable(OVERLAY_LAYER);
            }}
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
