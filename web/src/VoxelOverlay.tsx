/**
 * Voxel dialog solid fill + diff overlay (#369, dialog-scoped + lit #379).
 *
 * Reads `voxelDebugStore`:
 *   · `solidActive` (the Voxel dialog is open) → voxelise the current scene
 *     live and draw its occupancy as solid cubes, shaded like the real body:
 *     the object's own material colour/roughness/metalness under the overlay
 *     lights, no emissive self-lighting;
 *   · `diff` (published by the Voxel dialog / console) → draw the ADDED voxels
 *     green and the REMOVED voxels red (and, optionally, unchanged faint grey),
 *     so a before/after geometry change is visible where it happened — these
 *     stay colour-coded (self-lit), not object-shaded, since the point is to
 *     highlight the delta, not the material.
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
import { BODY_COLORS } from "./bodyColors";
import { DEFAULT_ROUGHNESS, DEFAULT_METALNESS } from "./pbrShading";
import { resolveMaterials } from "./materialBridge";
import type { Kernel, FeatureSummary } from "./kernel";

// Colours: added = green, removed = red, unchanged = grey (diff overlay only —
// self-lit, not object-shaded).
const C_ADDED = 0x22c55e;
const C_REMOVED = 0xef4444;
const C_UNCHANGED = 0xc2c8ce;

/** Shading for the solid fill: colour + PBR params, lit (no emissive). */
interface LitShade {
  color: THREE.Color;
  roughness: number;
  metalness: number;
}

/** The solid fill voxelises the WHOLE scene into one occupancy grid (no
 *  per-body split), so it can't shade per body — it takes body 0's resolved
 *  material (a user "Material" feature if targeted, else the default part
 *  colour), the same source `WebGLBodies`/the SDF path read. Good enough for
 *  the debug tool's typical single-object scene; falls back to the shared
 *  default part colour when there is no body yet. */
function resolvePrimaryShade(
  store: { kernel: Kernel } | null,
  features: FeatureSummary[] | undefined,
): LitShade {
  const [fr, fg, fb] = BODY_COLORS[0];
  const fallback: LitShade = {
    color: new THREE.Color(fr, fg, fb),
    roughness: DEFAULT_ROUGHNESS,
    metalness: DEFAULT_METALNESS,
  };
  if (!store || !features) return fallback;
  try {
    const um = resolveMaterials(store.kernel, features).get(0);
    if (!um) return fallback;
    const [r, g, b] = um.color;
    return {
      color: new THREE.Color(r, g, b),
      roughness: um.roughness,
      metalness: um.metalness,
    };
  } catch {
    return fallback;
  }
}

interface Grid {
  dims: [number, number, number];
  origin: [number, number, number];
  spacing: number;
}

/** Build one InstancedMesh over the voxels where `mask[i] !== 0`. Returns null
 *  when nothing is set (so the caller adds no empty mesh).
 *
 *  `shade` distinguishes the two use cases (#379): omitted (diff colours) →
 *  a flat, self-lit floor (`emissive = color`) so green/red stays legible as
 *  a colour code, not a lit surface; passed (solid fill) → a real lit PBR
 *  material with the object's roughness/metalness and NO emissive, so it
 *  reads as the shaded body under the overlay lights below, not a glowing
 *  cube swarm. */
function buildInstanced(
  mask: Uint8Array,
  grid: Grid,
  fillScale: number,
  color: THREE.Color | number,
  opacity: number,
  shade: { roughness: number; metalness: number } | null = null,
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
    emissive: shade ? 0x000000 : color,
    emissiveIntensity: shade ? 0 : 0.35,
    roughness: shade ? shade.roughness : 0.65,
    metalness: shade ? shade.metalness : 0,
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
        const shade = resolvePrimaryShade(store, state?.features);
        const solid = buildInstanced(
          snap.occupancy,
          grid,
          fillScale,
          shade.color,
          1,
          { roughness: shade.roughness, metalness: shade.metalness },
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
    state?.features,
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
