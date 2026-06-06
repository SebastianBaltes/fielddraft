import { describe, it, expect } from "vitest";
import {
  svgToSubCalls,
  dxfToSubCalls,
  importVectorToSubCalls,
  type SubCall,
} from "./importVector";

/** Sub-calls of a given type. */
function ofType(calls: SubCall[], type: string): SubCall[] {
  return calls.filter((c) => c.type === type);
}

describe("svgToSubCalls", () => {
  it("maps <line> to one sketch_line with y negated", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><line x1="0" y1="0" x2="10" y2="20"/></svg>`,
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_line");
    expect(calls[0].params.start).toEqual([0, -0]);
    // y is flipped: SVG y-down 20 → sketch y-up -20.
    expect(calls[0].params.end).toEqual([10, -20]);
  });

  it("maps <rect> to 4 lines (closed loop)", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="5"/></svg>`,
    );
    expect(ofType(calls, "sketch_line")).toHaveLength(4);
    // First corner at (0,0) flipped → (0,-0); opposite corner present.
    const ends = calls.map((c) => c.params.end);
    expect(ends).toContainEqual([10, -5]);
  });

  it("maps <circle> to sketch_circle (centre y negated)", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><circle cx="3" cy="4" r="7"/></svg>`,
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_circle");
    expect(calls[0].params.center).toEqual([3, -4]);
    expect(calls[0].params.radius).toBe(7);
  });

  it("maps <ellipse> to sketch_ellipse", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><ellipse cx="1" cy="2" rx="5" ry="3"/></svg>`,
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_ellipse");
    expect(calls[0].params.center).toEqual([1, -2]);
    expect(calls[0].params.radius_x).toBe(5);
    expect(calls[0].params.radius_y).toBe(3);
  });

  it("maps <polyline> to an open line chain", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><polyline points="0,0 10,0 10,10"/></svg>`,
    );
    // 3 points → 2 segments, NOT closed.
    expect(ofType(calls, "sketch_line")).toHaveLength(2);
  });

  it("maps <polygon> to a closed line chain", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><polygon points="0,0 10,0 10,10"/></svg>`,
    );
    // 3 points → 2 segments + 1 closing = 3.
    expect(ofType(calls, "sketch_line")).toHaveLength(3);
  });

  it("maps a path M/L/Z to 3 closing lines", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 L10 0 L10 10 Z"/></svg>`,
    );
    expect(ofType(calls, "sketch_line")).toHaveLength(3);
    // Z closes back to the subpath start (0,0) → (0,-0).
    const last = calls[calls.length - 1];
    expect(last.params.end).toEqual([0, -0]);
  });

  it("tessellates a path C cubic into a multi-segment line chain", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 C0 10 10 10 10 0"/></svg>`,
      { curveSteps: 16 },
    );
    const lines = ofType(calls, "sketch_line");
    expect(lines.length).toBeGreaterThan(1);
    expect(lines.length).toBe(16);
    // Endpoints match the cubic's P0 (0,0→0,-0) and P3 (10,0→10,-0).
    expect(lines[0].params.start).toEqual([0, -0]);
    expect(lines[lines.length - 1].params.end).toEqual([10, -0]);
  });

  it("handles relative path commands (lowercase)", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 l10 0 l0 10 z"/></svg>`,
    );
    expect(ofType(calls, "sketch_line")).toHaveLength(3);
  });

  it("tessellates an elliptical arc A command into lines", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 A5 5 0 0 1 10 0"/></svg>`,
      { curveSteps: 8 },
    );
    expect(ofType(calls, "sketch_line").length).toBeGreaterThan(1);
  });

  it("traverses nested <g> groups", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><g><g><circle cx="0" cy="0" r="2"/></g></g></svg>`,
    );
    expect(ofType(calls, "sketch_circle")).toHaveLength(1);
  });

  it("applies a root translate transform", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><g transform="translate(100,50)"><circle cx="0" cy="0" r="1"/></g></svg>`,
    );
    expect(calls[0].params.center).toEqual([100, -50]);
  });

  it("returns [] for empty / garbage input", () => {
    expect(svgToSubCalls("")).toEqual([]);
    expect(svgToSubCalls("not xml at all <<<")).toEqual([]);
    expect(svgToSubCalls("<html><body>nope</body></html>")).toEqual([]);
  });
});

