import { describe, expect, it } from "vitest";
import {
  axisDepthParam,
  faceQueryHashes,
  lengthParamToMm,
  lengthParamWithMm,
  pickTipFace,
  previewBandRanges,
  prismBandTriangles,
  resolveExtrudeAnchor,
  resolvePendingExtrudeAnchor,
  resolvePendingSketchExtrudeAnchor,
  type ExtrudeAnchor,
  type ExtrudeAnchorKernel,
  type TipFaceCandidate,
  type Vec3,
} from "./extrudeGizmo";
import type { BRepEdge, BRepFace, Placement } from "./kernel";

const IDENTITY: Placement = {
  translate: [0, 0, 0],
  rotation: [0, 0, 0, 1],
  scale: [1, 1, 1],
};

const cand = (normal: Vec3, centroid: Vec3, index: number): TipFaceCandidate => ({
  normal,
  centroid,
  index,
});

describe("pickTipFace", () => {
  it("picks the cap among prism walls (max perpendicular siblings)", () => {
    // Cap at +X plus 4 side walls of a prism grown in +X.
    const picked = pickTipFace([
      cand([0, 1, 0], [55, 50, 0], 0),
      cand([0, -1, 0], [55, -50, 0], 1),
      cand([1, 0, 0], [60, 0, 0], 2), // cap
      cand([0, 0, 1], [55, 0, 50], 3),
      cand([0, 0, -1], [55, 0, -50], 4),
    ]);
    expect(picked?.index).toBe(2);
  });

  it("breaks cap/base ties toward the outward-most face", () => {
    // An unmerged "New" prism: cap at +X 60, base at +X 40 (normal −X).
    const picked = pickTipFace([
      cand([1, 0, 0], [60, 0, 0], 0),
      cand([-1, 0, 0], [40, 0, 0], 1),
      cand([0, 1, 0], [50, 10, 0], 2),
      cand([0, -1, 0], [50, -10, 0], 3),
    ]);
    expect(picked?.index).toBe(0);
  });

  it("handles empty and single-candidate lists", () => {
    expect(pickTipFace([])).toBeNull();
    expect(pickTipFace([cand([0, 0, 1], [0, 0, 5], 7)])?.index).toBe(7);
  });

  it("picks the depth-matched cap of an off-origin closed prism (zzl1)", () => {
    // A standalone "Add" prism that kept BOTH caps: 6 axis-aligned faces,
    // so the perpendicular-sibling count is a 6-way tie. The prism sits
    // high in +Y (centroids offset like the ticket's stacked box), so the
    // +Y wall is the world-outermost face and the old tie-break wrongly
    // chose it. Extrude axis is +X, depth 16; the tip cap is at +X.
    const off = 76; // world-space offset along Y, like the ticket
    const tipCap = cand([1, 0, 0], [66, off, 0], 0); // +X cap, sep 16 to base
    const baseCap = cand([-1, 0, 0], [50, off, 0], 1); // -X cap
    const wallUp = cand([0, 1, 0], [58, off + 27, 0], 2); // +Y wall, outermost
    const wallDown = cand([0, -1, 0], [57, off - 26, 0], 3);
    const wallFront = cand([0, 0, 1], [57, off, 50], 4);
    const wallBack = cand([0, 0, -1], [58, off, -50], 5);
    const all = [tipCap, baseCap, wallUp, wallDown, wallFront, wallBack];
    // Without the depth signal the old heuristic picks the +Y wall.
    expect(pickTipFace(all)?.index).toBe(2);
    // With it, the +X cap (grown side of the depth-16 pair) wins.
    expect(pickTipFace(all, 16)?.index).toBe(0);
    expect(baseCap.index).toBe(1); // (silence unused-var lint paranoia)
  });
});

describe("axisDepthParam", () => {
  it("recovers the param of a point on the axis", () => {
    // Axis +Z from origin; ray from the side aiming at (0,0,30).
    const t = axisDepthParam(
      [0, 0, 0],
      [0, 0, 1],
      [100, 0, 30],
      [-1, 0, 0],
    );
    expect(t).toBeCloseTo(30, 6);
  });

  it("is null for a ray parallel to the axis", () => {
    expect(
      axisDepthParam([0, 0, 0], [0, 0, 1], [5, 0, 0], [0, 0, 1]),
    ).toBeNull();
  });
});

describe("length params", () => {
  it("reads wire lengths in any unit as mm", () => {
    expect(lengthParamToMm({ value: 10, unit: "mm" })).toBe(10);
    expect(lengthParamToMm({ value: 2.5, unit: "cm" })).toBe(25);
    expect(lengthParamToMm({ value: 1, unit: "in" })).toBeCloseTo(25.4);
    expect(lengthParamToMm(12)).toBe(12);
    expect(lengthParamToMm("7.5")).toBe(7.5);
    expect(lengthParamToMm({ value: "x", unit: "mm" })).toBeNull();
  });

  it("re-encodes mm preserving the original unit", () => {
    expect(lengthParamWithMm({ value: 1, unit: "cm" }, 25)).toEqual({
      value: 2.5,
      unit: "cm",
    });
    expect(lengthParamWithMm(undefined, 25)).toEqual({ value: 25, unit: "mm" });
  });
});

