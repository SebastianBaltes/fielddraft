/**
 * Ticket pzvt — "pick what you see": the render-camera snapshot must keep
 * pick projections at the last PRESENTED frame while the live camera moves
 * (frozen-frame window during SDF shader compiles).
 */

import { describe, expect, it, afterEach } from "vitest";
import * as THREE from "three";
import {
  publishRenderCamera,
  clearRenderCamera,
  getPickCamera,
} from "./renderCameraBridge";

function makeCamera(x: number): THREE.OrthographicCamera {
  const cam = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 100);
  cam.position.set(x, 0, 10);
  cam.updateMatrixWorld(true);
  cam.matrixWorldInverse.copy(cam.matrixWorld).invert();
  cam.updateProjectionMatrix();
  return cam;
}

afterEach(() => clearRenderCamera());

describe("renderCameraBridge", () => {
  it("falls back to the live camera when nothing was published", () => {
    const live = makeCamera(0);
    expect(getPickCamera(live)).toBe(live);
  });

  it("returns the presented frame's projection, immune to live moves", () => {
    const cam = makeCamera(0);
    publishRenderCamera(cam);

    // The live camera moves on (view restore / orbit during a compile)...
    cam.position.set(5, 0, 10);
    cam.updateMatrixWorld(true);
    cam.matrixWorldInverse.copy(cam.matrixWorld).invert();

    // ...but picks project a world point exactly as the presented frame did.
    const snap = getPickCamera(cam);
    expect(snap).not.toBe(cam);
    const atPresented = new THREE.Vector3(0, 0, 0).project(snap);
    expect(atPresented.x).toBeCloseTo(0, 10); // centered, as when published
    const atLive = new THREE.Vector3(0, 0, 0).project(cam);
    expect(atLive.x).not.toBeCloseTo(0, 3); // live view has moved away

    // Snapshot position (occlusion probes) is the presented one too.
    expect(snap.position.x).toBeCloseTo(0, 10);
  });

  it("a new publish updates the snapshot in place", () => {
    const cam = makeCamera(0);
    publishRenderCamera(cam);
    const first = getPickCamera(cam);

    cam.position.set(3, 0, 10);
    cam.updateMatrixWorld(true);
    cam.matrixWorldInverse.copy(cam.matrixWorld).invert();
    publishRenderCamera(cam);

    const second = getPickCamera(cam);
    expect(second).toBe(first); // reused instance, no per-frame allocation
    expect(second.position.x).toBeCloseTo(3, 10);
  });

  it("clearRenderCamera restores the live-camera fallback", () => {
    const cam = makeCamera(0);
    publishRenderCamera(cam);
    clearRenderCamera();
    expect(getPickCamera(cam)).toBe(cam);
  });
});
