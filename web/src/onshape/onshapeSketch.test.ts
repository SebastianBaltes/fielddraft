import { describe, expect, it } from "vitest";
import {
  translateOnshapeSketch,
  type OnshapeSketchFeature,
} from "./onshapeSketch";
import { buildDraftSketchState } from "../sketchSession";

/**
 * Fixtures mirror the shape the `.../partstudios/.../features` endpoint
 * returns for a `BTMSketch-151`. Coordinates are METRES (Onshape internal);
 * the translator scales to millimetres.
 */

describe("translateOnshapeSketch — geometry", () => {
  it("translates a line segment (metres → mm, params → endpoints)", () => {
    const sketch: OnshapeSketchFeature = {
      btType: "BTMSketch-151",
      featureType: "newSketch",
      geometry: [
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "Line1",
          startPointId: "Line1.start",
          endPointId: "Line1.end",
          startParam: 0,
          endParam: 0.05, // 50 mm along +x
          geometry: { btType: "BTCurveGeometryLine-117", pntX: 0.01, pntY: 0.02, dirX: 1, dirY: 0 },
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    expect(subCalls).toHaveLength(1);
    expect(subCalls[0]).toEqual({
      type: "sketch_line",
      params: { start: [10, 20], end: [60, 20] },
    });
  });

  it("translates a full circle", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [
        {
          btType: "BTMSketchCurve-4",
          entityId: "Circle1",
          geometry: { btType: "BTCurveGeometryCircle-115", xCenter: 0.03, yCenter: 0, radius: 0.01 },
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    expect(subCalls).toEqual([
      { type: "sketch_circle", params: { center: [30, 0], radius: 10 } },
    ]);
  });

  it("derives an arc's angles from its start/end point coordinates", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [
        // The arc's endpoints as sketch points (metres).
        { btType: "BTMSketchPoint-158", entityId: "Arc1.start", x: 0.01, y: 0 },
        { btType: "BTMSketchPoint-158", entityId: "Arc1.end", x: 0, y: 0.01 },
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "Arc1",
          startPointId: "Arc1.start",
          endPointId: "Arc1.end",
          geometry: { btType: "BTCurveGeometryCircle-115", xCenter: 0, yCenter: 0, radius: 0.01 },
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    // The two endpoints are curve endpoints → NOT emitted as standalone points.
    expect(subCalls).toHaveLength(1);
    const p = subCalls[0].params as Record<string, number | number[]>;
    expect(subCalls[0].type).toBe("sketch_arc");
    expect(p.center).toEqual([0, 0]);
    expect(p.radius).toBe(10);
    // Start at angle 0 (+x), CCW quarter turn to +y (π/2).
    expect(p.start_angle as number).toBeCloseTo(0, 6);
    expect(p.sweep_angle as number).toBeCloseTo(Math.PI / 2, 6);
  });

  it("emits a standalone point but not curve endpoints", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [
        { btType: "BTMSketchPoint-158", entityId: "P1", x: 0.005, y: 0.005 },
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "L1",
          startPointId: "L1.start",
          endPointId: "L1.end",
          startParam: 0,
          endParam: 0.02,
          geometry: { btType: "BTCurveGeometryLine-117", pntX: 0, pntY: 0, dirX: 0, dirY: 1 },
        },
        // A point that IS a curve endpoint — must NOT be emitted standalone.
        { btType: "BTMSketchPoint-158", entityId: "L1.start", x: 0, y: 0 },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    const points = subCalls.filter((c) => c.type === "sketch_point");
    expect(points).toEqual([{ type: "sketch_point", params: { at: [5, 5] } }]);
  });

  it("carries the construction flag", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [
        {
          btType: "BTMSketchCurve-4",
          entityId: "C",
          isConstruction: true,
          geometry: { btType: "BTCurveGeometryCircle-115", xCenter: 0, yCenter: 0, radius: 0.005 },
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    expect(subCalls[0].params.construction).toBe(true);
  });

  it("reports unhandled entity types instead of dropping them silently", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "S1",
          geometry: { btType: "BTCurveGeometrySpline-2410" },
        },
      ],
    };
    const { subCalls, diagnostics } = translateOnshapeSketch(sketch);
    expect(subCalls).toHaveLength(0);
    expect(diagnostics.unhandled["BTCurveGeometrySpline-2410"]).toBe(1);
  });
});

