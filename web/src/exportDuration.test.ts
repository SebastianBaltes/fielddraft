import { describe, expect, it } from "vitest";
import { formatDuration } from "./exportDuration";

describe("formatDuration", () => {
  it("shows sub-second runs in milliseconds", () => {
    expect(formatDuration(0)).toBe("0 ms");
    expect(formatDuration(412.4)).toBe("412 ms");
    expect(formatDuration(999)).toBe("999 ms");
  });

  it("shows seconds with one decimal", () => {
    expect(formatDuration(1000)).toBe("1.0 s");
    expect(formatDuration(12_351)).toBe("12.4 s");
    expect(formatDuration(59_940)).toBe("59.9 s");
  });

  it("carries into minutes", () => {
    expect(formatDuration(60_000)).toBe("1 min 0 s");
    expect(formatDuration(85_000)).toBe("1 min 25 s");
    // Must not read "1 min 60 s".
    expect(formatDuration(119_800)).toBe("2 min 0 s");
  });

  it("never invents a number", () => {
    expect(formatDuration(Number.NaN)).toBe("—");
    expect(formatDuration(-1)).toBe("—");
  });
});
