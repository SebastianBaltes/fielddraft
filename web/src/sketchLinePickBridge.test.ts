import { describe, it, expect, beforeEach } from "vitest";
import * as THREE from "three";
import {
  publishSketchLinePickables,
  clearSketchLinePickables,
  pickSketchLineAtRay,
  getHoveredSketchLine,
  setHoveredSketchLine,
  subscribeHoveredSketchLine,
} from "./sketchLinePickBridge";

// ticket fs23: the Revolve axis picker rays a committed sketch line. The pick
// tolerance must be SCREEN-constant (zoom-aware) so a line stays pickable on a
// large model — the old fixed 0.5 mm world radius was sub-pixel at metre scale.
describe("pickSketchLineAtRay zoom-aware tolerance (fs23)", () => {
  beforeEach(() => clearSketchLinePickables());

  // A line along world Y at x=0, z=0, far from a camera looking down -Z from
  // a large distance — so 1 px subtends several mm.
  const setupFarLine = () => {
    publishSketchLinePickables([
      { sketchId: 7, lines: [{ lineId: 3, a: [0, -50, 0], b: [0, 50, 0] }] },
    ]);
    const cam = new THREE.PerspectiveCamera(50, 1, 1, 4000);
    cam.position.set(0, 0, 600);
    cam.lookAt(0, 0, 0);
    cam.updateMatrixWorld(true);
    return cam;
  };

  it("hits a line a few px off the ray with the camera tolerance, misses with the fixed fallback", () => {
    const cam = setupFarLine();
    // Aim 3 mm to the side of the line's midpoint — well beyond 0.5 mm but a
    // small fraction of the screen at this distance.
    const target = new THREE.Vector3(3, 0, 0);
    const ray = new THREE.Ray(
      cam.position.clone(),
      target.clone().sub(cam.position).normalize(),
    );
    expect(pickSketchLineAtRay(ray, cam, 900)).toEqual({
      sketchId: 7,
      lineId: 3,
    });
    // Without a camera the tolerance is the fixed 0.5 mm fallback — 3 mm misses.
    expect(pickSketchLineAtRay(ray)).toBeNull();
  });

  it("still misses a line far outside the screen tolerance", () => {
    const cam = setupFarLine();
    const target = new THREE.Vector3(60, 0, 0); // way off the line
    const ray = new THREE.Ray(
      cam.position.clone(),
      target.clone().sub(cam.position).normalize(),
    );
    expect(pickSketchLineAtRay(ray, cam, 900)).toBeNull();
  });
});

describe("hovered-sketch-line bridge (fs23)", () => {
  beforeEach(() => setHoveredSketchLine(null));

  it("stores and clears the hovered line", () => {
    expect(getHoveredSketchLine()).toBeNull();
    setHoveredSketchLine({ sketchId: 1, lineId: 4 });
    expect(getHoveredSketchLine()).toEqual({ sketchId: 1, lineId: 4 });
    setHoveredSketchLine(null);
    expect(getHoveredSketchLine()).toBeNull();
  });

  it("notifies subscribers only on an actual change", () => {
    let n = 0;
    const unsub = subscribeHoveredSketchLine(() => n++);
    setHoveredSketchLine({ sketchId: 1, lineId: 4 });
    expect(n).toBe(1);
    // Same value again → no notify (pointer-move must not thrash).
    setHoveredSketchLine({ sketchId: 1, lineId: 4 });
    expect(n).toBe(1);
    setHoveredSketchLine({ sketchId: 1, lineId: 5 });
    expect(n).toBe(2);
    setHoveredSketchLine(null);
    expect(n).toBe(3);
    unsub();
    setHoveredSketchLine({ sketchId: 2, lineId: 1 });
    expect(n).toBe(3);
  });
});
