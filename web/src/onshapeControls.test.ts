import { describe, it, expect } from "vitest";
import * as THREE from "three";
import {
  sphereFitsViewport,
  rotationDelta,
  cursorZoomFocus,
  OnshapeControls,
} from "./onshapeControls";
import { publishSceneBounds } from "./sceneBoundsBridge";

function cameraLookingAt(
  pos: [number, number, number],
  at: [number, number, number],
): THREE.PerspectiveCamera {
  const cam = new THREE.PerspectiveCamera(45, 1, 0.1, 10000);
  cam.position.set(...pos);
  cam.lookAt(...at);
  cam.updateMatrixWorld(true);
  cam.updateProjectionMatrix();
  return cam;
}

/** Framing distance for a centred sphere at this camera's narrower FoV. */
function fitDistance(cam: THREE.PerspectiveCamera, r: number): number {
  const vFov = (cam.fov * Math.PI) / 180;
  const hFov = 2 * Math.atan(Math.tan(vFov / 2) * (cam.aspect || 1));
  return r / Math.sin(Math.min(vFov, hFov) / 2);
}

describe("sphereFitsViewport", () => {
  it("fits when the camera is far enough and the sphere is centred", () => {
    const r = 10;
    const cam = cameraLookingAt([0, 0, fitDistance(new THREE.PerspectiveCamera(45, 1), r) * 1.5], [0, 0, 0]);
    expect(sphereFitsViewport(cam, new THREE.Vector3(0, 0, 0), r)).toBe(true);
  });

  it("does not fit when zoomed in closer than the framing distance", () => {
    const r = 10;
    const fd = fitDistance(new THREE.PerspectiveCamera(45, 1), r);
    const cam = cameraLookingAt([0, 0, fd * 0.5], [0, 0, 0]);
    expect(sphereFitsViewport(cam, new THREE.Vector3(0, 0, 0), r)).toBe(false);
  });

  it("does not fit when the sphere is panned off to the side (off-frame)", () => {
    const r = 10;
    const fd = fitDistance(new THREE.PerspectiveCamera(45, 1), r);
    // Camera frames the origin from far away, but the sphere centre sits far
    // off-axis so it projects outside the frame — the sticky pivot must hold.
    const cam = cameraLookingAt([0, 0, fd * 1.5], [0, 0, 0]);
    expect(sphereFitsViewport(cam, new THREE.Vector3(10000, 0, 0), r)).toBe(false);
  });

  it("never fits a degenerate (zero-radius) sphere", () => {
    const cam = cameraLookingAt([0, 0, 100], [0, 0, 0]);
    expect(sphereFitsViewport(cam, new THREE.Vector3(0, 0, 0), 0)).toBe(false);
  });
});

describe("rotationDelta", () => {
  const right = new THREE.Vector3(1, 0, 0);
  const up = new THREE.Vector3(0, 1, 0);

  it("free yaw rotates about the supplied (camera) up axis", () => {
    const q = rotationDelta("free", Math.PI, 0, right, up, new THREE.Quaternion());
    const fwd = new THREE.Vector3(0, 0, -1).applyQuaternion(q);
    expect(fwd.x).toBeCloseTo(0, 5);
    expect(fwd.z).toBeCloseTo(1, 5); // flipped 180° about Y
  });

  it("upright yaw rotates about WORLD up, ignoring a rolled camera up", () => {
    // Camera up tilted onto Z; upright must still yaw about world +Y.
    const rolledUp = new THREE.Vector3(0, 0, 1);
    const q = rotationDelta("upright", Math.PI / 2, 0, right, rolledUp, new THREE.Quaternion());
    const v = new THREE.Vector3(1, 0, 0).applyQuaternion(q);
    // About world Y by -90°: (1,0,0) → (0,0,1). (About the rolled Z it would
    // have gone to (0,-1,0).)
    expect(v.x).toBeCloseTo(0, 5);
    expect(v.y).toBeCloseTo(0, 5);
    expect(v.z).toBeCloseTo(1, 5);
  });

  it("pitch is unbounded — a half-turn carries the offset over the pole", () => {
    // What OrbitControls cannot do: pitch past the pole. A π pitch flips the
    // view offset straight through the top.
    const q = rotationDelta("free", 0, Math.PI, right, up, new THREE.Quaternion());
    const offset = new THREE.Vector3(0, 0, 1).applyQuaternion(q);
    expect(offset.z).toBeCloseTo(-1, 5);
  });

  it("returns a unit quaternion", () => {
    const q = rotationDelta("free", 0.3, -0.2, right, up, new THREE.Quaternion());
    expect(q.length()).toBeCloseTo(1, 6);
  });
});

