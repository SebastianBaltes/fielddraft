/**
 * Debug ▸ Mesher overlay — step 1 of building a custom meshing method.
 *
 * When the "Debug ▸ Mesher" dialog publishes octree parameters to
 * `mesherDebugStore`, this asks the kernel for a narrow-band adaptive octree
 * (`mesherOctreeSamples`) and draws every leaf cell's centre:
 *   · a point coloured by the SDF value (blue inside, red outside, green at the
 *     surface — auto-ranged over the returned cells);
 *   · a short white line from the centre along the gradient ∇f (the surface
 *     normal), its length a fraction of the cell size.
 *
 * It lives on OVERLAY_LAYER (x-ray) so the whole adaptive structure is visible
 * through the solid — the point of the tool. Re-samples on parameter or
 * geometry (`brepVersion`) change and publishes the leaf-cell count.
 */

import { useEffect, useMemo, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import {
  getMesherDebug,
  subscribeMesherDebug,
  setMesherStats,
} from "./mesherDebugStore";
import { OVERLAY_LAYER } from "./viewportLayers";

const MAX_LEAVES = 200_000; // must match the kernel cap

export function MesherOverlay() {
  const { store, state } = useKernelStore();
  const params = useSyncExternalStore(subscribeMesherDebug, getMesherDebug);
  const invalidate = useThree((s) => s.invalidate);

  const pointsGeom = useMemo(() => new THREE.BufferGeometry(), []);
  const linesGeom = useMemo(() => new THREE.BufferGeometry(), []);
  useEffect(
    () => () => {
      pointsGeom.dispose();
      linesGeom.dispose();
    },
    [pointsGeom, linesGeom],
  );

  const brepVersion = state?.brepVersion ?? 0;
  useEffect(() => {
    if (!params || !store) {
      setMesherStats(null);
      return;
    }
    const data = store.mesherOctreeSamples(params.maxDepth, params.bandGuard);
    const n = Math.floor(data.length / 8);

    const pos = new Float32Array(n * 3);
    const col = new Float32Array(n * 3);
    const linePos = new Float32Array(n * 6);
    let maxAbs = 1e-6;
    for (let k = 0; k < n; k++) {
      const a = Math.abs(data[k * 8 + 3]);
      if (a > maxAbs) maxAbs = a;
    }
    for (let k = 0; k < n; k++) {
      const b = k * 8;
      const x = data[b],
        y = data[b + 1],
        z = data[b + 2],
        d = data[b + 3];
      const nx = data[b + 4],
        ny = data[b + 5],
        nz = data[b + 6],
        size = data[b + 7];
      pos[k * 3] = x;
      pos[k * 3 + 1] = y;
      pos[k * 3 + 2] = z;
      // green at the surface → blue inside / red outside.
      const t = Math.min(1, Math.abs(d) / maxAbs);
      if (d < 0) {
        col[k * 3] = 0.2;
        col[k * 3 + 1] = 1.0 - 0.5 * t;
        col[k * 3 + 2] = 0.4 + 0.6 * t;
      } else {
        col[k * 3] = 0.2 + 0.8 * t;
        col[k * 3 + 1] = 1.0 - 0.6 * t;
        col[k * 3 + 2] = 0.4 - 0.2 * t;
      }
      const len = size * params.normalScale;
      linePos[k * 6] = x;
      linePos[k * 6 + 1] = y;
      linePos[k * 6 + 2] = z;
      linePos[k * 6 + 3] = x + nx * len;
      linePos[k * 6 + 4] = y + ny * len;
      linePos[k * 6 + 5] = z + nz * len;
    }
    pointsGeom.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    pointsGeom.setAttribute("color", new THREE.BufferAttribute(col, 3));
    pointsGeom.computeBoundingSphere();
    linesGeom.setAttribute("position", new THREE.BufferAttribute(linePos, 3));
    linesGeom.computeBoundingSphere();

    setMesherStats({ cells: n, capped: n >= MAX_LEAVES });
    invalidate();
  }, [params, store, brepVersion, pointsGeom, linesGeom, invalidate]);

  if (!params) return null;

  return (
    <group>
      <points
        ref={(o) => o?.layers.enable(OVERLAY_LAYER)}
        geometry={pointsGeom}
        renderOrder={5}
      >
        <pointsMaterial
          vertexColors
          size={params.pointSize}
          sizeAttenuation={false}
          toneMapped={false}
        />
      </points>
      {params.normalScale > 0 && (
        <lineSegments
          ref={(o) => o?.layers.enable(OVERLAY_LAYER)}
          geometry={linesGeom}
          renderOrder={5}
        >
          <lineBasicMaterial color={0xffffff} toneMapped={false} />
        </lineSegments>
      )}
    </group>
  );
}
