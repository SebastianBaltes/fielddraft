import { describe, expect, it } from "vitest";
import {
  diffOccupancy,
  diffSnapshots,
  sameGrid,
  type VoxelSnapshot,
} from "./voxelSnapshot";

function snap(
  occ: number[],
  overrides: Partial<VoxelSnapshot> = {},
): VoxelSnapshot {
  const occupancy = Uint8Array.from(occ);
  return {
    dims: [occ.length, 1, 1],
    origin: [0, 0, 0],
    spacing: 1,
    occupancy,
    count: occupancy.reduce((n, v) => n + (v ? 1 : 0), 0),
    ...overrides,
  };
}

describe("diffOccupancy", () => {
  it("classifies added / removed / unchanged element-wise", () => {
    //          idx: 0  1  2  3  4
    const a = Uint8Array.from([1, 1, 0, 0, 1]);
    const b = Uint8Array.from([1, 0, 1, 0, 1]);
    // idx0: both → unchanged
    // idx1: only a → removed
    // idx2: only b → added
    // idx3: neither → nothing
    // idx4: both → unchanged
    const d = diffOccupancy(a, b);
    expect(d.added).toBe(1);
    expect(d.removed).toBe(1);
    expect(d.unchanged).toBe(2);
    expect(Array.from(d.addedMask)).toEqual([0, 0, 1, 0, 0]);
    expect(Array.from(d.removedMask)).toEqual([0, 1, 0, 0, 0]);
  });

  it("treats any non-zero byte as occupied", () => {
    const a = Uint8Array.from([2, 0, 0]);
    const b = Uint8Array.from([0, 7, 0]);
    const d = diffOccupancy(a, b);
    expect(d.removed).toBe(1); // idx0 only in a
    expect(d.added).toBe(1); // idx1 only in b
    expect(d.unchanged).toBe(0);
  });

  it("counts nothing for identical arrays", () => {
    const a = Uint8Array.from([1, 0, 1, 1]);
    const d = diffOccupancy(a, a.slice());
    expect(d.added).toBe(0);
    expect(d.removed).toBe(0);
    expect(d.unchanged).toBe(3);
  });

  it("throws on length mismatch", () => {
    expect(() =>
      diffOccupancy(Uint8Array.from([1]), Uint8Array.from([1, 0])),
    ).toThrow(/length mismatch/);
  });
});

describe("sameGrid", () => {
  it("is true for matching dims / spacing / origin", () => {
    expect(sameGrid(snap([1, 0]), snap([0, 1]))).toBe(true);
  });
  it("is false when dims differ", () => {
    expect(sameGrid(snap([1, 0]), snap([1, 0, 0]))).toBe(false);
  });
  it("is false when spacing differs", () => {
    expect(sameGrid(snap([1, 0]), snap([1, 0], { spacing: 2 }))).toBe(false);
  });
  it("is false when origin differs", () => {
    expect(sameGrid(snap([1, 0]), snap([1, 0], { origin: [5, 0, 0] }))).toBe(
      false,
    );
  });
});

describe("diffSnapshots", () => {
  it("rejects incompatible grids with an error", () => {
    const res = diffSnapshots(snap([1, 0]), snap([1, 0], { spacing: 2 }));
    expect("error" in res).toBe(true);
  });

  it("computes counts and volume delta on a shared grid", () => {
    // spacing 2 → voxelVolume 8 mm³. a occupies {0,1}; b occupies {1,2}.
    const a = snap([1, 1, 0], { spacing: 2, dims: [3, 1, 1] });
    const b = snap([0, 1, 1], { spacing: 2, dims: [3, 1, 1] });
    const res = diffSnapshots(a, b);
    expect("error" in res).toBe(false);
    if ("error" in res) return;
    expect(res.added).toBe(1); // idx2
    expect(res.removed).toBe(1); // idx0
    expect(res.unchanged).toBe(1); // idx1
    expect(res.voxelVolume).toBe(8);
    expect(res.volumeDelta).toBe(0); // (1 − 1) · 8
  });

  it("reports a positive volume delta when geometry is added", () => {
    const a = snap([1, 0, 0], { spacing: 1, dims: [3, 1, 1] });
    const b = snap([1, 1, 1], { spacing: 1, dims: [3, 1, 1] });
    const res = diffSnapshots(a, b);
    if ("error" in res) throw new Error(res.error);
    expect(res.added).toBe(2);
    expect(res.removed).toBe(0);
    expect(res.volumeDelta).toBe(2);
  });
});
