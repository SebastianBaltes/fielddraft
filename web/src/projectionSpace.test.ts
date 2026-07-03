import { describe, expect, it } from "vitest";
import * as THREE from "three";
import { ndcZVisible } from "./projectionSpace";

function orthoCam(
  coordinateSystem: THREE.CoordinateSystem,
): THREE.OrthographicCamera {
  const cam = new THREE.OrthographicCamera(-100, 100, 100, -100, 1, 1000);
  cam.coordinateSystem = coordinateSystem;
  cam.position.set(0, 0, 500);
  cam.updateProjectionMatrix();
  cam.updateMatrixWorld();
  return cam;
}

describe("ndcZVisible", () => {
  it("rejects a point behind a WebGPU ortho camera (z < 0)", () => {
    const cam = orthoCam(THREE.WebGPUCoordinateSystem);
    const behind = new THREE.Vector3(0, 0, 600).project(cam);
    expect(behind.z).toBeLessThan(0);
    expect(ndcZVisible(cam, behind.z)).toBe(false);
  });

  it("accepts a point inside the WebGPU frustum (z ∈ [0,1])", () => {
    const cam = orthoCam(THREE.WebGPUCoordinateSystem);
    const inside = new THREE.Vector3(0, 0, 0).project(cam);
    expect(inside.z).toBeGreaterThanOrEqual(0);
    expect(inside.z).toBeLessThanOrEqual(1);
    expect(ndcZVisible(cam, inside.z)).toBe(true);
  });

  it("keeps the WebGL [-1,1] range for WebGL cameras", () => {
    const cam = orthoCam(THREE.WebGLCoordinateSystem);
    const inside = new THREE.Vector3(0, 0, 400).project(cam);
    expect(inside.z).toBeLessThan(0); // near-plane side maps to -1 in GL
    expect(ndcZVisible(cam, inside.z)).toBe(true);
    const behind = new THREE.Vector3(0, 0, 600).project(cam);
    expect(ndcZVisible(cam, behind.z)).toBe(false);
  });

  it("rejects past-far points under both conventions", () => {
    for (const cs of [THREE.WebGPUCoordinateSystem, THREE.WebGLCoordinateSystem]) {
      const cam = orthoCam(cs);
      const past = new THREE.Vector3(0, 0, -2000).project(cam);
      expect(ndcZVisible(cam, past.z)).toBe(false);
    }
  });
});
