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

/** SDF value → RGB, a SHARP 3-way classification so `threshold` (Surface ±)
 *  directly controls the green band: `|d| ≤ threshold` = green (surface),
 *  `d < −threshold` = blue (inside), `d > threshold` = red (outside). No
 *  distance-magnitude ramp — a maxAbs-relative gradient made near-surface
 *  samples look green regardless of the threshold (ticket 0ly1). */
function sdfColor(
  d: number,
  threshold: number,
  o: Float32Array,
  i: number,
): void {
  const c = Math.abs(d) <= threshold ? C_SURFACE : d < 0 ? C_INSIDE : C_OUTSIDE;
  o[i] = c[0];
  o[i + 1] = c[1];
  o[i + 2] = c[2];
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

    // The kernel runs the whole jq2t point pipeline (octree → filters →
    // Newton fit → tangential repulsion); extra fields in the options JSON
    // are ignored by serde, so the debug params pass through verbatim.
    const data = store.mesherPoints(JSON.stringify(params));
    const total = data.length >= 2 ? data[0] : 0;
    const triCount = data.length >= 2 ? data[1] : 0;
    const STRIDE = 9; // [x,y,z,sdf,nx,ny,nz,size,shown]
    const P = 2; // points offset
    const I = 2 + total * STRIDE; // indices offset
    // Display filter: draw normals/dots only for points the kernel marks as
    // `shown` (index 8 = 1). That flag folds in ALL processing filters
    // (near-surface AND sign-change), so toggling any of them visibly culls
    // the cloud — the kernel owns the filter logic, the overlay just reads
    // the verdict (tickets jt76/gjuu). The mesh still uses every point.
    const keep: number[] = [];
    for (let k = 0; k < total; k++) {
      if (data[P + k * STRIDE + 8] > 0.5) keep.push(k);
    }
    const n = keep.length;
    setMesherStats({ cells: n, capped: total >= MAX_LEAVES, tris: triCount });

    // Debug octree-edge connectivity mesh over the SHOWN points — the
    // triangles connect filtered leaves by octree adjacency (NOT dual
    // contouring; holes/wrong facets are expected by design). Drawn as a
    // WIREFRAME: each triangle edge is one line segment, deduplicated, in the
    // WebGPU-visible fat-line path (Line2NodeMaterial), so it reads as the raw
    // octree edge graph over the point set.
    if (triCount > 0) {
      const seen = new Set<number>();
      const seg: number[] = [];
      const px = (i: number) => data[P + i * STRIDE];
      const py = (i: number) => data[P + i * STRIDE + 1];
      const pz = (i: number) => data[P + i * STRIDE + 2];
      const pushEdge = (a: number, b: number) => {
        const lo = Math.min(a, b);
        const hi = Math.max(a, b);
        const key = lo * total + hi; // unique per undirected edge
        if (seen.has(key)) return;
        seen.add(key);
        seg.push(px(a), py(a), pz(a), px(b), py(b), pz(b));
      };
      for (let t = 0; t < triCount; t++) {
        const a = data[I + t * 3];
        const b = data[I + t * 3 + 1];
        const c = data[I + t * 3 + 2];
        pushEdge(a, b);
        pushEdge(b, c);
        pushEdge(c, a);
      }
      if (seg.length > 0) {
        const lg = new LineSegmentsGeometry();
        lg.setPositions(new Float32Array(seg));
        const wire = new LineSegments2(
          lg,
          new Line2NodeMaterial({
            color: 0x111827,
            linewidth: Math.max(1, params.strokeWidth * 0.6),
            worldUnits: false,
            transparent: true,
            depthWrite: false,
          }),
        );
        wire.layers.enable(OVERLAY_LAYER);
        wire.renderOrder = 4;
        group.add(wire);
      }
    }
    if (n === 0) {
      invalidate();
      return;
    }


    const pos = new Float32Array(n * 3);
    const col = new Float32Array(n * 3);
    const linePos = new Float32Array(n * 6);
    const lineCol = new Float32Array(n * 6);
    for (let idx = 0; idx < n; idx++) {
      const b = P + keep[idx] * STRIDE;
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
      sdfColor(d, params.surfaceThreshold, col, idx * 3);
      const len = size * params.normalScale;
      linePos[idx * 6] = x;
      linePos[idx * 6 + 1] = y;
      linePos[idx * 6 + 2] = z;
      linePos[idx * 6 + 3] = x + nx * len;
      linePos[idx * 6 + 4] = y + ny * len;
      linePos[idx * 6 + 5] = z + nz * len;
      sdfColor(d, params.surfaceThreshold, lineCol, idx * 6);
      sdfColor(d, params.surfaceThreshold, lineCol, idx * 6 + 3);
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
