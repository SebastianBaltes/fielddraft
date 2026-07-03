import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import type { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import * as THREE from "three";
import { ndcZVisible } from "./projectionSpace";
import {
  createFatLineMaterials,
  isWebGPURenderer,
  makeFatLine,
} from "./overlayLineMaterials";
import { useKernelStore } from "./store";
import {
  FACE_INDEX_SENTINEL,
  localToWorld,
  type BRepEdge,
  type BRepFace,
  type BRepVertex,
  type HybridLeafInfo,
  type Placement,
} from "./kernel";
import { OVERLAY_LAYER } from "./viewportLayers";
import { disposeAfterGpuIdle } from "./sdfPipelineCommon";
import {
  applyLeafTransform,
  createPrimitiveGeometry,
  buildWarpedLeafGeometry,
} from "./leafMeshFactory";
import { triangulatePlanarFace } from "./faceFill";
import {
  getHoveredElementOfKind,
  setHoveredElementOfKind,
} from "./brepPickBridge";
import { filterAllows } from "./elementQuery";
import { getActive as getActiveQueryPicker } from "./queryPickerBridge";
import { useBodyFingerprint } from "./bodyFingerprint";
import {
  polylineToSegments,
  ELEMENT_SELECTION_COLOR,
  ELEMENT_HOVER_COLOR,
  ELEMENT_HOVER_LINEWIDTH,
} from "./viewportOverlayShared";

// ── B-Rep face / vertex selection overlay (Brocken B-W) ──────────────
//
// Token-driven highlight colours. THREE material colours can't live in CSS
// (they feed the WebGPU pipeline), so — like BRepEdgeOverlay's inline hex —
// they're module constants here (the shared element-selection/hover colours
// live in viewportOverlayShared). FACE_HOVER mirrors tokens.css
// --color-accent (#2f5fa8); VERTEX mirrors --color-warning (#b07820).
const FACE_HOVER_COLOR = 0x2f5fa8;
const VERTEX_COLOR = 0xb07820;
/** Vertex markers size in SCREEN pixels (ticket 7zs8): the selected edge
 *  draws as a 4px fat line, the hovered edge as a 3px fat line. World-space
 *  spheres looked huge at modelling zoom levels. */
const VERTEX_SELECT_DIAMETER_PX = 4;
const VERTEX_HOVER_DIAMETER_PX = 3;
/** Screen-space radius (in NDC units) within which a vertex marker counts as
 *  hovered. ~3.5% of the viewport half-extent. */
const VERTEX_PICK_NDC_RADIUS = 0.035;

/**
 * A point marker with a CONSTANT screen-space diameter (ticket 7zs8): a unit
 * sphere rescaled every frame to `px` pixels at its current depth, so the dot
 * neither balloons when zooming in nor vanishes when zooming out.
 */
function ScreenSpacePoint({
  position,
  color,
  px,
  renderOrder = 1000,
  opacity = 1,
}: {
  position: [number, number, number];
  color: number;
  px: number;
  renderOrder?: number;
  opacity?: number;
}) {
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  const meshRef = useRef<THREE.Mesh>(null);
  useFrame(() => {
    const m = meshRef.current;
    if (!m) return;
    // World units per CSS pixel at the marker's depth.
    let wpp: number;
    if ((camera as THREE.PerspectiveCamera).isPerspectiveCamera) {
      const cam = camera as THREE.PerspectiveCamera;
      const dist = m.getWorldPosition(scratchVec3).distanceTo(cam.position);
      wpp =
        (2 * dist * Math.tan(THREE.MathUtils.degToRad(cam.fov) / 2)) /
        size.height;
    } else {
      const cam = camera as THREE.OrthographicCamera;
      wpp = (cam.top - cam.bottom) / cam.zoom / size.height;
    }
    m.scale.setScalar(Math.max((px * wpp) / 2, 1e-9));
  });
  return (
    <mesh
      position={position}
      ref={(obj: THREE.Mesh | null) => {
        meshRef.current = obj;
        if (obj) obj.layers.enable(OVERLAY_LAYER);
      }}
      renderOrder={renderOrder}
    >
      <sphereGeometry args={[1, 12, 12]} />
      <meshBasicMaterial
        color={color}
        depthTest={false}
        transparent
        opacity={opacity}
      />
    </mesh>
  );
}

const scratchVec3 = new THREE.Vector3();

/** One body's selectable B-Rep data, resolved to world space for picking. */
interface BodyPickData {
  /** World-space triangle mesh (positions + indices), for face raycasts.
   *  `null` when the main-thread kernel holds no mesh for this body (any SDF
   *  render mode with worker meshing) — faces are then resolved through the
   *  kernel's analytic `pick()` raycast instead (ticket evq2). */
  geometry: THREE.BufferGeometry | null;
  /** Persistent raycast target wrapping {@link geometry} (reused per frame). */
  pickMesh: THREE.Mesh | null;
  /** Per-triangle B-Rep face arena index (= `bodyFaceIndexView`). */
  faceIndexView: Uint32Array | null;
  faces: BRepFace[];
  /** Vertices with world-space positions folded through the placement. */
  vertices: { idHash: string; world: [number, number, number] }[];
  /** Analytic edge polylines (world space) keyed for face-outline highlights.
   *  `faces` carries the FACE-precise arena association (== faceIndex,
   *  ticket b91c); the `(a, b)` branch signature stays as the fallback for
   *  picks that only know the branch (coplanar split faces share one). */
  edges: {
    a: number;
    b: number;
    idHash: string;
    points: number[];
    faces: number[];
  }[];
}

function buildBodyPickData(kernel: {
  bodyCount(): number;
  bodyVertexCount(i: number): number;
  bodyPositionsView(i: number): Float32Array;
  bodyIndicesView(i: number): Uint32Array;
  bodyFaceIndexView(i: number): Uint32Array;
  bodyBrepFaces(i: number): BRepFace[];
  bodyBrepVertices(i: number): BRepVertex[];
  bodyBrepEdges(i: number): BRepEdge[];
  bodyPlacement(i: number): Placement;
}): BodyPickData[] {
  const out: BodyPickData[] = [];
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let verts: BRepVertex[];
    let edges: BRepEdge[];
    let placement: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      verts = kernel.bodyBrepVertices(i);
      edges = kernel.bodyBrepEdges(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    // The triangle pick mesh exists only when this kernel instance actually
    // meshed the body (mesh render mode / sync meshing). The analytic B-Rep
    // data above is render-mode-independent, so faces and vertices stay
    // pickable without it.
    let geometry: THREE.BufferGeometry | null = null;
    let faceIndexView: Uint32Array | null = null;
    if (kernel.bodyVertexCount(i) > 0) {
      try {
        // Copy out of WASM linear memory — the views alias buffers that any
        // later mutating kernel call could relocate.
        faceIndexView = kernel.bodyFaceIndexView(i).slice();
        const positions = kernel.bodyPositionsView(i).slice();
        const indices = kernel.bodyIndicesView(i).slice();
        geometry = new THREE.BufferGeometry();
        geometry.setAttribute(
          "position",
          new THREE.BufferAttribute(positions, 3),
        );
        geometry.setIndex(new THREE.BufferAttribute(indices, 1));
        geometry.computeBoundingSphere();
      } catch {
        geometry = null;
        faceIndexView = null;
      }
    }
    const vertices = verts.map((v) => ({
      idHash: v.idHash,
      world: localToWorld(v.positionLocal, placement),
    }));
    out.push({
      geometry,
      pickMesh: geometry ? new THREE.Mesh(geometry) : null,
      faceIndexView,
      faces,
      vertices,
      edges: edges.map((e) => ({
        a: e.a,
        b: e.b,
        idHash: e.idHash,
        points: e.points,
        faces: e.faces,
      })),
    });
  }
  return out;
}

/** Flat LineSegments positions (6 floats per segment) for the boundary of
 *  the face with packed branch id `branchId`: every analytic edge whose
 *  signature contains the branch. Mesh-free face highlight (ticket evq2). */
function faceOutlineSegments(
  bodies: BodyPickData[],
  branchId: number,
): number[] {
  const segs: number[] = [];
  for (const body of bodies) {
    for (const e of body.edges) {
      if (e.a !== branchId && e.b !== branchId) continue;
      segs.push(...polylineToSegments(e.points));
    }
  }
  return segs;
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
  const gl = useThree((s) => s.gl);
  // Fat node materials only render on WebGPU; the classic WebGLRenderer needs a
  // plain LineBasicMaterial or it crashes in resolveIncludes (ticket gsoj).
  const isWebGPU = isWebGPURenderer(gl);
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
  const bodyFingerprint = useBodyFingerprint();

  const bodies = useMemo<BodyPickData[]>(() => {
    if (!store) return [];
    return buildBodyPickData(store.kernel);
  }, [store, bodyFingerprint]);

  useEffect(() => {
    return () => {
      for (const b of bodies) b.geometry?.dispose();
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
  const [vertexHoverIdx, setVertexHoverIdx] = useState<[number, number] | null>(
    null,
  );

  // Build the highlight triangle list for one arena face index. Bodies
  // without a triangle mesh (SDF render modes) contribute no triangles —
  // their hover feedback is the boundary outline below.
  const highlightFace = (body: BodyPickData, arena: number | null): void => {
    const tris: number[] = [];
    if (body.geometry && body.faceIndexView && arena !== null) {
      const idx = body.geometry.getIndex();
      const pos = body.geometry.getAttribute("position");
      if (idx && pos) {
        const triCount = body.faceIndexView.length;
        for (let t = 0; t < triCount; t++) {
          if (body.faceIndexView[t] !== arena) continue;
          for (let k = 0; k < 3; k++) {
            const vi = idx.getX(t * 3 + k);
            tris.push(pos.getX(vi), pos.getY(vi), pos.getZ(vi));
          }
        }
      }
    }
    faceHighlightGeom.current.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(tris, 3),
    );
    faceHighlightGeom.current.computeBoundingSphere();
  };

  // Boundary-outline hover highlight for the hovered face (ticket evq2) —
  // every analytic edge bounding the face lights up exactly like a hovered
  // edge. On WebGPU this is the ONLY face-hover feedback in the SDF render
  // modes (no triangle fill there), and it goes through the fat-line path
  // (LineSegments2 + Line2NodeMaterial): a plain LineBasicMaterial never
  // composited on the WebGPU backend (ticket bqvz). On the classic
  // WebGLRenderer the node material would crash in resolveIncludes (ticket
  // gsoj), so makeFatLine picks a plain LineSegments there instead.
  const outlineMatsRef = useRef(
    createFatLineMaterials({
      color: ELEMENT_HOVER_COLOR,
      linewidth: ELEMENT_HOVER_LINEWIDTH,
    }),
  );
  useEffect(() => {
    const mats = outlineMatsRef.current;
    return () => {
      mats.node.dispose();
      mats.classic.dispose();
    };
  }, []);
  // Hovered face, published from useFrame; the outline gets a BRAND-NEW
  // LineSegments2 + geometry per change — the WebGPU renderer does not
  // re-upload a line geometry mutated in place (ticket twxw). The functional
  // setter bails on identical (body, arena, branchId) so the per-frame
  // publish never re-renders.
  type HoverFace = {
    body: BodyPickData;
    arena: number | null;
    branchId: number;
  };
  const [hoverFace, setHoverFace] = useState<HoverFace | null>(null);
  const publishHoverFace = (next: HoverFace | null): void => {
    setHoverFace((prev) => {
      if (next === null) return prev === null ? prev : null;
      if (
        prev !== null &&
        prev.body === next.body &&
        prev.arena === next.arena &&
        prev.branchId === next.branchId
      )
        return prev;
      return next;
    });
  };
  const [outlineObj, setOutlineObj] = useState<THREE.Object3D | null>(null);
  useEffect(() => {
    if (hoverFace === null) {
      setOutlineObj(null);
      return;
    }
    // Face-precise outline when the triangle pick resolved an arena index
    // (edge.faces association, ticket b91c) — coplanar split faces share a
    // branch signature, so the branch fallback (analytic SDF pick, no
    // arena) lights the whole coplanar family.
    let segs: number[];
    if (hoverFace.arena !== null) {
      segs = [];
      for (const e of hoverFace.body.edges) {
        if (!e.faces.includes(hoverFace.arena)) continue;
        segs.push(...polylineToSegments(e.points));
      }
    } else {
      segs = faceOutlineSegments(bodies, hoverFace.branchId);
    }
    if (segs.length === 0) {
      setOutlineObj(null);
      return;
    }
    const { object: obj, geometry } = makeFatLine(
      new Float32Array(segs),
      outlineMatsRef.current,
      isWebGPU,
    );
    obj.frustumCulled = false;
    obj.renderOrder = 999;
    obj.layers.enable(OVERLAY_LAYER);
    setOutlineObj(obj);
    invalidate();
    // Same fg9o race: hovering sweeps `hoverFace` every frame — defer disposal.
    return () => disposeAfterGpuIdle(gl, () => geometry.dispose());
  }, [hoverFace, bodies, invalidate, isWebGPU, gl]);

  useFrame(() => {
    const picker = getActiveQueryPicker();
    // Hover detection runs whenever the cursor is over the viewport — tool-free
    // element selection (chunk 3a) needs the hovered element published even
    // with no active picker. When a picker IS active we honour its
    // entityFilter; with no picker, all kinds are pickable.
    // Ticket 94jk: while editing a sketch, B-Rep faces/vertices must NOT hover
    // (the 2D sketch canvas owns interaction) — unless a query picker is armed
    // mid-session (e.g. re-picking the sketch plane).
    const sketchBlocksHover =
      state?.sketchSession?.active === true && picker === null;
    if (!ndc.active || bodies.length === 0 || sketchBlocksHover) {
      if (faceHover) {
        setFaceHover(false);
        invalidate();
      }
      if (vertexHoverIdx) {
        setVertexHoverIdx(null);
        invalidate();
      }
      publishHoverFace(null);
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", null);
      return;
    }
    const filter = picker?.entityFilter;
    // Snapshot semantics: while a picker is active, faces of features that
    // come AFTER the one being edited are not selectable — don't light them on
    // hover (the click handler also rejects them with a message).
    const faceAllowed = (featureId: number): boolean =>
      !picker?.selectableFeatureIds ||
      picker.selectableFeatureIds.has(featureId);
    // An edge inside its pick band beats the face it bounds — the bridge
    // merge already resolves clicks that way (vertex > edge > face), so the
    // visuals must agree: with the face outline now drawn in the same hover
    // style (ticket bqvz), keeping both lit would mask WHICH edge a click
    // selects. A face-only picker keeps the face highlight alive instead.
    const edgeWins =
      getHoveredElementOfKind("edge") !== null &&
      (!picker || filterAllows("edge", filter));
    const wantFace =
      !edgeWins && (picker ? filterAllows("face", filter) : true);
    const wantVertex = picker ? filterAllows("vertex", filter) : true;

    raycaster.current.setFromCamera(new THREE.Vector2(ndc.x, ndc.y), camera);

    // ── Faces ──
    // Two resolution paths (ticket evq2): the triangle pick mesh where this
    // kernel instance holds one (mesh render mode), and the kernel's analytic
    // `pick()` raycast everywhere else (SDF modes keep main-thread meshes
    // empty, which used to make faces and vertices unhoverable). The analytic
    // hit maps its packed branch ids onto `BRepFace.branchId`.
    let resolvedFace: {
      body: BodyPickData;
      arena: number | null;
      branchId: number;
      idHash: string;
      dist: number;
    } | null = null;
    if (wantFace) {
      for (const body of bodies) {
        if (!body.pickMesh || !body.faceIndexView) continue;
        const hits = raycaster.current.intersectObject(body.pickMesh, false);
        for (const h of hits) {
          if (h.faceIndex === undefined || h.faceIndex === null) continue;
          const arena = body.faceIndexView[h.faceIndex];
          if (arena === undefined || arena === FACE_INDEX_SENTINEL) continue;
          const face = body.faces.find((f) => f.faceIndex === arena);
          if (!face) continue;
          if (!faceAllowed(face.featureId)) continue;
          if (!resolvedFace || h.distance < resolvedFace.dist) {
            resolvedFace = {
              body,
              arena,
              branchId: face.branchId,
              idHash: face.idHash,
              dist: h.distance,
            };
          }
          break;
        }
      }
      if (!resolvedFace && store) {
        const o = raycaster.current.ray.origin;
        const d = raycaster.current.ray.direction;
        const hit = store.kernel.pick([o.x, o.y, o.z], [d.x, d.y, d.z]);
        if (hit.hit) {
          outer: for (const body of bodies) {
            for (const face of body.faces) {
              if (!hit.branches.includes(face.branchId)) continue;
              if (!faceAllowed(face.featureId)) continue;
              resolvedFace = {
                body,
                arena: null,
                branchId: face.branchId,
                idHash: face.idHash,
                dist: hit.t,
              };
              break outer;
            }
          }
        }
      }
    }

    // ── Vertices (nearest in screen space) ──
    // A vertex within its snap radius wins over the face under the cursor —
    // clicking near a corner selects the corner (ticket evq2) — unless the
    // face hit is clearly in FRONT of it (occluded back-side corner).
    let resolvedVertex: {
      bi: number;
      vi: number;
      idHash: string;
      ndcDist: number;
    } | null = null;
    if (wantVertex) {
      const v = new THREE.Vector3();
      const camPos = camera.position;
      for (let bi = 0; bi < bodies.length; bi++) {
        const verts = bodies[bi].vertices;
        for (let vi = 0; vi < verts.length; vi++) {
          const w = verts[vi].world;
          v.set(w[0], w[1], w[2]);
          const camDist = v.distanceTo(camPos);
          if (resolvedFace && camDist > resolvedFace.dist * 1.05 + 1e-3)
            continue;
          v.project(camera);
          if (!ndcZVisible(camera, v.z)) continue;
          const d = Math.hypot(v.x - ndc.x, v.y - ndc.y);
          if (d > VERTEX_PICK_NDC_RADIUS) continue;
          if (!resolvedVertex || d < resolvedVertex.ndcDist) {
            resolvedVertex = { bi, vi, idHash: verts[vi].idHash, ndcDist: d };
          }
        }
      }
    }
    if (resolvedVertex) resolvedFace = null;

    if (resolvedFace) {
      highlightFace(resolvedFace.body, resolvedFace.arena);
      publishHoverFace({
        body: resolvedFace.body,
        arena: resolvedFace.arena,
        branchId: resolvedFace.branchId,
      });
      if (!faceHover) {
        setFaceHover(true);
        invalidate();
      }
      if (vertexHoverIdx) {
        setVertexHoverIdx(null);
        invalidate();
      }
      setHoveredElementOfKind("face", {
        kind: "face",
        idHash: resolvedFace.idHash,
      });
      setHoveredElementOfKind("vertex", null);
    } else if (resolvedVertex) {
      if (faceHover) {
        setFaceHover(false);
        invalidate();
      }
      publishHoverFace(null);
      const next: [number, number] = [resolvedVertex.bi, resolvedVertex.vi];
      if (
        !vertexHoverIdx ||
        vertexHoverIdx[0] !== next[0] ||
        vertexHoverIdx[1] !== next[1]
      ) {
        setVertexHoverIdx(next);
        invalidate();
      }
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", {
        kind: "vertex",
        idHash: resolvedVertex.idHash,
      });
    } else {
      if (faceHover) {
        setFaceHover(false);
        invalidate();
      }
      if (vertexHoverIdx) {
        setVertexHoverIdx(null);
        invalidate();
      }
      publishHoverFace(null);
      setHoveredElementOfKind("face", null);
      setHoveredElementOfKind("vertex", null);
    }
  });

  const picker = getActiveQueryPicker();
  const showVertexMarkers =
    picker !== null && filterAllows("vertex", picker.entityFilter);
  // Tool-free hovered vertex: render ITS marker even without a picker so a
  // click near a corner has visible feedback before the click (ticket evq2).
  const hoveredVertexWorld =
    !showVertexMarkers && vertexHoverIdx !== null
      ? (bodies[vertexHoverIdx[0]]?.vertices[vertexHoverIdx[1]]?.world ?? null)
      : null;

  return (
    <>
      {faceHover && (
        <mesh
          ref={(obj: THREE.Mesh | null) => {
            if (obj) obj.layers.enable(OVERLAY_LAYER);
          }}
          geometry={faceHighlightGeom.current}
          material={faceHighlightMat.current}
          frustumCulled={false}
          renderOrder={997}
        />
      )}
      {/* dispose={null}: fresh object per hovered face; the previous geometry
          is disposed by the outline effect, the material is shared. */}
      {outlineObj && <primitive object={outlineObj} dispose={null} />}
      {hoveredVertexWorld && (
        <ScreenSpacePoint
          position={hoveredVertexWorld}
          color={ELEMENT_HOVER_COLOR}
          px={VERTEX_HOVER_DIAMETER_PX}
        />
      )}
      {showVertexMarkers &&
        bodies.map((b, bi) =>
          b.vertices.map((v, vi) => {
            const hovered =
              vertexHoverIdx !== null &&
              vertexHoverIdx[0] === bi &&
              vertexHoverIdx[1] === vi;
            return (
              <ScreenSpacePoint
                key={`v-${bi}-${vi}`}
                position={v.world}
                color={hovered ? ELEMENT_HOVER_COLOR : VERTEX_COLOR}
                px={VERTEX_HOVER_DIAMETER_PX}
                opacity={hovered ? 1 : 0.85}
              />
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
  const gl = useThree((s) => s.gl);
  // Fat node materials only render on WebGPU; the classic WebGLRenderer needs a
  // plain LineBasicMaterial (ticket gsoj — resolveIncludes crash otherwise).
  const isWebGPU = isWebGPURenderer(gl);
  const selected = state?.selectedElement ?? null;
  // Full multi-select set: a face primary highlights every selected face
  // (the coplanar extrude gather); edges/vertices stay single-primary.
  const selectedList = state?.selectedElements ?? [];
  const selectedKey = selectedList
    .map((e) => `${e.kind}:${e.idHash}`)
    .join(",");

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
  // Selected edges use a *fat* line on WebGPU (LineSegments2 +
  // Line2NodeMaterial): a plain LineBasicMaterial silently ignores `linewidth`
  // there, so the chunky 4 px Onshape stroke (ticket 6btt) only materialises
  // through the instanced screen-space line shader. On the classic
  // WebGLRenderer the node material would crash in resolveIncludes (ticket
  // gsoj), so makeFatLine falls back to a plain LineSegments (1 px) there.
  const edgeMatsRef = useRef(
    createFatLineMaterials({ color: ELEMENT_SELECTION_COLOR, linewidth: 4 }),
  );
  // A selected FULL-PRIMITIVE face (a sphere/torus face — no mesh, no
  // boundary edges, not planar) tints the matching hybrid leaf primitive,
  // like the body highlight does (ticket yiis).
  const leafGroupRef = useRef<THREE.Group | null>(null);
  const disposeLeafGroup = () => {
    leafGroupRef.current?.traverse((o) => {
      const m = o as THREE.Mesh;
      if (m.geometry) m.geometry.dispose();
    });
    leafGroupRef.current = null;
  };
  // The selected edge gets a BRAND-NEW LineSegments2 + geometry per selection,
  // built in the edge branch of `resolved` below — never an in-place mutation
  // of one persistent geometry. The WebGPU renderer caches a
  // LineSegmentsGeometry's instanced GPU buffers and does NOT re-upload them
  // after `setPositions` overwrites them, so reusing one geometry froze the
  // highlight on the first edge ever selected (ticket twxw: "immer dieselbe
  // Kante"). Handing the renderer a fresh object forces fresh buffers. The ref
  // only tracks the current object so the resolver can dispose the previous.
  const edgeObjRef = useRef<LineSegments2 | THREE.LineSegments | null>(null);

  useEffect(() => {
    const fg = faceGeomRef.current;
    const fm = faceMatRef.current;
    const em = edgeMatsRef.current;
    return () => {
      fg.dispose();
      fm.dispose();
      edgeObjRef.current?.geometry.dispose();
      em.node.dispose();
      em.classic.dispose();
      disposeLeafGroup();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Body fingerprint: re-resolve the highlight when bodies actually change
  // (same trick as the hover overlays), since a regen invalidates idHashes.
  const bodyFingerprint = useBodyFingerprint();

  // Resolve the WHOLE multi-select set to renderable data (box select can
  // put many faces, edges and vertices in it at once): face triangles go
  // into the shared face geometry, face outlines + selected edges into one
  // fresh fat-line object, vertices into world positions for the dot
  // markers.
  const resolved = useMemo<{
    hasFace: boolean;
    hasLines: boolean;
    hasLeafFill: boolean;
    vertices: [number, number, number][];
  } | null>(() => {
    if (!store || selectedList.length === 0) return null;
    const k = store.kernel;
    const count = k.bodyCount();

    const faceHashes = new Set(
      selectedList.filter((e) => e.kind === "face").map((e) => e.idHash),
    );
    const edgeHashes = new Set(
      selectedList.filter((e) => e.kind === "edge").map((e) => e.idHash),
    );
    const vertexHashes = new Set(
      selectedList.filter((e) => e.kind === "vertex").map((e) => e.idHash),
    );

    const tris: number[] = [];
    const vertices: [number, number, number][] = [];
    // Selected face ARENA indices per body (edge.faces association,
    // ticket b91c) — matching by branch signature would light the whole
    // coplanar family, since split coplanar faces share one branch id.
    const wantedArenasByBody = new Map<number, Set<number>>();
    const leafFills: HybridLeafInfo[] = [];

    // Pass 1 — faces (triangles where this kernel holds a mesh; in SDF render
    // modes the fill stays empty and the outline below is the highlight,
    // ticket evq2) and vertices.
    for (let i = 0; i < count; i++) {
      let placement: Placement;
      try {
        placement = k.bodyPlacement(i);
      } catch {
        continue;
      }
      if (faceHashes.size > 0) {
        let faces: BRepFace[];
        try {
          faces = k.bodyBrepFaces(i);
        } catch {
          faces = [];
        }
        const wantedFaces = faces.filter((f) => faceHashes.has(f.idHash));
        if (wantedFaces.length > 0) {
          wantedArenasByBody.set(
            i,
            new Set(wantedFaces.map((f) => f.faceIndex)),
          );
        }
        if (wantedFaces.length > 0 && k.bodyVertexCount(i) > 0) {
          try {
            const faceIndexView = k.bodyFaceIndexView(i).slice();
            const positions = k.bodyPositionsView(i).slice();
            const indices = k.bodyIndicesView(i).slice();
            const wanted = new Set(wantedFaces.map((f) => f.faceIndex));
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
          } catch {
            // mesh views unavailable mid-regen — outline still renders
          }
        } else if (wantedFaces.length > 0) {
          // Mesh-free (SDF render modes): fill PLANAR faces by triangulating
          // their boundary edges (ticket por8) — Onshape-style full-face
          // highlight instead of outline-only. Curved faces keep the outline.
          let edges: BRepEdge[] = [];
          try {
            edges = k.bodyBrepEdges(i);
          } catch {
            // mid-regen — outline still renders
          }
          for (const f of wantedFaces) {
            if (f.surfaceKind === "sphere" || f.surfaceKind === "torus") {
              // A full-primitive face: no boundary edges to outline and no
              // planar profile to fill — tint the nearest hybrid leaf of the
              // same kind, exactly like the body highlight (ticket yiis).
              try {
                const leaves = k
                  .bodyHybridInfo(i)
                  .leaves.filter(
                    (l) => l.kind === f.surfaceKind && l.role !== "skip",
                  );
                const c = localToWorld(f.centroidLocal, placement);
                let best: (typeof leaves)[number] | null = null;
                let bestD = Infinity;
                for (const l of leaves) {
                  const d =
                    (l.transform[0] - c[0]) ** 2 +
                    (l.transform[1] - c[1]) ** 2 +
                    (l.transform[2] - c[2]) ** 2;
                  if (d < bestD) {
                    best = l;
                    bestD = d;
                  }
                }
                if (best) leafFills.push(best);
              } catch {
                // no hybrid info — face stays un-highlighted
              }
              continue;
            }
            if (f.surfaceKind !== "plane") continue;
            const boundary = edges
              .filter((e) => e.faces.includes(f.faceIndex))
              .map((e) => e.points);
            if (boundary.length === 0) continue;
            const originW = localToWorld(f.centroidLocal, placement);
            const [qx, qy, qz, qw] = placement.rotation;
            const nl = f.normalLocal;
            const tx = 2 * (qy * nl[2] - qz * nl[1]);
            const ty = 2 * (qz * nl[0] - qx * nl[2]);
            const tz = 2 * (qx * nl[1] - qy * nl[0]);
            const normalW: [number, number, number] = [
              nl[0] + qw * tx + qy * tz - qz * ty,
              nl[1] + qw * ty + qz * tx - qx * tz,
              nl[2] + qw * tz + qx * ty - qy * tx,
            ];
            tris.push(...triangulatePlanarFace(boundary, originW, normalW));
          }
        }
      }
      if (vertexHashes.size > 0) {
        try {
          for (const v of k.bodyBrepVertices(i)) {
            if (vertexHashes.has(v.idHash))
              vertices.push(localToWorld(v.positionLocal, placement));
          }
        } catch {
          // mid-regen
        }
      }
    }

    // Pass 2 — line work: every analytic edge that bounds a selected face
    // (outline, by face ARENA index — ticket b91c) or IS a selected edge.
    // BRepEdge.points are already world space (kernel.ts) — folding them
    // through the placement again would double-transform (see the hover
    // overlay).
    const segs: number[] = [];
    for (let i = 0; i < count; i++) {
      let edges: BRepEdge[];
      try {
        edges = k.bodyBrepEdges(i);
      } catch {
        continue;
      }
      const wantedArenas = wantedArenasByBody.get(i);
      for (const e of edges) {
        const boundsSelectedFace =
          wantedArenas !== undefined &&
          e.faces.some((fi) => wantedArenas.has(fi));
        if (!boundsSelectedFace && !edgeHashes.has(e.idHash)) continue;
        segs.push(...polylineToSegments(e.points));
      }
    }

    if (
      tris.length === 0 &&
      segs.length === 0 &&
      vertices.length === 0 &&
      leafFills.length === 0
    ) {
      disposeLeafGroup();
      return null;
    }

    // Full-primitive face fills (sphere/torus, ticket yiis): fresh group per
    // selection, same dispose pattern as the edge object.
    disposeLeafGroup();
    if (leafFills.length > 0) {
      const group = new THREE.Group();
      for (const leaf of leafFills) {
        const geo = createPrimitiveGeometry(leaf);
        if (!geo) continue;
        const mesh = new THREE.Mesh(geo, faceMatRef.current);
        applyLeafTransform(mesh, leaf);
        mesh.frustumCulled = false;
        mesh.renderOrder = 1001;
        mesh.layers.enable(OVERLAY_LAYER);
        group.add(mesh);
      }
      if (group.children.length > 0) leafGroupRef.current = group;
    }

    faceGeomRef.current.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(tris, 3),
    );
    faceGeomRef.current.computeBoundingSphere();

    if (segs.length > 0) {
      // Fresh fat-line object per selection — the WebGPU backend does not
      // re-upload a LineSegmentsGeometry mutated in place (ticket twxw).
      const { object: obj } = makeFatLine(segs, edgeMatsRef.current, isWebGPU);
      obj.frustumCulled = false;
      obj.renderOrder = 1002;
      obj.layers.enable(OVERLAY_LAYER);
      edgeObjRef.current?.geometry.dispose();
      edgeObjRef.current = obj as LineSegments2 | THREE.LineSegments;
    } else {
      edgeObjRef.current?.geometry.dispose();
      edgeObjRef.current = null;
    }

    return {
      hasFace: tris.length > 0,
      hasLines: segs.length > 0,
      hasLeafFill: leafGroupRef.current !== null,
      vertices,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, selected, selectedKey, bodyFingerprint, isWebGPU]);

  if (!resolved) return null;

  return (
    <>
      {resolved.hasFace && (
        <mesh
          ref={(obj: THREE.Mesh | null) => {
            if (obj) obj.layers.enable(OVERLAY_LAYER);
          }}
          geometry={faceGeomRef.current}
          material={faceMatRef.current}
          frustumCulled={false}
          renderOrder={1001}
        />
      )}
      {/* `dispose={null}`: the object identity changes per selection, so R3F
          remounts this primitive each time. Without it R3F would auto-dispose
          the outgoing object on unmount — including the *shared* material —
          breaking every later highlight. The previous geometry is disposed in
          the memo above. */}
      {resolved.hasLines && edgeObjRef.current && (
        <primitive object={edgeObjRef.current} dispose={null} />
      )}
      {resolved.hasLeafFill && leafGroupRef.current && (
        <primitive object={leafGroupRef.current} dispose={null} />
      )}
      {resolved.vertices.map((w, i) => (
        <ScreenSpacePoint
          key={`sv-${i}`}
          position={w}
          color={ELEMENT_SELECTION_COLOR}
          px={VERTEX_SELECT_DIAMETER_PX}
          renderOrder={1003}
        />
      ))}
    </>
  );
}

/**
 * Whole-body selection highlight (ticket evq2): when a body is selected —
 * second click on an already-selected face, or a Parts-list row — every
 * analytic B-Rep edge of `state.highlightedBodyIndex` lights up as a fat
 * line in the element-selection colour. Mesh-free, so it works in every
 * render mode, exactly like the edge/face outlines. A body WITHOUT any
 * B-Rep edge (a bare sphere/torus) tints its analytic leaf primitives
 * instead — the selection reads as the round body itself, Onshape-style,
 * not as its AABB wireframe (ticket por8).
 */
export function BodyHighlightOverlay() {
  const { store, state } = useKernelStore();
  const gl = useThree((s) => s.gl);
  // Fat node materials only render on WebGPU; the classic WebGLRenderer needs a
  // plain LineBasicMaterial (ticket gsoj — resolveIncludes crash otherwise).
  const isWebGPU = isWebGPURenderer(gl);
  const bodyIndex = state?.highlightedBodyIndex ?? null;

  const matsRef = useRef(
    createFatLineMaterials({ color: ELEMENT_SELECTION_COLOR, linewidth: 2 }),
  );
  const fillMatRef = useRef(
    new THREE.MeshBasicMaterial({
      color: ELEMENT_SELECTION_COLOR,
      transparent: true,
      opacity: 0.65,
      depthTest: false,
      depthWrite: false,
    }),
  );
  const objRef = useRef<THREE.Object3D | null>(null);
  const disposeObj = () => {
    objRef.current?.traverse((o) => {
      const m = o as THREE.Mesh;
      if (m.geometry) m.geometry.dispose();
    });
    objRef.current = null;
  };
  useEffect(() => {
    const mats = matsRef.current;
    const fillMat = fillMatRef.current;
    return () => {
      disposeObj();
      mats.node.dispose();
      mats.classic.dispose();
      fillMat.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const bodyFingerprint = useBodyFingerprint();

  const ready = useMemo(() => {
    if (!store || bodyIndex === null) return false;
    const k = store.kernel;
    if (bodyIndex >= k.bodyCount()) return false;
    let edges: BRepEdge[];
    try {
      edges = k.bodyBrepEdges(bodyIndex);
    } catch {
      return false;
    }
    const segs: number[] = [];
    for (const e of edges) segs.push(...polylineToSegments(e.points));
    if (segs.length === 0) {
      // Bodies without any B-Rep edge (a bare sphere/torus): tint the body's
      // analytic leaf primitives — the SAME hull shapes the deferred renderer
      // rasterises — so the selection is the round body itself, not its AABB
      // box (ticket por8). Leaf transforms are already world space.
      try {
        const hybrid = k.bodyHybridInfo(bodyIndex);
        const group = new THREE.Group();
        for (const leaf of hybrid.leaves) {
          if (leaf.role === "skip") continue;
          const warped = leaf.warp ? buildWarpedLeafGeometry(leaf) : null;
          const geo = warped ?? createPrimitiveGeometry(leaf);
          if (!geo) continue;
          const mesh = new THREE.Mesh(geo, fillMatRef.current);
          if (!warped) applyLeafTransform(mesh, leaf); // warped geo is world-space
          mesh.frustumCulled = false;
          mesh.renderOrder = 1001;
          mesh.layers.enable(OVERLAY_LAYER);
          group.add(mesh);
        }
        if (group.children.length > 0) {
          disposeObj();
          objRef.current = group;
          return true;
        }
      } catch {
        // no hybrid info — fall through to the AABB wireframe
      }
      // Last resort (no analytic leaves either, e.g. a warped TPMS body):
      // the 12 edges of the body's world AABB from the compiled SDF.
      try {
        const aabb = k.bodySdfAabb(bodyIndex);
        if (aabb.length >= 6) {
          const c: [number, number, number][] = [
            [aabb[0], aabb[1], aabb[2]],
            [aabb[3], aabb[1], aabb[2]],
            [aabb[3], aabb[4], aabb[2]],
            [aabb[0], aabb[4], aabb[2]],
            [aabb[0], aabb[1], aabb[5]],
            [aabb[3], aabb[1], aabb[5]],
            [aabb[3], aabb[4], aabb[5]],
            [aabb[0], aabb[4], aabb[5]],
          ];
          const boxEdges = [
            [0, 1],
            [1, 2],
            [2, 3],
            [3, 0],
            [4, 5],
            [5, 6],
            [6, 7],
            [7, 4],
            [0, 4],
            [1, 5],
            [2, 6],
            [3, 7],
          ];
          for (const [a, b] of boxEdges) segs.push(...c[a], ...c[b]);
        }
      } catch {
        // no compiled SDF either — nothing to draw
      }
    }
    if (segs.length === 0) return false;
    // Fresh geometry + object per change — the WebGPU backend does not
    // re-upload a LineSegmentsGeometry mutated in place (ticket twxw).
    const { object: obj } = makeFatLine(segs, matsRef.current, isWebGPU);
    obj.frustumCulled = false;
    obj.renderOrder = 1002;
    obj.layers.enable(OVERLAY_LAYER);
    disposeObj();
    objRef.current = obj;
    return true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, bodyIndex, bodyFingerprint, isWebGPU]);

  if (!ready || !objRef.current) return null;
  return <primitive object={objRef.current} dispose={null} />;
}