describe("resolveExtrudeAnchor", () => {
  const face = (
    idHash: string,
    featureId: number,
    branchId: number,
    centroid: Vec3,
    normal: Vec3,
    surfaceKind: BRepFace["surfaceKind"] = "plane",
  ): BRepFace => ({
    faceIndex: 0,
    idHash,
    kind: "face",
    branchId,
    featureId,
    surfaceKind,
    centroidLocal: centroid,
    normalLocal: normal,
  });

  const edge = (a: number, b: number, points: number[]): BRepEdge => ({
    a,
    b,
    points,
    controlPoints: [],
    faces: [],
    degree: 1,
    idHash: "e",
  });

  function kernelWith(faces: BRepFace[], edges: BRepEdge[]): ExtrudeAnchorKernel {
    return {
      bodyCount: () => 1,
      bodyBrepFaces: () => faces,
      bodyBrepEdges: () => edges,
      bodyPlacement: () => IDENTITY,
    };
  }

  it("finds the tip face and its outline for the extrude's branch", () => {
    const EXT = 7;
    const capBranch = (EXT << 12) | 1;
    const k = kernelWith(
      [
        face("box", 1, 1 << 12, [0, 0, 0], [0, 0, -1]),
        face("cap", EXT, capBranch, [55, 0, 0], [1, 0, 0]),
        face("side1", EXT, (EXT << 12) | 2, [52, 5, 0], [0, 1, 0]),
        face("side2", EXT, (EXT << 12) | 3, [52, -5, 0], [0, -1, 0]),
      ],
      [
        edge(capBranch, (EXT << 12) | 2, [55, 5, -5, 55, 5, 5]),
        edge((EXT << 12) | 3, capBranch, [55, -5, -5, 55, -5, 5]),
        edge(1 << 12, 1 << 12, [0, 0, 0, 1, 1, 1]), // unrelated
      ],
    );
    const anchor = resolveExtrudeAnchor(k, EXT);
    expect(anchor).not.toBeNull();
    expect(anchor!.origin).toEqual([55, 0, 0]);
    expect(anchor!.normal[0]).toBeCloseTo(1);
    expect(anchor!.outline).toHaveLength(2);
  });

  it("returns null when the feature owns no planar face", () => {
    const k = kernelWith([face("f", 3, 3 << 12, [0, 0, 0], [0, 0, 1])], []);
    expect(resolveExtrudeAnchor(k, 99)).toBeNull();
  });

  describe("resolvePendingExtrudeAnchor (ticket x7af)", () => {
    const baseBranch = 1 << 12;
    // "top" sits at arena index 0 (the face() helper hardcodes faceIndex 0);
    // the boundary association runs through edge.faces (ticket b91c).
    const k = kernelWith(
      [
        face("top", 1, baseBranch, [0, 0, 50], [0, 0, 1]),
        face("side", 1, (1 << 12) | 1, [50, 0, 0], [1, 0, 0]),
      ],
      [
        { ...edge(baseBranch, (1 << 12) | 1, [50, -50, 50, 50, 50, 50]), faces: [0, 1] },
        { ...edge((1 << 12) | 1, (1 << 12) | 2, [50, -50, -50, 50, 50, -50]), faces: [1, 2] },
      ],
    );

    it("anchors the would-be tip above the profile face", () => {
      const a = resolvePendingExtrudeAnchor(k, ["top"], 25);
      expect(a).not.toBeNull();
      expect(a!.origin).toEqual([0, 0, 75]);
      expect(a!.normal[2]).toBeCloseTo(1);
      // outline = the profile face's boundary, lifted onto the tip plane
      expect(a!.outline).toHaveLength(1);
      expect(a!.outline[0][2]).toBeCloseTo(75);
      expect(a!.outline[0][5]).toBeCloseTo(75);
    });

    it("a negative depth anchors below the face with a flipped tip-out normal", () => {
      const a = resolvePendingExtrudeAnchor(k, ["top"], -30);
      expect(a).not.toBeNull();
      expect(a!.origin).toEqual([0, 0, 20]);
      expect(a!.normal[2]).toBeCloseTo(-1);
      expect(a!.outline[0][2]).toBeCloseTo(20);
    });

    it("returns null when no profile face matches", () => {
      expect(resolvePendingExtrudeAnchor(k, ["nope"], 10)).toBeNull();
      expect(resolvePendingExtrudeAnchor(k, [], 10)).toBeNull();
    });
  });

  describe("resolvePendingSketchExtrudeAnchor", () => {
    // A sketch (feature 7) on the XY datum plane with one 10×6 rectangle.
    // The plane is a datum, so the resolver never touches body B-Reps —
    // a pending sketch extrude must anchor even on an empty scene.
    const sketchKernel = {
      bodyCount: () => 0,
      bodyBrepFaces: () => [],
      bodyPlacement: () => IDENTITY,
      getFeatureCallParams: (id: number): Record<string, unknown> => {
        if (id !== 7) throw new Error("unknown feature");
        return {
          plane: "qCreatedBy(origin_plane_xy)",
          sub_calls: [
            {
              type: "sketch_rectangle",
              params: { corner_a: [0, 0], corner_b: [10, 6] },
            },
          ],
        };
      },
    };

    it("anchors on the tip plane with the rectangle outline", () => {
      const a = resolvePendingSketchExtrudeAnchor(
        sketchKernel,
        { profiles: "qCreatedBy(7)" },
        12.5,
      );
      expect(a).not.toBeNull();
      expect(a!.normal[2]).toBeCloseTo(1);
      expect(a!.origin[0]).toBeCloseTo(5);
      expect(a!.origin[1]).toBeCloseTo(3);
      expect(a!.origin[2]).toBeCloseTo(12.5);
      expect(a!.outline).toHaveLength(4);
      for (const seg of a!.outline) {
        expect(seg[2]).toBeCloseTo(12.5);
        expect(seg[5]).toBeCloseTo(12.5);
      }
    });

    it("a negative depth anchors below the plane with a flipped normal", () => {
      const a = resolvePendingSketchExtrudeAnchor(
        sketchKernel,
        { profiles: "qCreatedBy(7)" },
        -8,
      );
      expect(a).not.toBeNull();
      expect(a!.normal[2]).toBeCloseTo(-1);
      expect(a!.origin[2]).toBeCloseTo(-8);
    });

    it("returns null without a parsable profiles query or sketch", () => {
      expect(
        resolvePendingSketchExtrudeAnchor(sketchKernel, { profiles: "" }, 10),
      ).toBeNull();
      expect(
        resolvePendingSketchExtrudeAnchor(
          sketchKernel,
          { profiles: "qCreatedBy(99)" },
          10,
        ),
      ).toBeNull();
    });
  });
});

