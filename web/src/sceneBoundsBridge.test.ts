import { describe, it, expect, beforeEach } from "vitest";
import * as THREE from "three";
import { publishSceneBounds, getSceneBounds } from "./sceneBoundsBridge";
import { adaptClipPlanes } from "./viewportCamera";

describe("adaptClipPlanes (ticket lbzq)", () => {
  beforeEach(() => publishSceneBounds(null));

  it("leaves the camera untouched for an empty scene", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    expect(adaptClipPlanes(cam)).toBe(false);
    expect(cam.near).toBe(1);
    expect(cam.far).toBe(4000);
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
