import { describe, expect, it } from "vitest";
import {
  buildTypedLineBundle,
  lineFromLengthAngle,
  measuredAngleDeg,
  measuredLength,
  parseField,
  resolveLengthAngle,
} from "./sketchLiveInput";
import { predictNextLineIds } from "./sketchSession";

describe("lineFromLengthAngle", () => {
  it("length 10 angle 0 from origin → (10, 0)", () => {
    const [x, y] = lineFromLengthAngle([0, 0], 10, 0);
    expect(x).toBeCloseTo(10, 9);
    expect(y).toBeCloseTo(0, 9);
  });

  it("length 10 angle 90 from origin → (0, 10)", () => {
    const [x, y] = lineFromLengthAngle([0, 0], 10, 90);
    expect(x).toBeCloseTo(0, 9);
    expect(y).toBeCloseTo(10, 9);
  });

  it("angle 180 flips along −X", () => {
    const [x, y] = lineFromLengthAngle([5, 5], 4, 180);
    expect(x).toBeCloseTo(1, 9);
    expect(y).toBeCloseTo(5, 9);
  });

  it("respects a non-origin start", () => {
    const [x, y] = lineFromLengthAngle([3, 7], 5, 0);
    expect(x).toBeCloseTo(8, 9);
    expect(y).toBeCloseTo(7, 9);
  });
});

describe("measuredLength / measuredAngleDeg", () => {
  it("measures a horizontal segment", () => {
    expect(measuredLength([0, 0], [10, 0])).toBeCloseTo(10, 9);
    expect(measuredAngleDeg([0, 0], [10, 0])).toBeCloseTo(0, 9);
  });

  it("measures a vertical segment", () => {
    expect(measuredLength([0, 0], [0, 7])).toBeCloseTo(7, 9);
    expect(measuredAngleDeg([0, 0], [0, 7])).toBeCloseTo(90, 9);
  });

  it("round-trips through lineFromLengthAngle", () => {
    const end = lineFromLengthAngle([1, 2], 13, 37);
    expect(measuredLength([1, 2], end)).toBeCloseTo(13, 9);
    expect(measuredAngleDeg([1, 2], end)).toBeCloseTo(37, 9);
  });
});

describe("parseField", () => {
  it("blank → null (no override)", () => {
    expect(parseField("")).toBeNull();
    expect(parseField("   ")).toBeNull();
  });
  it("parses finite numbers incl. negatives + decimals", () => {
    expect(parseField("10")).toBe(10);
    expect(parseField(" -2.5 ")).toBe(-2.5);
  });
  it("garbage → null", () => {
    expect(parseField("abc")).toBeNull();
    expect(parseField("Infinity")).toBeNull();
  });
});

describe("resolveLengthAngle", () => {
  it("typed length + typed angle override the live values", () => {
    expect(resolveLengthAngle("50", "30", 12, 99)).toEqual({
      length: 50,
      angleDeg: 30,
    });
  });
  it("blank length falls back to the live length; typed angle wins", () => {
    expect(resolveLengthAngle("", "45", 17, 99)).toEqual({
      length: 17,
      angleDeg: 45,
    });
  });
  it("blank angle falls back to the live angle; typed length wins", () => {
    expect(resolveLengthAngle("20", "", 17, 88)).toEqual({
      length: 20,
      angleDeg: 88,
    });
  });
  it("both blank → both live values", () => {
    expect(resolveLengthAngle("", "", 5, 5)).toEqual({
      length: 5,
      angleDeg: 5,
    });
  });
});

describe("buildTypedLineBundle", () => {
  it("emits a sketch_line + a DRIVEN distance referencing the predicted endpoints", () => {
    const subCalls: { type: string; params: Record<string, unknown> }[] = [];
    const ids = predictNextLineIds(subCalls);
    const bundle = buildTypedLineBundle(subCalls, [0, 0], 10, 0);
    expect(bundle).not.toBeNull();
    const [line, dist] = bundle!;

    expect(line.type).toBe("sketch_line");
    expect(line.params.start).toEqual([0, 0]);
    const end = line.params.end as [number, number];
    expect(end[0]).toBeCloseTo(10, 9);
    expect(end[1]).toBeCloseTo(0, 9);

    expect(dist.type).toBe("sketch_constraint_distance");
    expect(dist.params.entity_a).toBe(ids.start);
    expect(dist.params.entity_b).toBe(ids.end);
    expect(dist.params.value_mm).toBe(10);
    expect(dist.params.driven).toBe(true);
  });

  it("predicts ids relative to the existing spine", () => {
    const subCalls = [
      { type: "sketch_line", params: { start: [0, 0], end: [1, 0] } },
    ];
    const ids = predictNextLineIds(subCalls);
    const bundle = buildTypedLineBundle(subCalls, [2, 2], 5, 90)!;
    expect(bundle[1].params.entity_a).toBe(ids.start);
    expect(bundle[1].params.entity_b).toBe(ids.end);
    // The second line's endpoint ids must sit above the first line's.
    expect(ids.start).toBeGreaterThan(0);
  });

  it("length 0 → no commit (null)", () => {
    expect(buildTypedLineBundle([], [0, 0], 0, 0)).toBeNull();
  });

  it("negative length → no commit (null)", () => {
    expect(buildTypedLineBundle([], [0, 0], -5, 0)).toBeNull();
  });

  it("non-finite length → no commit (null)", () => {
    expect(buildTypedLineBundle([], [0, 0], Number.NaN, 0)).toBeNull();
    expect(buildTypedLineBundle([], [0, 0], Number.POSITIVE_INFINITY, 0)).toBeNull();
  });

  it("non-finite angle → no commit (null)", () => {
    expect(buildTypedLineBundle([], [0, 0], 10, Number.NaN)).toBeNull();
  });
});
