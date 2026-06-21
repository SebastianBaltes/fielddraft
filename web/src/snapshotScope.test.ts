import { describe, it, expect } from "vitest";
import { computeSelectableFeatureIds } from "./snapshotScope";

const features = [{ id: 10 }, { id: 20 }, { id: 30 }, { id: 40 }];

describe("computeSelectableFeatureIds (ticket 2ukj forward-ref guard)", () => {
  it("allows only features BEFORE the edited one (own output excluded)", () => {
    // Editing feature 30 → may reference 10 and 20, NOT 30 (its own output)
    // nor 40 (a later feature). This is exactly the Extrude-2 profile case:
    // the field must not pick the B-Reps Extrude 2 itself created.
    const scope = computeSelectableFeatureIds({
      selection: new Set([30]),
      features,
    });
    expect(scope).toBeDefined();
    expect([...scope!].sort((a, b) => a - b)).toEqual([10, 20]);
    expect(scope!.has(30)).toBe(false);
    expect(scope!.has(40)).toBe(false);
  });

  it("the FIRST feature has an empty scope (nothing earlier to reference)", () => {
    const scope = computeSelectableFeatureIds({
      selection: new Set([10]),
      features,
    });
    expect(scope).toBeDefined();
    expect(scope!.size).toBe(0);
  });

  it("the LAST feature may reference every earlier feature", () => {
    const scope = computeSelectableFeatureIds({
      selection: new Set([40]),
      features,
    });
    expect([...scope!].sort((a, b) => a - b)).toEqual([10, 20, 30]);
  });

  it("returns undefined (no restriction) when not exactly one is selected", () => {
    expect(
      computeSelectableFeatureIds({ selection: new Set(), features }),
    ).toBeUndefined();
    expect(
      computeSelectableFeatureIds({ selection: new Set([10, 20]), features }),
    ).toBeUndefined();
  });

  it("returns undefined when the selected feature isn't in the tree", () => {
    expect(
      computeSelectableFeatureIds({ selection: new Set([999]), features }),
    ).toBeUndefined();
  });

  it("returns undefined for a null/empty store", () => {
    expect(computeSelectableFeatureIds(null)).toBeUndefined();
    expect(computeSelectableFeatureIds(undefined)).toBeUndefined();
  });
});
