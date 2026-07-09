/**
 * Debug ▸ Mesher Lichen overlay.
 *
 * When the "Debug ▸ Mesher: Lichen" dialog publishes parameters to
 * `lichenDebugStore`, this asks the kernel (`lichenMesh`) for the same flat
 * overlay format the PASM mesher emits and draws:
 *   · a round point per particle coloured by the SDF value (surface particle),
 *     or orange (edge chain) / red (corner);
 *   · optional stroke-width normal needles along ∇f.
 *
 * Copied verbatim from `MesherOverlay`; only the store, the kernel call and the
 * kernel-params object differ. The flat format is identical, so the parsing is
 * unchanged. Built imperatively into a group because the stroke-width lines
 * need the WebGPU fat-line material (`Line2NodeMaterial`) but a plain
 * `LineSegments` on the WebGL fallback. On OVERLAY_LAYER (x-ray).
 */

import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { useThree, useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import { Line2NodeMaterial } from "three/webgpu";
import { useKernelStore } from "./store";
import {
  getLichenDebug,
  subscribeLichenDebug,
  setLichenStats,
} from "./lichenDebugStore";
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

// Scratch for the constant-pixel box rescale (single overlay instance).
const _v = new THREE.Vector3();
const _c = new THREE.Vector3();
const _u = new THREE.Vector3();
const _w = new THREE.Vector3();
const _nn = new THREE.Vector3();
const _m = new THREE.Matrix4();

/** Rescale the point-marker boxes so each reads at a CONSTANT pixel size (`px`)
 *  regardless of zoom, keeping its per-point normal orientation. */
function rescaleBoxes(
  pd: { mesh: THREE.InstancedMesh; centers: Float32Array; basis: Float32Array; px: number },
  camera: THREE.Camera,
  viewportH: number,
): void {
  camera.getWorldPosition(_v);
  const persp = camera instanceof THREE.PerspectiveCamera;
  const tanHalf = persp
    ? Math.tan(((camera as THREE.PerspectiveCamera).fov * 0.5 * Math.PI) / 180)
    : 0;
  const orthoWpp =
    camera instanceof THREE.OrthographicCamera
      ? (camera.top - camera.bottom) / camera.zoom / (viewportH || 1)
      : 0.01;
  for (let i = 0; i < pd.mesh.count; i++) {
    _c.set(pd.centers[i * 3], pd.centers[i * 3 + 1], pd.centers[i * 3 + 2]);
    const wpp = persp ? (2 * _v.distanceTo(_c) * tanHalf) / (viewportH || 1) : orthoWpp;
    const s = pd.px * wpp;
    _u.set(pd.basis[i * 9], pd.basis[i * 9 + 1], pd.basis[i * 9 + 2]).multiplyScalar(s);
    _w.set(pd.basis[i * 9 + 3], pd.basis[i * 9 + 4], pd.basis[i * 9 + 5]).multiplyScalar(s);
    _nn.set(pd.basis[i * 9 + 6], pd.basis[i * 9 + 7], pd.basis[i * 9 + 8]).multiplyScalar(s * 1.4);
    _m.makeBasis(_u, _w, _nn);
    _m.setPosition(_c.x, _c.y, _c.z);
    pd.mesh.setMatrixAt(i, _m);
  }
  pd.mesh.instanceMatrix.needsUpdate = true;
}

export function LichenOverlay() {
  const { store, state } = useKernelStore();
  const params = useSyncExternalStore(subscribeLichenDebug, getLichenDebug);
  const invalidate = useThree((s) => s.invalidate);
  const gl = useThree((s) => s.gl);
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  const groupRef = useRef<THREE.Group>(null);
  const dotsRef = useRef<THREE.InstancedMesh[]>([]);
  // Per-point data for the zoom-independent (constant-pixel) box markers,
  // rescaled every frame in a useFrame below.
  const pointDataRef = useRef<{
    mesh: THREE.InstancedMesh;
    centers: Float32Array; // n×3
    basis: Float32Array; // n×9 (u,v,n unit)
    px: number;
  } | null>(null);
  const circle = useMemo(makeCircleTexture, []);

  const brepVersion = state?.brepVersion ?? 0;
  useEffect(() => {
    const group = groupRef.current;
    if (!group) return;
    disposeChildren(group);
    for (const d of dotsRef.current) d.dispose();
    dotsRef.current = [];
    if (!params || !store) {
      setLichenStats(null);
      invalidate();
      return;
    }

    // The kernel reads the Lichen params (targetEdgeLength, grow, curvAngleDeg,
    // useReach, maxPoints) and returns the SAME flat overlay format the PASM
    // mesher uses: [nPoints, nTris, nFallbackTris, points×9, indices].
    const data = store.lichenMesh(
      JSON.stringify({
        targetEdgeLength: params.targetEdgeLength,
        grow: params.grow,
        curvAngleDeg: params.curvAngleDeg,
        useReach: params.useReach,
        maxPoints: params.maxPoints,
      }),
    );
    const total = data.length >= 3 ? data[0] : 0;
    const triCount = data.length >= 3 ? data[1] : 0;
    // The LAST fallbackTris triangles are the §6-F3 patches/zippers of
    // chartless faces — drawn as a separate orange wireframe.
    const fallbackTris = data.length >= 3 ? data[2] : 0;
    const STRIDE = 9; // [x,y,z,sdf,nx,ny,nz,size,shown]
    const P = 3; // points offset
    const I = 3 + total * STRIDE; // indices offset
    // Punkte werden an ihrer ECHTEN Position gezeichnet (kein Versatz im World-/
    // Kamera-Raum). Dass die Overlay-Elemente vor dem Körper lesbar bleiben,
    // regelt der OVERLAY_LAYER (x-ray, Tiefe geleert) zusammen mit
    // `depthWrite: false` — exakt wie im PASM-`MesherOverlay`. Kein Lift, kein
    // `depthTest`-Override.
    const rawX = (b: number) => data[b];
    const rawY = (b: number) => data[b + 1];
    const rawZ = (b: number) => data[b + 2];
    // Z-Puffer-Bias (nur WebGPU): LineSegments2 (fette Linien) UND die Box-Marker
    // rastern Dreiecke, daher wirkt `polygonOffset` als WebGPU-`depthBias` und
    // zieht die Debug-Objekte im TIEFENPUFFER minimal Richtung Kamera — sie
    // liegen so leicht vor dem Solid und werden nicht so schnell verdeckt (kein
    // geometrischer World-/Kamera-Versatz). Auf WebGL (plain lines/points greift
    // depthBias nicht) bewusst weggelassen.
    const webgpu = isWebGPURenderer(gl);
    const depthBias = webgpu
      ? { polygonOffset: true, polygonOffsetFactor: -2, polygonOffsetUnits: -4 }
      : {};
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
    setLichenStats({
      cells: n,
      capped: total >= MAX_LEAVES,
      tris: triCount,
      fallbackTris,
    });

    // Debug octree-edge connectivity mesh over the SHOWN points — the
    // triangles connect filtered leaves by octree adjacency (NOT dual
    // contouring; holes/wrong facets are expected by design). Drawn as a
    // WIREFRAME: each triangle edge is one line segment, deduplicated, in the
    // WebGPU-visible fat-line path (Line2NodeMaterial), so it reads as the raw
    // octree edge graph over the point set.
    if (triCount > 0 && params.showTriangles) {
      const px = (i: number) => rawX(P + i * STRIDE);
      const py = (i: number) => rawY(P + i * STRIDE);
      const pz = (i: number) => rawZ(P + i * STRIDE);
      // Two wireframes: CDT/DC triangles in ink, §6-F3 fallback patches in
      // orange — the split is positional (fallback tris sit LAST).
      const buildWire = (from: number, to: number, color: number) => {
        const seen = new Set<number>();
        const seg: number[] = [];
        const pushEdge = (a: number, b: number) => {
          const lo = Math.min(a, b);
          const hi = Math.max(a, b);
          const key = lo * total + hi; // unique per undirected edge
          if (seen.has(key)) return;
          seen.add(key);
          seg.push(px(a), py(a), pz(a), px(b), py(b), pz(b));
        };
        for (let t = from; t < to; t++) {
          const a = data[I + t * 3];
          const b = data[I + t * 3 + 1];
          const c = data[I + t * 3 + 2];
          pushEdge(a, b);
          pushEdge(b, c);
          pushEdge(c, a);
        }
        if (seg.length === 0) return;
        const lg = new LineSegmentsGeometry();
        lg.setPositions(new Float32Array(seg));
        const wire = new LineSegments2(
          lg,
          new Line2NodeMaterial({
            color,
            linewidth: Math.max(1, params.strokeWidth * 0.6),
            worldUnits: false,
            transparent: true,
            depthWrite: false,
            ...depthBias,
          }),
        );
        wire.layers.enable(OVERLAY_LAYER);
        wire.renderOrder = 4;
        group.add(wire);
      };
      buildWire(0, triCount - fallbackTris, 0x111827);
      buildWire(triCount - fallbackTris, triCount, 0xea8c1a);
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
      const x = rawX(b),
        y = rawY(b),
        z = rawZ(b),
        d = data[b + 3];
      const nx = data[b + 4],
        ny = data[b + 5],
        nz = data[b + 6],
        size = data[b + 7];
      pos[idx * 3] = x;
      pos[idx * 3 + 1] = y;
      pos[idx * 3 + 2] = z;
      const flag = data[b + 8];
      if (flag >= 2.5) {
        // Skeleton corner (pinned): red.
        col[idx * 3] = 0.85;
        col[idx * 3 + 1] = 0.1;
        col[idx * 3 + 2] = 0.1;
      } else if (flag >= 1.5) {
        // Skeleton edge chain (fixed): orange.
        col[idx * 3] = 0.95;
        col[idx * 3 + 1] = 0.55;
        col[idx * 3 + 2] = 0.1;
      } else {
        sdfColor(d, params.surfaceThreshold, col, idx * 3);
      }
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

    // ── Point markers ── small FILLED boxes (Rechtecke) oriented to the surface
    //    normal, with a Z-extent (depth) along the normal so they poke out of
    //    the surface and read clearly. Matrices are set DIRECTLY (not deferred
    //    to onBeforeRender like the sketch-dot util, which never fired in this
    //    overlay pass → invisible). Filled InstancedMesh meshes DO render here.
    //    Colour by TYPE via per-instance colour: red corner / orange edge /
    //    green surface.
    if (params.showPoints && n > 0) {
      const geo = new THREE.BoxGeometry(1, 1, 1);
      const mat = new THREE.MeshBasicMaterial({
        toneMapped: false,
        depthWrite: false,
        ...depthBias,
      });
      const im = new THREE.InstancedMesh(geo, mat, n);
      im.frustumCulled = false;
      const centers = new Float32Array(n * 3);
      const basis = new Float32Array(n * 9); // je Punkt u(3), v(3), n(3) — Einheit
      const eu = new THREE.Vector3();
      const ev = new THREE.Vector3();
      const en = new THREE.Vector3();
      const col = new THREE.Color();
      for (let idx = 0; idx < n; idx++) {
        const b = P + keep[idx] * STRIDE;
        centers[idx * 3] = rawX(b);
        centers[idx * 3 + 1] = rawY(b);
        centers[idx * 3 + 2] = rawZ(b);
        const nx = data[b + 4], ny = data[b + 5], nz = data[b + 6];
        en.set(nx, ny, nz).normalize();
        const sx = Math.abs(nx) < 0.9 ? 1 : 0;
        const sy = Math.abs(nx) < 0.9 ? 0 : 1;
        const dsn = sx * en.x + sy * en.y;
        eu.set(sx - dsn * en.x, sy - dsn * en.y, -dsn * en.z).normalize();
        ev.crossVectors(en, eu);
        basis.set([eu.x, eu.y, eu.z, ev.x, ev.y, ev.z, en.x, en.y, en.z], idx * 9);
        const flag = data[b + 8];
        col.setHex(
          flag >= 3.5
            ? 0xd91a1a // seed (priority — wins over corner/edge): rot
            : flag >= 2.5
              ? 0x2563eb // corner: blau
              : flag >= 1.5
                ? 0xf28c1a // edge chain: orange
                : 0x2ea043, // surface particle: grün
        );
        im.setColorAt(idx, col);
      }
      if (im.instanceColor) im.instanceColor.needsUpdate = true;
      im.layers.enable(OVERLAY_LAYER);
      im.renderOrder = 6;
      dotsRef.current.push(im);
      group.add(im);
      // `pointSize` = konstante Pixel-Kantenlänge; die Matrizen werden pro Frame
      // (useFrame) zoom-abhängig gesetzt, initial hier einmal.
      pointDataRef.current = { mesh: im, centers, basis, px: params.pointSize };
      rescaleBoxes(pointDataRef.current, camera, size.height);
    } else {
      pointDataRef.current = null;
    }

    // ── U(p) reach circles (radius = per-point U, index 7) in the tangent
    //    plane ⟂ face normal — the signature Lichen visualisation (ticket kfie).
    //    Driven by the trailing FACE-NORMAL block, NOT the single averaged
    //    LPoint normal: a boundary point (edge/corner) lies on ≥2 faces and
    //    therefore emits one record PER adjacent face → „2 Kreise pro Randpunkt".
    //    A surface particle emits exactly one record (its own ∇f). Block layout:
    //    [..indices.., nFaceRec, (pointIdx,nx,ny,nz)×nFaceRec]. ──
    if (params.showCircles) {
      const SEG = 240; // 10× feiner aufgelöst
      const seg: number[] = [];
      // Shown-Lookup über Original-Punkt-Index (Records referenzieren Punkt-
      // Indizes, nicht keep-Positionen).
      const shown = new Uint8Array(total);
      for (const k of keep) shown[k] = 1;
      const faceBase = I + triCount * 3;
      const nFaceRec = data.length > faceBase ? data[faceBase] : 0;
      const recBase = faceBase + 1;
      for (let rr = 0; rr < nFaceRec; rr++) {
        const rb = recBase + rr * 4;
        const pi = data[rb];
        if (!shown[pi]) continue;
        const b = P + pi * STRIDE;
        const u = data[b + 7];
        if (!(u > 0)) continue;
        const x = rawX(b), y = rawY(b), z = rawZ(b);
        const nx = data[rb + 1], ny = data[rb + 2], nz = data[rb + 3];
        // Tangent basis (e ⟂ f ⟂ n), matching the kernel's construction.
        const sx = Math.abs(nx) < 0.9 ? 1 : 0;
        const sy = Math.abs(nx) < 0.9 ? 0 : 1;
        const sdotn = sx * nx + sy * ny;
        let ex = sx - sdotn * nx, ey = sy - sdotn * ny, ez = -sdotn * nz;
        const el = Math.hypot(ex, ey, ez) || 1;
        ex /= el; ey /= el; ez /= el;
        const fx = ny * ez - nz * ey;
        const fy = nz * ex - nx * ez;
        const fz = nx * ey - ny * ex;
        let px0 = 0, py0 = 0, pz0 = 0;
        for (let k = 0; k <= SEG; k++) {
          const a = (k / SEG) * Math.PI * 2;
          const ca = Math.cos(a) * u, sa = Math.sin(a) * u;
          const rx = x + ex * ca + fx * sa;
          const ry = y + ey * ca + fy * sa;
          const rz = z + ez * ca + fz * sa;
          if (k > 0) seg.push(px0, py0, pz0, rx, ry, rz);
          px0 = rx; py0 = ry; pz0 = rz;
        }
      }
      if (seg.length > 0) {
        let circles: THREE.Object3D;
        if (isWebGPURenderer(gl)) {
          const lg = new LineSegmentsGeometry();
          lg.setPositions(new Float32Array(seg));
          circles = new LineSegments2(
            lg,
            new Line2NodeMaterial({
              color: 0x2f7f5f,
              linewidth: Math.max(1, params.strokeWidth * 0.5),
              worldUnits: false,
              transparent: true,
              opacity: 0.7,
              depthWrite: false,
              ...depthBias,
            }),
          );
        } else {
          const lg = new THREE.BufferGeometry();
          lg.setAttribute("position", new THREE.BufferAttribute(new Float32Array(seg), 3));
          circles = new THREE.LineSegments(
            lg,
            new THREE.LineBasicMaterial({
              color: 0x2f7f5f,
              transparent: true,
              opacity: 0.7,
              depthWrite: false,
              toneMapped: false,
            }),
          );
        }
        circles.layers.enable(OVERLAY_LAYER);
        circles.renderOrder = 5;
        group.add(circles);
      }
    }

    // ── Point normals ── a needle from each point along its ∇f normal, to
    //    inspect orientation (esp. at edge/corner points, where the normal is
    //    ambiguous — one per adjacent face).
    if (params.showNormals && params.normalLength > 0) {
      const seg: number[] = [];
      for (let idx = 0; idx < n; idx++) {
        const b = P + keep[idx] * STRIDE;
        const x = rawX(b), y = rawY(b), z = rawZ(b);
        const nx = data[b + 4], ny = data[b + 5], nz = data[b + 6];
        const l = params.normalLength;
        seg.push(x, y, z, x + nx * l, y + ny * l, z + nz * l);
      }
      if (seg.length > 0) {
        let needles: THREE.Object3D;
        if (isWebGPURenderer(gl)) {
          const lg = new LineSegmentsGeometry();
          lg.setPositions(new Float32Array(seg));
          needles = new LineSegments2(
            lg,
            new Line2NodeMaterial({
              color: 0x2288ee,
              linewidth: Math.max(1, params.strokeWidth),
              worldUnits: false,
              transparent: true,
              depthWrite: false,
              ...depthBias,
            }),
          );
        } else {
          const lg = new THREE.BufferGeometry();
          lg.setAttribute("position", new THREE.BufferAttribute(new Float32Array(seg), 3));
          needles = new THREE.LineSegments(
            lg,
            new THREE.LineBasicMaterial({
              color: 0x2288ee,
              transparent: true,
              depthWrite: false,
              toneMapped: false,
            }),
          );
        }
        needles.layers.enable(OVERLAY_LAYER);
        needles.renderOrder = 7;
        group.add(needles);
      }
    }

    // ── Normal lines (stroke width, SDF-coloured) ──
    if (params.normalScale > 0) {
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
            ...depthBias,
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

  // Keep the point boxes at a constant PIXEL size on zoom/orbit (mirrors the
  // sketch-dot approach, but driven here so it works in this overlay pass).
  useFrame((s) => {
    if (pointDataRef.current) rescaleBoxes(pointDataRef.current, s.camera, s.size.height);
  });

  useEffect(() => {
    return () => {
      if (groupRef.current) disposeChildren(groupRef.current);
      for (const d of dotsRef.current) d.dispose();
      dotsRef.current = [];
      circle.dispose();
    };
  }, [circle]);

  return <group ref={groupRef} />;
}
