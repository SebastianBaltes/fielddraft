import { describe, it, expect, beforeEach } from "vitest";
import {
  setHoveredElementOfKind,
  getHoveredElement,
  getHoveredElementOfKind,
} from "./brepPickBridge";

const face = { kind: "face", idHash: "1" } as const;
const edge = { kind: "edge", idHash: "2" } as const;
const vertex = { kind: "vertex", idHash: "3" } as const;

beforeEach(() => {
  setHoveredElementOfKind("face", null);
  setHoveredElementOfKind("edge", null);
  setHoveredElementOfKind("vertex", null);
});

describe("getHoveredElement merge priority (ticket evq2)", () => {
  it("empty slots yield null", () => {
    expect(getHoveredElement()).toBeNull();
  });

  it("the most specific element wins: vertex > edge > face", () => {
    setHoveredElementOfKind("face", face);
    expect(getHoveredElement()).toEqual(face);
    setHoveredElementOfKind("edge", edge);
    expect(getHoveredElement()).toEqual(edge);
    setHoveredElementOfKind("vertex", vertex);
    expect(getHoveredElement()).toEqual(vertex);
  });

  it("an edge near the cursor beats the face under it", () => {
    setHoveredElementOfKind("face", face);
    setHoveredElementOfKind("edge", edge);
    expect(getHoveredElement()).toEqual(edge);
  });

  it("per-kind access ignores the merge priority", () => {
    setHoveredElementOfKind("face", face);
    setHoveredElementOfKind("vertex", vertex);
    expect(getHoveredElementOfKind("face")).toEqual(face);
  });

  it("publishing a mismatched kind clears the slot", () => {
    setHoveredElementOfKind("face", face);
    setHoveredElementOfKind("face", { kind: "edge", idHash: "9" });
    expect(getHoveredElementOfKind("face")).toBeNull();
  });
});
