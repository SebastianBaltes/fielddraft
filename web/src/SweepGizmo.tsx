/**
 * Interactive Sweep 3D path gizmo.
 *
 * While a single `sweep` feature whose path is `kind: "gizmo3d"` is selected,
 * each control point gets a draggable handle sphere in the viewport and the
 * resolved spine renders as a light-blue spline preview. Dragging a handle
 * moves that control point in a screen-parallel plane through its current
 * position (orbit to reach the third axis), rewriting `path.nodes`:
 *   - with the "Live drag" setting ON (default), the node commits throttled
 *     (50ms) WHILE the pointer is down — the body re-weaves live and the heavy
 *     B-Rep weave is suppressed for the drag via begin/endGizmoDrag;
 *   - with it OFF (or for a PENDING ghost, whose body excludes the sweep),
 *     nothing commits during the drag: handle + spline follow the pointer every
 *     frame and the new node commits ONCE on pointerup.
 *
 * Pointer handling is imperative (capture-phase listener on the WebGPU canvas +
 * raycast against the handle spheres) because the SDF pipeline bypasses R3F's
 * own event raycasting; OrbitControls is disabled for the duration of a drag.
 * This mirrors ExtrudeGizmo.tsx — the difference is N free-moving 3D handles
 * instead of one axis-locked arrow.
 */

