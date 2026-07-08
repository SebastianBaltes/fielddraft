/**
 * Debug ▸ Part-AABB overlay.
 *
 * When the Debug menu enables "Part-AABB", this boxes the axis-aligned bounding
 * box of every selected part (a body). The bound is the kernel's analytic,
 * pipeline-maintained `Body.aabb` (tight for boolean cuts, ticket f88u), so a
 * half-carved sphere is boxed by its visible remainder, not the phantom
 * original.
 *
 * Drawn imperatively into a group (the stroke-width wireframe needs the WebGPU
 * fat-line material on WebGPU but a plain `LineSegments` on the WebGL fallback,
 * both via `makeFatLine`). On OVERLAY_LAYER (x-ray) so the box stays visible
 * through the solid.
 */

import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { getPartAabbVisible, subscribePartAabb } from "./partAabbStore";
import { selectedPartAabbs, aabbEdgeSegments } from "./partAabb";
import {
  createFatLineMaterials,
  makeFatLine,
  isWebGPURenderer,
} from "./overlayLineMaterials";
import { OVERLAY_LAYER } from "./viewportLayers";

const BOX_COLOR = 0x33d6ff; // cyan — distinct from the accent + body shading
const BOX_WIDTH = 2; // screen-space px (WebGPU); 1px on the WebGL fallback

function disposeChildren(group: THREE.Group): void {
  for (const child of [...group.children]) {
    group.remove(child);
    (child as unknown as { geometry?: { dispose?: () => void } }).geometry?.dispose?.();
  }
}

export function PartAabbOverlay() {
  const { store, state } = useKernelStore();
  const visible = useSyncExternalStore(subscribePartAabb, getPartAabbVisible);
  const invalidate = useThree((s) => s.invalidate);
  const gl = useThree((s) => s.gl);
  const groupRef = useRef<THREE.Group>(null);

  // One shared material pair (WebGPU + WebGL); disposed on unmount.
  const mats = useMemo(
    () => createFatLineMaterials({ color: BOX_COLOR, linewidth: BOX_WIDTH }),
    [],
  );

  const selection = state?.selection;
  // Ticket 72a141f: clicking a row in the Parts panel HIGHLIGHTS the body
  // (`highlightedBodyIndex`), it does not add to the feature `selection`. Box
  // that part too, else "select a part → no AABB". Union so a viewport/list
  // feature selection AND a highlighted part both draw.
  const highlightedBodyIndex = state?.highlightedBodyIndex ?? null;
  const brepVersion = state?.brepVersion ?? 0;
  useEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    disposeChildren(group);
    const featureIds = new Set<number>(selection ?? []);
    if (highlightedBodyIndex != null && store) {
      const seed = store.listBodies()[highlightedBodyIndex]?.seedId;
      if (seed != null) featureIds.add(seed);
    }
    if (!visible || !store || featureIds.size === 0) {
      invalidate();
      return;
    }

    const webgpu = isWebGPURenderer(gl);
    for (const box of selectedPartAabbs(store.kernel, featureIds)) {
      // makeFatLine attaches the geometry to the object; disposeChildren frees
      // it. depthTest is off (createFatLineMaterials default) → x-ray box.
      const { object } = makeFatLine(
        aabbEdgeSegments(box.min, box.max),
        mats,
        webgpu,
      );
      object.layers.enable(OVERLAY_LAYER);
      object.renderOrder = 7;
      group.add(object);
    }
    invalidate();
  }, [visible, store, selection, highlightedBodyIndex, brepVersion, gl, mats, invalidate]);

  useEffect(() => {
    return () => {
      if (groupRef.current) disposeChildren(groupRef.current);
      mats.node.dispose();
      mats.classic.dispose();
    };
  }, [mats]);

  return <group ref={groupRef} />;
}
