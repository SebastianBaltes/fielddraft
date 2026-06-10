/**
 * Extrude depth gizmo (ticket jn3t, Onshape-style).
 *
 * While a single extrude feature is selected, an arrow sits on the
 * extrude's tip face pointing along its outward normal:
 *   - the extruded slab renders as a translucent prism (tip outline
 *     swept back by the current depth) so the affected region reads
 *     at a glance;
 *   - dragging the arrow adjusts the `depth` param strictly along the
 *     normal, with throttled live commits (the SelectionTransformGizmo
 *     pattern) so the body follows the cursor;
 *   - while the drag SHRINKS the extrude relative to its drag-start
 *     depth, the prism tints red (verkleinern), otherwise accent blue
 *     (erweitern).
 *
 * Pointer handling is imperative (capture-phase listener on the WebGPU
 * canvas + raycast against the arrow meshes) because the SDF pipeline
 * bypasses R3F's own event raycasting; OrbitControls is disabled for
 * the duration of a drag.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { findSingleSelectedFeature } from "./viewportOverlays";
import { OVERLAY_LAYER } from "./viewportSdfOverlays";
import {
  axisDepthParam,
  lengthParamToMm,
  lengthParamWithMm,
  resolveExtrudeAnchor,
  type ExtrudeAnchor,
} from "./extrudeGizmo";

/** Arrow length in CSS pixels (shaft + head), Onshape-sized. */
const ARROW_PX = 96;
const GROW_COLOR = 0x2563eb;
const SHRINK_COLOR = 0xdc2626;
/** Arrow color (selection gold-orange — the Move gizmo already owns
 *  green/blue/red axes at the same anchor, so the depth arrow must
 *  read as its own control). */
const ARROW_COLOR = 0xff7a00;
/** Throttle for live depth commits during a drag (ms) — matches the
 *  transform gizmo's cadence. */
const COMMIT_THROTTLE_MS = 50;
const MIN_DEPTH_MM = 0.1;

const UP = new THREE.Vector3(0, 1, 0);
const scratchCam = new THREE.Vector3();