describe("dxfToSubCalls", () => {
  const wrap = (entities: string) =>
    `0\nSECTION\n2\nENTITIES\n${entities}0\nENDSEC\n0\nEOF\n`;

  it("parses LINE without y-flip", () => {
    const calls = dxfToSubCalls(
      wrap(`0\nLINE\n10\n0\n20\n0\n11\n10\n21\n20\n`),
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_line");
    expect(calls[0].params.start).toEqual([0, 0]);
    // DXF is y-up already → y NOT negated.
    expect(calls[0].params.end).toEqual([10, 20]);
  });

  it("parses CIRCLE", () => {
    const calls = dxfToSubCalls(wrap(`0\nCIRCLE\n10\n3\n20\n4\n40\n7\n`));
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_circle");
    expect(calls[0].params.center).toEqual([3, 4]);
    expect(calls[0].params.radius).toBe(7);
  });

  it("parses ARC, converting degrees to radians + sweep", () => {
    const calls = dxfToSubCalls(
      wrap(`0\nARC\n10\n0\n20\n0\n40\n5\n50\n0\n51\n90\n`),
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_arc");
    expect(calls[0].params.start_angle).toBeCloseTo(0, 9);
    expect(calls[0].params.sweep_angle).toBeCloseTo(Math.PI / 2, 9);
    expect(calls[0].params.radius).toBe(5);
  });

  it("normalises an ARC sweep that wraps past 0", () => {
    // 270° → 90° CCW = 180° sweep.
    const calls = dxfToSubCalls(
      wrap(`0\nARC\n10\n0\n20\n0\n40\n5\n50\n270\n51\n90\n`),
    );
    expect(calls[0].params.sweep_angle).toBeCloseTo(Math.PI, 9);
  });

  it("parses a closed LWPOLYLINE", () => {
    const calls = dxfToSubCalls(
      wrap(`0\nLWPOLYLINE\n70\n1\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n`),
    );
    // 3 vertices, closed → 3 lines.
    expect(ofType(calls, "sketch_line")).toHaveLength(3);
  });

  it("parses an open LWPOLYLINE", () => {
    const calls = dxfToSubCalls(
      wrap(`0\nLWPOLYLINE\n70\n0\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n`),
    );
    // 3 vertices, open → 2 lines.
    expect(ofType(calls, "sketch_line")).toHaveLength(2);
  });

  it("parses ELLIPSE (major axis vector → radius_x + rotation)", () => {
    const calls = dxfToSubCalls(
      wrap(`0\nELLIPSE\n10\n0\n20\n0\n11\n10\n21\n0\n40\n0.5\n`),
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_ellipse");
    expect(calls[0].params.radius_x).toBeCloseTo(10, 9);
    expect(calls[0].params.radius_y).toBeCloseTo(5, 9);
    expect(calls[0].params.rotation).toBeCloseTo(0, 9);
  });

  it("skips unknown entities gracefully", () => {
    const calls = dxfToSubCalls(
      wrap(`0\nMTEXT\n10\n0\n20\n0\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n`),
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_line");
  });

  it("parses legacy POLYLINE / VERTEX entities", () => {
    const calls = dxfToSubCalls(
      wrap(
        `0\nPOLYLINE\n70\n1\n0\nVERTEX\n10\n0\n20\n0\n0\nVERTEX\n10\n10\n20\n0\n0\nVERTEX\n10\n10\n20\n10\n0\nSEQEND\n`,
      ),
    );
    // 3 vertices, closed → 3 lines.
    expect(ofType(calls, "sketch_line")).toHaveLength(3);
  });

  it("returns [] for empty input", () => {
    expect(dxfToSubCalls("")).toEqual([]);
    expect(dxfToSubCalls("0\nSECTION\n2\nHEADER\n0\nENDSEC\n0\nEOF\n")).toEqual(
      [],
    );
  });
});

describe("importVectorToSubCalls dispatcher", () => {
  it("dispatches by .svg extension", () => {
    const calls = importVectorToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><circle cx="0" cy="0" r="1"/></svg>`,
      "drawing.svg",
    );
    expect(calls[0].type).toBe("sketch_circle");
  });

  it("dispatches by .dxf extension", () => {
    const calls = importVectorToSubCalls(
      `0\nSECTION\n2\nENTITIES\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDSEC\n0\nEOF\n`,
      "drawing.dxf",
    );
    expect(calls[0].type).toBe("sketch_line");
  });

  it("sniffs SVG when extension is unknown", () => {
    const calls = importVectorToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><circle cx="0" cy="0" r="1"/></svg>`,
      "drawing.txt",
    );
    expect(calls[0].type).toBe("sketch_circle");
  });

  it("sniffs DXF when extension is unknown", () => {
    const calls = importVectorToSubCalls(
      `0\nSECTION\n2\nENTITIES\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDSEC\n0\nEOF\n`,
      "drawing.bin",
    );
    expect(calls[0].type).toBe("sketch_line");
  });

  it("returns [] for unrecognised content", () => {
    expect(importVectorToSubCalls("hello world", "x.txt")).toEqual([]);
  });
});
