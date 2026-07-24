import { describe, it, expect } from "vitest";
import {
  allowedKinds,
  filterAllows,
  filterAllowsSketchLine,
  filterAllowsSketchPoint,
  filterAllowsSketchRegion,
  queryAcceptedByFilter,
  elementQueryString,
  mapPickToQuery,
  resolveElementClick,
  type ElementKind,
  type PickedElement,
} from "./elementQuery";

describe("allowedKinds", () => {
  it("undefined/empty filter allows every element kind", () => {
    const set = allowedKinds(undefined);
    expect(set.has("face")).toBe(true);
    expect(set.has("edge")).toBe(true);
    expect(set.has("vertex")).toBe(true);
    expect(allowedKinds("").size).toBe(3);
  });

  it("maps the schema FACE/EDGE/VERTEX tokens to their kinds (case-insensitive)", () => {
    expect([...allowedKinds("FACE")]).toEqual(["face"]);
    expect([...allowedKinds("edge")]).toEqual(["edge"]);
    expect([...allowedKinds("Vertex")]).toEqual(["vertex"]);
  });

  it("BODY (and unknown tokens) contribute no element kind", () => {
    expect(allowedKinds("BODY").size).toBe(0);
    expect(allowedKinds("solid").size).toBe(0);
  });

  it("accepts multiple kinds in one filter", () => {
    const set = allowedKinds("FACE,EDGE");
    expect(set.has("face")).toBe(true);
    expect(set.has("edge")).toBe(true);
    expect(set.has("vertex")).toBe(false);
  });
});

describe("filterAllows", () => {
  it("respects the filter per kind", () => {
    expect(filterAllows("face", "FACE")).toBe(true);
    expect(filterAllows("edge", "FACE")).toBe(false);
    expect(filterAllows("vertex", undefined)).toBe(true);
    expect(filterAllows("face", "BODY")).toBe(false);
  });
});

describe("elementQueryString", () => {
  it("builds the canonical element-query forms", () => {
    expect(elementQueryString({ kind: "face", idHash: "12345" })).toBe(
      "qFace(12345)",
    );
    expect(elementQueryString({ kind: "edge", idHash: "67" })).toBe("qEdge(67)");
    expect(elementQueryString({ kind: "vertex", idHash: "9" })).toBe(
      "qVertex(9)",
    );
  });
});

describe("mapPickToQuery", () => {
  const face: PickedElement = { kind: "face", idHash: "100" };
  const edge: PickedElement = { kind: "edge", idHash: "200" };
  const vertex: PickedElement = { kind: "vertex", idHash: "300" };

  it("undefined filter maps every kind to its element query", () => {
    expect(mapPickToQuery(face, undefined)).toBe("qFace(100)");
    expect(mapPickToQuery(edge, undefined)).toBe("qEdge(200)");
    expect(mapPickToQuery(vertex, undefined)).toBe("qVertex(300)");
  });

  it("a restrictive filter only maps its allowed kind, else null", () => {
    expect(mapPickToQuery(face, "FACE")).toBe("qFace(100)");
    expect(mapPickToQuery(edge, "FACE")).toBeNull();
    expect(mapPickToQuery(vertex, "EDGE")).toBeNull();
    expect(mapPickToQuery(edge, "EDGE")).toBe("qEdge(200)");
  });

  it("a BODY filter rejects every element kind", () => {
    for (const el of [face, edge, vertex]) {
      expect(mapPickToQuery(el, "BODY")).toBeNull();
    }
  });

  it("a multi-kind filter maps each of its kinds", () => {
    expect(mapPickToQuery(face, "FACE,VERTEX")).toBe("qFace(100)");
    expect(mapPickToQuery(vertex, "FACE,VERTEX")).toBe("qVertex(300)");
    expect(mapPickToQuery(edge, "FACE,VERTEX")).toBeNull();
  });

  // Exhaustiveness guard: every ElementKind round-trips.
  it("covers all element kinds", () => {
    const kinds: ElementKind[] = ["face", "edge", "vertex"];
    for (const kind of kinds) {
      const q = mapPickToQuery({ kind, idHash: "1" }, undefined);
      expect(q).toMatch(/^q(Face|Edge|Vertex)\(1\)$/);
    }
  });
});

