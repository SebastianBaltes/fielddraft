import { describe, expect, it } from "vitest";
import {
  chamferLines,
  chamferRecipe,
  chamferRecipeOf,
  reeditChamferDistance,
  type Curve,
  type Vec2,
} from "./sketchChamfer";
import { FD_ID_PARAM } from "./sketchFillet";
import { subCallOfCurve, type SubCall } from "./sketchTrim";

const line = (a: Vec2, b: Vec2): Curve => ({ kind: "line", a, b });

const close = (a: Vec2, b: Vec2, tol = 1e-6) =>
  Math.hypot(a[0] - b[0], a[1] - b[1]) < tol;

/** Both endpoints of a line, regardless of which is a/b. */
const endsOf = (cv: Curve): [Vec2, Vec2] => {
  if (cv.kind !== "line") throw new Error("expected line");
  return [cv.a, cv.b];
};
const hasEnd = (cv: Curve, p: Vec2) => endsOf(cv).some((e) => close(e, p));

describe("chamferLines — right-angle corner", () => {
  // L1 along +x from origin, L2 along +y from origin, dist = 2.
  const l1 = line([0, 0], [10, 0]);
  const l2 = line([0, 0], [0, 10]);
  const res = chamferLines(l1, l2, 2);

  it("produces a result", () => {
    expect(res).not.toBeNull();
  });

  it("setback points at (2,0) and (0,2)", () => {
    if (!res) throw new Error("no result");
    // The corner endpoint (nearest origin) becomes the setback point.
    expect(hasEnd(res.trimmed1, [2, 0])).toBe(true);
    expect(hasEnd(res.trimmed2, [0, 2])).toBe(true);
    // Far ends preserved.
    expect(hasEnd(res.trimmed1, [10, 0])).toBe(true);
    expect(hasEnd(res.trimmed2, [0, 10])).toBe(true);
  });

  it("connecting line runs between the two setback points", () => {
    if (!res || res.line.kind !== "line") throw new Error("expected line");
    expect(hasEnd(res.line, [2, 0])).toBe(true);
    expect(hasEnd(res.line, [0, 2])).toBe(true);
  });
});

describe("chamferLines — angle variations", () => {
  it("acute (45°) corner sets back along each line by dist", () => {
    // L1 +x, L2 at 45°. Setbacks are at dist along each direction.
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [10, 10]);
    const res = chamferLines(l1, l2, 3);
    if (!res || res.line.kind !== "line") throw new Error("expected result");
    const s2: Vec2 = [3 / Math.SQRT2, 3 / Math.SQRT2];
    expect(hasEnd(res.trimmed1, [3, 0])).toBe(true);
    expect(hasEnd(res.trimmed2, s2)).toBe(true);
    expect(hasEnd(res.line, [3, 0])).toBe(true);
    expect(hasEnd(res.line, s2)).toBe(true);
  });

  it("obtuse (135°) corner sets back along each line by dist", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [-10, 10]);
    const res = chamferLines(l1, l2, 2);
    if (!res || res.line.kind !== "line") throw new Error("expected result");
    const s2: Vec2 = [-2 / Math.SQRT2, 2 / Math.SQRT2];
    expect(hasEnd(res.trimmed1, [2, 0])).toBe(true);
    expect(hasEnd(res.trimmed2, s2)).toBe(true);
  });

  it("works when the shared corner is the b-end of both lines", () => {
    // Lines drawn toward the corner: far ends are the a-ends.
    const l1 = line([10, 0], [0, 0]);
    const l2 = line([0, 10], [0, 0]);
    const res = chamferLines(l1, l2, 2);
    if (!res) throw new Error("expected result");
    // Far ends (10,0) and (0,10) preserved; corner b-ends moved to setbacks.
    expect(hasEnd(res.trimmed1, [10, 0])).toBe(true);
    expect(hasEnd(res.trimmed1, [2, 0])).toBe(true);
    expect(hasEnd(res.trimmed2, [0, 10])).toBe(true);
    expect(hasEnd(res.trimmed2, [0, 2])).toBe(true);
  });

  it("works with a small corner gap (lines nearly meet)", () => {
    const l1 = line([0, 0], [10, 0.05]);
    const l2 = line([0.05, 0], [0, 10]);
    const res = chamferLines(l1, l2, 2);
    expect(res).not.toBeNull();
  });
});

