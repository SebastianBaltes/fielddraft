import { describe, expect, it } from "vitest";
import * as THREE from "three";
import {
  getCameraPose,
  saveCameraPose,
  parseCameraPose,
  requestCameraRestore,
  subscribeCameraRestore,
} from "./cameraPoseBridge";

describe("cameraPoseBridge", () => {
  it("round-trips the camera pose + orbit target", () => {
    const cam = new THREE.PerspectiveCamera(45, 1, 1, 4000);
    cam.position.set(12, 34, 56);
    cam.up.set(0, 0, 1);
    cam.lookAt(1, 2, 3);
    cam.updateMatrixWorld();
    const target = new THREE.Vector3(1, 2, 3);

    saveCameraPose(cam, target);
    const p = getCameraPose();
    expect(p).not.toBeNull();
    expect(p!.position).toEqual([12, 34, 56]);
    expect(p!.up).toEqual([0, 0, 1]);
    expect(p!.target).toEqual([1, 2, 3]);
    expect(p!.quaternion).toHaveLength(4);
    // A perspective camera reports zoom 1.
    expect(p!.zoom).toBe(1);
  });

  it("captures orthographic zoom", () => {
    const ortho = new THREE.OrthographicCamera(-1, 1, 1, -1, 1, 4000);
    ortho.zoom = 7.5;
    saveCameraPose(ortho, new THREE.Vector3());
    expect(getCameraPose()!.zoom).toBe(7.5);
  });

  it("parseCameraPose accepts a well-formed pose and rejects junk (pxff)", () => {
    const good = {
      position: [1, 2, 3],
      quaternion: [0, 0, 0, 1],
      up: [0, 1, 0],
      zoom: 1,
      target: [4, 5, 6],
    };
    expect(parseCameraPose(good)).toEqual(good);
    for (const bad of [
      null,
      {},
      { ...good, position: [1, 2] }, // wrong length
      { ...good, quaternion: [0, 0, 0] }, // needs 4
      { ...good, zoom: "x" },
      { ...good, target: [1, 2, NaN] },
    ]) {
      expect(parseCameraPose(bad)).toBeNull();
    }
  });

  it("requestCameraRestore seeds the pose and notifies listeners (pxff)", () => {
    let fired = 0;
    const unsub = subscribeCameraRestore(() => fired++);
    const pose = {
      position: [9, 9, 9] as [number, number, number],
      quaternion: [0, 0, 0, 1] as [number, number, number, number],
      up: [0, 1, 0] as [number, number, number],
      zoom: 2,
      target: [1, 1, 1] as [number, number, number],
    };
    requestCameraRestore(pose);
    expect(fired).toBe(1);
    expect(getCameraPose()).toEqual(pose);
    // Null request is a no-op (legacy project without a view).
    requestCameraRestore(null);
    expect(fired).toBe(1);
    unsub();
  });
});
