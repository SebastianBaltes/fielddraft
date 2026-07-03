/**
 * FeatureDialog integration test (K6a.1).
 *
 * Builds a hand-authored schema that mirrors the Onshape extrude
 * dialog from `assets/onshape/3d-extrude.png` — NAIO tabs, an
 * end-condition enum with conditional sub-fields, an inline flip
 * button on the depth length, three collapsible groups
 * (Direction / Starting offset / Second end position), the
 * symmetric and draft checkboxes, and a view-state transparency
 * slider in the footer — and checks that the rendered DOM
 * matches the structure.
 *
 * This test is the guardrail that keeps the generic widget kit
 * honest against the richest feature dialog the MVP needs. If a
 * future refactor drops a widget kind or breaks conditional
 * visibility, the Onshape-structural assertions below fail loudly.
 */

import { afterEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

// React's createRoot path wants an explicit `IS_REACT_ACT_ENVIRONMENT`
// flag set before the first render; without it React logs a
// "testing environment is not configured" warning on every
// act() call. Vitest's jsdom env leaves the flag unset, so we
// flip it here for the lifetime of this test file.
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean })
  .IS_REACT_ACT_ENVIRONMENT = true;
import {
  buildDefaultDefinition,
  parseFeatureSchema,
  updateParam,
} from "./schema";
import { FeatureDialog } from "./FeatureDialog";

// ─── fixture: Onshape-shaped extrude schema ────────────────────

const extrudeDialogRaw = {
  type: "extrude",
  params: [
    {
      key: "operation",
      name: "Operation",
      type: "enum",
      required: true,
      ui_hint: "horizontal_enum",
      variants: [
        { key: "new", label: "New" },
        { key: "add", label: "Add" },
        { key: "remove", label: "Remove" },
        { key: "intersect", label: "Intersect" },
      ],
    },
    {
      key: "profiles",
      name: "Faces and sketch regions to extrude",
      type: "query",
      required: true,
      entity_filter: "face",
    },
    {
      key: "end_condition",
      name: "End condition",
      type: "enum",
      required: true,
      variants: [
        { key: "blind", label: "Blind" },
        { key: "symmetric", label: "Symmetric" },
        { key: "through_all", label: "Through all" },
        { key: "up_to_face", label: "Up to face" },
      ],
    },
    {
      key: "depth",
      name: "Depth",
      type: "length",
      required: true,
      visible_when: { param: "end_condition", equals: "blind" },
    },
    {
      key: "flip_depth",
      name: "Flip depth",
      type: "bool",
      required: false,
      ui_hint: "opposite_direction",
      visible_when: { param: "end_condition", equals: "blind" },
    },
    {
      key: "up_to_face_target",
      name: "Up-to face target",
      type: "query",
      required: true,
      visible_when: { param: "end_condition", equals: "up_to_face" },
    },
    {
      key: "use_direction",
      name: "Use explicit direction",
      type: "bool",
      required: false,
    },
    {
      type: "group",
      key: "direction_group",
      name: "Direction",
      driving_param: "use_direction",
      children: [
        {
          key: "direction_query",
          name: "Extrude direction",
          type: "query",
          required: false,
        },
      ],
    },
    {
      key: "use_starting_offset",
      name: "Use starting offset",
      type: "bool",
      required: false,
    },
    {
      type: "group",
      key: "starting_offset_group",
      name: "Starting offset",
      driving_param: "use_starting_offset",
      children: [
        {
          key: "starting_offset_depth",
          name: "Offset depth",
          type: "length",
          required: false,
        },
      ],
    },
    { key: "symmetric", name: "Symmetric", type: "bool", required: false },
    { key: "draft", name: "Draft", type: "bool", required: false },
    {
      key: "use_second_end",
      name: "Use second end position",
      type: "bool",
      required: false,
    },
    {
      type: "group",
      key: "second_end_group",
      name: "Second end position",
      driving_param: "use_second_end",
      children: [
        {
          key: "second_depth",
          name: "Depth",
          type: "length",
          required: false,
        },
        { key: "second_draft", name: "Draft", type: "bool", required: false },
      ],
    },
  ],
};

// ─── DOM rendering harness ─────────────────────────────────────
//
// Every rendered root is tracked and unmounted in afterEach. Without
// the unmount, React keeps the tree mounted past the test file's end;
// its scheduler then fires `performWorkUntilDeadline` after the jsdom
// environment is torn down, throwing "window is not defined" as an
// uncaught exception (harmless but noisy — 14 per full run).

const mounted: { root: Root; container: HTMLElement }[] = [];

function render(element: React.ReactElement): HTMLElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(element);
  });
  mounted.push({ root, container });
  return container;
}

afterEach(() => {
  for (const { root, container } of mounted.splice(0)) {
    act(() => root.unmount());
    container.remove();
  }
});

