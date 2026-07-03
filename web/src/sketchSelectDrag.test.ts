import { describe, expect, it } from "vitest";
import { planSelectDrag } from "./sketchSelectDrag";

/**
 * Ticket matf: the Select-mode drag planner decides between reshaping one
 * handle's degree of freedom (grip) and rigid-translating an entity / group.
 */

describe("planSelectDrag", () => {
  it("grips a lone handle with no selection", () => {
    expect(
      planSelectDrag({ selection: [], handleEntityId: 5, hitEntityId: 5, handleOnSelection: false }),
    ).toEqual({ kind: "grip", selectId: 5 });
  });

  it("grips a handle that is the sole current selection", () => {
    expect(
      planSelectDrag({ selection: [5], handleEntityId: 5, hitEntityId: 5, handleOnSelection: false }),
    ).toEqual({ kind: "grip", selectId: 5 });
  });

  it("translates a single entity body (no handle) and selects it", () => {
    expect(
      planSelectDrag({ selection: [], handleEntityId: null, hitEntityId: 7, handleOnSelection: false }),
    ).toEqual({ kind: "translate", entityIds: [7], selectIds: [7] });
  });

  it("translates the whole group when grabbing inside a multi-selection", () => {
    const plan = planSelectDrag({
      selection: [2, 7, 9],
      handleEntityId: null,
      hitEntityId: 7, handleOnSelection: false,
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
      hitEntityId: 7, handleOnSelection: false,
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
      hitEntityId: 7, handleOnSelection: false,
    });
    expect(plan).toEqual({ kind: "translate", entityIds: [7], selectIds: [7] });
  });

  it("keeps a handle reshape when the handle entity is the sole selection even with a hit", () => {
    // handle == hit == the only selected id → reshape, don't translate.
    expect(
      planSelectDrag({ selection: [4], handleEntityId: 4, hitEntityId: 4, handleOnSelection: false }),
    ).toEqual({ kind: "grip", selectId: 4 });
  });

  it("returns none over empty space", () => {
    expect(
      planSelectDrag({ selection: [1, 2], handleEntityId: null, hitEntityId: null, handleOnSelection: false }),
    ).toEqual({ kind: "none" });
  });

  // A line's endpoint is its own POINT entity, distinct from the line. Clicking
  // the line body selects the LINE, but grabbing its endpoint grip resolves to
  // the endpoint POINT — so handleEntityId (the point) never equals the
  // selection (the line). `handleOnSelection` carries the fact that the grip
  // belongs to the selected curve, so the reshape still wins.
  it("grips an endpoint handle that belongs to the sole selected curve", () => {
    expect(
      planSelectDrag({
        selection: [3],
        handleEntityId: 2,
        hitEntityId: 2,
        handleOnSelection: true,
      }),
    ).toEqual({ kind: "grip", selectId: null });
  });

  it("translates when a handle belongs to a DIFFERENT single selection", () => {
    // The grip isn't on the selected entity (handleOnSelection false) and its
    // point (2) isn't the selection (9) → fall through to a body translate.
    expect(
      planSelectDrag({
        selection: [9],
        handleEntityId: 2,
        hitEntityId: 2,
        handleOnSelection: false,
      }),
    ).toEqual({ kind: "translate", entityIds: [2], selectIds: [2] });
  });

  it("translates the whole multi-selection when grabbing a member's endpoint grip", () => {
    // handleOnSelection true but MULTI-select → the group moves as one.
    expect(
      planSelectDrag({
        selection: [3, 8],
        handleEntityId: 2,
        hitEntityId: 2,
        handleOnSelection: true,
      }),
    ).toEqual({ kind: "translate", entityIds: [3, 8], selectIds: [3, 8] });
  });
});
