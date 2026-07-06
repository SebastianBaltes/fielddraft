/**
 * Debug ▸ Mesher overlay — step 1 of building a custom meshing method.
 *
 * When the "Debug ▸ Mesher" dialog publishes octree parameters to
 * `mesherDebugStore`, this asks the kernel for a narrow-band adaptive octree
 * (`mesherOctreeSamples`) and draws every leaf cell's centre:
 *   · a round point coloured by the SDF value (blue inside, red outside, green
 *     at the surface — auto-ranged), diameter = 3× the stroke width;
 *   · a stroke-width line from the centre along the gradient ∇f (the surface
 *     normal), coloured by the SAME SDF value.
 *
 * "Near surface only" restricts the drawn cells to |sdf| ≤ a distance. Built
 * imperatively into a group because the stroke-width lines need the WebGPU
 * fat-line material (`Line2NodeMaterial`) but a plain `LineSegments` on the
 * WebGL fallback. On OVERLAY_LAYER (x-ray) so the whole structure is visible.
 */

import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { Line2NodeMaterial } from "three/webgpu";
import { useKernelStore } from "./store";
import {
  getMesherDebug,
  subscribeMesherDebug,
  setMesherStats,
} from "./mesherDebugStore";
import { isWebGPURenderer } from "./overlayLineMaterials";
import { OVERLAY_LAYER } from "./viewportLayers";

const MAX_LEAVES = 200_000; // must match the kernel cap

/** A soft filled disc, so square points render as round dots. */
function makeCircleTexture(): THREE.Texture {
  const s = 64;
  const c = document.createElement("canvas");
  c.width = c.height = s;
  const ctx = c.getContext("2d")!;
  ctx.beginPath();
  ctx.arc(s / 2, s / 2, s / 2 - 2, 0, Math.PI * 2);
  ctx.fillStyle = "#fff";
  ctx.fill();
  const tex = new THREE.CanvasTexture(c);
  tex.needsUpdate = true;
  return tex;
}

// Legend colours (match the dialog swatches): surface green, inside blue,
// outside red.
const C_SURFACE = [0.16, 1.0, 0.47];
const C_INSIDE = [0.24, 0.51, 0.9];
const C_OUTSIDE = [0.88, 0.43, 0.24];

/** SDF value → RGB. Samples with `|d| ≤ threshold` are pure green (surface);
 *  beyond it the colour ramps green→blue (inside) / green→red (outside),
 *  auto-ranged to `maxAbs`. */
function sdfColor(
  d: number,
  maxAbs: number,
  threshold: number,
  o: Float32Array,
  i: number,
): void {
  const ad = Math.abs(d);
  if (ad <= threshold) {
    o[i] = C_SURFACE[0];
    o[i + 1] = C_SURFACE[1];
    o[i + 2] = C_SURFACE[2];
    return;
  }
  const t = Math.min(1, (ad - threshold) / Math.max(1e-6, maxAbs - threshold));
  const to = d < 0 ? C_INSIDE : C_OUTSIDE;
  o[i] = C_SURFACE[0] + (to[0] - C_SURFACE[0]) * t;
  o[i + 1] = C_SURFACE[1] + (to[1] - C_SURFACE[1]) * t;
  o[i + 2] = C_SURFACE[2] + (to[2] - C_SURFACE[2]) * t;
}

function disposeChildren(group: THREE.Group): void {
  for (const child of [...group.children]) {
    group.remove(child);
    const withGeo = child as unknown as {
      geometry?: { dispose?: () => void };
      material?: { dispose?: () => void };
    };
    withGeo.geometry?.dispose?.();
    withGeo.material?.dispose?.();
  }
}

