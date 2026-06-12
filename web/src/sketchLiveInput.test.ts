import { describe, expect, it } from "vitest";
import {
  buildLineLengthEdit,
  measuredLength,
  parseField,
} from "./sketchLiveInput";
import { idsOfSubCall } from "./sketchSession";

describe("measuredLength", () => {
  it("measures a horizontal segment", () => {
    expect(measuredLength([0, 0], [10, 0])).toBeCloseTo(10, 9);
  });

  it("measures a diagonal segment", () => {
    expect(measuredLength([1, 2], [4, 6])).toBeCloseTo(5, 9);
  });
});

describe("parseField", () => {
  it("blank → null (no value)", () => {
    expect(parseField("")).toBeNull();
    expect(parseField("   ")).toBeNull();
  });

  it("parses finite numbers incl. negatives + decimals", () => {
    expect(parseField("42")).toBe(42);
    expect(parseField(" 3.5 ")).toBe(3.5);
    expect(parseField("-1.25")).toBe(-1.25);
  });

  it("garbage → null", () => {
    expect(parseField("abc")).toBeNull();
    expect(parseField("Infinity")).toBeNull();
  });
});

describe("buildLineLengthEdit", () => {
  const line = (
    start: [number, number],
    end: [number, number],
  ): { type: string; params: Record<string, unknown> } => ({
    type: "sketch_line",
    params: { start, end },
  });

  it("moves the end point to exactly the typed length, keeping direction", () => {
    const subCalls = [line([0, 0], [3, 4])]; // length 5 toward (0.6, 0.8)
    const edit = buildLineLengthEdit(subCalls, 0, 10);
    expect(edit).not.toBeNull();
    expect(edit!.line.type).toBe("sketch_line");
    expect(edit!.line.params.start).toEqual([0, 0]);
    const end = edit!.line.params.end as [number, number];
    expect(end[0]).toBeCloseTo(6, 9);
    expect(end[1]).toBeCloseTo(8, 9);
  });

  it("emits a DRIVEN distance constraint between the line's own ids", () => {
    const subCalls = [line([0, 0], [1, 0]), line([2, 2], [5, 2])];
    const edit = buildLineLengthEdit(subCalls, 1, 7)!;
    const [startId, endId] = idsOfSubCall(subCalls, 1);
    expect(edit.constraint.type).toBe("sketch_constraint_distance");
    expect(edit.constraint.params.entity_a).toBe(startId);
    expect(edit.constraint.params.entity_b).toBe(endId);
    expect(edit.constraint.params.value_mm).toBe(7);
    expect(edit.constraint.params.driven).toBe(true);
    expect(edit.existingConstraintIndex).toBeNull();
  });

  it("updates an existing distance constraint instead of appending", () => {
    const subCalls = [line([0, 0], [4, 0])];
    const [startId, endId] = idsOfSubCall(subCalls, 0);
    const spine = [
      ...subCalls,
      {
        type: "sketch_constraint_distance",
        params: { entity_a: endId, entity_b: startId, value_mm: 4, driven: true },
      },
    ];
    const edit = buildLineLengthEdit(spine, 0, 9)!;
    expect(edit.existingConstraintIndex).toBe(1);
    expect(edit.constraint.params.value_mm).toBe(9);
    // The existing call's other params survive (incl. reversed id order).
    expect(edit.constraint.params.entity_a).toBe(endId);
    expect(edit.constraint.params.entity_b).toBe(startId);
    expect(edit.constraint.params.driven).toBe(true);
  });

  it("ignores a distance constraint between other entities", () => {
    const subCalls = [
      line([0, 0], [1, 0]),
      line([2, 0], [3, 0]),
      {
        type: "sketch_constraint_distance",
        params: { entity_a: 99, entity_b: 100, value_mm: 1, driven: true },
      },
    ];
    const edit = buildLineLengthEdit(subCalls, 0, 5)!;
    expect(edit.existingConstraintIndex).toBeNull();
  });

  it("rejects a non-line index", () => {
    const subCalls = [{ type: "sketch_point", params: { at: [0, 0] } }];
    expect(buildLineLengthEdit(subCalls, 0, 5)).toBeNull();
    expect(buildLineLengthEdit(subCalls, 3, 5)).toBeNull();
  });

  it("rejects a zero-length line (no direction)", () => {
    expect(buildLineLengthEdit([line([1, 1], [1, 1])], 0, 5)).toBeNull();
  });

  it("rejects a non-positive or non-finite typed length", () => {
    const subCalls = [line([0, 0], [1, 0])];
    expect(buildLineLengthEdit(subCalls, 0, 0)).toBeNull();
    expect(buildLineLengthEdit(subCalls, 0, -2)).toBeNull();
    expect(buildLineLengthEdit(subCalls, 0, Number.NaN)).toBeNull();
    expect(
      buildLineLengthEdit(subCalls, 0, Number.POSITIVE_INFINITY),
    ).toBeNull();
  });
});
