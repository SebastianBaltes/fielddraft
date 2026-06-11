/**
 * Extrude depth gizmo (tickets jn3t + qqgo, Onshape-style).
 *
 * While a single extrude feature is selected, an arrow sits on the
 * extrude's tip face pointing along its outward normal:
 *   - the extruded slab renders as a translucent prism (tip outline
 *     swept back by the current depth) so the affected region reads
 *     at a glance;
 *   - dragging the arrow adjusts the `depth` param strictly along the
 *     normal. NOTHING is committed while the drag runs — the arrow and
 *     a local preview prism follow the pointer every frame (the kernel
 *     recompute would otherwise stall the gizmo), and the new depth is
 *     committed ONCE on pointerup. Delta bands tint what gets added
 *     blue and what gets removed red; since the body stays frozen at
 *     its drag-start shape, the grown region is genuinely see-through.
 *   - the arrow itself is black-and-white (white fill, dark outline),
 *     matching Onshape; grow/shrink is signalled by the bands only.
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
  faceQueryHashes,
  lengthParamToMm,
  lengthParamWithMm,
  previewBandRanges,
  prismBandTriangles,
  resolveExtrudeAnchor,
  resolvePendingExtrudeAnchor,
  type ExtrudeAnchor,
} from "./extrudeGizmo";

/** Arrow length in CSS pixels (shaft + head), Onshape-sized. */
const ARROW_PX = 96;
const GROW_COLOR = 0x2563eb;
const SHRINK_COLOR = 0xdc2626;
/** Onshape-style black-and-white arrow: white fill, near-black rim. */
const ARROW_FILL = 0xffffff;
const ARROW_OUTLINE = 0x1f2937;
/** BackSide outline shell scale (≈1px rim at ARROW_PX). */
const OUTLINE_SCALE = 1.18;
const PRISM_OPACITY = 0.16;
const DELTA_OPACITY = 0.3;
const MIN_DEPTH_MM = 0.1;

const UP = new THREE.Vector3(0, 1, 0);
const scratchCam = new THREE.Vector3();

interface DragPreview {
  depthMm: number;
  startDepthMm: number;
}