describe("filterAllowsSketchLine", () => {
  it("only the SKETCHLINE token enables sketch lines", () => {
    expect(filterAllowsSketchLine("EDGE,SKETCHLINE")).toBe(true);
    expect(filterAllowsSketchLine("sketchline")).toBe(true);
    expect(filterAllowsSketchLine("EDGE")).toBe(false);
    expect(filterAllowsSketchLine("FACE")).toBe(false);
    expect(filterAllowsSketchLine(undefined)).toBe(false);
    expect(filterAllowsSketchLine("")).toBe(false);
  });
});

describe("filterAllowsSketchPoint (#404 Hole locations)", () => {
  it("only the SKETCHPOINT token enables sketch points", () => {
    expect(filterAllowsSketchPoint("VERTEX,SKETCHPOINT")).toBe(true);
    expect(filterAllowsSketchPoint("sketchpoint")).toBe(true);
    expect(filterAllowsSketchPoint("VERTEX")).toBe(false);
    expect(filterAllowsSketchPoint("SKETCHLINE")).toBe(false);
    expect(filterAllowsSketchPoint(undefined)).toBe(false);
    expect(filterAllowsSketchPoint("")).toBe(false);
  });
});

describe("queryAcceptedByFilter — SKETCHPOINT location picker (#404)", () => {
  const f = "VERTEX,SKETCHPOINT";
  it("accepts sketch-point and vertex location queries", () => {
    expect(queryAcceptedByFilter("qSketchPoint(5, 1)", f)).toBe(true);
    expect(queryAcceptedByFilter("qVertex(123)", f)).toBe(true);
  });
  it("rejects a sketch-point query when only VERTEX is allowed", () => {
    expect(queryAcceptedByFilter("qSketchPoint(5, 1)", "VERTEX")).toBe(false);
  });
  it("rejects faces and edges", () => {
    expect(queryAcceptedByFilter("qFace(1)", f)).toBe(false);
    expect(queryAcceptedByFilter("qEdge(1)", f)).toBe(false);
  });
});

describe("filterAllowsSketchRegion (ticket dhjn)", () => {
  it("only the SKETCHREGION token enables sketch regions", () => {
    expect(filterAllowsSketchRegion("SKETCHREGION")).toBe(true);
    expect(filterAllowsSketchRegion("sketchregion")).toBe(true);
    expect(filterAllowsSketchRegion("FACE")).toBe(false);
    expect(filterAllowsSketchRegion("SKETCHLINE")).toBe(false);
    expect(filterAllowsSketchRegion(undefined)).toBe(false);
    expect(filterAllowsSketchRegion("")).toBe(false);
  });
});

describe("queryAcceptedByFilter — SKETCHREGION profile picker (ticket dhjn)", () => {
  const f = "SKETCHREGION";
  it("accepts whole-sketch and single-region profile queries", () => {
    expect(queryAcceptedByFilter("qCreatedBy(5)", f)).toBe(true); // whole sketch
    expect(queryAcceptedByFilter("qAllRegions(5)", f)).toBe(true);
    expect(queryAcceptedByFilter("qRegion(5, 0)", f)).toBe(true);
    expect(queryAcceptedByFilter("qRegions(5, [0, 1])", f)).toBe(true);
  });
  it("rejects datum planes, edges and bodies", () => {
    expect(queryAcceptedByFilter("qCreatedBy(origin_plane_xy)", f)).toBe(false);
    expect(queryAcceptedByFilter("qEdge(1)", f)).toBe(false);
    expect(queryAcceptedByFilter("qVertex(1)", f)).toBe(false);
  });
});

describe("queryAcceptedByFilter — wall/composed queries (Schritt 3)", () => {
  it("qWall and composed forms are FACE-level", () => {
    for (const q of [
      "qWall(3, 7)",
      "qIntersection(qCreatedBy(2, WALL), qCreatedBy(9, BLEND))",
      "qAdjacentTo(qWall(2, 3), qCreatedBy(9, BLEND))",
    ]) {
      expect(queryAcceptedByFilter(q, "FACE")).toBe(true);
      expect(queryAcceptedByFilter(q, "EDGE")).toBe(false);
      expect(queryAcceptedByFilter(q, "BODY")).toBe(false);
    }
  });
});

