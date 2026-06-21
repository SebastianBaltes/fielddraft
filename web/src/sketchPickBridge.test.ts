import { describe, it, expect, afterEach } from "vitest";
import * as THREE from "three";
import {
  publishSketchPickables,
  clearSketchPickables,
  pickSketchAtRay,
  type SketchPickFrame,
} from "./sketchPickBridge";

// Identity XY frame: world (x, y, z) → uv = (x, y).
const XY: SketchPickFrame = {
  origin: [0, 0, 0],
  xAxis: [1, 0, 0],
  yAxis: [0, 1, 0],
};

afterEach(() => clearSketchPickables());

describe("pickSketchAtRay (ticket dhjn — region resolution)", () => {
  it("returns the world hit point and its sketch-space uv", () => {
    // One filled triangle in the z=0 plane spanning (0,0)–(10,0)–(0,10).
    const fill = new Float32Array([0, 0, 0, 10, 0, 0, 0, 10, 0]);
    publishSketchPickables([{ sketchId: 7, fill, lines: null, frame: XY }]);

    // Ray straight down the −z axis through world (2, 3).
    const ray = new THREE.Ray(
      new THREE.Vector3(2, 3, 10),
      new THREE.Vector3(0, 0, -1),
    );
    const hit = pickSketchAtRay(ray);
    expect(hit).not.toBeNull();
    expect(hit!.sketchId).toBe(7);
    expect(hit!.point[0]).toBeCloseTo(2, 5);
    expect(hit!.point[1]).toBeCloseTo(3, 5);
    expect(hit!.point[2]).toBeCloseTo(0, 5);
    // On the identity XY frame, uv == (x, y).
    expect(hit!.uv[0]).toBeCloseTo(2, 5);
    expect(hit!.uv[1]).toBeCloseTo(3, 5);
  });

  it("maps world→uv through a non-identity frame", () => {
    // A frame on the XZ plane: xAxis = +x, yAxis = +z.
    const XZ: SketchPickFrame = {
      origin: [0, 0, 0],
      xAxis: [1, 0, 0],
      yAxis: [0, 0, 1],
    };
    // Triangle in the y=0 plane.
    const fill = new Float32Array([0, 0, 0, 10, 0, 0, 0, 0, 10]);
    publishSketchPickables([{ sketchId: 3, fill, lines: null, frame: XZ }]);

    const ray = new THREE.Ray(
      new THREE.Vector3(2, 10, 3),
      new THREE.Vector3(0, -1, 0),
    );
    const hit = pickSketchAtRay(ray);
    expect(hit).not.toBeNull();
    // uv = ((P)·xAxis, (P)·yAxis) = (x, z) = (2, 3).
    expect(hit!.uv[0]).toBeCloseTo(2, 5);
    expect(hit!.uv[1]).toBeCloseTo(3, 5);
  });

  it("returns null when the ray misses every sketch", () => {
    const fill = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]);
    publishSketchPickables([{ sketchId: 1, fill, lines: null, frame: XY }]);
    const ray = new THREE.Ray(
      new THREE.Vector3(100, 100, 10),
      new THREE.Vector3(0, 0, -1),
    );
    expect(pickSketchAtRay(ray)).toBeNull();
  });
});
