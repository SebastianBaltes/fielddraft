/**
 * Shared pick helpers for tool controllers (architecture review, Befund 2).
 */

import { curveOfSubCall, distToCurve } from "../sketchTrim";
import type { SketchSubCall } from "./toolTypes";

/**
 * F10 fillet pick: index of the line sub-call nearest `click` (within
 * `tolMm`), or null. Restricted to `sketch_line` calls — fillet only
 * rounds the corner between two plain lines, so composites and curves
 * are skipped. Mirrors `nearestTrimTarget` but line-only. Also the pick
 * used by the Extend tool, the Mirror axis pick and the constraint-tool
 * hover highlight.
 */
export function nearestLineSubCall(
  subCalls: ReadonlyArray<SketchSubCall>,
  click: [number, number],
  tolMm: number,
): number | null {
  let bestIdx: number | null = null;
  let bestDist = tolMm;
  subCalls.forEach((call, i) => {
    const cv = curveOfSubCall(call);
    if (!cv || cv.kind !== "line") return;
    const d = distToCurve(click, cv);
    if (d < bestDist) {
      bestDist = d;
      bestIdx = i;
    }
  });
  return bestIdx;
}
