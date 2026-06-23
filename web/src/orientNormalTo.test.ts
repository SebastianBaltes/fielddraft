import { describe, it, expect, beforeEach } from "vitest";
import * as THREE from "three";
import { orientNormalTo } from "./viewportCamera";
import { publishSceneBounds } from "./sceneBoundsBridge";

/** Unit forward direction the camera is looking along (−Z in camera space). */
function viewDir(cam: THREE.Camera): THREE.Vector3 {
  return new THREE.Vector3(0, 0, -1).applyQuaternion(cam.quaternion).normalize();
}

describe("orientNormalTo (ticket z0vk — face/sketch shared 'normal to' math)", () => {
  beforeEach(() => publishSceneBounds({ center: [0, 0, 0], radius: 100 }));

  it("looks straight down a +Z face normal toward the centroid", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    cam.position.set(0, 0, 500);
    expect(orientNormalTo(cam, [0, 0, 0], [0, 0, 1])).toBe(true);
    // The camera sits on the +Z side and looks back toward the origin: view
    // direction ≈ −Z.
    const dir = viewDir(cam);
    expect(dir.z).toBeCloseTo(-1, 5);
    expect(cam.position.z).toBeGreaterThan(0);
  });

  it("stays on the camera's current side of the plane (no 180° flip)", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    // Camera is on the −Z side, but the face normal points +Z. The result must
    // keep the camera on the −Z side and look toward the face.
    cam.position.set(0, 0, -500);
    orientNormalTo(cam, [0, 0, 0], [0, 0, 1]);
    expect(cam.position.z).toBeLessThan(0);
    expect(viewDir(cam).z).toBeCloseTo(1, 5);
  });

  it("frames an off-origin face about its own centroid", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    cam.position.set(0, 100, 0);
    orientNormalTo(cam, [50, 0, 0], [0, 1, 0]);
    // Looking down +Y normal at centroid (50,0,0): camera stays on +Y side,
    // aimed at the centroid's X.
    expect(cam.position.y).toBeGreaterThan(0);
    expect(cam.position.x).toBeCloseTo(50, 4);
    expect(viewDir(cam).y).toBeCloseTo(-1, 5);
  });

  it("rejects a degenerate (zero-length) normal without moving the camera", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    cam.position.set(1, 2, 3);
    expect(orientNormalTo(cam, [0, 0, 0], [0, 0, 0])).toBe(false);
    expect(cam.position.toArray()).toEqual([1, 2, 3]);
  });

  it("keeps a valid up vector even when the current up is parallel to the normal", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    // Camera up is +Y and the face normal is also +Y → the projected up is
    // degenerate; the fallback must still produce a unit up ⊥ normal.
    cam.up.set(0, 1, 0);
    cam.position.set(0, 500, 0);
    orientNormalTo(cam, [0, 0, 0], [0, 1, 0]);
    expect(cam.up.length()).toBeCloseTo(1, 5);
    expect(Math.abs(cam.up.dot(new THREE.Vector3(0, 1, 0)))).toBeLessThan(1e-5);
  });

  it("empty scene: keeps the current standoff and just faces the plane", () => {
    publishSceneBounds(null);
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    cam.position.set(0, 0, 250);
    orientNormalTo(cam, [0, 0, 0], [0, 0, 1]);
    expect(cam.position.length()).toBeCloseTo(250, 3);
    expect(viewDir(cam).z).toBeCloseTo(-1, 5);
  });
});
