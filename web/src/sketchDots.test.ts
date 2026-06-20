import { describe, it, expect } from "vitest";
import { makeSketchDots, SketchDots } from "./sketchDots";

describe("makeSketchDots", () => {
  it("returns null for empty input", () => {
    expect(makeSketchDots([], 0xffffff, 7, 998)).toBeNull();
    expect(makeSketchDots(null, 0xffffff, 7, 998)).toBeNull();
  });
  it("builds an InstancedMesh sized to the point count", () => {
    const d = makeSketchDots([0, 0, 0, 1, 0, 0], 0x123456, 7, 998)!;
    expect(d).toBeInstanceOf(SketchDots);
    expect(d.mesh.count).toBe(2);
    expect(d.mesh.renderOrder).toBe(998);
  });
});

describe("SketchDots.tryUpdate (ticket vwzj)", () => {
  it("updates in place when the point count matches", () => {
    const d = makeSketchDots([0, 0, 0, 1, 0, 0], 0x111111, 7, 998)!;
    const meshBefore = d.mesh;
    const ok = d.tryUpdate([5, 5, 5, 9, 9, 9], 0x222222, 9);
    expect(ok).toBe(true);
    // Same mesh object reused — the stable reference is what stops the flicker.
    expect(d.mesh).toBe(meshBefore);
  });

  it("refuses (returns false) when the count differs", () => {
    const d = makeSketchDots([0, 0, 0, 1, 0, 0], 0x111111, 7, 998)!;
    expect(d.tryUpdate([0, 0, 0], 0x111111, 7)).toBe(false); // 1 point vs 2
    expect(d.tryUpdate([0, 0, 0, 1, 0, 0, 2, 0, 0], 0x111111, 7)).toBe(false); // 3 vs 2
  });
});