export function ExtrudeDepthGizmo() {
  const { store, state } = useKernelStore();
  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);
  const defaultControls = useThree((s) => s.controls) as { enabled: boolean } | null;
  const invalidate = useThree((s) => s.invalidate);

  const selected = findSingleSelectedFeature(state);
  // A PENDING extrude (created suppressed, awaiting dialog confirmation —
  // ticket x7af) keeps the gizmo alive: the ghost prism IS its preview.
  const pending =
    selected != null && store !== null && store.isPendingFeature(selected.id);
  const extrudeId =
    selected?.kind === "extrude" &&
    !selected.rolled_back &&
    (!selected.suppressed || pending)
      ? selected.id
      : null;

  // Drag state in a ref (pointer handlers are imperative); `preview`
  // mirrors it into React for the band meshes.
  const dragRef = useRef<{
    active: boolean;
    startDepth: number;
    startT: number;
    lastDepth: number;
  }>({ active: false, startDepth: 0, startT: 0, lastDepth: 0 });
  const [preview, setPreview] = useState<DragPreview | null>(null);
  // Tip travel (mm along the normal) during a drag — read by useFrame
  // so the arrow glues to the pointer without waiting for React.
  const dragTipOffRef = useRef(0);

  const arrowGroupRef = useRef<THREE.Group>(null);
  const headRef = useRef<THREE.Mesh>(null);
  const shaftRef = useRef<THREE.Mesh>(null);

  // Current depth (mm) straight from the feature params. `depthMm` is the
  // prism's axial EXTENT along the tip-out normal (always positive; depth,
  // or depth+depth_back for one-sided), which doubles as the
  // tip-cap-vs-wall discriminator below. The wire `depth` may be NEGATIVE
  // for a blind extrude growing in the −normal direction (ticket x7af) —
  // `sign` maps the gizmo's extent back onto the signed wire param.
  const depthInfo = useMemo<
    { depthMm: number; symmetric: boolean; sign: 1 | -1; rawMm: number } | null
  >(() => {
    if (!store || extrudeId === null) return null;
    try {
      const params = store.getFeatureParams(extrudeId);
      const rawMm = lengthParamToMm(params.depth);
      if (rawMm === null) return null;
      const backMm = Math.max(0, lengthParamToMm(params.depth_back) ?? 0);
      const symmetric = params.symmetric !== false;
      if (symmetric) {
        return { depthMm: Math.abs(rawMm), symmetric, sign: 1, rawMm };
      }
      const lo = Math.min(-backMm, rawMm);
      const hi = Math.max(-backMm, rawMm);
      return { depthMm: hi - lo, symmetric, sign: rawMm < 0 ? -1 : 1, rawMm };
    } catch {
      return null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, extrudeId, state]);

  // Resolve the anchor against the live B-Rep. `state` in the deps
  // re-resolves after the pointerup commit. The axial extent picks the
  // tip cap unambiguously even when the prism keeps both caps (ticket
  // zzl1) — see pickTipFace. A PENDING ghost contributes no faces of its
  // own, so its anchor derives from the PROFILE faces on the unextruded
  // body instead (ticket x7af) — that body is the depth's true reference.
  const anchor = useMemo<ExtrudeAnchor | null>(() => {
    if (!store || extrudeId === null) return null;
    try {
      if (pending) {
        if (!depthInfo) return null;
        const params = store.getFeatureParams(extrudeId);
        return resolvePendingExtrudeAnchor(
          store.kernel,
          faceQueryHashes(params.faces),
          depthInfo.symmetric ? depthInfo.depthMm / 2 : depthInfo.rawMm,
        );
      }
      return resolveExtrudeAnchor(store.kernel, extrudeId, depthInfo?.depthMm);
    } catch {
      return null;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, extrudeId, state, depthInfo, pending]);

  // Translucent preview bands: at rest, the whole extruded prism in
  // light blue; during a drag, the would-be prism plus delta bands
  // (blue = added, red = removed) — all driven locally, no commits.
  const bands = useMemo<
    Array<{ geom: THREE.BufferGeometry; color: number; opacity: number }>
  >(() => {
    if (!anchor || !depthInfo || anchor.outline.length === 0) return [];
    const out: Array<{ geom: THREE.BufferGeometry; color: number; opacity: number }> = [];
    const push = (a: number, b: number, color: number, opacity: number) => {
      if (Math.abs(a - b) < 1e-6) return;
      const tris = prismBandTriangles(anchor, a, b);
      if (tris.length === 0) return;
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute(tris, 3));
      g.computeBoundingSphere();
      out.push({ geom: g, color, opacity });
    };
    if (!preview) {
      // The at-rest prism: for a PENDING ghost this is the only rendering
      // the extrude gets at all (the body excludes it), so it draws at the
      // stronger delta opacity (ticket x7af).
      push(-depthInfo.depthMm, 0, GROW_COLOR, pending ? DELTA_OPACITY : PRISM_OPACITY);
    } else {
      const ranges = previewBandRanges(
        preview.startDepthMm,
        preview.depthMm,
        depthInfo.symmetric,
      );
      push(ranges.prism[0], ranges.prism[1], GROW_COLOR, PRISM_OPACITY);
      const tint = ranges.grow ? GROW_COLOR : SHRINK_COLOR;
      for (const [a, b] of ranges.delta) push(a, b, tint, DELTA_OPACITY);
    }
    return out;
  }, [anchor, depthInfo, preview, pending]);

  useEffect(
    () => () => {
      for (const b of bands) b.geom.dispose();
    },
    [bands],
  );

  // Keep the arrow at a constant screen size, anchored on the (possibly
  // drag-shifted) tip.
  useFrame(({ camera: cam, size }) => {
    const g = arrowGroupRef.current;
    if (!g || !anchor) return;
    const off = dragTipOffRef.current;
    g.position.set(
      anchor.origin[0] + anchor.normal[0] * off,
      anchor.origin[1] + anchor.normal[1] * off,
      anchor.origin[2] + anchor.normal[2] * off,
    );
    g.quaternion.setFromUnitVectors(
      UP,
      scratchCam.set(anchor.normal[0], anchor.normal[1], anchor.normal[2]),
    );
    let wpp: number;
    if ((cam as THREE.PerspectiveCamera).isPerspectiveCamera) {
      const pc = cam as THREE.PerspectiveCamera;
      const dist = scratchCam
        .set(g.position.x, g.position.y, g.position.z)
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
  // them (re-subscribing mid-drag would kill the drag).
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
    // Axis frozen at drag start — robust against any anchor churn while
    // the pointer is down.
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
        // `mm` is signed along the TIP-OUT axis; `sign` maps it back to the
        // wire depth along the profile normal. Crossing the base plane
        // (negative `mm`) flips the extrude direction (ticket x7af).
        const sign = depthInfoRef.current?.sign ?? 1;
        store.updateFeatureParams(extrudeId, {
          ...params,
          depth: lengthParamWithMm(params.depth, sign * mm),
        });
      } catch {
        // kernel rejected — the preview collapses back to the old depth
      }
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
      drag.lastDepth = di.depthMm;
      dragTipOffRef.current = 0;
      setPreview({ depthMm: di.depthMm, startDepthMm: di.depthMm });
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
      const di = depthInfoRef.current;
      const factor = di?.symmetric ? 2 : 1;
      let next = drag.startDepth + (t - drag.startT) * factor;
      if (di?.symmetric) {
        // Symmetric grows about the plane — sign has no direction meaning.
        next = Math.max(MIN_DEPTH_MM, next);
      } else if (Math.abs(next) < MIN_DEPTH_MM) {
        // Blind extrudes may cross the base plane and grow the other way
        // (ticket x7af) — only the degenerate zero extent is skipped.
        next = next >= 0 ? MIN_DEPTH_MM : -MIN_DEPTH_MM;
      }
      drag.lastDepth = next;
      dragTipOffRef.current = (next - drag.startDepth) / factor;
      setPreview({ depthMm: next, startDepthMm: drag.startDepth });
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
      // Single commit at drag end — the anchor jumps to the new tip in
      // the same flush that resets the local offset, so the arrow does
      // not move visually.
      if (Math.abs(drag.lastDepth - drag.startDepth) > 1e-6) {
        commitDepth(drag.lastDepth);
      }
      dragTipOffRef.current = 0;
      setPreview(null);
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
      if (drag.active && defaultControls) defaultControls.enabled = true;
      drag.active = false;
      dragTipOffRef.current = 0;
    };
  }, [store, extrudeId, gl, camera, defaultControls, invalidate]);

  if (!anchor || !depthInfo) return null;

  const enableOverlay = (obj: THREE.Object3D | null) => {
    if (obj) obj.layers.enable(OVERLAY_LAYER);
  };

  return (
    <>
      {bands.map((b, i) => (
        <mesh
          key={i}
          ref={enableOverlay}
          geometry={b.geom}
          frustumCulled={false}
          renderOrder={1000}
        >
          <meshBasicMaterial
            color={b.color}
            transparent
            opacity={b.opacity}
            depthWrite={false}
            side={THREE.DoubleSide}
          />
        </mesh>
      ))}
      {/* Arrow: unit-sized (1 = ARROW_PX on screen), rescaled per frame.
          Shaft along +Y from the face, cone head at the far end. The
          outline is a BackSide shell slightly scaled up behind the
          white fill (toon-outline trick). */}
      <group ref={arrowGroupRef}>
        <mesh
          position={[0, 0.36, 0]}
          scale={OUTLINE_SCALE}
          frustumCulled={false}
          renderOrder={1003}
          ref={enableOverlay}
        >
          <cylinderGeometry args={[0.05, 0.05, 0.72, 12]} />
          <meshBasicMaterial
            color={ARROW_OUTLINE}
            side={THREE.BackSide}
            depthTest={false}
            depthWrite={false}
            transparent
          />
        </mesh>
        <mesh
          position={[0, 0.86, 0]}
          scale={OUTLINE_SCALE}
          frustumCulled={false}
          renderOrder={1003}
          ref={enableOverlay}
        >
          <coneGeometry args={[0.16, 0.28, 16]} />
          <meshBasicMaterial
            color={ARROW_OUTLINE}
            side={THREE.BackSide}
            depthTest={false}
            depthWrite={false}
            transparent
          />
        </mesh>
        <mesh
          ref={(m: THREE.Mesh | null) => { shaftRef.current = m; enableOverlay(m); }}
          position={[0, 0.36, 0]}
          frustumCulled={false}
          renderOrder={1004}
        >
          <cylinderGeometry args={[0.05, 0.05, 0.72, 12]} />
          <meshBasicMaterial color={ARROW_FILL} depthTest={false} depthWrite={false} transparent />
        </mesh>
        <mesh
          ref={(m: THREE.Mesh | null) => { headRef.current = m; enableOverlay(m); }}
          position={[0, 0.86, 0]}
          frustumCulled={false}
          renderOrder={1004}
        >
          <coneGeometry args={[0.16, 0.28, 16]} />
          <meshBasicMaterial color={ARROW_FILL} depthTest={false} depthWrite={false} transparent />
        </mesh>
      </group>
    </>
  );
}