// ─── tests ─────────────────────────────────────────────────────

describe("FeatureDialog — Onshape-structural parity (K6a.1)", () => {
  it("renders NAIO as a tab row with four tabs", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
      />,
    );
    const tabRow = container.querySelector(".tab-row")!;
    expect(tabRow).toBeTruthy();
    const tabs = tabRow.querySelectorAll(".tab");
    expect(tabs).toHaveLength(4);
    expect([...tabs].map((t) => t.textContent)).toEqual([
      "New",
      "Add",
      "Remove",
      "Intersect",
    ]);
    // The default is the first variant → "new" is active.
    expect(tabs[0].classList.contains("is-active")).toBe(true);
  });

  it("renders the flip button inline for bool+opposite_direction", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
      />,
    );
    const flip = container.querySelector(".flip-button");
    expect(flip).toBeTruthy();
    expect(flip?.getAttribute("aria-pressed")).toBe("false");
  });

  it("hides depth/flip when the end-condition enum changes", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    let def = buildDefaultDefinition(schema);
    // Default: end_condition = "blind" → depth visible.
    let container = render(
      <FeatureDialog schema={schema} definition={def} onChange={() => {}} />,
    );
    expect(container.querySelector('[data-feature-type="extrude"]')).toBeTruthy();
    expect(container.textContent).toContain("Depth");
    expect(container.textContent).not.toContain("Up-to face target");

    // Switch to "up_to_face" — depth + flip disappear, target appears.
    def = updateParam(def, "end_condition", "up_to_face");
    container = render(
      <FeatureDialog schema={schema} definition={def} onChange={() => {}} />,
    );
    expect(container.textContent).not.toContain("Flip depth");
    expect(container.textContent).toContain("Up-to face target");
  });

  it("renders three collapsible groups, initially collapsed", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
      />,
    );
    const groups = container.querySelectorAll(".group");
    expect(groups).toHaveLength(3);
    // All three driving bools default to false → no group-body.
    for (const g of groups) {
      expect(g.querySelector(".group-body")).toBeNull();
    }
  });

  it("expands a group when its driving bool becomes true", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    let def = buildDefaultDefinition(schema);
    def = updateParam(def, "use_direction", true);
    const container = render(
      <FeatureDialog schema={schema} definition={def} onChange={() => {}} />,
    );
    const directionGroup = container.querySelector(
      '[data-group-key="direction_group"]',
    )!;
    expect(directionGroup).toBeTruthy();
    expect(directionGroup.querySelector(".group-body")).toBeTruthy();
    expect(directionGroup.textContent).toContain("Extrude direction");
  });

  it("renders header confirm / cancel buttons when callbacks are provided", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
        onCommit={() => {}}
        onCancel={() => {}}
      />,
    );
    expect(container.querySelector(".header-action.commit")).toBeTruthy();
    expect(container.querySelector(".header-action.cancel")).toBeTruthy();
  });

  it("renders a footer slot (e.g. transparency slider)", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
        footer={<input type="range" data-testid="transparency" />}
      />,
    );
    const footer = container.querySelector(".footer");
    expect(footer).toBeTruthy();
    expect(footer?.querySelector('[data-testid="transparency"]')).toBeTruthy();
  });

  it("omits the header chrome when no callbacks are provided (backward compat)", () => {
    const schema = parseFeatureSchema(extrudeDialogRaw);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
      />,
    );
    expect(container.querySelector(".header-actions")).toBeNull();
  });
});

// ─── boolean op + target editing on primitives ─────────────────
//
// A primitive (box, sphere, cylinder, …) can carry a boolean op +
// target (e.g. a cylinder that subtracts from another body). The
// derive emits an `op` enum (horizontal_enum) and a `targets`
// BODY multi_query on every primitive struct. The dialog must
// surface BOTH as editable controls — previously `op` was filtered
// out of the primitive schema, so a boolean primitive rendered with
// no way to see or change its op/target.

/** Cylinder schema as the Rust derive now emits it: geometry +
 *  the boolean `op` enum + a `targets` BODY multi_query + the
 *  modifier "noise" params the dialog filters out. */
const cylinderRaw = {
  type: "cylinder",
  params: [
    { key: "radius", name: "Radius mm", type: "length", required: true, unit: "mm" },
    { key: "half_height", name: "Half height mm", type: "length", required: true, unit: "mm" },
    { key: "center", name: "Center", type: "vec3", required: false },
    { key: "translate", name: "Translate", type: "vec3", required: false },
    {
      key: "op",
      name: "Op",
      type: "enum",
      required: false,
      ui_hint: "horizontal_enum",
      variants: [
        { key: "add", label: "Add" },
        { key: "new", label: "New" },
        { key: "remove", label: "Remove" },
        { key: "intersect", label: "Intersect" },
      ],
      default: "new",
    },
    {
      key: "targets",
      name: "Targets",
      type: "multi_query",
      required: false,
      entity_filter: "BODY",
    },
    { key: "blend_radius", name: "Blend radius", type: "f32", required: false, default: 0 },
    { key: "shell_thickness", name: "Shell thickness", type: "f32", required: false, default: 0 },
  ],
};

