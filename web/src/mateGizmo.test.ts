import { describe, it, expect } from "vitest";
import * as THREE from "three";
import {
  orthonormalMateFrame,
  buildMateGizmo,
  disposeMateGizmo,
  mateGizmoScreenScale,
  MATE_GIZMO_TARGET_PX,
} from "./mateGizmo";

describe("orthonormalMateFrame", () => {
  it("keeps Z as primary and yields a right-handed orthonormal basis", () => {
    const f = orthonormalMateFrame({
      origin: [1, 2, 3],
      z: [0, 0, 2], // non-unit
      x: [3, 0, 0], // non-unit, already ⟂ Z
    })!;
    expect(f).not.toBeNull();
    expect(f.origin.toArray()).toEqual([1, 2, 3]);
    // Unit length.
    expect(f.x.length()).toBeCloseTo(1);
    expect(f.y.length()).toBeCloseTo(1);
    expect(f.z.length()).toBeCloseTo(1);
    // Orthogonal.
    expect(f.x.dot(f.z)).toBeCloseTo(0);
    expect(f.y.dot(f.z)).toBeCloseTo(0);
    expect(f.x.dot(f.y)).toBeCloseTo(0);
    // Right-handed: X × Y == Z.
    const cross = new THREE.Vector3().crossVectors(f.x, f.y);
    expect(cross.distanceTo(f.z)).toBeCloseTo(0);
    // Z stayed the supplied direction (normalised).
    expect(f.z.toArray()).toEqual([0, 0, 1]);
  });

  it("re-projects an X that is not perpendicular to Z", () => {
    const f = orthonormalMateFrame({
      origin: [0, 0, 0],
      z: [0, 0, 1],
      x: [1, 0, 1], // 45° out of the plane
    })!;
    expect(f.x.dot(f.z)).toBeCloseTo(0);
    expect(f.x.toArray()[0]).toBeCloseTo(1);
    expect(f.x.toArray()[2]).toBeCloseTo(0);
  });

  it("falls back to a sane in-plane X when the supplied X is parallel to Z", () => {
    const f = orthonormalMateFrame({
      origin: [0, 0, 0],
      z: [0, 0, 1],
      x: [0, 0, 5], // parallel to Z → degenerate
    })!;
    expect(f).not.toBeNull();
    expect(f.x.dot(f.z)).toBeCloseTo(0);
    expect(f.x.length()).toBeCloseTo(1);
  });

  it("returns null for a degenerate (zero) Z", () => {
    expect(
      orthonormalMateFrame({ origin: [0, 0, 0], z: [0, 0, 0], x: [1, 0, 0] }),
    ).toBeNull();
  });
});

describe("buildMateGizmo", () => {
  it("places the group at the origin and orients it to the frame", () => {
    const frame = orthonormalMateFrame({
      origin: [5, 6, 7],
      z: [0, 1, 0],
      x: [1, 0, 0],
    })!;
    const g = buildMateGizmo(frame, { webgpu: false });
    expect(g.position.toArray()).toEqual([5, 6, 7]);
    // Local +Z maps to the frame's world Z (=world +Y here).
    const worldZ = new THREE.Vector3(0, 0, 1).applyQuaternion(g.quaternion);
    expect(worldZ.distanceTo(frame.z)).toBeCloseTo(0);
    // 3 axes + circle loop + sector = 5 children.
    expect(g.children.length).toBe(5);
    disposeMateGizmo(g);
    expect(g.children.length).toBe(0);
  });
});

describe("mateGizmoScreenScale", () => {
  const frame = orthonormalMateFrame({ origin: [0, 0, 0], z: [0, 0, 1], x: [1, 0, 0] })!;
  const H = 800;

  it("scales the built arm to MATE_GIZMO_TARGET_PX pixels (ortho, zoom-independent)", () => {
    const size = 14;
    const g = buildMateGizmo(frame, { size, webgpu: false });
    const cam = new THREE.OrthographicCamera(-10, 10, 10, -10, 0.1, 100);
    // Two zoom levels: the resulting on-screen arm length must be identical.
    const armPx = (zoom: number) => {
      cam.zoom = zoom;
      const s = mateGizmoScreenScale(g, cam, H);
      const wpp = (cam.top - cam.bottom) / cam.zoom / H;
      return (size * s) / wpp; // world arm → pixels
    };
    expect(armPx(1)).toBeCloseTo(MATE_GIZMO_TARGET_PX, 4);
    expect(armPx(4)).toBeCloseTo(MATE_GIZMO_TARGET_PX, 4);
    disposeMateGizmo(g);
  });

  it("is size-agnostic — different built sizes reach the same screen px", () => {
    const cam = new THREE.OrthographicCamera(-10, 10, 10, -10, 0.1, 100);
    const armPxFor = (size: number) => {
      const g = buildMateGizmo(frame, { size, webgpu: false });
      const s = mateGizmoScreenScale(g, cam, H);
      const wpp = (cam.top - cam.bottom) / cam.zoom / H;
      const px = (size * s) / wpp;
      disposeMateGizmo(g);
      return px;
    };
    expect(armPxFor(11)).toBeCloseTo(armPxFor(14), 4);
  });

  it("returns 1 for an untagged object (no crash)", () => {
    const cam = new THREE.OrthographicCamera(-10, 10, 10, -10, 0.1, 100);
    expect(mateGizmoScreenScale(new THREE.Object3D(), cam, H)).toBe(1);
  });
});
