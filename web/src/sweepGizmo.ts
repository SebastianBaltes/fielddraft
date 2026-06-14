/**
 * Math helpers for the interactive Sweep 3D path gizmo (ticket: draggable
 * control-point handles for `path.kind === "gizmo3d"`).
 *
 * `catmullRomSpline` is a 1:1 port of `crate::path_provider::catmull_rom`
 * (uniform Catmull-Rom, τ=0.5, reflected phantom endpoints for open paths) so
 * the gizmo's live spine preview matches the kernel's resolved spine exactly —
 * no drift between what the user drags and what gets swept.
 */

export type V3 = [number, number, number];

/** Phantom control point: `2·boundary − inner` (mirror of `reflect`). */
function reflect(boundary: V3, inner: V3): V3 {
  return [
    2 * boundary[0] - inner[0],
    2 * boundary[1] - inner[1],
    2 * boundary[2] - inner[2],
  ];
}

/** Uniform Catmull-Rom (τ=0.5) point on the P1→P2 span at `t∈[0,1]`. */
function crPoint(p0: V3, p1: V3, p2: V3, p3: V3, t: number): V3 {
  const t2 = t * t;
  const t3 = t2 * t;
  const out: V3 = [0, 0, 0];
  for (let k = 0; k < 3; k++) {
    out[k] =
      0.5 *
      (2 * p1[k] +
        (-p0[k] + p2[k]) * t +
        (2 * p0[k] - 5 * p1[k] + 4 * p2[k] - p3[k]) * t2 +
        (-p0[k] + 3 * p1[k] - 3 * p2[k] + p3[k]) * t3);
  }
  return out;
}

/**
 * Sample the spine spline through `nodes`. Two open nodes degenerate to a
 * straight segment (matches the kernel's `Gizmo3dPath::sample`). `sps` samples
 * per control-point span.
 */
export function catmullRomSpline(nodes: V3[], closed: boolean, sps: number): V3[] {
  const n = nodes.length;
  if (n < 2) return nodes.map((p) => [...p] as V3);
  if (n === 2 && !closed) return nodes.map((p) => [...p] as V3);
  const s = Math.max(1, Math.floor(sps));
  const get = (i: number): V3 => {
    if (closed) return nodes[((i % n) + n) % n];
    if (i < 0) return reflect(nodes[0], nodes[1]);
    if (i >= n) return reflect(nodes[n - 1], nodes[n - 2]);
    return nodes[i];
  };
  const spans = closed ? n : n - 1;
  const out: V3[] = [];
  for (let span = 0; span < spans; span++) {
    const p0 = get(span - 1);
    const p1 = get(span);
    const p2 = get(span + 1);
    const p3 = get(span + 2);
    for (let k = 0; k < s; k++) out.push(crPoint(p0, p1, p2, p3, k / s));
  }
  if (!closed) out.push([...nodes[n - 1]] as V3);
  return out;
}

/**
 * Flatten a polyline into `LineSegmentsGeometry` endpoint pairs (6 floats per
 * segment). `closed` appends the wrap segment from the last point to the first.
 */
export function polylineToSegments(pts: V3[], closed: boolean): number[] {
  const seg: number[] = [];
  for (let i = 0; i + 1 < pts.length; i++) {
    const a = pts[i];
    const b = pts[i + 1];
    seg.push(a[0], a[1], a[2], b[0], b[1], b[2]);
  }
  if (closed && pts.length > 1) {
    const a = pts[pts.length - 1];
    const b = pts[0];
    seg.push(a[0], a[1], a[2], b[0], b[1], b[2]);
  }
  return seg;
}
