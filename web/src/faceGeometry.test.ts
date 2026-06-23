import { describe, it, expect } from "vitest";
import { resolveFaceWorldGeometry, rotateByPlacement } from "./faceGeometry";
import type { BRepFace, Kernel, Placement } from "./kernel";

/** Minimal fake kernel exposing just the three methods the resolver reads. */
function fakeKernel(bodies: { faces: BRepFace[]; placement: Placement }[]) {
  return {
    bodyCount: () => bodies.length,
    bodyBrepFaces: (i: number) => bodies[i].faces,
    bodyPlacement: (i: number) => bodies[i].placement,
  } as Pick<Kernel, "bodyCount" | "bodyBrepFaces" | "bodyPlacement">;
}

function planeFace(idHash: string, centroid: [number, number, number], normal: [number, number, number]): BRepFace {
  return {
    faceIndex: 0,
    idHash,
    kind: "face",
    branchId: 0,
    featureId: 0,
    surfaceKind: "plane",
    centroidLocal: centroid,
    normalLocal: normal,
  };
}

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

describe("rotateByPlacement", () => {
  it("rotates a +X local dir by a +90° turn about Z into +Y world", () => {
    const half = Math.PI / 4;
    const pl: Placement = {
      translate: [10, 20, 30],
      rotation: [0, 0, Math.sin(half), Math.cos(half)],
      scale: [1, 1, 1],
    };
    const out = rotateByPlacement([1, 0, 0], pl);
    expect(out[0]).toBeCloseTo(0, 6);
    expect(out[1]).toBeCloseTo(1, 6);
    expect(out[2]).toBeCloseTo(0, 6);
  });

  it("ignores translation (it's a pure direction rotation)", () => {
    const pl: Placement = { translate: [99, 99, 99], rotation: [0, 0, 0, 1], scale: [1, 1, 1] };
    expect(rotateByPlacement([0, 0, 1], pl)).toEqual([0, 0, 1]);
  });
});

describe("resolveFaceWorldGeometry (ticket z0vk)", () => {
  it("returns null when no body carries a face with that idHash", () => {
    const k = fakeKernel([{ faces: [planeFace("1", [0, 0, 0], [0, 0, 1])], placement: IDENTITY }]);
    expect(resolveFaceWorldGeometry(k, "999")).toBeNull();
  });

  it("returns null for an empty scene", () => {
    expect(resolveFaceWorldGeometry(fakeKernel([]), "1")).toBeNull();
  });

  it("folds the centroid through the placement translation (world space)", () => {
    const k = fakeKernel([
      { faces: [planeFace("7", [1, 2, 3], [0, 0, 1])], placement: { ...IDENTITY, translate: [10, 0, 0] } },
    ]);
    const g = resolveFaceWorldGeometry(k, "7")!;
    expect(g.surfaceKind).toBe("plane");
    expect(g.centroid).toEqual([11, 2, 3]);
    expect(g.normal).toEqual([0, 0, 1]);
  });

  it("rotates the normal by the placement (centroid translated, normal direction-only)", () => {
    const half = Math.PI / 4; // 90° about Z
    const k = fakeKernel([
      {
        faces: [planeFace("7", [1, 0, 0], [1, 0, 0])],
        placement: { translate: [0, 0, 5], rotation: [0, 0, Math.sin(half), Math.cos(half)], scale: [1, 1, 1] },
      },
    ]);
    const g = resolveFaceWorldGeometry(k, "7")!;
    // local +X centroid → +Y, then + the placement translate on Z.
    expect(g.centroid[0]).toBeCloseTo(0, 6);
    expect(g.centroid[1]).toBeCloseTo(1, 6);
    expect(g.centroid[2]).toBeCloseTo(5, 6);
    // local +X normal → +Y world, translation does NOT shift a direction.
    expect(g.normal[0]).toBeCloseTo(0, 6);
    expect(g.normal[1]).toBeCloseTo(1, 6);
    expect(g.normal[2]).toBeCloseTo(0, 6);
  });

  it("reports a curved face's surfaceKind so callers can gate planarity", () => {
    const cyl: BRepFace = { ...planeFace("3", [0, 0, 0], [0, 1, 0]), surfaceKind: "cylinder" };
    const k = fakeKernel([{ faces: [cyl], placement: IDENTITY }]);
    expect(resolveFaceWorldGeometry(k, "3")!.surfaceKind).toBe("cylinder");
  });

  it("scans multiple bodies for the match", () => {
    const k = fakeKernel([
      { faces: [planeFace("1", [0, 0, 0], [0, 0, 1])], placement: IDENTITY },
      { faces: [planeFace("2", [5, 5, 5], [0, 1, 0])], placement: { ...IDENTITY, translate: [0, 100, 0] } },
    ]);
    const g = resolveFaceWorldGeometry(k, "2")!;
    expect(g.centroid).toEqual([5, 105, 5]);
    expect(g.normal).toEqual([0, 1, 0]);
  });
});