describe("cursorZoomFocus", () => {
  const camPos = new THREE.Vector3(0, 0, 10);
  const target = new THREE.Vector3(0, 0, 0);

  it("a centred ray (toward target) focuses on the target itself", () => {
    const rayDir = new THREE.Vector3(0, 0, -1);
    const f = cursorZoomFocus(camPos, target, rayDir, new THREE.Vector3());
    expect(f).not.toBeNull();
    expect(f!.distanceTo(target)).toBeCloseTo(0, 5);
  });

  it("an off-centre ray focuses on the target plane under the cursor", () => {
    // 45° ray → hits the z=0 plane (through target ⊥ view) at x = dist·tan45 = 10.
    const rayDir = new THREE.Vector3(1, 0, -1).normalize();
    const f = cursorZoomFocus(camPos, target, rayDir, new THREE.Vector3());
    expect(f).not.toBeNull();
    expect(f!.x).toBeCloseTo(10, 4);
    expect(f!.z).toBeCloseTo(0, 4); // on the target plane
  });

  it("scaling camera+target about the focus keeps the focus pinned", () => {
    const rayDir = new THREE.Vector3(0.3, -0.2, -1).normalize();
    const f = cursorZoomFocus(camPos, target, rayDir, new THREE.Vector3())!;
    // The cursor ray from the camera must still pass through the focus after a
    // uniform scale of both camera and target about that focus.
    const factor = 0.5;
    const newCam = camPos.clone().sub(f).multiplyScalar(factor).add(f);
    const newTarget = target.clone().sub(f).multiplyScalar(factor).add(f);
    // View direction unchanged (parallel) and focus still on the ray cam→focus.
    const oldDir = target.clone().sub(camPos).normalize();
    const newDir = newTarget.clone().sub(newCam).normalize();
    expect(newDir.dot(oldDir)).toBeCloseTo(1, 6);
    const toFocus = f.clone().sub(newCam).normalize();
    expect(toFocus.dot(rayDir)).toBeCloseTo(1, 6);
  });

  it("returns null for a grazing ray and a degenerate camera", () => {
    expect(cursorZoomFocus(camPos, target, new THREE.Vector3(1, 0, 0), new THREE.Vector3())).toBeNull();
    expect(cursorZoomFocus(target, target, new THREE.Vector3(0, 0, -1), new THREE.Vector3())).toBeNull();
  });
});

