/**
 * Voxel render mode + diff overlay (#369).
 *
 * Reads `voxelDebugStore`:
 *   · `solidActive` (the "voxel" render mode) → voxelise the current scene live
 *     and draw its occupancy as solid grey cubes;
 *   · `diff` (published by the Voxel dialog / console) → draw the ADDED voxels
 *     green and the REMOVED voxels red (and, optionally, unchanged faint grey),
 *     so a before/after geometry change is visible where it happened.
 *
 * Built imperatively into a group (like `MesherOverlay`): one `InstancedMesh`
 * per colour class, so even a dense fill is one draw call. The cubes carry
 * their own lights and enable OVERLAY_LAYER so they show in the SDF render
 * paths too (which don't composite plain layer 0).
 */

import { useEffect, useRef, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { getVoxelDisplay, subscribeVoxelDisplay } from "./voxelDebugStore";
import { OVERLAY_LAYER } from "./viewportLayers";

// Colours: added = green, removed = red, solid/unchanged = grey.
const C_ADDED = 0x22c55e;
const C_REMOVED = 0xef4444;
const C_SOLID = 0x9aa3ad;
const C_UNCHANGED = 0xc2c8ce;

interface Grid {
  dims: [number, number, number];
  origin: [number, number, number];
  spacing: number;
}

/** Build one InstancedMesh over the voxels where `mask[i] !== 0`. Returns null
 *  when nothing is set (so the caller adds no empty mesh). */
function buildInstanced(
  mask: Uint8Array,
  grid: Grid,
  fillScale: number,
  color: number,
  opacity: number,
): THREE.InstancedMesh | null {
  const [nx, ny, nz] = grid.dims;
  const [ox, oy, oz] = grid.origin;
  const s = grid.spacing;
  let count = 0;
  for (let i = 0; i < mask.length; i++) if (mask[i]) count++;
  if (count === 0) return null;

  const geo = new THREE.BoxGeometry(1, 1, 1);
  const mat = new THREE.MeshStandardMaterial({
    color,
    // Self-lit floor so the shape stays legible in the SDF paths (no scene
    // lights there); the directional light below adds form.
    emissive: color,
    emissiveIntensity: 0.35,
    roughness: 0.65,
    metalness: 0,
    transparent: opacity < 1,
    opacity,
    toneMapped: false,
  });
  const mesh = new THREE.InstancedMesh(geo, mat, count);
  const cube = s * fillScale;
  const m = new THREE.Matrix4();
  const q = new THREE.Quaternion();
  const scale = new THREE.Vector3(cube, cube, cube);
  const pos = new THREE.Vector3();
  let inst = 0;
  let i = 0;
  for (let iz = 0; iz < nz; iz++) {
    for (let iy = 0; iy < ny; iy++) {
      for (let ix = 0; ix < nx; ix++, i++) {
        if (!mask[i]) continue;
        pos.set(
          ox + (ix + 0.5) * s,
          oy + (iy + 0.5) * s,
          oz + (iz + 0.5) * s,
        );
        m.compose(pos, q, scale);
        mesh.setMatrixAt(inst++, m);
      }
    }
  }
  mesh.instanceMatrix.needsUpdate = true;
  mesh.layers.enable(OVERLAY_LAYER);
  mesh.frustumCulled = false;
  return mesh;
}

function disposeChildren(group: THREE.Group): void {
  for (const child of [...group.children]) {
    group.remove(child);
    const inst = child as unknown as {
      geometry?: { dispose?: () => void };
      material?: { dispose?: () => void };
      dispose?: () => void;
    };
    inst.geometry?.dispose?.();
    inst.material?.dispose?.();
    inst.dispose?.();
  }
}

export function VoxelOverlay() {
  const { store, state } = useKernelStore();
  const display = useSyncExternalStore(
    subscribeVoxelDisplay,
    getVoxelDisplay,
  );
  const invalidate = useThree((s) => s.invalidate);
  const groupRef = useRef<THREE.Group>(null);

  const brepVersion = state?.brepVersion ?? 0;
  const { solidActive, resolution, diff, showUnchanged, fillScale } = display;

  useEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    disposeChildren(group);

    const active = solidActive || diff !== null;
    if (!active) {
      invalidate();
      return;
    }

    // Lights so the shaded cubes read as 3D. On the overlay layer too, so the
    // SDF overlay pass lights them (it doesn't composite plain layer 0).
    const ambient = new THREE.AmbientLight(0xffffff, 0.75);
    ambient.layers.enable(OVERLAY_LAYER);
    const dir = new THREE.DirectionalLight(0xffffff, 1.1);
    dir.position.set(0.6, 1, 0.8);
    dir.layers.enable(OVERLAY_LAYER);
    group.add(ambient, dir);

    if (diff) {
      const grid: Grid = {
        dims: diff.dims,
        origin: diff.origin,
        spacing: diff.spacing,
      };
      if (showUnchanged) {
        const u = buildInstanced(
          diff.unchangedMask,
          grid,
          fillScale * 0.75,
          C_UNCHANGED,
          0.25,
        );
        if (u) group.add(u);
      }
      const removed = buildInstanced(
        diff.removedMask,
        grid,
        fillScale,
        C_REMOVED,
        1,
      );
      if (removed) group.add(removed);
      const added = buildInstanced(
        diff.addedMask,
        grid,
        fillScale,
        C_ADDED,
        1,
      );
      if (added) group.add(added);
    } else if (solidActive && store) {
      const snap = store.voxelize(resolution);
      if (snap) {
        const grid: Grid = {
          dims: snap.dims,
          origin: snap.origin,
          spacing: snap.spacing,
        };
        const solid = buildInstanced(
          snap.occupancy,
          grid,
          fillScale,
          C_SOLID,
          1,
        );
        if (solid) group.add(solid);
      }
    }
    invalidate();
  }, [
    solidActive,
    resolution,
    diff,
    showUnchanged,
    fillScale,
    store,
    brepVersion,
    invalidate,
  ]);

  useEffect(() => {
    return () => {
      if (groupRef.current) disposeChildren(groupRef.current);
    };
  }, []);

  return <group ref={groupRef} />;
}