import { useEffect, useMemo, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import {
  createFatLineMaterials,
  isWebGPURenderer,
  makeFatLine,
} from "./overlayLineMaterials";
import { useKernelStore } from "./store";
import { findSingleSelectedFeature } from "./viewportOverlays";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import { catmullRomSpline, polylineToSegments, type V3 } from "./sweepGizmo";

/** Handle sphere radius in CSS pixels (constant screen size). */
const HANDLE_PX = 8;
/** BackSide outline shell scale (toon-outline trick, ≈1px rim). */
const OUTLINE_SCALE = 1.3;
const HANDLE_FILL = 0xffffff;
const HANDLE_OUTLINE = 0x1f2937;
const HANDLE_HOVER = 0x2563eb;
const HANDLE_ACTIVE = 0xf59e0b;
const PATH_COLOR = 0x2563eb;
/** Spline samples per control-point span for the preview line. */
const SPLINE_SPS = 24;

interface PathInfo {
  nodes: V3[];
  closed: boolean;
}

/** World units per CSS pixel at `pos`, for constant-screen-size handles. */
function worldPerPixel(cam: THREE.Camera, pos: THREE.Vector3, height: number): number {
  const pc = cam as THREE.PerspectiveCamera;
  if (pc.isPerspectiveCamera) {
    const dist = pos.distanceTo(pc.position);
    return (2 * dist * Math.tan((pc.fov * Math.PI) / 360)) / height;
  }
  const oc = cam as THREE.OrthographicCamera;
  return (oc.top - oc.bottom) / oc.zoom / height;
}

export function SweepPathGizmo() {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const defaultControls = useThree((s) => s.controls) as { enabled: boolean } | null;
  const invalidate = useThree((s) => s.invalidate);

  const selected = findSingleSelectedFeature(state);
  // A PENDING sweep (created suppressed, awaiting dialog confirmation) keeps
  // the gizmo alive: the spline preview IS its preview.
  const pending =
    selected != null && store !== null && store.isPendingFeature(selected.id);
  const sweepId =
    selected?.kind === "sweep" &&
    !selected.rolled_back &&
    (!selected.suppressed || pending)
      ? selected.id
      : null;

  // The gizmo only handles the Gizmo3d path source (control points). Other path
  // sources (Sketch2d, EdgeChain, OrthogonalSketches) are edited at the source.
  const pathInfo = useMemo<PathInfo | null>(() => {
    if (!store || sweepId === null) return null;
    try {
      const params = store.getFeatureParams(sweepId);
      const path = params.path as
        | { kind?: string; nodes?: unknown; closed?: unknown }
        | undefined;
      if (!path || path.kind !== "gizmo3d" || !Array.isArray(path.nodes)) return null;
      const nodes = path.nodes
        .filter((n): n is number[] => Array.isArray(n) && n.length >= 3)
        .map((n) => [n[0], n[1], n[2]] as V3);
      if (nodes.length < 2) return null;
      return { nodes, closed: path.closed === true };
    } catch {
      return null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, sweepId, state]);

  // Live node positions read by useFrame (so handles glue to the pointer
  // without waiting for React). Reset from pathInfo whenever the feature
  // params change — but NEVER mid-drag (that would clobber the in-flight node,
  // mirroring ExtrudeGizmo's frozen anchor).
  const displayNodesRef = useRef<V3[]>([]);
  const splineDirtyRef = useRef(true);
  const hoverRef = useRef(-1);
  const dragRef = useRef<{
    active: boolean;
    index: number;
    live: boolean;
    plane: THREE.Plane;
    grab: THREE.Vector3;
  }>({ active: false, index: -1, live: false, plane: new THREE.Plane(), grab: new THREE.Vector3() });

  useEffect(() => {
    if (dragRef.current.active) return; // don't clobber an in-progress drag
    displayNodesRef.current = (pathInfo?.nodes ?? []).map((p) => [...p] as V3);
    splineDirtyRef.current = true;
    invalidate();
  }, [pathInfo, invalidate]);

  // Refs the drag effect reads without re-subscribing.
  const pathInfoRef = useRef(pathInfo);
  pathInfoRef.current = pathInfo;

  const nodeGroupRefs = useRef<(THREE.Group | null)[]>([]);
  const fillMeshRefs = useRef<(THREE.Mesh | null)[]>([]);

  // Shared geometries (one unit sphere for every handle).
  const sphereGeo = useMemo(() => new THREE.SphereGeometry(1, 16, 12), []);
  useEffect(() => () => sphereGeo.dispose(), [sphereGeo]);

  // Fat node materials only render on WebGPU; the classic WebGLRenderer crashes
  // in resolveIncludes on a node material (ticket gsoj), so build the preview
  // through the shared backend-aware fat-line factory.
  const isWebGPU = isWebGPURenderer(gl);

  // The spine spline preview, built imperatively so the geometry is rebuilt
  // each drag frame without remounting. `setPathPositions` swaps the geometry
  // backend-correctly: a fat `LineSegmentsGeometry` on WebGPU, a plain
  // BufferGeometry attribute on the classic WebGLRenderer.
  const pathLine = useMemo(() => {
    const mats = createFatLineMaterials({ color: PATH_COLOR, linewidth: 2 });
    const { object: o } = makeFatLine(new Float32Array(0), mats, isWebGPU);
    o.frustumCulled = false;
    o.renderOrder = 1001;
    o.layers.enable(OVERLAY_LAYER);
    o.visible = false;
    return { object: o, mats };
  }, [isWebGPU]);
  const setPathPositions = (seg: Float32Array): void => {
    const o = pathLine.object as
      | THREE.LineSegments
      | { geometry: { setPositions(p: Float32Array): void } };
    if (isWebGPU) {
      (o.geometry as { setPositions(p: Float32Array): void }).setPositions(seg);
    } else {
      const geom = (o as THREE.LineSegments).geometry as THREE.BufferGeometry;
      geom.setAttribute("position", new THREE.Float32BufferAttribute(seg, 3));
      geom.computeBoundingSphere();
    }
  };
  useEffect(
    () => () => {
      (pathLine.object as unknown as { geometry: { dispose(): void } }).geometry.dispose();
      pathLine.mats.node.dispose();
      pathLine.mats.classic.dispose();
    },
    [pathLine],
  );

  // Per-frame: glue handles to their (possibly dragged) nodes at constant
  // screen size, recolor for hover/active, and rebuild the spline when dirty.
  useFrame(({ camera: cam, size }) => {
    const nodes = displayNodesRef.current;
    for (let i = 0; i < nodes.length; i++) {
      const g = nodeGroupRefs.current[i];
      if (!g) continue;
      g.position.set(nodes[i][0], nodes[i][1], nodes[i][2]);
      const s = Math.max(1e-6, worldPerPixel(cam, g.position, size.height) * HANDLE_PX);
      g.scale.setScalar(s);
      const fill = fillMeshRefs.current[i];
      if (fill) {
        const want =
          dragRef.current.active && dragRef.current.index === i
            ? HANDLE_ACTIVE
            : hoverRef.current === i
              ? HANDLE_HOVER
              : HANDLE_FILL;
        const mat = fill.material as THREE.MeshBasicMaterial;
        if (mat.color.getHex() !== want) mat.color.setHex(want);
      }
    }
    if (splineDirtyRef.current) {
      const closed = pathInfoRef.current?.closed ?? false;
      const seg = polylineToSegments(catmullRomSpline(nodes, closed, SPLINE_SPS), closed);
      if (seg.length >= 6) {
        setPathPositions(new Float32Array(seg));
        pathLine.object.visible = true;
      } else {
        pathLine.object.visible = false;
      }
      splineDirtyRef.current = false;
    }
  });

  // Imperative drag wiring (capture phase beats OrbitControls + the viewport
  // click handler on the same canvas).
  useEffect(() => {
    if (!store || sweepId === null) return;
    const dom = gl.domElement;
    const raycaster = new THREE.Raycaster();
    raycaster.layers.enable(OVERLAY_LAYER);
    const drag = dragRef.current;
    const camDir = new THREE.Vector3();
    const hitPt = new THREE.Vector3();

    // Live-drag commit throttle (50ms, mirrors ExtrudeGizmo).
    let liveTimer: ReturnType<typeof setTimeout> | undefined;
    let livePending = false;
    let liveSnapshotTaken = false;
    let lastKey = "";

    const rayFromEvent = (e: PointerEvent) => {
      const rect = dom.getBoundingClientRect();
      const ndc = new THREE.Vector2(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1,
      );
      raycaster.setFromCamera(ndc, camera);
      return raycaster.ray;
    };

    const pickHandle = (e: PointerEvent): number => {
      rayFromEvent(e);
      const meshes = fillMeshRefs.current.filter(Boolean) as THREE.Mesh[];
      const hits = raycaster.intersectObjects(meshes, false);
      if (hits.length === 0) return -1;
      return fillMeshRefs.current.indexOf(hits[0].object as THREE.Mesh);
    };

    const commitNodes = (nodes: V3[], coalesce: boolean): boolean => {
      try {
        const params = store.getFeatureParams(sweepId);
        const path = params.path as Record<string, unknown> | undefined;
        if (!path || path.kind !== "gizmo3d") return false;
        return store.updateFeatureParams(
          sweepId,
          { ...params, path: { ...path, nodes: nodes.map((p) => [...p]) } },
          coalesce,
        );
      } catch {
        return false;
      }
    };

    const liveCommit = () => {
      livePending = false;
      const key = JSON.stringify(displayNodesRef.current[drag.index]);
      if (key === lastKey) return;
      if (commitNodes(displayNodesRef.current, liveSnapshotTaken)) {
        liveSnapshotTaken = true;
        lastKey = key;
      }
    };
    const scheduleLiveCommit = () => {
      livePending = true;
      if (liveTimer !== undefined) return;
      liveCommit();
      liveTimer = setTimeout(() => {
        liveTimer = undefined;
        if (livePending && drag.active) liveCommit();
      }, 50);
    };

    const onDown = (e: PointerEvent) => {
      if (e.button !== 0) return;
      const index = pickHandle(e);
      if (index < 0) return;
      const node = displayNodesRef.current[index];
      if (!node) return;
      // Screen-parallel drag plane through the grabbed node.
      const nodeV = new THREE.Vector3(node[0], node[1], node[2]);
      camera.getWorldDirection(camDir);
      drag.plane.setFromNormalAndCoplanarPoint(camDir, nodeV);
      if (!rayFromEvent(e).intersectPlane(drag.plane, hitPt)) return;
      drag.active = true;
      drag.index = index;
      drag.grab = nodeV.clone().sub(hitPt); // node = planeHit + grab
      // Live drag commits into the kernel — impossible for a PENDING ghost,
      // whose body excludes the suppressed sweep entirely.
      drag.live = store.isLiveExtrudeDrag() && !store.isPendingFeature(sweepId);
      liveSnapshotTaken = false;
      lastKey = "";
      hoverRef.current = index;
      if (drag.live) store.beginGizmoDrag();
      if (defaultControls) defaultControls.enabled = false;
      dom.style.cursor = "grabbing";
      invalidate();
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    const onMove = (e: PointerEvent) => {
      if (!drag.active) {
        // Hover highlight (no preventDefault — must not eat orbit drags).
        const idx = pickHandle(e);
        if (idx !== hoverRef.current) {
          hoverRef.current = idx;
          dom.style.cursor = idx >= 0 ? "grab" : "";
          invalidate();
        }
        return;
      }
      if (!rayFromEvent(e).intersectPlane(drag.plane, hitPt)) return;
      hitPt.add(drag.grab);
      displayNodesRef.current[drag.index] = [hitPt.x, hitPt.y, hitPt.z];
      splineDirtyRef.current = true;
      if (drag.live) scheduleLiveCommit();
      invalidate();
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    // The pointerup of a finished drag still spawns a synthetic `click`; swallow
    // it so the viewport click handler doesn't reselect under the cursor and
    // kill the gizmo mid-interaction (mirrors ExtrudeGizmo).
    let suppressNextClick = false;

    const onUp = (e: PointerEvent) => {
      if (!drag.active) return;
      drag.active = false;
      suppressNextClick = true;
      if (drag.live) {
        clearTimeout(liveTimer);
        liveTimer = undefined;
        commitNodes(displayNodesRef.current, liveSnapshotTaken);
        store.endGizmoDrag();
      } else {
        commitNodes(displayNodesRef.current, false);
      }
      drag.index = -1;
      hoverRef.current = -1;
      dom.style.cursor = "";
      if (defaultControls) defaultControls.enabled = true;
      invalidate();
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    const onClick = (e: MouseEvent) => {
      if (!suppressNextClick) return;
      suppressNextClick = false;
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    dom.addEventListener("pointerdown", onDown, { capture: true });
    window.addEventListener("pointermove", onMove, { capture: true });
    window.addEventListener("pointerup", onUp, { capture: true });
    dom.addEventListener("click", onClick, { capture: true });
    return () => {
      dom.removeEventListener("pointerdown", onDown, { capture: true });
      window.removeEventListener("pointermove", onMove, { capture: true });
      window.removeEventListener("pointerup", onUp, { capture: true });
      dom.removeEventListener("click", onClick, { capture: true });
      clearTimeout(liveTimer);
      dom.style.cursor = "";
      if (drag.active && defaultControls) defaultControls.enabled = true;
      if (drag.active && drag.live) store.endGizmoDrag();
      drag.active = false;
      drag.index = -1;
      hoverRef.current = -1;
    };
  }, [store, sweepId, gl, camera, defaultControls, invalidate]);

  if (!pathInfo) return null;

  const enableOverlay = (obj: THREE.Object3D | null) => {
    if (obj) obj.layers.enable(OVERLAY_LAYER);
  };

  return (
    <>
      <primitive object={pathLine.object} />
      {pathInfo.nodes.map((_, i) => (
        <group key={i} ref={(g: THREE.Group | null) => (nodeGroupRefs.current[i] = g)}>
          {/* Dark outline shell behind the white fill (toon-outline trick). */}
          <mesh
            geometry={sphereGeo}
            scale={OUTLINE_SCALE}
            frustumCulled={false}
            renderOrder={1003}
            ref={enableOverlay}
          >
            <meshBasicMaterial
              color={HANDLE_OUTLINE}
              side={THREE.BackSide}
              depthTest={false}
              depthWrite={false}
              transparent
            />
          </mesh>
          <mesh
            geometry={sphereGeo}
            frustumCulled={false}
            renderOrder={1004}
            ref={(m: THREE.Mesh | null) => {
              fillMeshRefs.current[i] = m;
              enableOverlay(m);
            }}
          >
            <meshBasicMaterial
              color={HANDLE_FILL}
              depthTest={false}
              depthWrite={false}
              transparent
            />
          </mesh>
        </group>
      ))}
    </>
  );
}
