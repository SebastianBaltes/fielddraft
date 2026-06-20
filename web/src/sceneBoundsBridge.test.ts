import { describe, it, expect, beforeEach } from "vitest";
import * as THREE from "three";
import {
  publishSceneBounds,
  getSceneBounds,
  GRID_BOUNDS,
} from "./sceneBoundsBridge";
import { adaptClipPlanes } from "./viewportCamera";

describe("adaptClipPlanes (ticket lbzq)", () => {
  beforeEach(() => publishSceneBounds(null));

  it("brackets the always-present world grid even with no bodies (xv9c)", () => {
    // The world grid is drawn even in an empty scene, so its foreground edge
    // must never be clipped — adaptClipPlanes brackets it instead of leaving
    // the camera's fixed defaults.
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    cam.position.set(0, 0, 800);
    expect(adaptClipPlanes(cam)).toBe(true);
    expect(cam.near).toBeGreaterThan(0);
    // Far must cover the grid's far corner from the camera.
    expect(cam.far).toBeGreaterThanOrEqual(800 + GRID_BOUNDS.radius);
    // Near must reach in for the grid's near corner (closer than the model).
    expect(cam.near).toBeLessThan(800 - GRID_BOUNDS.radius + 1);
  });

  it("brackets the scene so geometry never clips, even zoomed far out", () => {
    publishSceneBounds({ center: [0, 0, 0], radius: 100 });
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);

    // Zoomed out far beyond the old fixed far=4000: the farthest scene point is
    // 8000 + 100 from the camera; far must still cover it.
    cam.position.set(0, 0, 8000);
    adaptClipPlanes(cam);
    expect(cam.far).toBeGreaterThanOrEqual(8100);
    // Near must stay in front of the scene and never go non-positive.
    expect(cam.near).toBeGreaterThan(0);
    expect(cam.near).toBeLessThan(cam.far);

    // Zoomed in close: near clamps to a positive floor, far still covers the
    // far side of the scene.
    cam.position.set(0, 0, 150);
    adaptClipPlanes(cam);
    expect(cam.near).toBeGreaterThan(0);
    expect(cam.far).toBeGreaterThanOrEqual(250);
  });

  it("publishes and reads back bounds", () => {
    expect(getSceneBounds()).toBeNull();
    publishSceneBounds({ center: [1, 2, 3], radius: 5 });
    expect(getSceneBounds()).toEqual({ center: [1, 2, 3], radius: 5 });
  });
});
