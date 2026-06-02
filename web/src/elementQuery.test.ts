import { describe, it, expect } from "vitest";
import {
  allowedKinds,
  filterAllows,
  elementQueryString,
  mapPickToQuery,
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
