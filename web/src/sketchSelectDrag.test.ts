import { describe, expect, it } from "vitest";
import { planSelectDrag } from "./sketchSelectDrag";

/**
 * Ticket matf: the Select-mode drag planner decides between reshaping one
 * handle's degree of freedom (grip) and rigid-translating an entity / group.
 */

describe("planSelectDrag", () => {
  it("grips a lone handle with no selection", () => {
    expect(
      planSelectDrag({ selection: [], handleEntityId: 5, hitEntityId: 5 }),
    ).toEqual({ kind: "grip", selectId: 5 });
  });

  it("grips a handle that is the sole current selection", () => {
    expect(
      planSelectDrag({ selection: [5], handleEntityId: 5, hitEntityId: 5 }),
    ).toEqual({ kind: "grip", selectId: 5 });
  });

  it("translates a single entity body (no handle) and selects it", () => {
    expect(
      planSelectDrag({ selection: [], handleEntityId: null, hitEntityId: 7 }),
    ).toEqual({ kind: "translate", entityIds: [7], selectIds: [7] });
  });

  it("translates the whole group when grabbing inside a multi-selection", () => {
    const plan = planSelectDrag({
      selection: [2, 7, 9],
      handleEntityId: null,
      hitEntityId: 7,
    });
    expect(plan).toEqual({
      kind: "translate",
      entityIds: [2, 7, 9],
      selectIds: [2, 7, 9],
    });
  });

  it("translates the group even when the grab lands on a handle inside it", () => {
    // A handle inside a multi-selection grabs the GROUP, not the DoF.
    const plan = planSelectDrag({
      selection: [2, 7],
      handleEntityId: 7,
      hitEntityId: 7,
    });
    expect(plan).toEqual({
      kind: "translate",
      entityIds: [2, 7],
      selectIds: [2, 7],
    });
  });

  it("translates a fresh entity body, replacing an unrelated selection", () => {
    // Cursor is over entity 7's body; the live selection is {2,9} (does not
    // include 7) → grab 7 alone and make it the selection.
    const plan = planSelectDrag({
      selection: [2, 9],
      handleEntityId: null,
      hitEntityId: 7,
    });
    expect(plan).toEqual({ kind: "translate", entityIds: [7], selectIds: [7] });
  });

  it("keeps a handle reshape when the handle entity is the sole selection even with a hit", () => {
    // handle == hit == the only selected id → reshape, don't translate.
    expect(
      planSelectDrag({ selection: [4], handleEntityId: 4, hitEntityId: 4 }),
    ).toEqual({ kind: "grip", selectId: 4 });
  });

  it("returns none over empty space", () => {
    expect(
      planSelectDrag({ selection: [1, 2], handleEntityId: null, hitEntityId: null }),
    ).toEqual({ kind: "none" });
  });
});