export function MesherOverlay() {
  const { store, state } = useKernelStore();
  const params = useSyncExternalStore(subscribeMesherDebug, getMesherDebug);
  const invalidate = useThree((s) => s.invalidate);
  const gl = useThree((s) => s.gl);
  const groupRef = useRef<THREE.Group>(null);
  const circle = useMemo(makeCircleTexture, []);

  const brepVersion = state?.brepVersion ?? 0;
  useEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    disposeChildren(group);
    if (!params || !store) {
      setMesherStats(null);
      invalidate();
      return;
    }

    const data = store.mesherOctreeSamples(params.maxDepth, params.bandGuard);
    const total = Math.floor(data.length / 8);
    // Keep leaf indices that pass the near-surface filter. When the part is
    // faded (partOpacity < 1) the interior is visible, so interior samples
    // (sdf < 0) are kept even beyond the near-surface distance.
    const showInterior = params.partOpacity < 1;
    const keep: number[] = [];
    for (let k = 0; k < total; k++) {
      const d = data[k * 8 + 3];
      if (
        params.nearSurfaceOnly &&
        Math.abs(d) > params.nearSurfaceDist &&
        !(showInterior && d < 0)
      )
        continue;
      keep.push(k);
    }
    const n = keep.length;
    setMesherStats({ cells: n, capped: total >= MAX_LEAVES });
    if (n === 0) {
      invalidate();
      return;
    }

    let maxAbs = 1e-6;
    for (const k of keep) maxAbs = Math.max(maxAbs, Math.abs(data[k * 8 + 3]));

    const pos = new Float32Array(n * 3);
    const col = new Float32Array(n * 3);
    const linePos = new Float32Array(n * 6);
    const lineCol = new Float32Array(n * 6);
    for (let idx = 0; idx < n; idx++) {
      const b = keep[idx] * 8;
      const x = data[b],
        y = data[b + 1],
        z = data[b + 2],
        d = data[b + 3];
      const nx = data[b + 4],
        ny = data[b + 5],
        nz = data[b + 6],
        size = data[b + 7];
      pos[idx * 3] = x;
      pos[idx * 3 + 1] = y;
      pos[idx * 3 + 2] = z;
      sdfColor(d, maxAbs, params.surfaceThreshold, col, idx * 3);
      const len = size * params.normalScale;
      linePos[idx * 6] = x;
      linePos[idx * 6 + 1] = y;
      linePos[idx * 6 + 2] = z;
      linePos[idx * 6 + 3] = x + nx * len;
      linePos[idx * 6 + 4] = y + ny * len;
      linePos[idx * 6 + 5] = z + nz * len;
      sdfColor(d, maxAbs, params.surfaceThreshold, lineCol, idx * 6);
      sdfColor(d, maxAbs, params.surfaceThreshold, lineCol, idx * 6 + 3);
    }

    // ── Centre dots (round, SDF-coloured, diameter = 3× stroke width) ──
    const pg = new THREE.BufferGeometry();
    pg.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    pg.setAttribute("color", new THREE.BufferAttribute(col, 3));
    pg.computeBoundingSphere();
    const points = new THREE.Points(
      pg,
      new THREE.PointsMaterial({
        vertexColors: true,
        size: params.strokeWidth * 3,
        sizeAttenuation: false,
        map: circle,
        alphaTest: 0.5,
        transparent: true,
        toneMapped: false,
      }),
    );
    points.layers.enable(OVERLAY_LAYER);
    points.renderOrder = 6;
    group.add(points);

    // ── Normal lines (stroke width, SDF-coloured) ──
    if (params.normalScale > 0) {
      const webgpu = isWebGPURenderer(gl);
      let lines: THREE.Object3D;
      if (webgpu) {
        const lg = new LineSegmentsGeometry();
        lg.setPositions(linePos);
        lg.setColors(lineCol);
        lines = new LineSegments2(
          lg,
          new Line2NodeMaterial({
            vertexColors: true,
            linewidth: params.strokeWidth,
            worldUnits: false,
            transparent: true,
            depthWrite: false,
          }),
        );
      } else {
        const lg = new THREE.BufferGeometry();
        lg.setAttribute("position", new THREE.BufferAttribute(linePos, 3));
        lg.setAttribute("color", new THREE.BufferAttribute(lineCol, 3));
        lines = new THREE.LineSegments(
          lg,
          new THREE.LineBasicMaterial({
            vertexColors: true,
            transparent: true,
            depthWrite: false,
            toneMapped: false,
          }),
        );
      }
      lines.layers.enable(OVERLAY_LAYER);
      lines.renderOrder = 5;
      group.add(lines);
    }

    invalidate();
  }, [params, store, brepVersion, gl, circle, invalidate]);

  useEffect(() => {
    return () => {
      if (groupRef.current) disposeChildren(groupRef.current);
      circle.dispose();
    };
  }, [circle]);

  return <group ref={groupRef} />;
}
