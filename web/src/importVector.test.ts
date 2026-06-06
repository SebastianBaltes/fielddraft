import { describe, it, expect } from "vitest";
import {
  svgToSubCalls,
  dxfToSubCalls,
  importVectorToSubCalls,
  type SubCall,
  type PathContour,
} from "./importVector";

/** Sub-calls of a given type. */
function ofType(calls: SubCall[], type: string): SubCall[] {
  return calls.filter((c) => c.type === type);
}

/** Walk a sketch_path contour at global parameter `u` ∈ [0,1] across all
 *  its segments, evaluating the line / quad at the local parameter. Used
 *  to spot-check that the reconstructed curve matches the analytic one. */
function sampleSketchPathAt(c: PathContour, u: number): [number, number] {
  const n = c.segments.length;
  const idx = Math.min(n - 1, Math.floor(u * n));
  const t = u * n - idx;
  let prev = c.start;
  for (let i = 0; i < idx; i++) {
    prev = (c.segments[i] as { to: [number, number] }).to;
  }
  const seg = c.segments[idx];
  if (seg.kind === "line") {
    return [
      prev[0] + (seg.to[0] - prev[0]) * t,
      prev[1] + (seg.to[1] - prev[1]) * t,
    ];
  }
  const mt = 1 - t;
  return [
    mt * mt * prev[0] + 2 * mt * t * seg.ctrl[0] + t * t * seg.to[0],
    mt * mt * prev[1] + 2 * mt * t * seg.ctrl[1] + t * t * seg.to[1],
  ];
}

