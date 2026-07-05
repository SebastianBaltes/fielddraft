import { describe, it, expect } from "vitest";
import { selectedPartAabbs, aabbEdgeSegments, type AabbKernel } from "./partAabb";

/** Fake kernel: feature id → body AABB (or [] for "no body"). */
function kernel(map: Record<number, number[]>): AabbKernel {
  return { bodyAabbForFeature: (id) => map[id] ?? [] };
}

describe("selectedPartAabbs", () => {
  it("maps a selected feature to its part's box with the size derived", () => {
    const k = kernel({ 1: [-10, -2, -3, 0, 4, 6] });
    const boxes = selectedPartAabbs(k, [1]);
    expect(boxes).toHaveLength(1);
    expect(boxes[0].min).toEqual([-10, -2, -3]);
    expect(boxes[0].max).toEqual([0, 4, 6]);
    expect(boxes[0].size).toEqual([10, 6, 9]);
  });

  it("de-duplicates features that resolve to the same body", () => {
    // Two features, one shared body box → a single drawn box.
    const box = [0, 0, 0, 5, 5, 5];
    const k = kernel({ 1: box, 2: box });
    expect(selectedPartAabbs(k, [1, 2])).toHaveLength(1);
  });

  it("keeps distinct parts as separate boxes", () => {
    const k = kernel({ 1: [0, 0, 0, 1, 1, 1], 2: [9, 9, 9, 10, 10, 10] });
    expect(selectedPartAabbs(k, [1, 2])).toHaveLength(2);
  });

  it("skips features that belong to no body (empty bound)", () => {
    const k = kernel({ 1: [] });
    expect(selectedPartAabbs(k, [1, 99])).toHaveLength(0);
  });
});

describe("aabbEdgeSegments", () => {
  it("emits 12 edges (24 vertices) for a box", () => {
    const seg = aabbEdgeSegments([0, 0, 0], [1, 1, 1]);
    expect(seg).toHaveLength(12 * 6); // 12 segments × 2 points × xyz
  });

  it("every emitted coordinate is a corner coordinate of the box", () => {
    const seg = aabbEdgeSegments([-1, -2, -3], [4, 5, 6]);
    for (let i = 0; i < seg.length; i += 3) {
      expect([-1, 4]).toContain(seg[i]);
      expect([-2, 5]).toContain(seg[i + 1]);
      expect([-3, 6]).toContain(seg[i + 2]);
    }
  });

  it("each segment is an axis-aligned edge (differs on exactly one axis)", () => {
    const seg = aabbEdgeSegments([0, 0, 0], [2, 3, 4]);
    for (let s = 0; s < seg.length; s += 6) {
      const diff = [0, 1, 2].filter((a) => seg[s + a] !== seg[s + 3 + a]);
      expect(diff).toHaveLength(1);
    }
  });
});
