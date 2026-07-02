import { useMemo } from "react";
import { useKernelStore } from "./store";

// ── Shared helpers of the viewport overlay modules ───────────────────
//
// Small common ground of the split overlay files (sketchSceneOverlays,
// transformGizmo, brepEdgeOverlays, brepSelectionOverlays, webglBodies,
// mateOverlays, datumOverlay, viewportHud): the body fingerprint hook,
// polyline→segment decimation and the element highlight colours.

// Token-driven highlight colours. THREE material colours can't live in CSS
// (they feed the WebGPU pipeline), so — like BRepEdgeOverlay's inline hex —
// they're module constants here.
/** Persistent element-selection colour: a saturated gold-orange (Onshape-like
 *  selection highlight), distinct from the blue picker-highlight. */
export const ELEMENT_SELECTION_COLOR = 0xff7a00;
/** Hover colour for B-Rep elements: the SAME gold-orange hue as
 *  {@link ELEMENT_SELECTION_COLOR}, lightened (ticket m0yw, brightened again
 *  in ticket bqvz) — hover previews the selection it would make, selection
 *  then deepens the tone. */
export const ELEMENT_HOVER_COLOR = 0xffc080;
/** Stroke width of hovered-element fat lines (ticket bqvz: the old 1px hover
 *  was barely distinguishable from the base wireframe). Clearly wider
 *  than the 2.5px base edges, clearly slimmer than the 4px selection stroke. */
export const ELEMENT_HOVER_LINEWIDTH = 3;

/** Build the per-edge `LineSegments` positions from a body-relative polyline. */
export function polylineToSegments(points: number[]): Float32Array {
  const n = points.length / 3;
  if (n < 2) return new Float32Array(0);

  // Collinear decimation (ticket mcvp): the fat-line shader renders every
  // segment as its own round-capped quad, so a STRAIGHT edge sampled into
  // many sub-segments shows a small notch at every interior joint once the
  // stroke is 4px wide. Greedily extend each output segment as long as all
  // skipped sample points stay within a hair of the chord — a straight edge
  // collapses to ONE segment, curved edges keep their samples (the chord
  // tolerance is far below a pixel at any usable zoom).
  const px = (i: number) => points[i * 3];
  const py = (i: number) => points[i * 3 + 1];
  const pz = (i: number) => points[i * 3 + 2];
  const keep: number[] = [0];
  let anchor = 0;
  for (let cand = 2; cand < n; cand++) {
    const acx = px(cand) - px(anchor);
    const acy = py(cand) - py(anchor);
    const acz = pz(cand) - pz(anchor);
    const acLen2 = acx * acx + acy * acy + acz * acz;
    const tol2 = Math.max(1e-8 * acLen2, 1e-12);
    // O(1) probes instead of re-checking the whole span: the mid sample
    // is where a monotone arc deviates most from the chord, and the
    // sample just before the candidate catches sharp local kinks.
    let straight = acLen2 > 0;
    const probes = [(anchor + cand) >> 1, cand - 1];
    for (const m of probes) {
      if (!straight || m <= anchor || m >= cand) continue;
      const abx = px(m) - px(anchor);
      const aby = py(m) - py(anchor);
      const abz = pz(m) - pz(anchor);
      // squared distance of M from the chord anchor→cand
      const cx = aby * acz - abz * acy;
      const cy = abz * acx - abx * acz;
      const cz = abx * acy - aby * acx;
      straight = (cx * cx + cy * cy + cz * cz) / acLen2 <= tol2;
    }
    if (!straight) {
      anchor = cand - 1;
      keep.push(anchor);
    }
  }
  keep.push(n - 1);

  const seg = new Float32Array((keep.length - 1) * 6);
  for (let s = 0; s < keep.length - 1; s++) {
    const a = keep[s];
    const b = keep[s + 1];
    seg[s * 6] = px(a);
    seg[s * 6 + 1] = py(a);
    seg[s * 6 + 2] = pz(a);
    seg[s * 6 + 3] = px(b);
    seg[s * 6 + 4] = py(b);
    seg[s * 6 + 5] = pz(b);
  }
  return seg;
}

/** Fingerprint that bumps on any body geometry edit or placement move — count
 *  plus per-body `id.version`. Shared by the two debug overlays below so their
 *  kernel reads re-run exactly when (and only when) the bodies change. Mirrors
 *  {@link BRepEdgeOverlay}'s own `bodyFingerprint`. */
export function useBodyFingerprint(): string {
  const { store, state } = useKernelStore();
  return useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++)
      fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
  }, [store, state]);
}