describe("queryAcceptedByFilter — role-filtered qCreatedBy(op, ROLE)", () => {
  it("is a FACE-level query: accepted by FACE, rejected elsewhere", () => {
    for (const role of [
      "WALL",
      "CAP_TOP",
      "CAP_BOTTOM",
      "INNER_WALL",
      "INNER_CAP_TOP",
      "INNER_CAP_BOTTOM",
      "BLEND",
    ]) {
      const q = `qCreatedBy(3, ${role})`;
      expect(queryAcceptedByFilter(q, "FACE")).toBe(true);
      expect(queryAcceptedByFilter(q, "BODY")).toBe(false);
      expect(queryAcceptedByFilter(q, "SKETCHREGION")).toBe(false);
      expect(queryAcceptedByFilter(q, "EDGE")).toBe(false);
    }
  });
});

describe("queryAcceptedByFilter (axis picker hardening)", () => {
  it("no filter accepts every query", () => {
    for (const q of ["qEdge(1)", "qFace(1)", "qCreatedBy(2)", "qCreatedBy(origin_plane_xy)"]) {
      expect(queryAcceptedByFilter(q, undefined)).toBe(true);
      expect(queryAcceptedByFilter(q, "")).toBe(true);
    }
  });

  it("an EDGE,SKETCHLINE axis filter accepts only edges and sketch lines", () => {
    const f = "EDGE,SKETCHLINE";
    expect(queryAcceptedByFilter("qEdge(123)", f)).toBe(true);
    expect(queryAcceptedByFilter("qEdgeBetween(1, 2)", f)).toBe(true);
    expect(queryAcceptedByFilter("qSketchLine(1, 10)", f)).toBe(true);
    // The reported leak: a datum plane must be rejected.
    expect(queryAcceptedByFilter("qCreatedBy(origin_plane_xy)", f)).toBe(false);
    expect(queryAcceptedByFilter("qFace(123)", f)).toBe(false);
    expect(queryAcceptedByFilter("qVertex(123)", f)).toBe(false);
    expect(queryAcceptedByFilter("qCreatedBy(7)", f)).toBe(false);
    expect(queryAcceptedByFilter("qAllRegions(3)", f)).toBe(false);
  });

  it("a FACE picker keeps datums, regions and face-level queries", () => {
    const f = "FACE";
    expect(queryAcceptedByFilter("qFace(1)", f)).toBe(true);
    expect(queryAcceptedByFilter("qCreatedBy(origin_plane_xy)", f)).toBe(true);
    expect(queryAcceptedByFilter("qCreatedBy(5)", f)).toBe(true); // sketch region
    expect(queryAcceptedByFilter("qAllRegions(5)", f)).toBe(true);
    expect(queryAcceptedByFilter("qEdge(1)", f)).toBe(false);
  });

  it("a BODY picker accepts bodies but not datum planes or edges", () => {
    const f = "BODY";
    expect(queryAcceptedByFilter("qCreatedBy(7)", f)).toBe(true);
    expect(queryAcceptedByFilter("qCreatedBy(origin_plane_xy)", f)).toBe(false);
    expect(queryAcceptedByFilter("qEdge(1)", f)).toBe(false);
  });
});

describe("resolveElementClick (ticket evq2)", () => {
  const faceA = { kind: "face", idHash: "100" } as const;
  const faceB = { kind: "face", idHash: "101" } as const;
  const edge = { kind: "edge", idHash: "200" } as const;
  const vertex = { kind: "vertex", idHash: "300" } as const;

  it("additive click always toggles, even on the selected face", () => {
    expect(resolveElementClick(faceA, faceA, true)).toBe("toggle");
    expect(resolveElementClick(edge, null, true)).toBe("toggle");
  });

  it("plain click on an unselected element selects it", () => {
    expect(resolveElementClick(faceA, null, false)).toBe("select");
    expect(resolveElementClick(faceA, faceB, false)).toBe("select");
    expect(resolveElementClick(faceA, edge, false)).toBe("select");
  });

  it("plain click on the already-selected face escalates to the body", () => {
    expect(resolveElementClick(faceA, faceA, false)).toBe("escalate-body");
  });

  it("re-clicking a selected edge or vertex does NOT escalate", () => {
    expect(resolveElementClick(edge, edge, false)).toBe("select");
    expect(resolveElementClick(vertex, vertex, false)).toBe("select");
  });

  it("presets can turn the body escalation off (docs/UX-PRESETS.md)", () => {
    expect(resolveElementClick(faceA, faceA, false, false)).toBe("select");
    expect(resolveElementClick(faceA, faceA, true, false)).toBe("toggle");
  });
});
