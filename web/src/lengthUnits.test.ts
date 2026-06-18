import { describe, it, expect } from "vitest";
import {
  isLengthUnit,
  mmToUnit,
  unitToMm,
  formatLength,
} from "./lengthUnits";

describe("lengthUnits", () => {
  it("recognises known length units case-insensitively", () => {
    expect(isLengthUnit("mm")).toBe(true);
    expect(isLengthUnit("CM")).toBe(true);
    expect(isLengthUnit("in")).toBe(true);
    expect(isLengthUnit("deg")).toBe(false);
    expect(isLengthUnit("")).toBe(false);
  });

  it("converts mm to a display unit", () => {
    expect(mmToUnit(150, "cm")).toBeCloseTo(15);
    expect(mmToUnit(1000, "m")).toBeCloseTo(1);
    expect(mmToUnit(25.4, "in")).toBeCloseTo(1);
    expect(mmToUnit(42, "mm")).toBe(42);
  });

  it("converts a display value back to mm (round trip)", () => {
    expect(unitToMm(15, "cm")).toBeCloseTo(150);
    expect(unitToMm(1, "ft")).toBeCloseTo(304.8);
    for (const u of ["mm", "cm", "m", "in", "ft"]) {
      expect(unitToMm(mmToUnit(123.4, u), u)).toBeCloseTo(123.4);
    }
  });

  it("leaves unknown units unchanged", () => {
    expect(mmToUnit(7, "furlong")).toBe(7);
    expect(unitToMm(7, "furlong")).toBe(7);
  });

  it("formats with unit label and decimals", () => {
    expect(formatLength(150, "cm", 1)).toBe("15.0 cm");
    expect(formatLength(42, "mm", 2)).toBe("42.00 mm");
    expect(formatLength(1000, "m", 0)).toBe("1 m");
  });
});
