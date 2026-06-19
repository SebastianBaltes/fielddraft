import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { ensureOutwardOrientation, assembleShell } from "./sweptShell";
import type { ContourData } from "./kernel";

/** Signed volume (·6) of an indexed mesh — positive ⇔ outward winding. */
function signedVol6(geo: THREE.BufferGeometry): number {
  const idx = geo.getIndex()!;
  const pos = geo.getAttribute("position");
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const t = new THREE.Vector3();
  let v = 0;
  for (let i = 0; i < idx.count; i += 3) {
    a.fromBufferAttribute(pos, idx.getX(i));
    b.fromBufferAttribute(pos, idx.getX(i + 1));
    c.fromBufferAttribute(pos, idx.getX(i + 2));
    v += a.dot(t.crossVectors(b, c));
  }
  return v / 6;
}

/** Σ (geometric face normal · summed stored vertex normals) — positive ⇔
 *  the normal attribute is consistent with (outward) winding everywhere. */
function normalAgreement(geo: THREE.BufferGeometry): number {
  const idx = geo.getIndex()!;
  const pos = geo.getAttribute("position");
  const nor = geo.getAttribute("normal");
  const a = new THREE.Vector3();
  const b = new THREE.Vector3();
  const c = new THREE.Vector3();
  const e1 = new THREE.Vector3();
  const e2 = new THREE.Vector3();
  const fn = new THREE.Vector3();
  let s = 0;
  for (let i = 0; i < idx.count; i += 3) {
    const ia = idx.getX(i);
    const ib = idx.getX(i + 1);
    const ic = idx.getX(i + 2);
    a.fromBufferAttribute(pos, ia);
    b.fromBufferAttribute(pos, ib);
    c.fromBufferAttribute(pos, ic);
    fn.crossVectors(e1.subVectors(b, a), e2.subVectors(c, a));
    s +=
      fn.x * (nor.getX(ia) + nor.getX(ib) + nor.getX(ic)) +
      fn.y * (nor.getY(ia) + nor.getY(ib) + nor.getY(ic)) +
      fn.z * (nor.getZ(ia) + nor.getZ(ic) + nor.getZ(ib));
  }
  return s;
}

/** Build a straight prism shell from a CCW square cross-section (the kernel
 *  always emits CCW outers — `sketch2d.rs` normalises winding), stacking the
 *  rings along +Z or −Z. A −Z spine flips the lateral-wall winding inward even
 *  though the contour is CCW: the realistic inside-out case for a sweep/helix
 *  whose path runs "backwards". assembleShell must hand back an outward shell
 *  either way. */
function sweptSquare(spineDir: 1 | -1): THREE.BufferGeometry {
  const sq: [number, number][] = [[-1, -1], [1, -1], [1, 1], [-1, 1]];
  const contour: ContourData = { outer: sq, holes: [] };
  const rings = 5;
  const ppr = 4;
  const positions = new Float32Array(rings * ppr * 3);
  for (let k = 0; k < rings; k++) {
    for (let j = 0; j < ppr; j++) {
      const o = (k * ppr + j) * 3;
      positions[o] = sq[j][0];
      positions[o + 1] = sq[j][1];
      positions[o + 2] = k * 2 * spineDir;
    }
  }
  return assembleShell(positions, rings, contour);
}

describe("ensureOutwardOrientation", () => {
  it("forces a CW-wound LatheGeometry outward (winding + normals)", () => {
    // Profile top→bottom on +x → LatheGeometry winds inward.
    const pts = [
      new THREE.Vector2(2, 1),
      new THREE.Vector2(3, 0),
      new THREE.Vector2(2, -1),
    ];
    const geo = new THREE.LatheGeometry(pts, 48);
    expect(signedVol6(geo)).toBeLessThan(0); // inside-out as built
    ensureOutwardOrientation(geo);
    expect(signedVol6(geo)).toBeGreaterThan(0);
    expect(normalAgreement(geo)).toBeGreaterThan(0);
  });

  it("handles a torus-loop revolve (profile never meets the axis)", () => {
    const pts: THREE.Vector2[] = [];
    for (let i = 0; i <= 40; i++) {
      const t = (2 * Math.PI * i) / 40;
      pts.push(new THREE.Vector2(5 + 1.5 * Math.cos(t), 1.5 * Math.sin(t)));
    }
    // CW order → inward; the radial heuristic would near-cancel on a torus,
    // the signed-volume test must not.
    const geo = new THREE.LatheGeometry([...pts].reverse(), 48);
    expect(signedVol6(geo)).toBeLessThan(0);
    ensureOutwardOrientation(geo);
    expect(signedVol6(geo)).toBeGreaterThan(0);
    expect(normalAgreement(geo)).toBeGreaterThan(0);
  });

  it("is a no-op on an already-outward mesh", () => {
    const geo = new THREE.LatheGeometry(
      [
        new THREE.Vector2(2, -1),
        new THREE.Vector2(3, 0),
        new THREE.Vector2(2, 1),
      ],
      48,
    );
    const before = signedVol6(geo);
    expect(before).toBeGreaterThan(0);
    ensureOutwardOrientation(geo);
    expect(signedVol6(geo)).toBeCloseTo(before, 6);
  });
});

describe("assembleShell orientation", () => {
  it("yields an outward shell regardless of the spine direction", () => {
    const fwd = sweptSquare(1);
    const bwd = sweptSquare(-1);
    for (const g of [fwd, bwd]) {
      expect(signedVol6(g)).toBeGreaterThan(0);
      expect(normalAgreement(g)).toBeGreaterThan(0);
    }
    // Same 2×2×8 prism either way → identical enclosed volume.
    expect(signedVol6(fwd)).toBeCloseTo(signedVol6(bwd), 5);
    expect(signedVol6(fwd)).toBeCloseTo(32, 5);
  });
});