describe("translateOnshapeSketch — constraints", () => {
  it("maps a coincident between two line endpoints to FieldDraft ids", () => {
    // Two lines; a coincident tying L1's end to L2's start. In FieldDraft the
    // allocation is L1:[1,2,3], L2:[4,5,6] → L1.end=2, L2.start=4.
    const sketch: OnshapeSketchFeature = {
      geometry: [
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "L1",
          startPointId: "L1.start",
          endPointId: "L1.end",
          startParam: 0,
          endParam: 0.01,
          geometry: { btType: "BTCurveGeometryLine-117", pntX: 0, pntY: 0, dirX: 1, dirY: 0 },
        },
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "L2",
          startPointId: "L2.start",
          endPointId: "L2.end",
          startParam: 0,
          endParam: 0.01,
          geometry: { btType: "BTCurveGeometryLine-117", pntX: 0.01, pntY: 0, dirX: 0, dirY: 1 },
        },
      ],
      constraints: [
        {
          btType: "BTMSketchConstraint-2",
          constraintType: "COINCIDENT",
          parameters: [
            { btType: "BTMParameterString-149", parameterId: "localFirst", value: "L1.end" },
            { btType: "BTMParameterString-149", parameterId: "localSecond", value: "L2.start" },
          ],
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    const coincident = subCalls.find((c) => c.type === "sketch_constraint_coincident");
    expect(coincident?.params).toEqual({ a: 2, b: 4 });
  });

  it("maps horizontal onto the line's FieldDraft id", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "L1",
          startPointId: "L1.start",
          endPointId: "L1.end",
          startParam: 0,
          endParam: 0.01,
          geometry: { btType: "BTCurveGeometryLine-117", pntX: 0, pntY: 0, dirX: 1, dirY: 0 },
        },
      ],
      constraints: [
        {
          btType: "BTMSketchConstraint-2",
          constraintType: "HORIZONTAL",
          parameters: [
            { btType: "BTMParameterString-149", parameterId: "localFirst", value: "L1" },
          ],
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    // L1 line id = 3 (start=1, end=2, line=3).
    expect(subCalls.find((c) => c.type === "sketch_constraint_horizontal")?.params).toEqual({
      line: 3,
    });
  });

  it("reports an unhandled dimension constraint", () => {
    const sketch: OnshapeSketchFeature = {
      geometry: [],
      constraints: [{ btType: "BTMSketchConstraint-2", constraintType: "DISTANCE" }],
    };
    const { diagnostics } = translateOnshapeSketch(sketch);
    expect(diagnostics.unhandled["constraint:DISTANCE"]).toBe(1);
  });
});

describe("translateOnshapeSketch — FieldDraft id allocation matches the kernel", () => {
  it("produces sub-calls whose entity ids line up with buildDraftSketchState", () => {
    // A point, a line, a circle — the translator's internal id map must match
    // what buildDraftSketchState actually allocates for the emitted sub-calls.
    const sketch: OnshapeSketchFeature = {
      geometry: [
        { btType: "BTMSketchPoint-158", entityId: "P", x: 0, y: 0.05 },
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "L",
          startPointId: "L.s",
          endPointId: "L.e",
          startParam: 0,
          endParam: 0.02,
          geometry: { btType: "BTCurveGeometryLine-117", pntX: 0, pntY: 0, dirX: 1, dirY: 0 },
        },
        {
          btType: "BTMSketchCurve-4",
          entityId: "C",
          geometry: { btType: "BTCurveGeometryCircle-115", xCenter: 0.03, yCenter: 0, radius: 0.01 },
        },
      ],
    };
    const { subCalls } = translateOnshapeSketch(sketch);
    // point=1; line start=2,end=3,line=4; circle centre=5,circle=6 → next=7.
    const state = buildDraftSketchState(subCalls);
    expect(state.next_entity_id).toBe(7);
    // The circle entity is the last one allocated.
    const circle = state.entities.find((e) => e.kind === "circle");
    expect(circle?.id).toBe(6);
  });
});

