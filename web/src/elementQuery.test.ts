import { describe, it, expect } from "vitest";
import {
  allowedKinds,
  filterAllows,
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
