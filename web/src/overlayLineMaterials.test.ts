import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { LineSegments2 } from "three/examples/jsm/lines/webgpu/LineSegments2.js";
import {
  createFatLineMaterials,
  isWebGPURenderer,
  makeFatLine,
} from "./overlayLineMaterials";

describe("isWebGPURenderer", () => {
  it("detects the WebGPU renderer flag", () => {
    expect(isWebGPURenderer({ isWebGPURenderer: true })).toBe(true);
  });
  it("treats a classic WebGLRenderer (no flag) as not WebGPU", () => {
    expect(isWebGPURenderer({})).toBe(false);
    expect(isWebGPURenderer({ isWebGPURenderer: false })).toBe(false);
  });
});

describe("createFatLineMaterials", () => {
  it("makes a node + classic pair sharing the colour", () => {
    const m = createFatLineMaterials({ color: 0x123456, linewidth: 3 });
    expect(m.classic).toBeInstanceOf(THREE.LineBasicMaterial);
    // The node material is a three/webgpu material with no classic GLSL
    // source — that's exactly why it must never reach the WebGLRenderer.
    expect(m.classic.color.getHex()).toBe(0x123456);
    expect(m.node.depthTest).toBe(false);
    expect(m.classic.depthTest).toBe(false);
  });
  it("honours an explicit depthTest", () => {
    const m = createFatLineMaterials({
      color: 0xffffff,
      linewidth: 1,
      depthTest: true,
    });
    expect(m.node.depthTest).toBe(true);
    expect(m.classic.depthTest).toBe(true);
  });
});

describe("makeFatLine", () => {
  const positions = new Float32Array([0, 0, 0, 1, 1, 1]);
  const mats = createFatLineMaterials({ color: 0x000000, linewidth: 2 });

  it("builds an instanced LineSegments2 with the NODE material on WebGPU", () => {
    const { object, geometry } = makeFatLine(positions, mats, true);
    expect(object).toBeInstanceOf(LineSegments2);
    expect((object as LineSegments2).material).toBe(mats.node);
    geometry.dispose();
  });

  it("builds a classic LineSegments with the CLASSIC material on WebGL", () => {
    const { object, geometry } = makeFatLine(positions, mats, false);
    expect(object).toBeInstanceOf(THREE.LineSegments);
    // Crucially NOT a node material — that is what crashed resolveIncludes.
    expect((object as THREE.LineSegments).material).toBe(mats.classic);
    geometry.dispose();
  });

  it("accepts a plain number[] segment buffer", () => {
    const { object, geometry } = makeFatLine([0, 0, 0, 2, 0, 0], mats, false);
    expect(object).toBeInstanceOf(THREE.LineSegments);
    geometry.dispose();
  });
});
