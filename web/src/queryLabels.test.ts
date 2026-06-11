import { describe, expect, it } from "vitest";
import {
  DATUM_PLANE_QUERIES,
  QUERY_DISPLAY_LABELS,
  formatQueryDisplay,
} from "./queryLabels";

describe("datum plane queries", () => {
  it("covers exactly the three plane datums (origin has no plane query)", () => {
    expect(Object.keys(DATUM_PLANE_QUERIES).sort()).toEqual([
      "front",
      "right",
      "top",
    ]);
    expect(DATUM_PLANE_QUERIES["origin"]).toBeUndefined();
  });

  it("every datum query has a display label naming its datum", () => {
    // The two maps live side by side so they can't drift: the label of
    // front's query must say "Front", etc.
    for (const [datum, query] of Object.entries(DATUM_PLANE_QUERIES)) {
      const label = QUERY_DISPLAY_LABELS[query];
      expect(label, query).toBeDefined();
      expect(label.toLowerCase()).toContain(datum);
    }
  });

  it("formatQueryDisplay uses the outliner-style names", () => {
    expect(formatQueryDisplay("qCreatedBy(origin_plane_xy)")).toBe("Front (XY)");
    expect(formatQueryDisplay("qCreatedBy(origin_plane_xz)")).toBe("Top (XZ)");
    expect(formatQueryDisplay("qCreatedBy(origin_plane_yz)")).toBe("Right (YZ)");
  });
});