export function ExtrudeDepthGizmo() {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const defaultControls = useThree((s) => s.controls) as { enabled: boolean } | null;
  const invalidate = useThree((s) => s.invalidate);

  const selected = findSingleSelectedFeature(state);
  const extrudeId =
    selected?.kind === "extrude" && !selected.suppressed && !selected.rolled_back
      ? selected.id
      : null;

  // Drag state in a ref (pointer handlers are imperative); the small
  // `shrinking` state only drives the tint re-render.
  const dragRef = useRef<{
    active: boolean;
    startDepth: number;
    startT: number;
    timer: ReturnType<typeof setTimeout> | undefined;
    pending: number | null;
  }>({ active: false, startDepth: 0, startT: 0, timer: undefined, pending: null });
  const [shrinking, setShrinking] = useState(false);
  const [dragging, setDragging] = useState(false);

  const arrowGroupRef = useRef<THREE.Group>(null);
  const headRef = useRef<THREE.Mesh>(null);
  const shaftRef = useRef<THREE.Mesh>(null);

  // Resolve the anchor against the live B-Rep. `state` in the deps
  // re-resolves after every commit (features array identity changes),
  // which also makes the prism follow live depth updates mid-drag.
  const anchor = useMemo<ExtrudeAnchor | null>(() => {
    if (!store || extrudeId === null) return null;
    try {
      return resolveExtrudeAnchor(store.kernel, extrudeId);
    } catch {
      return null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, extrudeId, state]);

  // Current depth (mm) straight from the feature params.
  const depthInfo = useMemo<{ depthMm: number; symmetric: boolean } | null>(() => {
    if (!store || extrudeId === null) return null;
    try {
      const params = store.getFeatureParams(extrudeId);
      const depthMm = lengthParamToMm(params.depth);
      if (depthMm === null) return null;
      const backMm = lengthParamToMm(params.depth_back) ?? 0;
      const symmetric = params.symmetric !== false;
      return { depthMm: depthMm + (symmetric ? 0 : backMm), symmetric };
    } catch {
      return null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, extrudeId, state]);

  // Translucent slab prism: tip outline swept back by the total depth,
  // plus a centroid-fan cap on the tip. The fan is exact for profiles
  // star-shaped around the centroid (every primitive profile) and only
  // cosmetically off for exotic concave ones.
  const slabGeom = useMemo<THREE.BufferGeometry | null>(() => {
    if (!anchor || !depthInfo || anchor.outline.length === 0) return null;
    const [nx, ny, nz] = anchor.normal;
    const d = depthInfo.depthMm;
    const dx = -nx * d;
    const dy = -ny * d;
    const dz = -nz * d;
    const tris: number[] = [];
    const [cx, cy, cz] = anchor.origin;
    for (const pts of anchor.outline) {
      for (let i = 0; i + 5 < pts.length; i += 3) {
        const px = pts[i], py = pts[i + 1], pz = pts[i + 2];
        const qx = pts[i + 3], qy = pts[i + 4], qz = pts[i + 5];
        // Side wall quad (p,q swept by −depth·n).
        tris.push(px, py, pz, qx, qy, qz, qx + dx, qy + dy, qz + dz);
        tris.push(px, py, pz, qx + dx, qy + dy, qz + dz, px + dx, py + dy, pz + dz);
        // Tip cap fan around the centroid.
        tris.push(cx, cy, cz, px, py, pz, qx, qy, qz);
      }
    }
    if (tris.length === 0) return null;
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.Float32BufferAttribute(tris, 3));
    g.computeBoundingSphere();
    return g;
  }, [anchor, depthInfo]);

  useEffect(() => () => slabGeom?.dispose(), [slabGeom]);

  // Keep the arrow at a constant screen size, anchored on the tip face.
  useFrame(({ camera: cam, size }) => {
    const g = arrowGroupRef.current;
    if (!g || !anchor) return;
    g.position.set(anchor.origin[0], anchor.origin[1], anchor.origin[2]);
    g.quaternion.setFromUnitVectors(
      UP,
      scratchCam.set(anchor.normal[0], anchor.normal[1], anchor.normal[2]),
    );
    let wpp: number;
    if ((cam as THREE.PerspectiveCamera).isPerspectiveCamera) {
      const pc = cam as THREE.PerspectiveCamera;
      const dist = scratchCam
        .set(anchor.origin[0], anchor.origin[1], anchor.origin[2])
        .distanceTo(pc.position);
      wpp = (2 * dist * Math.tan((pc.fov * Math.PI) / 360)) / size.height;
    } else {
      const oc = cam as THREE.OrthographicCamera;
      wpp = (oc.top - oc.bottom) / oc.zoom / size.height;
    }
    const s = Math.max(1e-6, wpp * ARROW_PX);
    g.scale.setScalar(s);
  });

  // Live anchor/depth in refs so the drag effect does NOT depend on
  // them: every throttled commit changes `state`, and tearing the
  // listeners down mid-drag would kill the drag.
  const anchorRef = useRef(anchor);
  anchorRef.current = anchor;
  const depthInfoRef = useRef(depthInfo);
  depthInfoRef.current = depthInfo;

  // Imperative drag wiring (capture phase beats OrbitControls + the
  // viewport click handler on the same canvas).
  useEffect(() => {
    if (!store || extrudeId === null) return;
    const dom = gl.domElement;
    const raycaster = new THREE.Raycaster();
    raycaster.layers.enable(OVERLAY_LAYER);
    const drag = dragRef.current;
    // Axis frozen at drag start — the tip face (and with it the live
    // anchor) moves while commits land, and measuring the pointer
    // against a moving axis would feed back into the depth.
    let axisOrigin: [number, number, number] = [0, 0, 0];
    let axisNormal: [number, number, number] = [0, 0, 1];

    const rayFromEvent = (e: PointerEvent) => {
      const rect = dom.getBoundingClientRect();
      const ndc = new THREE.Vector2(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1,
      );
      raycaster.setFromCamera(ndc, camera);
      return raycaster.ray;
    };

    const axisParam = (ray: THREE.Ray): number | null =>
      axisDepthParam(
        axisOrigin,
        axisNormal,
        [ray.origin.x, ray.origin.y, ray.origin.z],
        [ray.direction.x, ray.direction.y, ray.direction.z],
      );

    const commitDepth = (mm: number) => {
      try {
        const params = store.getFeatureParams(extrudeId);
        store.updateFeatureParams(extrudeId, {
          ...params,
          depth: lengthParamWithMm(params.depth, mm),
        });
      } catch {
        // kernel rejected — drop this sample, keep dragging
      }
    };

    const flushThrottled = (mm: number) => {
      drag.pending = mm;
      if (drag.timer !== undefined) return;
      drag.pending = null;
      commitDepth(mm);
      drag.timer = setTimeout(() => {
        drag.timer = undefined;
        if (drag.pending !== null) {
          const p = drag.pending;
          drag.pending = null;
          commitDepth(p);
        }
      }, COMMIT_THROTTLE_MS);
    };

    const onDown = (e: PointerEvent) => {
      if (e.button !== 0) return;
      const a = anchorRef.current;
      const di = depthInfoRef.current;
      const head = headRef.current;
      const shaft = shaftRef.current;
      if (!a || !di || !head || !shaft) return;
      const ray = rayFromEvent(e);
      const hits = raycaster.intersectObjects([head, shaft], false);
      if (hits.length === 0) return;
      axisOrigin = [...a.origin] as [number, number, number];
      axisNormal = [...a.normal] as [number, number, number];
      const t0 = axisParam(ray);
      if (t0 === null) return;
      drag.active = true;
      drag.startDepth = di.depthMm;
      drag.startT = t0;
      setDragging(true);
      setShrinking(false);
      if (defaultControls) defaultControls.enabled = false;
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    const onMove = (e: PointerEvent) => {
      if (!drag.active) return;
      const t = axisParam(rayFromEvent(e));
      if (t === null) return;
      // Symmetric extrudes grow ±depth/2 about the plane — the tip only
      // moves half a depth unit per drag unit, so double the delta to
      // keep the arrow glued to the cursor.
      const factor = depthInfoRef.current?.symmetric ? 2 : 1;
      const next = Math.max(MIN_DEPTH_MM, drag.startDepth + (t - drag.startT) * factor);
      setShrinking(next < drag.startDepth - 1e-6);
      flushThrottled(next);
      invalidate();
      e.preventDefault();
      e.stopImmediatePropagation();
    };

    // The pointerup of a finished drag still spawns a synthetic `click`
    // on the canvas; without swallowing it, the viewport click handler
    // picks whatever is under the cursor and REPLACES the extrude
    // selection (killing the gizmo mid-interaction).
    let suppressNextClick = false;

    const onUp = (e: PointerEvent) => {
      if (!drag.active) return;
      drag.active = false;
      suppressNextClick = true;
      setDragging(false);
      setShrinking(false);
      clearTimeout(drag.timer);
      drag.timer = undefined;
      if (drag.pending !== null) {
        const p = drag.pending;
        drag.pending = null;
        commitDepth(p);
      }
      if (defaultControls) defaultControls.enabled = true;
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
      clearTimeout(drag.timer);
      drag.timer = undefined;
      if (drag.active && defaultControls) defaultControls.enabled = true;
      drag.active = false;
    };
  }, [store, extrudeId, gl, camera, defaultControls, invalidate]);

  if (!anchor || !depthInfo) return null;

  const slabTint = shrinking ? SHRINK_COLOR : GROW_COLOR;
  const arrowTint = shrinking ? SHRINK_COLOR : ARROW_COLOR;
  const enableOverlay = (obj: THREE.Object3D | null) => {
    if (obj) obj.layers.enable(OVERLAY_LAYER);
  };

  return (
    <>
      {slabGeom && (
        <mesh ref={enableOverlay} geometry={slabGeom} frustumCulled={false} renderOrder={1000}>
          <meshBasicMaterial
            color={slabTint}
            transparent
            opacity={dragging ? 0.28 : 0.16}
            depthWrite={false}
            side={THREE.DoubleSide}
          />
        </mesh>
      )}
      {/* Arrow: unit-sized (1 = ARROW_PX on screen), rescaled per frame.
          Shaft along +Y from the face, cone head at the far end. */}
      <group ref={arrowGroupRef}>
        <mesh
          ref={(m: THREE.Mesh | null) => { shaftRef.current = m; enableOverlay(m); }}
          position={[0, 0.36, 0]}
          frustumCulled={false}
          renderOrder={1004}
        >
          <cylinderGeometry args={[0.05, 0.05, 0.72, 12]} />
          <meshBasicMaterial color={arrowTint} depthTest={false} depthWrite={false} transparent />
        </mesh>
        <mesh
          ref={(m: THREE.Mesh | null) => { headRef.current = m; enableOverlay(m); }}
          position={[0, 0.86, 0]}
          frustumCulled={false}
          renderOrder={1004}
        >
          <coneGeometry args={[0.16, 0.28, 16]} />
          <meshBasicMaterial color={arrowTint} depthTest={false} depthWrite={false} transparent />
        </mesh>
      </group>
    </>
  );
}
