import { describe, expect, it } from "vitest";
import {
  DEFAULT_SNAP_DISTANCE_MM,
  InferencePriority,
  inferCandidates,
} from "./inferenceEngine";
import { sketchWithOrigin, type SketchState } from "./types";

function sketchWithPoint(
  x: number,
  y: number,
  id = 1,
): SketchState {
  const s = sketchWithOrigin();
  s.entities.push({ kind: "point", id, at: { x, y } });
  s.next_entity_id = id + 1;
  return s;
}

describe("inferCandidates: suppress flag", () => {
  it("returns an empty list when suppress is true", () => {
    const sketch = sketchWithPoint(0, 0);
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 0, y: 0 },
      suppress: true,
    });
    expect(candidates).toEqual([]);
  });
});

describe("inferCandidates: endpoint snapping", () => {
  it("suggests coincident-on-endpoint when cursor is near an existing point", () => {
    const sketch = sketchWithPoint(10, 0);
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 10.2, y: 0.1 },
    });
    expect(candidates.length).toBeGreaterThan(0);
    const top = candidates[0];
    expect(top.priority).toBe(InferencePriority.CoincidentEndpoint);
    expect(top.label).toBe("Endpoint");
    expect(top.distanceMm).toBeLessThan(DEFAULT_SNAP_DISTANCE_MM);
  });

  it("doesn't suggest a coincident when the cursor is outside the snap radius", () => {
    const sketch = sketchWithPoint(10, 0);
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 20, y: 20 },
    });
    const hasEndpoint = candidates.some(
      (c) => c.priority === InferencePriority.CoincidentEndpoint,
    );
    expect(hasEndpoint).toBe(false);
  });

  it("snaps to the origin when the cursor is near (0, 0)", () => {
    const sketch = sketchWithOrigin();
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 0.4, y: 0.2 },
    });
    expect(candidates[0].label).toBe("Origin");
    expect(candidates[0].priority).toBe(InferencePriority.CoincidentEndpoint);
  });
});

describe("inferCandidates: midpoint snapping", () => {
  it("snaps to the midpoint of a line when the cursor hovers there", () => {
    const sketch = sketchWithOrigin();
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    sketch.next_entity_id = 4;
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 5, y: 0 },
    });
    // Midpoint of (0,0)-(10,0) = (5, 0).
    const midpointHits = candidates.filter((c) => c.label === "Midpoint");
    expect(midpointHits.length).toBe(1);
    expect(midpointHits[0].priority).toBe(InferencePriority.CoincidentMidpoint);
  });
});

describe("inferCandidates: orientation while drawing a line", () => {
  it("suggests horizontal when the drag direction is near-horizontal", () => {
    const sketch = sketchWithOrigin();
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.next_entity_id = 2;
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 50, y: 0.2 },
      drawingLineFrom: 1,
    });
    const hit = candidates.find(
      (c) => c.priority === InferencePriority.Horizontal,
    );
    expect(hit).toBeDefined();
    expect(hit?.label).toBe("Horizontal");
  });

  it("suggests vertical when the drag direction is near-vertical", () => {
    const sketch = sketchWithOrigin();
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.next_entity_id = 2;
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 0.3, y: 40 },
      drawingLineFrom: 1,
    });
    const hit = candidates.find(
      (c) => c.priority === InferencePriority.Vertical,
    );
    expect(hit).toBeDefined();
  });

  it("suggests parallel-to-existing-line when the drag vector matches an existing slope", () => {
    const sketch = sketchWithOrigin();
    // Existing line: (0,0) -> (10,10) — slope 1.
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 10 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    // Drawing line from (20, 0) toward roughly (30, 10) — same slope.
    sketch.entities.push({ kind: "point", id: 4, at: { x: 20, y: 0 } });
    sketch.next_entity_id = 5;
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 30, y: 10.1 },
      drawingLineFrom: 4,
    });
    const parallelHit = candidates.find(
      (c) => c.priority === InferencePriority.Parallel,
    );
    expect(parallelHit).toBeDefined();
    expect(parallelHit?.label).toBe("Parallel");
  });
});

describe("inferCandidates: priority ordering", () => {
  it("returns endpoint snap ahead of parallel when the cursor is near both", () => {
    const sketch = sketchWithOrigin();
    // Existing line (0,0) -> (10,10).
    sketch.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
    sketch.entities.push({ kind: "point", id: 2, at: { x: 10, y: 10 } });
    sketch.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
    // Drawing from (5, 0), cursor lands near point 2.
    sketch.entities.push({ kind: "point", id: 4, at: { x: 5, y: 0 } });
    sketch.next_entity_id = 5;
    const candidates = inferCandidates({
      state: sketch,
      cursor: { x: 10.1, y: 10.05 },
      drawingLineFrom: 4,
    });
    // The very first candidate should be the endpoint snap,
    // not the parallel hint, because endpoint has priority 10
    // vs parallel's 50.
    expect(candidates[0].priority).toBe(InferencePriority.CoincidentEndpoint);
  });
});

describe("inferCandidates: custom snap radius", () => {
  it("honours the snapDistanceMm override", () => {
    const sketch = sketchWithPoint(10, 0);
    // Cursor 3 mm off: default 1.5 mm radius misses, 5 mm radius hits.
    const misses = inferCandidates({
      state: sketch,
      cursor: { x: 13, y: 0 },
    });
    const hits = inferCandidates({
      state: sketch,
      cursor: { x: 13, y: 0 },
      snapDistanceMm: 5,
    });
    expect(misses.length).toBe(0);
    expect(hits.length).toBeGreaterThan(0);
  });
});
