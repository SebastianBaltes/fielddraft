/**
 * Debug (#350) — fillet-zone overlay.
 *
 * For every `fillet_edge` whose detail panel has "Zone anzeigen" on, draws the
 * masked capsule the blend is confined to: a translucent tube of radius
 * `maskRadius` (= `blendRadius × FILLET_MASK_SCALE`) along the rounded edge,
 * with rounded caps — exactly the region that shapes the SDF. This is the
 * "langgezogene Pill um die Kante" the ticket asked for, so a fillet's reach
 * is visible even where the rounding is subtle.
 *
 * The zone geometry comes straight from the kernel (`Kernel.filletZones()`),
 * so it always matches what the field actually uses. Drawn on OVERLAY_LAYER
 * (x-ray) with depth test off so the pill stays visible through the solid.
 */

import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { getFilletZoneIds, subscribeFilletZones } from "./filletDebugStore";
import { OVERLAY_LAYER } from "./viewportLayers";

const ZONE_COLOR = 0xffa726; // amber — a debug tint distinct from body shading
const ZONE_OPACITY = 0.28;

function disposeChildren(group: THREE.Group): void {
  for (const child of [...group.children]) {
    group.remove(child);
    (child as unknown as { geometry?: { dispose?: () => void } }).geometry?.dispose?.();
  }
}

export function FilletZoneOverlay() {
  const { store, state } = useKernelStore();
  const shownIds = useSyncExternalStore(subscribeFilletZones, getFilletZoneIds);
  const invalidate = useThree((s) => s.invalidate);
  const groupRef = useRef<THREE.Group>(null);

  // One shared translucent material for every zone; disposed on unmount.
  const material = useMemo(
    () =>
      new THREE.MeshBasicMaterial({
        color: ZONE_COLOR,
        transparent: true,
        opacity: ZONE_OPACITY,
        depthTest: false,
        depthWrite: false,
        side: THREE.DoubleSide,
      }),
    [],
  );

  // Rebuild when the shown set changes or the geometry regenerates.
  const brepVersion = state?.brepVersion ?? 0;
  useEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    disposeChildren(group);
    if (!store || shownIds.size === 0) {
      invalidate();
      return;
    }

    for (const zone of store.kernel.filletZones()) {
      if (zone.filletId == null || !shownIds.has(zone.filletId)) continue;
      if (zone.polyline.length < 2 || zone.maskRadius <= 0) continue;

      const pts = zone.polyline.map((p) => new THREE.Vector3(p[0], p[1], p[2]));
      const curve = new THREE.CatmullRomCurve3(pts, false, "catmullrom", 0);
      const segments = Math.max(1, pts.length - 1) * 6;
      const tube = new THREE.TubeGeometry(curve, segments, zone.maskRadius, 16, false);
      const tubeMesh = new THREE.Mesh(tube, material);
      tubeMesh.layers.enable(OVERLAY_LAYER);
      tubeMesh.renderOrder = 6;
      group.add(tubeMesh);

      // Rounded caps so the tube reads as a capsule/pill.
      for (const end of [pts[0], pts[pts.length - 1]]) {
        const cap = new THREE.Mesh(
          new THREE.SphereGeometry(zone.maskRadius, 16, 12),
          material,
        );
        cap.position.copy(end);
        cap.layers.enable(OVERLAY_LAYER);
        cap.renderOrder = 6;
        group.add(cap);
      }
    }
    invalidate();
  }, [store, shownIds, brepVersion, material, invalidate]);

  useEffect(
    () => () => {
      if (groupRef.current) disposeChildren(groupRef.current);
      material.dispose();
    },
    [material],
  );

  return <group ref={groupRef} />;
}
