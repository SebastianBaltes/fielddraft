/**
 * Debug SDF-plane overlay.
 *
 * When the "Debug ▸ SDF-Plane" dialog publishes a plane (point + normal +
 * half-extent) to `sdfPlaneStore`, this samples the scene SDF on that plane
 * (kernel `sdfPlaneSample`, the exact field the meshers contour) and draws a
 * colour-coded quad:
 *   · blue inside (SDF < 0), red outside (SDF > 0), value → saturation;
 *   · dark iso-contour bands at regular distances (a topographic map of the
 *     field — reveals thin channels and how the surface is approached);
 *   · a bright green zero-surface line (the contour the meshers chase).
 *
 * The quad is built with the SAME (u, v) in-plane frame the kernel samples in,
 * so the texture aligns exactly. It lives on OVERLAY_LAYER so the SDF render
 * paths composite it, and re-samples whenever the plane params or the geometry
 * (`brepVersion`) change. Returns null when no plane is published.
 */

import { useEffect, useMemo, useSyncExternalStore } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useKernelStore } from "./store";
import { getSdfPlane, subscribeSdfPlane } from "./sdfPlaneStore";
import { OVERLAY_LAYER } from "./viewportLayers";

const RES = 256;

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

export function SdfPlaneOverlay() {
  const { store, state } = useKernelStore();
  const plane = useSyncExternalStore(subscribeSdfPlane, getSdfPlane);
  const invalidate = useThree((s) => s.invalidate);

  const geometry = useMemo(() => new THREE.BufferGeometry(), []);
  const texture = useMemo(() => {
    const t = new THREE.DataTexture(
      new Uint8Array(RES * RES * 4),
      RES,
      RES,
      THREE.RGBAFormat,
    );
    t.minFilter = THREE.NearestFilter;
    t.magFilter = THREE.NearestFilter;
    // Authored 0–255 colours are sRGB: without this three treats them as
    // linear and the debug palette washes out.
    t.colorSpace = THREE.SRGBColorSpace;
    return t;
  }, []);

  useEffect(
    () => () => {
      geometry.dispose();
      texture.dispose();
    },
    [geometry, texture],
  );

  // Re-sample the field + rebuild the quad whenever the plane params or the
  // geometry change. `state?.brepVersion` bumps on every kernel edit/weave.
  const brepVersion = state?.brepVersion ?? 0;
  useEffect(() => {
    if (!plane || !store) return;
    const { point, normal, halfExtent } = plane;
    const he = Math.max(halfExtent, 1e-3);
    const { u, v } = planeFrame(normal);

    // Quad corners at point ± u·he ± v·he, UVs matching the sample grid
    // (texel (i,j) at u=i/(RES-1), v=j/(RES-1); (0,0) = point − u·he − v·he).
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

    const samples = store.sdfPlaneSample(point, normal, he, RES);
    const band = Math.max(he / 8, 0.05); // iso-band spacing
    const data = texture.image.data as Uint8Array;
    for (let k = 0; k < RES * RES; k++) {
      colorFor(samples[k] ?? Number.MAX_VALUE, band, data, k * 4);
    }
    texture.needsUpdate = true;
    invalidate();
  }, [plane, store, brepVersion, geometry, texture, invalidate]);

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
