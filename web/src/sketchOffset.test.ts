import { describe, it, expect } from "vitest";
import {
  offsetCurve,
  offsetChain,
  buildChain,
  type Chain,
  type LineOrArc,
} from "./sketchOffset";
import type { Curve, Vec2 } from "./sketchTrim";

describe("offsetCurve — line", () => {
  const line: Curve = { kind: "line", a: [0, 0], b: [10, 0] };

  it("offsets toward a click above the line (+y side)", () => {
    const out = offsetCurve(line, 2, [5, 5]);
    expect(out).not.toBeNull();
    expect(out!.kind).toBe("line");
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(2, 9);
    expect(out!.b[1]).toBeCloseTo(2, 9);
    expect(out!.a[0]).toBeCloseTo(0, 9);
    expect(out!.b[0]).toBeCloseTo(10, 9);
  });

  it("offsets toward a click below the line (−y side)", () => {
    const out = offsetCurve(line, 2, [5, -5]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(-2, 9);
    expect(out!.b[1]).toBeCloseTo(-2, 9);
  });

  it("a click exactly on the line picks the deterministic +n side", () => {
    // +n side of a→b = (10,0) is +y (n = (−dy,dx)/len = (0,1)).
    const out = offsetCurve(line, 2, [5, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(2, 9);
    expect(out!.b[1]).toBeCloseTo(2, 9);
  });

  it("respects the segment direction for the normal sign", () => {
    // Reversed line b→a: n flips, so a click above now maps to −n side.
    const rev: Curve = { kind: "line", a: [10, 0], b: [0, 0] };
    const out = offsetCurve(rev, 2, [5, 5]);
    expect(out).not.toBeNull();
    if (out!.kind !== "line") throw new Error("expected line");
    expect(out!.a[1]).toBeCloseTo(2, 9);
    expect(out!.b[1]).toBeCloseTo(2, 9);
  });
});

describe("offsetCurve — circle", () => {
  const circle: Curve = { kind: "circle", c: [0, 0], r: 5 };

  it("grows the radius when the click is outside", () => {
    const out = offsetCurve(circle, 2, [10, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "circle") throw new Error("expected circle");
    expect(out!.r).toBeCloseTo(7, 9);
    expect(out!.c).toEqual([0, 0]);
  });

  it("shrinks the radius when the click is inside", () => {
    const out = offsetCurve(circle, 2, [1, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "circle") throw new Error("expected circle");
    expect(out!.r).toBeCloseTo(3, 9);
    expect(out!.c).toEqual([0, 0]);
  });

  it("returns null when an inner offset collapses the circle", () => {
    expect(offsetCurve(circle, 6, [1, 0])).toBeNull();
  });
});

describe("offsetCurve — arc", () => {
  const arc: Curve = {
    kind: "arc",
    c: [0, 0],
    r: 5,
    start: 0,
    sweep: Math.PI / 2,
  };

  it("grows the radius when the click is outside, keeping centre/angles", () => {
    const out = offsetCurve(arc, 2, [10, 0]);
    expect(out).not.toBeNull();
    if (out!.kind !== "arc") throw new Error("expected arc");
    expect(out!.r).toBeCloseTo(7, 9);
    expect(out!.c).toEqual([0, 0]);
    expect(out!.start).toBeCloseTo(0, 9);
    expect(out!.sweep).toBeCloseTo(Math.PI / 2, 9);
  });

  it("shrinks the radius when the click is inside, keeping centre/angles", () => {
    const out = offsetCurve(arc, 2, [1, 1]);
    expect(out).not.toBeNull();
    if (out!.kind !== "arc") throw new Error("expected arc");
    expect(out!.r).toBeCloseTo(3, 9);
    expect(out!.c).toEqual([0, 0]);
    expect(out!.start).toBeCloseTo(0, 9);
    expect(out!.sweep).toBeCloseTo(Math.PI / 2, 9);
  });

  it("returns null when an inner offset collapses the arc", () => {
    expect(offsetCurve(arc, 6, [1, 1])).toBeNull();
  });
});

describe("offsetCurve — edge cases", () => {
  const line: Curve = { kind: "line", a: [0, 0], b: [10, 0] };

  it("returns null for distance <= 0", () => {
    expect(offsetCurve(line, 0, [5, 5])).toBeNull();
    expect(offsetCurve(line, -2, [5, 5])).toBeNull();
  });

  it("returns null for a non-finite distance", () => {
    expect(offsetCurve(line, NaN, [5, 5])).toBeNull();
    expect(offsetCurve(line, Infinity, [5, 5])).toBeNull();
  });

  it("returns null for a zero-length line", () => {
    const dot: Curve = { kind: "line", a: [3, 3], b: [3, 3] };
    expect(offsetCurve(dot, 2, [5, 5])).toBeNull();
  });
});

// ── helpers for the chain suite ─────────────────────────────────────

const line = (a: Vec2, b: Vec2): LineOrArc => ({ kind: "line", a, b });
const allFinite = (curves: Curve[]): boolean =>
  curves.every((c) => {
    if (c.kind === "line") {
      return c.a.every(Number.isFinite) && c.b.every(Number.isFinite);
    }
    if (c.kind === "circle") {
      return c.c.every(Number.isFinite) && Number.isFinite(c.r);
    }
    return (
      c.c.every(Number.isFinite) &&
      Number.isFinite(c.r) &&
      Number.isFinite(c.start) &&
      Number.isFinite(c.sweep)
    );
  });

function endpoints(cv: Curve): [Vec2, Vec2] {
  if (cv.kind === "line") return [cv.a, cv.b];
  if (cv.kind === "circle") {
    const p: Vec2 = [cv.c[0] + cv.r, cv.c[1]];
    return [p, p];
  }
  const s: Vec2 = [
    cv.c[0] + cv.r * Math.cos(cv.start),
    cv.c[1] + cv.r * Math.sin(cv.start),
  ];
  const e: Vec2 = [
    cv.c[0] + cv.r * Math.cos(cv.start + cv.sweep),
    cv.c[1] + cv.r * Math.sin(cv.start + cv.sweep),
  ];
  return [s, e];
}

const d = (p: Vec2, q: Vec2): number => Math.hypot(p[0] - q[0], p[1] - q[1]);

/** Every consecutive emitted curve must chain up end→start. */
function isConnected(curves: Curve[], closed: boolean): boolean {
  for (let i = 0; i + 1 < curves.length; i++) {
    const [, e0] = endpoints(curves[i]);
    const [s1] = endpoints(curves[i + 1]);
    if (d(e0, s1) > 1e-4) return false;
  }
  if (closed && curves.length > 1) {
    const [, eLast] = endpoints(curves[curves.length - 1]);
    const [sFirst] = endpoints(curves[0]);
    if (d(eLast, sFirst) > 1e-4) return false;
  }
  return true;
}

describe("buildChain — connectivity walk", () => {
  it("walks an open 3-line chain in order from any seed", () => {
    const curves = [
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
    ];
    const chain = buildChain(curves, 1)!;
    expect(chain).not.toBeNull();
    expect(chain.curves.length).toBe(3);
    expect(chain.closed).toBe(false);
    expect(isConnected(chain.curves, false)).toBe(true);
  });

  it("detects a closed rectangle loop", () => {
    const curves = [
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
    ];
    const chain = buildChain(curves, 0)!;
    expect(chain.curves.length).toBe(4);
    expect(chain.closed).toBe(true);
  });

  it("a lone line is a length-1 open chain", () => {
    const chain = buildChain([line([0, 0], [10, 0])], 0)!;
    expect(chain.curves.length).toBe(1);
    expect(chain.closed).toBe(false);
  });

  it("returns null when seeded on a full circle", () => {
    const curves: Array<Curve | null> = [{ kind: "circle", c: [0, 0], r: 5 }];
    expect(buildChain(curves, 0)).toBeNull();
  });

  it("ignores a disconnected curve elsewhere in the list", () => {
    const curves = [
      line([0, 0], [10, 0]),
      line([10, 0], [20, 0]),
      line([100, 100], [110, 100]), // unrelated island
    ];
    const chain = buildChain(curves, 0)!;
    expect(chain.curves.length).toBe(2);
    expect(chain.indices).not.toContain(2);
  });
});

describe("offsetChain — open polyline of lines", () => {
  // L-shape: (0,0)→(10,0)→(10,10). Inside of the corner is the +x/+y
  // quadrant region; the concave side is up-and-right.
  const chain: Chain = {
    curves: [line([0, 0], [10, 0]), line([10, 0], [10, 10])],
    indices: [0, 1],
    closed: false,
  };

  it("inner (concave) offset trims the segments to a sharp corner", () => {
    // Click ABOVE the first segment → toward the inside of the L.
    const out = offsetChain(chain, 0, 2, [5, 1])!;
    expect(out).not.toBeNull();
    expect(allFinite(out)).toBe(true);
    // Concave: no rounding arc inserted, two trimmed lines meeting.
    expect(out.filter((c) => c.kind === "arc").length).toBe(0);
    expect(out.length).toBe(2);
    expect(isConnected(out, false)).toBe(true);
    // The shared corner should land near (8, 2): first line offset up by 2
    // and second line offset left by 2.
    const [, e0] = endpoints(out[0]);
    expect(e0[0]).toBeCloseTo(8, 4);
    expect(e0[1]).toBeCloseTo(2, 4);
  });

  it("outer (convex) offset inserts a rounding fillet arc", () => {
    // Click BELOW the first segment → toward the outside of the L corner.
    const out = offsetChain(chain, 0, 2, [5, -1])!;
    expect(out).not.toBeNull();
    expect(allFinite(out)).toBe(true);
    const arcs = out.filter((c) => c.kind === "arc");
    expect(arcs.length).toBe(1);
    // The fillet is centred at the original vertex (10,0) with radius = dist.
    const arc = arcs[0];
    if (arc.kind !== "arc") throw new Error("expected arc");
    expect(arc.c[0]).toBeCloseTo(10, 6);
    expect(arc.c[1]).toBeCloseTo(0, 6);
    expect(arc.r).toBeCloseTo(2, 6);
    expect(isConnected(out, false)).toBe(true);
  });

  it("side consistency: clicking either side flips the whole chain", () => {
    const inner = offsetChain(chain, 0, 2, [5, 1])!;
    const outer = offsetChain(chain, 0, 2, [5, -1])!;
    // Inner: first line offset to y≈+2; outer: first line offset to y≈−2.
    expect(endpoints(inner[0])[0][1]).toBeCloseTo(2, 4);
    expect(endpoints(outer[0])[0][1]).toBeCloseTo(-2, 4);
  });

  it("a 3-segment chain offsets all segments", () => {
    const c3: Chain = {
      curves: [
        line([0, 0], [10, 0]),
        line([10, 0], [10, 10]),
        line([10, 10], [20, 10]),
      ],
      indices: [0, 1, 2],
      closed: false,
    };
    const out = offsetChain(c3, 0, 2, [5, 1])!;
    expect(out).not.toBeNull();
    expect(allFinite(out)).toBe(true);
    // Three offset lines + reconciliations; at least 3 segments survive.
    expect(out.filter((c) => c.kind === "line").length).toBeGreaterThanOrEqual(3);
    expect(isConnected(out, false)).toBe(true);
  });
});

describe("offsetChain — closed rectangle loop", () => {
  const rect: Chain = {
    curves: [
      line([0, 0], [10, 0]),
      line([10, 0], [10, 10]),
      line([10, 10], [0, 10]),
      line([0, 10], [0, 0]),
    ],
    indices: [0, 1, 2, 3],
    closed: true,
  };

  it("offsets inward (click inside) to a smaller rectangle, no fillets", () => {
    const out = offsetChain(rect, 0, 2, [5, 1])!;
    expect(out).not.toBeNull();
    expect(allFinite(out)).toBe(true);
    // Inner offset of a convex rectangle is concave at every corner →
    // pure trims, four lines, closed.
    expect(out.filter((c) => c.kind === "arc").length).toBe(0);
    expect(out.filter((c) => c.kind === "line").length).toBe(4);
    expect(isConnected(out, true)).toBe(true);
    // Inner rectangle spans (2,2)→(8,8): check one corner.
    const corners = out.map((c) => endpoints(c)[0]);
    const xs = corners.map((p) => p[0]).sort((a, b) => a - b);
    const ys = corners.map((p) => p[1]).sort((a, b) => a - b);
    expect(xs[0]).toBeCloseTo(2, 3);
    expect(xs[xs.length - 1]).toBeCloseTo(8, 3);
    expect(ys[0]).toBeCloseTo(2, 3);
    expect(ys[ys.length - 1]).toBeCloseTo(8, 3);
  });

  it("offsets outward (click outside) inserting a rounding arc per corner", () => {
    const out = offsetChain(rect, 0, 2, [5, -1])!;
    expect(out).not.toBeNull();
    expect(allFinite(out)).toBe(true);
    // Outer offset of a convex loop rounds all four corners.
    expect(out.filter((c) => c.kind === "arc").length).toBe(4);
    expect(out.filter((c) => c.kind === "line").length).toBe(4);
    expect(isConnected(out, true)).toBe(true);
  });
});

describe("offsetChain — line→arc joint", () => {
  it("offsets a line joined to an arc and reconciles the joint", () => {
    // Line (0,0)→(10,0) then a quarter arc from (10,0) curving up to
    // (20,10): centre (10,10), r=10, start at angle −π/2, sweep +π/2.
    const arc: LineOrArc = {
      kind: "arc",
      c: [10, 10],
      r: 10,
      start: -Math.PI / 2,
      sweep: Math.PI / 2,
    };
    const chain: Chain = {
      curves: [line([0, 0], [10, 0]), arc],
      indices: [0, 1],
      closed: false,
    };
    const out = offsetChain(chain, 0, 2, [5, 1])!;
    expect(out).not.toBeNull();
    expect(allFinite(out)).toBe(true);
    expect(out.length).toBeGreaterThanOrEqual(2);
    expect(isConnected(out, false)).toBe(true);
    // The offset still has a line and an arc.
    expect(out.some((c) => c.kind === "line")).toBe(true);
    expect(out.some((c) => c.kind === "arc")).toBe(true);
  });
});

describe("offsetChain — degenerate / invalid", () => {
  const chain: Chain = {
    curves: [line([0, 0], [10, 0]), line([10, 0], [10, 10])],
    indices: [0, 1],
    closed: false,
  };

  it("returns null for a non-positive / non-finite distance", () => {
    expect(offsetChain(chain, 0, 0, [5, 1])).toBeNull();
    expect(offsetChain(chain, 0, -3, [5, 1])).toBeNull();
    expect(offsetChain(chain, 0, NaN, [5, 1])).toBeNull();
    expect(offsetChain(chain, 0, Infinity, [5, 1])).toBeNull();
  });

  it("drops a collapsing arc segment rather than emitting NaN", () => {
    // Tiny arc (r=1) inside a chain; offsetting inward by 5 would invert
    // it → it must be dropped, the surviving line stays finite.
    const arc: LineOrArc = {
      kind: "arc",
      c: [10, 1],
      r: 1,
      start: -Math.PI / 2,
      sweep: Math.PI / 2,
    };
    const c: Chain = {
      curves: [line([0, 0], [10, 0]), arc],
      indices: [0, 1],
      closed: false,
    };
    const out = offsetChain(c, 0, 5, [5, 1]);
    if (out !== null) {
      expect(allFinite(out)).toBe(true);
      // The collapsing arc is gone.
      expect(out.filter((x) => x.kind === "arc").length).toBe(0);
    }
  });

  it("never emits NaN/Inf coordinates for a closed inward over-offset", () => {
    const rect: Chain = {
      curves: [
        line([0, 0], [10, 0]),
        line([10, 0], [10, 10]),
        line([10, 10], [0, 10]),
        line([0, 10], [0, 0]),
      ],
      indices: [0, 1, 2, 3],
      closed: true,
    };
    // Offset inward by 4 (rect half-size 5) — still valid, must be finite.
    const out = offsetChain(rect, 0, 4, [5, 1]);
    if (out) expect(allFinite(out)).toBe(true);
  });
});