describe("chamferLines — edge cases return null", () => {
  it("parallel lines", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 5], [10, 5]);
    expect(chamferLines(l1, l2, 2)).toBeNull();
  });

  it("antiparallel (collinear) lines", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([10, 0], [20, 0]);
    expect(chamferLines(l1, l2, 2)).toBeNull();
  });

  it("setback so large it passes a segment end", () => {
    // Right angle, short legs of length 1: dist 5 ≫ leg length.
    const l1 = line([0, 0], [1, 0]);
    const l2 = line([0, 0], [0, 1]);
    expect(chamferLines(l1, l2, 5)).toBeNull();
  });

  it("setback exceeds exactly one leg", () => {
    // L1 long (10), L2 short (3); dist 4 fits L1 but exceeds L2.
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 3]);
    expect(chamferLines(l1, l2, 4)).toBeNull();
  });

  it("lines that don't share a near corner (far apart)", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([100, 100], [100, 110]);
    expect(chamferLines(l1, l2, 2)).toBeNull();
  });

  it("zero distance", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(chamferLines(l1, l2, 0)).toBeNull();
  });

  it("negative distance", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(chamferLines(l1, l2, -2)).toBeNull();
  });

  it("non-finite distance", () => {
    const l1 = line([0, 0], [10, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(chamferLines(l1, l2, Number.NaN)).toBeNull();
    expect(chamferLines(l1, l2, Number.POSITIVE_INFINITY)).toBeNull();
  });

  it("zero-length line", () => {
    const l1 = line([0, 0], [0, 0]);
    const l2 = line([0, 0], [0, 10]);
    expect(chamferLines(l1, l2, 2)).toBeNull();
  });

  it("non-line operands", () => {
    const circle: Curve = { kind: "circle", c: [0, 0], r: 3 };
    const l2 = line([0, 0], [0, 10]);
    expect(chamferLines(circle, l2, 2)).toBeNull();
  });
});

// Plan item 4: post-hoc re-edit of a baked chamfer via its recipe tag.
describe("reeditChamferDistance — recipe-tagged corner", () => {
  // Build the spine `chamferSketchAt` would produce for a right-angle
  // corner (L1 along +x, L2 along +y) at setback 2: the two trimmed source
  // lines (tagged with stable ids) + the bevel carrying the recipe.
  const ID_A = "lineA";
  const ID_B = "lineB";
  const ID_BEVEL = "bevel1";
  const origA = line([0, 0], [10, 0]) as Extract<Curve, { kind: "line" }>;
  const origB = line([0, 0], [0, 10]) as Extract<Curve, { kind: "line" }>;

  /** An endpoint list contains `p` within float tolerance. */
  const hasNearEnd = (ends: unknown[], p: Vec2): boolean =>
    ends.some(
      (e) => Array.isArray(e) && Math.hypot(e[0] - p[0], e[1] - p[1]) < 1e-9,
    );

  const bake = (distance: number): SubCall[] => {
    const c = chamferLines(origA, origB, distance)!;
    return [
      subCallOfCurve(c.trimmed1, { [FD_ID_PARAM]: ID_A }),
      subCallOfCurve(c.trimmed2, { [FD_ID_PARAM]: ID_B }),
      subCallOfCurve(c.line, {
        [FD_ID_PARAM]: ID_BEVEL,
        chamfer: chamferRecipe(ID_A, ID_B, origA, origB, distance),
      }),
    ];
  };

  it("the baked bevel carries a readable recipe", () => {
    const recipe = chamferRecipeOf(bake(2)[2]);
    expect(recipe).not.toBeNull();
    expect(recipe!.distance).toBe(2);
    expect(recipe!.line_a_id).toBe(ID_A);
    expect(recipe!.orig_b.b).toEqual([0, 10]);
  });

  it("re-edits distance: bevel + both trimmed lines update", () => {
    const out = reeditChamferDistance(bake(2), ID_BEVEL, 3)!;
    expect(out).not.toBeNull();

    // Bevel: setback 3 → endpoints (3,0)–(0,3); recipe distance refreshed.
    const bevel = out[2];
    expect(bevel.type).toBe("sketch_line");
    const bevelEnds = [bevel.params.start, bevel.params.end];
    expect(hasNearEnd(bevelEnds, [3, 0])).toBe(true);
    expect(hasNearEnd(bevelEnds, [0, 3])).toBe(true);
    expect(bevel.params[FD_ID_PARAM]).toBe(ID_BEVEL);
    expect(chamferRecipeOf(bevel)!.distance).toBe(3);

    // Trimmed lines: setback points move to (3,0)/(0,3); ids + far ends kept.
    expect(out[0].params[FD_ID_PARAM]).toBe(ID_A);
    expect(out[1].params[FD_ID_PARAM]).toBe(ID_B);
    const endsA = [out[0].params.start, out[0].params.end];
    const endsB = [out[1].params.start, out[1].params.end];
    expect(hasNearEnd(endsA, [3, 0])).toBe(true);
    expect(hasNearEnd(endsA, [10, 0])).toBe(true);
    expect(hasNearEnd(endsB, [0, 3])).toBe(true);
    expect(hasNearEnd(endsB, [0, 10])).toBe(true);
  });

  it("re-derives from ORIGINAL geometry, not the trimmed remainder", () => {
    const out = reeditChamferDistance(bake(2), ID_BEVEL, 1)!;
    const endsA = [out[0].params.start, out[0].params.end];
    expect(hasNearEnd(endsA, [1, 0])).toBe(true);
  });

  it("returns null for an unknown bevel id", () => {
    expect(reeditChamferDistance(bake(2), "nope", 3)).toBeNull();
  });

  it("returns null when a referenced source line is gone", () => {
    const spine = bake(2).filter((c) => c.params[FD_ID_PARAM] !== ID_B);
    expect(reeditChamferDistance(spine, ID_BEVEL, 3)).toBeNull();
  });

  it("returns null for a geometrically impossible distance", () => {
    expect(reeditChamferDistance(bake(2), ID_BEVEL, 50)).toBeNull();
  });
});
