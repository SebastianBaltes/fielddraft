import { describe, it, expect } from "vitest";
import * as THREE from "three";
import {
  subdivideAlongAxis,
  applyWarp,
  warpAxis,
  maxStepForWarp,
} from "./warpMesh";
import type { LeafWarp } from "./kernel";

/** A unit box centered at origin, spanning [-1,1] on each axis. */
function box(): THREE.BufferGeometry {
  return new THREE.BoxGeometry(2, 2, 2);
}

describe("warpAxis / maxStepForWarp", () => {
  it("twist deforms around Y, bend around X", () => {
    expect(warpAxis({ kind: "twist", rate: 0.1 })).toBe(1);
    expect(warpAxis({ kind: "bend", rate: 0.1 })).toBe(0);
  });

  it("step shrinks as the rate grows", () => {
    const slow = maxStepForWarp({ kind: "twist", rate: 0.01 });
    const fast = maxStepForWarp({ kind: "twist", rate: 0.5 });
    expect(slow).toBeGreaterThan(fast);
    // ~7° band at rate 0.1 → ~1.2 units.
    expect(maxStepForWarp({ kind: "twist", rate: 0.1 })).toBeCloseTo(1.2, 4);
  });
});

describe("subdivideAlongAxis", () => {
  it("resolves the axis extent to ≤ maxStep per band", () => {
    const geo = box(); // Y extent = 2
    subdivideAlongAxis(geo, 1, 0.5); // want ≤0.5 → 2 levels (2→1→0.5)
    const pos = geo.getAttribute("position");
    // Gather distinct Y levels and check the largest gap.
    const ys = [...new Set(Array.from({ length: pos.count }, (_, i) =>
      Math.round(pos.getY(i) * 1e4) / 1e4))].sort((a, b) => a - b);
    let maxGap = 0;
    for (let i = 1; i < ys.length; i++) maxGap = Math.max(maxGap, ys[i] - ys[i - 1]);
    expect(maxGap).toBeLessThanOrEqual(0.5 + 1e-6);
  });

  it("stays watertight (shared midpoints, no vertex explosion)", () => {
    const geo = box();
    subdivideAlongAxis(geo, 1, 0.5);
    const pos = geo.getAttribute("position");
    const idx = geo.getIndex()!;
    // 12 tris → 4^2 = 16× = 192 tris.
    expect(idx.count / 3).toBe(12 * 16);
    // Watertight: far fewer unique verts than 3·triangles (shared edges).
    const uniq = new Set<string>();
    for (let i = 0; i < pos.count; i++)
      uniq.add(`${pos.getX(i)},${pos.getY(i)},${pos.getZ(i)}`);
    expect(uniq.size).toBeLessThan(idx.count);
  });

  it("is a no-op when the extent already fits", () => {
    const geo = box();
    const before = geo.getAttribute("position").count;
    subdivideAlongAxis(geo, 1, 100);
    expect(geo.getAttribute("position").count).toBe(before);
  });

  it("caps subdivision levels (never explodes)", () => {
    const geo = box();
    subdivideAlongAxis(geo, 1, 1e-9); // would demand huge K → capped at 5
    expect(geo.getIndex()!.count / 3).toBe(12 * 4 ** 5);
  });
});

describe("applyWarp twist", () => {
  const warp: LeafWarp = { kind: "twist", rate: 0.2 };

  it("leaves the y=0 plane fixed and preserves radius", () => {
    const geo = new THREE.BufferGeometry();
    // A few points at varying radius/height.
    const pts = [
      [3, 0, 0], // y=0 → unchanged
      [2, 5, 0], // rotated by 0.2·5 = 1.0 rad
      [0, -4, 2],
    ];
    geo.setAttribute("position", new THREE.Float32BufferAttribute(pts.flat(), 3));
    applyWarp(geo, warp);
    const p = geo.getAttribute("position");
    // y=0 stays put.
    expect(p.getX(0)).toBeCloseTo(3, 5);
    expect(p.getZ(0)).toBeCloseTo(0, 5);
    // radius (√(x²+z²)) preserved for every vertex; y preserved.
    for (let i = 0; i < pts.length; i++) {
      const r0 = Math.hypot(pts[i][0], pts[i][2]);
      const r1 = Math.hypot(p.getX(i), p.getZ(i));
      expect(r1).toBeCloseTo(r0, 5);
      expect(p.getY(i)).toBeCloseTo(pts[i][1], 5);
    }
  });

  it("rotates XZ by −rate·y", () => {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.Float32BufferAttribute([2, 5, 0], 3));
    applyWarp(geo, warp);
    const p = geo.getAttribute("position");
    // q=(2,5,0): c=cos(1),s=sin(1); x'=c·2, z'=-s·2.
    expect(p.getX(0)).toBeCloseTo(Math.cos(1) * 2, 5);
    expect(p.getZ(0)).toBeCloseTo(-Math.sin(1) * 2, 5);
  });
});

describe("applyWarp bend", () => {
  const rate = 0.05;
  const warp: LeafWarp = { kind: "bend", rate };

  // The kernel evaluates the child at rotateZ(+rate·px)·p. Re-apply that
  // forward map to the warp result and confirm we recover the input vertex.
  function kernelForward(px: number, py: number): [number, number] {
    const c = Math.cos(rate * px);
    const s = Math.sin(rate * px);
    return [c * px - s * py, s * px + c * py];
  }

  it("inverts the kernel bend warp (round-trips within the gate)", () => {
    const inputs = [
      [10, 3],
      [-8, 5],
      [15, -4],
      [0, 6],
    ];
    const geo = new THREE.BufferGeometry();
    geo.setAttribute(
      "position",
      new THREE.Float32BufferAttribute(inputs.flatMap(([x, y]) => [x, y, 0]), 3),
    );
    applyWarp(geo, warp);
    const p = geo.getAttribute("position");
    for (let i = 0; i < inputs.length; i++) {
      const [fx, fy] = kernelForward(p.getX(i), p.getY(i));
      expect(fx).toBeCloseTo(inputs[i][0], 3);
      expect(fy).toBeCloseTo(inputs[i][1], 3);
    }
  });

  it("leaves x=0 vertices on the y axis", () => {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.Float32BufferAttribute([0, 7, 0], 3));
    applyWarp(geo, warp);
    const p = geo.getAttribute("position");
    expect(p.getX(0)).toBeCloseTo(0, 5);
    expect(p.getY(0)).toBeCloseTo(7, 5);
  });
});