describe("translateOnshapeSketch — dimensions and curve points", () => {
  /** A line plus a tangent fillet arc — the shape every rounded corner has. */
  const roundedCorner: OnshapeSketchFeature = {
    btType: "BTMSketch-151",
    featureType: "newSketch",
    geometry: [
      {
        btType: "BTMSketchCurveSegment-155",
        entityId: "Line1",
        startPointId: "Line1.start",
        endPointId: "Line1.end",
        startParam: 0,
        endParam: 0.05,
        geometry: { btType: "BTCurveGeometryLine-117", pntX: 0, pntY: 0, dirX: 1, dirY: 0 },
      },
      { btType: "BTMSketchPoint-158", entityId: "Arc1.start", x: 0.05, y: 0 },
      { btType: "BTMSketchPoint-158", entityId: "Arc1.end", x: 0.06, y: 0.01 },
      {
        btType: "BTMSketchCurveSegment-155",
        entityId: "Arc1",
        startPointId: "Arc1.start",
        endPointId: "Arc1.end",
        centerId: "Arc1.center",
        geometry: {
          btType: "BTCurveGeometryCircle-115",
          xCenter: 0.05,
          yCenter: 0.01,
          radius: 0.01,
        },
      },
    ],
  };

  it("registers a curve's own points, so a fillet coincidence resolves", () => {
    // Onshape ties the arc's ENDPOINT to the line's endpoint. Without the
    // arc's point ids in the map this constraint was silently dropped and
    // the rounded corner fell apart under the solver.
    const sketch: OnshapeSketchFeature = {
      ...roundedCorner,
      constraints: [
        {
          btType: "BTMSketchConstraint-2",
          constraintType: "COINCIDENT",
          parameters: [
            { btType: "BTMParameterString-149", parameterId: "localFirst", value: "Arc1.start" },
            { btType: "BTMParameterString-149", parameterId: "localSecond", value: "Line1.end" },
          ],
        },
      ],
    };
    const { subCalls, diagnostics } = translateOnshapeSketch(sketch);
    expect(diagnostics.unhandled).toEqual({});
    // line: start=1, end=2, line=3 → arc: centre=4, start=5, end=6, arc=7.
    expect(subCalls).toContainEqual({
      type: "sketch_constraint_coincident",
      params: { a: 5, b: 2 },
    });
  });

  it("translates the dimension constraints with their values", () => {
    const dim = (
      constraintType: string,
      parameters: { btType: string; parameterId: string; value?: string; expression?: string }[],
    ) => ({ btType: "BTMSketchConstraint-2", constraintType, parameters });
    const ref = (parameterId: string, value: string) => ({
      btType: "BTMParameterString-149",
      parameterId,
      value,
    });
    const qty = (parameterId: string, expression: string) => ({
      btType: "BTMParameterQuantity-147",
      parameterId,
      expression,
    });

    const sketch: OnshapeSketchFeature = {
      ...roundedCorner,
      constraints: [
        dim("RADIUS", [ref("localFirst", "Arc1"), qty("length", "10 mm")]),
        dim("LENGTH", [ref("localFirst", "Line1"), qty("length", "5 cm")]),
        dim("DISTANCE", [
          ref("localFirst", "Line1.start"),
          ref("localSecond", "Arc1.end"),
          qty("length", "1 in"),
        ]),
        dim("ANGLE", [ref("localFirst", "Line1"), ref("localSecond", "Arc1"), qty("angle", "45 deg")]),
      ],
    };
    const { subCalls, diagnostics } = translateOnshapeSketch(sketch);
    expect(diagnostics.unhandled).toEqual({});
    // Radius on the arc entity (id 7).
    expect(subCalls).toContainEqual({
      type: "sketch_constraint_radius",
      params: { entity: 7, value_mm: 10 },
    });
    // LENGTH names the LINE (3) but measures between its endpoints (1, 2).
    expect(subCalls).toContainEqual({
      type: "sketch_constraint_distance",
      params: { entity_a: 1, entity_b: 2, value_mm: 50 },
    });
    // Units convert: 1 in = 25.4 mm.
    expect(subCalls).toContainEqual({
      type: "sketch_constraint_distance",
      params: { entity_a: 1, entity_b: 6, value_mm: 25.4 },
    });
    expect(subCalls).toContainEqual({
      type: "sketch_constraint_angle",
      params: { line_a: 3, line_b: 7, value_deg: 45 },
    });
  });

  it("keeps enum params out of the reference slots", () => {
    // `sketchToolType=FILLET` is an enum, not an entity ref — reading it as
    // one would fail the lookup and drop the whole constraint.
    const sketch: OnshapeSketchFeature = {
      ...roundedCorner,
      constraints: [
        {
          btType: "BTMSketchConstraint-2",
          constraintType: "TANGENT",
          parameters: [
            { btType: "BTMParameterString-149", parameterId: "localFirst", value: "Line1" },
            { btType: "BTMParameterString-149", parameterId: "localSecond", value: "Arc1" },
            { btType: "BTMParameterEnum-145", parameterId: "sketchToolType", value: "FILLET" },
          ],
        },
      ],
    };
    const { subCalls, diagnostics } = translateOnshapeSketch(sketch);
    expect(diagnostics.unhandled).toEqual({});
    expect(subCalls).toContainEqual({
      type: "sketch_constraint_tangent",
      params: { line: 3, curve: 7 },
    });
  });
});