describe("OnshapeControls touch gestures", () => {
  const touch = (id: number, x: number, y: number) =>
    ({
      pointerType: "touch",
      pointerId: id,
      clientX: x,
      clientY: y,
      button: 0,
      stopPropagation: () => {},
    }) as unknown as PointerEvent;

  // The private pointer handlers are the gesture entry points; call them
  // directly (jsdom PointerEvent is flaky) with the fields they read.
  const handlers = (c: OnshapeControls) =>
    c as unknown as {
      onPointerDown: (e: PointerEvent) => void;
      onPointerMove: (e: PointerEvent) => void;
      onPointerUp: (e: PointerEvent) => void;
    };

  // jsdom reports clientHeight 0 and an empty rect; give the element realistic
  // metrics so rotate (∝ dx/clientHeight) and cursor-zoom aren't degenerate.
  const makeEl = (): HTMLElement => {
    const el = document.createElement("div");
    Object.defineProperty(el, "clientHeight", { value: 600, configurable: true });
    el.getBoundingClientRect = () =>
      ({ left: 0, top: 0, right: 800, bottom: 600, width: 800, height: 600, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    return el;
  };

  it("one-finger drag past the tap threshold rotates the camera", () => {
    const cam = cameraLookingAt([0, 0, 100], [0, 0, 0]);
    const c = new OnshapeControls(cam, makeEl());
    const h = handlers(c);
    const before = cam.position.clone();
    h.onPointerDown(touch(1, 100, 100));
    h.onPointerMove(touch(1, 160, 100)); // 60px > threshold → rotate
    h.onPointerUp(touch(1, 160, 100));
    expect(cam.position.distanceTo(before)).toBeGreaterThan(0.1);
    c.dispose();
  });

  it("a one-finger tap (below threshold) leaves the camera untouched", () => {
    const cam = cameraLookingAt([0, 0, 100], [0, 0, 0]);
    const c = new OnshapeControls(cam, makeEl());
    const h = handlers(c);
    const before = cam.position.clone();
    h.onPointerDown(touch(1, 100, 100));
    h.onPointerMove(touch(1, 102, 101)); // < threshold → still a tap
    h.onPointerUp(touch(1, 102, 101));
    expect(cam.position.distanceTo(before)).toBe(0);
    c.dispose();
  });

  it("rotate re-centres on the VISIBLE pivot, not the conservative centre (f88u)", () => {
    // The conservative `center` still sits on the phantom original primitive
    // after a boolean carve; the pivot must follow the tight visible centre.
    publishSceneBounds({ center: [0, 0, 0], radius: 5, pivot: [5, 6, 7] });
    const cam = cameraLookingAt([0, 0, 100], [0, 0, 0]); // far → part fits
    const c = new OnshapeControls(cam, makeEl());
    const h = handlers(c);
    h.onPointerDown(touch(1, 100, 100));
    h.onPointerMove(touch(1, 160, 100)); // past threshold → rotate → recentre
    expect(c.target.x).toBeCloseTo(5, 5);
    expect(c.target.y).toBeCloseTo(6, 5);
    expect(c.target.z).toBeCloseTo(7, 5);
    c.dispose();
    publishSceneBounds(null);
  });

  it("rotate keeps the pivot STICKY when the part no longer fits the viewport (f88u)", () => {
    // A huge part seen from close up cannot frame → the pivot must stay put
    // (at its last fitting position, here the origin) instead of lurching onto
    // an off-screen centre.
    publishSceneBounds({ center: [0, 0, 0], radius: 500, pivot: [9, 9, 9] });
    const cam = cameraLookingAt([0, 0, 50], [0, 0, 0]);
    const c = new OnshapeControls(cam, makeEl());
    const h = handlers(c);
    h.onPointerDown(touch(1, 100, 100));
    h.onPointerMove(touch(1, 160, 100)); // rotate, but the part does not fit
    expect(c.target.x).toBeCloseTo(0, 5); // sticky → unchanged
    expect(c.target.y).toBeCloseTo(0, 5);
    expect(c.target.z).toBeCloseTo(0, 5);
    c.dispose();
    publishSceneBounds(null);
  });

  it("two-finger spread zooms in (perspective camera moves closer)", () => {
    const cam = cameraLookingAt([0, 0, 100], [0, 0, 0]);
    const c = new OnshapeControls(cam, makeEl());
    const h = handlers(c);
    const distBefore = cam.position.distanceTo(c.target);
    h.onPointerDown(touch(1, 100, 100));
    h.onPointerDown(touch(2, 200, 100)); // pinch begins, dist 100
    h.onPointerMove(touch(2, 300, 100)); // dist 200 → spread → zoom in
    expect(cam.position.distanceTo(c.target)).toBeLessThan(distBefore);
    c.dispose();
  });
});
