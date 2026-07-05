import { describe, expect, it } from "vitest";
import * as THREE from "three";
import { getCameraPose, saveCameraPose } from "./cameraPoseBridge";

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
});