describe("faceQueryHashes", () => {
  it("parses qFace multi-queries and ignores everything else", () => {
    expect(faceQueryHashes("qFace(abc),qFace(def)")).toEqual(["abc", "def"]);
    expect(faceQueryHashes("qFace(x)")).toEqual(["x"]);
    expect(faceQueryHashes("qCreatedBy(3)")).toEqual([]);
    expect(faceQueryHashes("")).toEqual([]);
    expect(faceQueryHashes(undefined)).toEqual([]);
  });
});

describe("previewBandRanges", () => {
  it("grow, asymmetric: tip travels the full delta, back stays put", () => {
    const r = previewBandRanges(10, 25, false);
    expect(r.grow).toBe(true);
    expect(r.prism).toEqual([-10, 15]);
    expect(r.delta).toEqual([[0, 15]]);
  });

  it("shrink, asymmetric: red band covers old tip → new tip", () => {
    const r = previewBandRanges(10, 4, false);
    expect(r.grow).toBe(false);
    expect(r.prism).toEqual([-10, -6]);
    expect(r.delta).toEqual([[-6, 0]]);
  });

  it("grow, symmetric: tip travels half, back band mirrors", () => {
    const r = previewBandRanges(10, 30, true);
    expect(r.grow).toBe(true);
    expect(r.prism).toEqual([-20, 10]);
    expect(r.delta).toEqual([
      [0, 10],
      [-20, -10],
    ]);
  });

  it("no change: no delta bands", () => {
    const r = previewBandRanges(10, 10, true);
    expect(r.prism).toEqual([-10, 0]);
    expect(r.delta).toEqual([]);
  });

  it("crossing the base plane, asymmetric: prism extends below, delta spans old tip → new tip (x7af)", () => {
    const r = previewBandRanges(10, -5, false);
    expect(r.grow).toBe(false);
    // tip offset −15 (the new tip sits 5 below the base, base itself is −10)
    expect(r.prism).toEqual([-10, -15]);
    expect(r.delta).toEqual([[-15, 0]]);
  });
});

describe("prismBandTriangles", () => {
  // One outline segment along +Y on a tip face whose normal is +Z.
  const anchor: ExtrudeAnchor = {
    origin: [0, 0, 5],
    normal: [0, 0, 1],
    outline: [[1, 0, 5, 1, 2, 5]],
  };

  it("emits 4 triangles per segment (2 wall + 2 caps)", () => {
    const tris = prismBandTriangles(anchor, -3, 0);
    expect(tris.length).toBe(4 * 9);
    // Every z must sit on one of the two ring planes (5−3=2 or 5).
    for (let i = 2; i < tris.length; i += 3) {
      expect([2, 5]).toContain(tris[i]);
    }
  });

  it("shifts both rings along the normal by the offsets", () => {
    const tris = prismBandTriangles(anchor, 1, 4);
    for (let i = 2; i < tris.length; i += 3) {
      expect([6, 9]).toContain(tris[i]);
    }
  });
});