describe("svgToSubCalls", () => {
  it("maps <line> to one sketch_line with y negated", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><line x1="0" y1="0" x2="10" y2="20"/></svg>`,
    );
    expect(calls).toHaveLength(1);
    expect(calls[0].type).toBe("sketch_line");
    expect(calls[0].params.start).toEqual([0, 0]);
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

  it("maps <rect rx ry> to a closed sketch_path with quad corner arcs", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="20" height="10" rx="3" ry="2"/></svg>`,
    );
    // No sharp line chain; exactly one rounded-rect sketch_path.
    expect(ofType(calls, "sketch_line")).toHaveLength(0);
    const paths = ofType(calls, "sketch_path");
    expect(paths).toHaveLength(1);
    const c = (paths[0].params.contours as PathContour[])[0];
    expect(c.closed).toBe(true);
    // 4 straight edges (line segments) + 4 corner arcs (≥1 quad each).
    const lines = c.segments.filter((s) => s.kind === "line");
    const quads = c.segments.filter((s) => s.kind === "quad");
    expect(lines).toHaveLength(4);
    expect(quads.length).toBeGreaterThanOrEqual(4);
  });

  it("a plain <rect> still emits the sharp 4-line chain (no rx/ry)", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="5"/></svg>`,
    );
    expect(ofType(calls, "sketch_path")).toHaveLength(0);
    expect(ofType(calls, "sketch_line")).toHaveLength(4);
  });

  it("clamps rx to w/2 for a rounded rect", () => {
    // rx=999 must clamp to w/2=10 → corner arcs span the full half-width,
    // so the top straight edge collapses to a degenerate (zero-length)
    // line between the two top corners at x=10.
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="20" height="20" rx="999"/></svg>`,
    );
    const c = (
      ofType(calls, "sketch_path")[0].params.contours as PathContour[]
    )[0];
    // Start = (x+rx, y) flipped = (10, 0). Sane finite coords (no NaN).
    expect(c.start[0]).toBeCloseTo(10, 6);
    expect(Number.isFinite(c.start[0]) && Number.isFinite(c.start[1])).toBe(
      true,
    );
  });

  it("mirrors one-of rx/ry to both (rx only → ry=rx)", () => {
    // rx present, ry absent → ry := rx. A circular-corner rounded rect.
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="20" height="20" rx="4"/></svg>`,
    );
    const c = (
      ofType(calls, "sketch_path")[0].params.contours as PathContour[]
    )[0];
    // start = (x+rx, y) = (4, 0) flipped → (4, -0).
    expect(c.start[0]).toBeCloseTo(4, 6);
    expect(
      c.segments.filter((s) => s.kind === "quad").length,
    ).toBeGreaterThanOrEqual(4);
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

  it("maps a path M/L/Z to one closed sketch_path contour of lines", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 L10 0 L10 10 Z"/></svg>`,
    );
    const paths = ofType(calls, "sketch_path");
    expect(paths).toHaveLength(1);
    const contours = paths[0].params.contours as PathContour[];
    expect(contours).toHaveLength(1);
    expect(contours[0].closed).toBe(true);
    expect(contours[0].start).toEqual([0, 0]);
    // Two explicit L segments, all lines (Z is implicit in `closed`).
    expect(contours[0].segments).toHaveLength(2);
    expect(contours[0].segments.every((s) => s.kind === "line")).toBe(true);
    expect((contours[0].segments[1] as { to: number[] }).to).toEqual([10, -10]);
  });

  it("preserves a path C cubic as quad-bezier segments (not a line chain)", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 C0 10 10 10 10 0"/></svg>`,
    );
    const paths = ofType(calls, "sketch_path");
    expect(paths).toHaveLength(1);
    expect(ofType(calls, "sketch_line")).toHaveLength(0);
    const c = (paths[0].params.contours as PathContour[])[0];
    // Cubic → a small number of quad segments (curve preserved as curve).
    expect(c.segments.length).toBeGreaterThanOrEqual(1);
    expect(c.segments.length).toBeLessThan(8);
    expect(c.segments.every((s) => s.kind === "quad")).toBe(true);
    // Endpoints match the cubic P0 (0,0→0,-0) and P3 (10,0→10,-0).
    expect(c.start).toEqual([0, 0]);
    const last = c.segments[c.segments.length - 1] as { to: number[] };
    expect(last.to[0]).toBeCloseTo(10, 6);
    expect(last.to[1]).toBeCloseTo(-0, 6);
    // The reconstructed curve midpoint matches the analytic cubic point.
    // Cubic at t=0.5 (y-down): (5, 7.5) → sketch (5, -7.5).
    const mid = sampleSketchPathAt(c, 0.5);
    expect(mid[0]).toBeCloseTo(5, 2);
    expect(mid[1]).toBeCloseTo(-7.5, 2);
  });

  it("maps a quadratic Q exactly to one quad segment", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 Q5 10 10 0"/></svg>`,
    );
    const c = (
      ofType(calls, "sketch_path")[0].params.contours as PathContour[]
    )[0];
    expect(c.segments).toHaveLength(1);
    const q = c.segments[0] as { kind: string; ctrl: number[]; to: number[] };
    expect(q.kind).toBe("quad");
    // Control + end transformed (y-flipped), EXACT.
    expect(q.ctrl).toEqual([5, -10]);
    expect(q.to).toEqual([10, 0]);
  });

  it("handles relative path commands (lowercase) as a sketch_path", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 l10 0 l0 10 z"/></svg>`,
    );
    const c = (
      ofType(calls, "sketch_path")[0].params.contours as PathContour[]
    )[0];
    expect(c.closed).toBe(true);
    expect(c.segments).toHaveLength(2);
  });

  it("converts an elliptical arc A command into quad segments", () => {
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 A5 5 0 0 1 10 0"/></svg>`,
    );
    const c = (
      ofType(calls, "sketch_path")[0].params.contours as PathContour[]
    )[0];
    expect(c.segments.length).toBeGreaterThanOrEqual(1);
    expect(c.segments.every((s) => s.kind === "quad")).toBe(true);
  });

  it("groups M…Z M…Z subpaths into ONE sketch_path with 2 contours (donut)", () => {
    // Outer square + inner square (a glyph-like donut). Both subpaths
    // land in a single sketch_path so the kernel cuts a real hole.
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0 L20 0 L20 20 L0 20 Z M5 5 L15 5 L15 15 L5 15 Z"/></svg>`,
    );
    const paths = ofType(calls, "sketch_path");
    expect(paths).toHaveLength(1);
    const contours = paths[0].params.contours as PathContour[];
    expect(contours).toHaveLength(2);
    expect(contours[0].closed).toBe(true);
    expect(contours[1].closed).toBe(true);
    expect(contours[1].start).toEqual([5, -5]);
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

  it("applies a rotate(90) transform (with y-flip)", () => {
    // A point at (10,0) rotated +90° in SVG space → (0,10), then
    // y-flipped to sketch space → (0,-10).
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><g transform="rotate(90)"><line x1="10" y1="0" x2="10" y2="0"/></g></svg>`,
    );
    const s = calls[0].params.start as number[];
    expect(s[0]).toBeCloseTo(0, 6);
    expect(s[1]).toBeCloseTo(-10, 6);
  });

  it("rotates a rect about a point: rotate(a cx cy)", () => {
    // 180° about (5,5) maps corner (0,0)→(10,10); y-flip → (10,-10).
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="10" height="10" transform="rotate(180 5 5)"/></svg>`,
    );
    const lines = ofType(calls, "sketch_line");
    expect(lines).toHaveLength(4);
    const starts = lines.map((l) => l.params.start as number[]);
    // The (0,0) corner became (10,10) → sketch (10,-10) appears.
    const hit = starts.some(
      (p) => Math.abs(p[0] - 10) < 1e-6 && Math.abs(p[1] + 10) < 1e-6,
    );
    expect(hit).toBe(true);
  });

  it("applies skewX", () => {
    // skewX(45): x' = x + tan(45)·y = x + y. Point (0,10): x'=10,y'=10;
    // y-flip → (10,-10).
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><line x1="0" y1="10" x2="0" y2="10" transform="skewX(45)"/></svg>`,
    );
    const s = calls[0].params.start as number[];
    expect(s[0]).toBeCloseTo(10, 6);
    expect(s[1]).toBeCloseTo(-10, 6);
  });

  it("applies skewY", () => {
    // skewY(45): y' = y + tan(45)·x = y + x. Point (10,0): x'=10,y'=10;
    // y-flip → (10,-10).
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><line x1="10" y1="0" x2="10" y2="0" transform="skewY(45)"/></svg>`,
    );
    const s = calls[0].params.start as number[];
    expect(s[0]).toBeCloseTo(10, 6);
    expect(s[1]).toBeCloseTo(-10, 6);
  });

  it("composes nested g transforms in document order (rotate ∘ translate)", () => {
    // Outer translate(10,0), inner rotate(90). A point at (5,0) is first
    // rotated +90° → (0,5), then translated → (10,5); y-flip → (10,-5).
    const calls = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><g transform="translate(10,0)"><g transform="rotate(90)"><line x1="5" y1="0" x2="5" y2="0"/></g></g></svg>`,
    );
    const s = calls[0].params.start as number[];
    expect(s[0]).toBeCloseTo(10, 6);
    expect(s[1]).toBeCloseTo(-5, 6);
  });

  it("keeps a circle a circle but turns a non-uniform scale into an ellipse", () => {
    // Uniform scale(2): circle stays a circle, radius doubles.
    const uniform = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><circle cx="0" cy="0" r="3" transform="scale(2)"/></svg>`,
    );
    expect(uniform[0].type).toBe("sketch_circle");
    expect(uniform[0].params.radius).toBeCloseTo(6, 6);
    // Anisotropic scale(2,1): circle → ellipse with radii 6 and 3.
    const aniso = svgToSubCalls(
      `<svg xmlns="http://www.w3.org/2000/svg"><circle cx="0" cy="0" r="3" transform="scale(2,1)"/></svg>`,
    );
    expect(aniso[0].type).toBe("sketch_ellipse");
    const rx = aniso[0].params.radius_x as number;
    const ry = aniso[0].params.radius_y as number;
    expect(Math.max(rx, ry)).toBeCloseTo(6, 6);
    expect(Math.min(rx, ry)).toBeCloseTo(3, 6);
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
      wrap(
        `0\nLWPOLYLINE\n70\n1\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n`,
      ),
    );
    // 3 vertices, closed → 3 lines.
    expect(ofType(calls, "sketch_line")).toHaveLength(3);
  });

  it("parses an open LWPOLYLINE", () => {
    const calls = dxfToSubCalls(
      wrap(
        `0\nLWPOLYLINE\n70\n0\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n`,
      ),
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
