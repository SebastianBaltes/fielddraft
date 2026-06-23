import { describe, it, expect } from "vitest";
import { SNAP_RADIUS_PX, snapRadiusMmFor } from "./SketchCanvas2d";

/**
 * Regression guard for ticket zuw9 ("can't close a sketch polygon — the end
 * line doesn't snap to the start point").
 *
 * Root cause: the line/inference snap radius was a FIXED 10 mm. The sketch
 * camera's `zoom` is px-per-mm, so a fixed-mm radius collapses to a handful of
 * screen pixels when the user zooms out — at the zoom captured in the ticket
 * (~0.30 px/mm) 10 mm is only ~3 px, far too tight to land on the start point.
 *
 * The fix makes the snap radius a constant SCREEN-PIXEL tolerance, converted to
 * mm per the current zoom (the Onshape behaviour). These tests pin that
 * contract so nobody reverts to a fixed-mm radius.
 */
describe("snapRadiusMmFor — zoom-independent snap tolerance", () => {
  it("keeps the on-screen snap radius constant: mm = SNAP_RADIUS_PX / zoom", () => {
    for (const zoom of [0.1, 0.3, 1, 5, 25]) {
      const mm = snapRadiusMmFor({ zoom });
      // The screen-space radius (mm * px-per-mm) is always SNAP_RADIUS_PX.
      expect(mm * zoom).toBeCloseTo(SNAP_RADIUS_PX, 6);
    }
  });

  it("grows the mm tolerance when zoomed out so closing a polygon stays reachable", () => {
    // The exact zoom from ticket zuw9's camera (perspective at z≈2507, fov 45°,
    // ~620–1080 px tall canvas → ~0.30–0.52 px/mm). At the most zoomed-out end
    // the old fixed 10 mm was ~3 px; the pixel-based radius is far more forgiving.
    const zuw9Zoom = 0.2986; // px/mm
    const mm = snapRadiusMmFor({ zoom: zuw9Zoom });
    expect(mm).toBeGreaterThan(10); // strictly more forgiving than the old 10 mm
    expect(mm).toBeCloseTo(SNAP_RADIUS_PX / zuw9Zoom, 6);
  });

  it("shrinks the mm tolerance when zoomed in so precise placement still works", () => {
    const zoomedIn = snapRadiusMmFor({ zoom: 20 }); // 20 px/mm
    const zoomedOut = snapRadiusMmFor({ zoom: 0.5 }); // 0.5 px/mm
    expect(zoomedIn).toBeLessThan(zoomedOut);
    expect(zoomedIn).toBeCloseTo(SNAP_RADIUS_PX / 20, 6);
  });

  it("guards against a divide-by-zero at a degenerate zoom", () => {
    const mm = snapRadiusMmFor({ zoom: 0 });
    expect(Number.isFinite(mm)).toBe(true);
    expect(mm).toBeGreaterThan(0);
  });
});
