/**
 * Debug SDF-plane overlay.
 *
 * When the "Debug ▸ SDF-Plane" dialog publishes a plane (point + normal +
 * half-extent + resolution + iso-band count) to `sdfPlaneStore`, this samples
 * the scene SDF on that plane (kernel `sdfPlaneSample`, the exact field the
 * meshers contour) and draws a colour-coded quad:
 *   · blue inside (SDF < 0), red outside (SDF > 0), value → saturation;
 *   · dark iso-contour bands at regular distances (a topographic map of the
 *     field — reveals thin channels and how the surface is approached);
 *   · a bright green zero-surface line (the contour the meshers chase).
 *
 * The quad is built with the SAME (u, v) in-plane frame the kernel samples in,
 * so the texture aligns exactly. It lives on OVERLAY_LAYER so the SDF render
 * paths composite it, and re-samples whenever the plane params or the geometry
 * (`brepVersion`) change. It also projects the mouse onto the plane and
 * publishes the SDF value there for the dialog's readout. Returns null when no
 * plane is published.
 */

import { useEffect, useMemo, useRef, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import {
  getSdfPlane,
  subscribeSdfPlane,
  setSdfPlaneHover,
} from "./sdfPlaneStore";
import { OVERLAY_LAYER } from "./viewportLayers";

const DEFAULT_RES = 256;
const MAX_RES = 1024;

/** Unit-normalise, falling back to +Z for a degenerate normal. */
function unit(n: [number, number, number]): [number, number, number] {
  const l = Math.hypot(n[0], n[1], n[2]);
  if (l < 1e-9) return [0, 0, 1];
  return [n[0] / l, n[1] / l, n[2] / l];
}
function cross(
  a: [number, number, number],
  b: [number, number, number],
): [number, number, number] {
  return [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ];
}

/** The in-plane (u, v) frame — must mirror `Kernel::sdf_plane_sample`. */
function planeFrame(normal: [number, number, number]): {
  n: [number, number, number];
  u: [number, number, number];
  v: [number, number, number];
} {
  const n = unit(normal);
  const up: [number, number, number] =
    Math.abs(n[1]) < 0.9 ? [0, 1, 0] : [1, 0, 0];
  const u = unit(cross(up, n));
  const v = cross(n, u);
  return { n, u, v };
}

/** Colour-map one SDF sample (mm) into RGBA, given the iso-band spacing. */
function colorFor(d: number, band: number, out: Uint8Array, o: number): void {
  const zeroHalf = band * 0.12; // green surface line width
  if (Math.abs(d) <= zeroHalf) {
    out[o] = 40;
    out[o + 1] = 255;
    out[o + 2] = 120;
    out[o + 3] = 255;
    return;
  }
  // Saturation grows with distance (clamped a few bands out).
  const mag = Math.min(1, Math.abs(d) / (band * 6));
  // Iso-band: darken near each multiple of `band`.
  const f = Math.abs(((d / band) % 1) + 1) % 1; // fract, always 0..1
  const edge = Math.min(f, 1 - f); // 0 at a band line
  const shade = 0.55 + 0.45 * Math.min(1, edge / 0.12);
  let r: number, g: number, b: number;
  if (d < 0) {
    // inside: blue
    r = 30 + 40 * (1 - mag);
    g = 90 + 80 * (1 - mag);
    b = 200 + 55 * mag;
  } else {
    // outside: red/orange
    r = 210 + 45 * mag;
    g = 90 + 60 * (1 - mag);
    b = 40 + 40 * (1 - mag);
  }
  out[o] = Math.round(r * shade);
  out[o + 1] = Math.round(g * shade);
  out[o + 2] = Math.round(b * shade);
  out[o + 3] = 255;
}

/** In-plane frame the sampling used, cached for the hover projection. */
interface HoverFrame {
  point: [number, number, number];
  n: [number, number, number];
  u: [number, number, number];
  v: [number, number, number];
  he: number;
  res: number;
  samples: Float32Array;
}

export function SdfPlaneOverlay() {
  const { store, state } = useKernelStore();
  const plane = useSyncExternalStore(subscribeSdfPlane, getSdfPlane);
  const invalidate = useThree((s) => s.invalidate);
  const camera = useThree((s) => s.camera);
  const gl = useThree((s) => s.gl);

  const res = Math.max(
    16,
    Math.min(MAX_RES, Math.round(plane?.resolution ?? DEFAULT_RES)),
  );

  const geometry = useMemo(() => new THREE.BufferGeometry(), []);
  const texture = useMemo(() => {
    const t = new THREE.DataTexture(
      new Uint8Array(res * res * 4),
      res,
      res,
      THREE.RGBAFormat,
    );
    t.minFilter = THREE.NearestFilter;
    t.magFilter = THREE.NearestFilter;
    // Authored 0–255 colours are sRGB: without this three treats them as
    // linear and the debug palette washes out.
    t.colorSpace = THREE.SRGBColorSpace;
    return t;
  }, [res]);

  useEffect(
    () => () => {
      geometry.dispose();
    },
    [geometry],
  );
  useEffect(() => () => texture.dispose(), [texture]);

  // Frame + samples the hover projection reads (kept in a ref so the pointer
  // handler always sees the latest without re-binding).
  const hoverRef = useRef<HoverFrame | null>(null);

  // Re-sample the field + rebuild the quad whenever the plane params or the
  // geometry change. `state?.brepVersion` bumps on every kernel edit/weave.
  const brepVersion = state?.brepVersion ?? 0;
  const bands = Math.max(1, Math.round(plane?.bands ?? 16));
  useEffect(() => {
    if (!plane || !store) return;
    const { point, normal, halfExtent } = plane;
    const he = Math.max(halfExtent, 1e-3);
    const { n, u, v } = planeFrame(normal);

    // Quad corners at point ± u·he ± v·he, UVs matching the sample grid
    // (texel (i,j) at u=i/(res-1), v=j/(res-1); (0,0) = point − u·he − v·he).
    const corner = (su: number, sv: number): [number, number, number] => [
      point[0] + u[0] * su * he + v[0] * sv * he,
      point[1] + u[1] * su * he + v[1] * sv * he,
      point[2] + u[2] * su * he + v[2] * sv * he,
    ];
    const positions = new Float32Array([
      ...corner(-1, -1),
      ...corner(1, -1),
      ...corner(1, 1),
      ...corner(-1, 1),
    ]);
    const uvs = new Float32Array([0, 0, 1, 0, 1, 1, 0, 1]);
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.setAttribute("uv", new THREE.BufferAttribute(uvs, 2));
    geometry.setIndex([0, 1, 2, 0, 2, 3]);
    geometry.computeBoundingSphere();

    const samples = store.sdfPlaneSample(point, normal, he, res);
    const band = Math.max(he / bands, 1e-4); // iso-band spacing
    const data = texture.image.data as Uint8Array;
    for (let k = 0; k < res * res; k++) {
      colorFor(samples[k] ?? Number.MAX_VALUE, band, data, k * 4);
    }
    texture.needsUpdate = true;
    hoverRef.current = { point, n, u, v, he, res, samples };
    invalidate();
  }, [plane, store, brepVersion, res, bands, geometry, texture, invalidate]);

  // Project the mouse onto the plane and publish the SDF value there (bilinear
  // in the sampled grid, so it matches exactly what is drawn).
  useEffect(() => {
    if (!plane) return;
    const el = gl.domElement;
    const raycaster = new THREE.Raycaster();
    const ndc = new THREE.Vector2();
    const hitPlane = new THREE.Plane();
    const nrm = new THREE.Vector3();
    const pt = new THREE.Vector3();
    const hit = new THREE.Vector3();
    const rel = new THREE.Vector3();

    const onMove = (e: PointerEvent) => {
      const f = hoverRef.current;
      if (!f) return;
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      ndc.set(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1,
      );
      raycaster.setFromCamera(ndc, camera);
      nrm.set(f.n[0], f.n[1], f.n[2]);
      pt.set(f.point[0], f.point[1], f.point[2]);
      hitPlane.setFromNormalAndCoplanarPoint(nrm, pt);
      if (!raycaster.ray.intersectPlane(hitPlane, hit)) {
        setSdfPlaneHover(null);
        return;
      }
      rel.copy(hit).sub(pt);
      const lu = rel.x * f.u[0] + rel.y * f.u[1] + rel.z * f.u[2];
      const lv = rel.x * f.v[0] + rel.y * f.v[1] + rel.z * f.v[2];
      if (Math.abs(lu) > f.he || Math.abs(lv) > f.he) {
        setSdfPlaneHover(null); // outside the drawn quad
        return;
      }
      // Bilinear lookup in the sampled grid (v outer, u inner).
      const R = f.res;
      const fx = ((lu / f.he + 1) / 2) * (R - 1);
      const fy = ((lv / f.he + 1) / 2) * (R - 1);
      const i0 = Math.max(0, Math.min(R - 1, Math.floor(fx)));
      const j0 = Math.max(0, Math.min(R - 1, Math.floor(fy)));
      const i1 = Math.min(R - 1, i0 + 1);
      const j1 = Math.min(R - 1, j0 + 1);
      const tx = fx - i0;
      const ty = fy - j0;
      const s = f.samples;
      const s00 = s[j0 * R + i0] ?? 0;
      const s10 = s[j0 * R + i1] ?? 0;
      const s01 = s[j1 * R + i0] ?? 0;
      const s11 = s[j1 * R + i1] ?? 0;
      const value =
        s00 * (1 - tx) * (1 - ty) +
        s10 * tx * (1 - ty) +
        s01 * (1 - tx) * ty +
        s11 * tx * ty;
      setSdfPlaneHover({ value, point: [hit.x, hit.y, hit.z] });
    };
    const onLeave = () => setSdfPlaneHover(null);

    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerleave", onLeave);
    return () => {
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerleave", onLeave);
      setSdfPlaneHover(null);
    };
  }, [plane, gl, camera]);

  if (!plane) return null;

  return (
    <mesh
      ref={(o) => o?.layers.set(OVERLAY_LAYER)}
      geometry={geometry}
      renderOrder={4}
    >
      <meshBasicMaterial
        map={texture}
        side={THREE.DoubleSide}
        transparent
        opacity={0.92}
        depthWrite={false}
        toneMapped={false}
      />
    </mesh>
  );
}