/** A feature with no boolean op at all (e.g. fillet) — the op
 *  selector + target picker must NOT appear. */
const filletRaw = {
  type: "fillet_edge",
  params: [
    { key: "edges", name: "Edges", type: "multi_query", required: true, entity_filter: "EDGE" },
    { key: "radius", name: "Radius mm", type: "length", required: true, unit: "mm" },
  ],
};

describe("FeatureDialog — primitive boolean op + target editing", () => {
  it("renders the op selector as a horizontal tab row for a primitive", () => {
    const schema = parseFeatureSchema(cylinderRaw);
    // `op` survives the primitive param filter.
    expect(schema.params.some((p) => p.key === "op")).toBe(true);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={{ op: "remove", targets: "qCreatedBy(1)" }}
        onChange={() => {}}
      />,
    );
    const tabRow = container.querySelector(".tab-row")!;
    expect(tabRow).toBeTruthy();
    const tabs = [...tabRow.querySelectorAll(".tab")];
    expect(tabs.map((t) => t.textContent)).toEqual([
      "Add",
      "New",
      "Remove",
      "Intersect",
    ]);
    // The supplied definition has op = "remove" → the Remove tab is active.
    const remove = tabs.find((t) => t.textContent === "Remove")!;
    expect(remove.classList.contains("is-active")).toBe(true);
  });

  it("renders the target picker showing the current query for a primitive", () => {
    const schema = parseFeatureSchema(cylinderRaw);
    expect(schema.params.some((p) => p.key === "targets")).toBe(true);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={{ op: "remove", targets: "qCreatedBy(1)" }}
        onChange={() => {}}
      />,
    );
    // The targets multi_query renders a chip for the current query. With no
    // feature-name map (no store in the test env) the chip shows the resolved
    // inner id "1" rather than the raw qCreatedBy(1) expression.
    const chips = container.querySelector(".query-chips");
    expect(chips).toBeTruthy();
    const chip = container.querySelector(".query-chip");
    expect(chip).toBeTruthy();
    expect(chip!.textContent).toContain("1");
  });

  it("still filters out the modifier noise params for primitives", () => {
    const schema = parseFeatureSchema(cylinderRaw);
    const keys = schema.params.map((p) => p.key);
    expect(keys).not.toContain("center");
    expect(keys).not.toContain("translate");
    expect(keys).not.toContain("blend_radius");
    expect(keys).not.toContain("shell_thickness");
    // …but keeps geometry, op and targets.
    expect(keys).toContain("radius");
    expect(keys).toContain("op");
    expect(keys).toContain("targets");
  });

  it("gates out op selector + target picker for a feature without a boolean op", () => {
    const schema = parseFeatureSchema(filletRaw);
    expect(schema.params.some((p) => p.uiHint === "horizontal_enum")).toBe(false);
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={buildDefaultDefinition(schema)}
        onChange={() => {}}
      />,
    );
    // No horizontal-enum tab row (that's the op selector) for fillet.
    expect(container.querySelector(".tab-row")).toBeNull();
  });

  it("round-trips an op change through onChange", () => {
    const schema = parseFeatureSchema(cylinderRaw);
    let def: Record<string, unknown> = { op: "new", targets: "" };
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={def}
        onChange={(next) => {
          def = next;
        }}
      />,
    );
    const tabRow = container.querySelector(".tab-row")!;
    const removeTab = [...tabRow.querySelectorAll(".tab")].find(
      (t) => t.textContent === "Remove",
    ) as HTMLButtonElement;
    act(() => {
      removeTab.click();
    });
    expect(def.op).toBe("remove");
    // targets is preserved across the op edit.
    expect(def.targets).toBe("");
  });

  it("round-trips a target edit through onChange", () => {
    const schema = parseFeatureSchema(cylinderRaw);
    let def: Record<string, unknown> = { op: "remove", targets: "qCreatedBy(1)" };
    const container = render(
      <FeatureDialog
        schema={schema}
        definition={def}
        onChange={(next) => {
          def = next;
        }}
      />,
    );
    // Remove the only target chip → targets becomes empty (the
    // missing-scope warning then applies kernel-side for Remove).
    const removeChip = container.querySelector(
      ".query-chip .chip-remove",
    ) as HTMLButtonElement | null;
    expect(removeChip).toBeTruthy();
    act(() => {
      removeChip!.click();
    });
    expect(def.targets).toBe("");
    expect(def.op).toBe("remove");
  });
});
