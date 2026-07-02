import { describe, it, expect } from "vitest";
import { isWebglParametricBody } from "./webglBodies";

// Mock kernel returning a fixed hybrid info for body 0.
function mk(info: unknown) {
  return { bodyHybridInfo: () => info } as Parameters<
    typeof isWebglParametricBody
  >[0];
}
const leaf = (kind: string, role = "mesh") => ({ kind, role });
const base = { sdf_regions: [], needs_full_sdf: false, is_union_only: true };

describe("isWebglParametricBody", () => {
  it("single parametric leaf → true", () => {
    expect(isWebglParametricBody(mk({ ...base, leaves: [leaf("box")] }), 0)).toBe(
      true,
    );
  });

  it("pure union of two parametric leaves → true (mdo0)", () => {
    expect(
      isWebglParametricBody(
        mk({ ...base, leaves: [leaf("revolve"), leaf("extrude")] }),
        0,
      ),
    ).toBe(true);
  });

  it("multi-leaf but NOT union-only (difference/intersection) → false", () => {
    expect(
      isWebglParametricBody(
        mk({
          ...base,
          is_union_only: false,
          leaves: [leaf("box"), leaf("cylinder")],
        }),
        0,
      ),
    ).toBe(false);
  });

  it("sdf_regions present → false", () => {
    expect(
      isWebglParametricBody(
        mk({ ...base, sdf_regions: [{}], leaves: [leaf("box")] }),
        0,
      ),
    ).toBe(false);
  });

  it("needs_full_sdf → false", () => {
    expect(
      isWebglParametricBody(
        mk({ ...base, needs_full_sdf: true, leaves: [leaf("box")] }),
        0,
      ),
    ).toBe(false);
  });

  it("non-parametric leaf kind → false", () => {
    expect(
      isWebglParametricBody(mk({ ...base, leaves: [leaf("gyroid")] }), 0),
    ).toBe(false);
  });

  it("skip leaves ignored — one real parametric leaf → true", () => {
    expect(
      isWebglParametricBody(
        mk({ ...base, leaves: [leaf("box"), leaf("cylinder", "skip")] }),
        0,
      ),
    ).toBe(true);
  });

  it("no real leaves → false", () => {
    expect(
      isWebglParametricBody(
        mk({ ...base, leaves: [leaf("box", "skip")] }),
        0,
      ),
    ).toBe(false);
  });
});
