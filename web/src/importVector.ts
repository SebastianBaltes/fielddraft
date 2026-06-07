/**
 * F22: SVG / DXF vector import → sketch sub-calls.
 *
 * Pure, UI-side parsers (no kernel involvement). They translate a vector
 * file's geometry into the same `{ type, params }` sub-calls the sketch
 * tools emit, so imported geometry is indistinguishable from hand-drawn
 * geometry once `store.importVectorSubCalls` appends it to the active
 * session.
 *
 * ── Coordinate conventions ─────────────────────────────────────────────
 *  • SVG is y-DOWN; the sketch is y-UP. Every SVG coordinate has its y
 *    NEGATED on the way out (see `flipY`). DXF is already y-UP — no flip.
 *  • User units map to millimetres 1:1. SVG has no intrinsic unit; we
 *    treat one user unit as one mm. DXF likewise passes coordinates
 *    through unscaled.
 *
 * ── Curve handling (F22-full) ───────────────────────────────────────────
 *  SVG path Béziers are preserved AS curves, not flattened. A path becomes a
 *  single `sketch_path` sub-call carrying one contour per subpath (so a glyph
 *  donut imports with a real hole via the kernel's even-odd nesting). Quads
 *  (Q/T) map 1:1 to an EXACT quad segment; cubics (C/S) are degree-reduced
 *  into quad segments by recursive subdivision to a tight flatness tolerance
 *  (`CUBIC_TOL`). Lines (L/H/V) and the implicit Z close become line
 *  segments. SVG elliptical arcs (A) are converted to quad segments via the
 *  endpoint→centre parameterisation and a per-quarter-turn quad fit.
 *
 *  The SVG y-down → sketch y-up flip is FOLDED into the transform matrix as
 *  the outermost factor, so every control point and curve angle comes out in
 *  sketch space correctly (a rotated/skewed curve transforms as a whole).
 *
 *  DXF still tessellates curves into `sketch_line` chains (out of scope here;
 *  unchanged) and connects SPLINE fit/control points with lines.
 */

export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

/** One segment of a `sketch_path` contour. Tagged enum matching the
 *  kernel `PathSegmentDef` wire shape (snake_case `kind`). */
export type PathSeg =
  | { kind: "line"; to: Pt }
  | { kind: "quad"; ctrl: Pt; to: Pt };

/** One contour of a `sketch_path`: a start point + ordered segments. */
export interface PathContour {
  start: Pt;
  segments: PathSeg[];
  closed: boolean;
}

export interface ImportOpts {
  /** Bézier / elliptical-arc tessellation steps per segment. Default 16. */
  curveSteps?: number;
}

/** Default tessellation step count per curve segment. */
export const CURVE_STEPS = 16;

type Pt = [number, number];

function line(start: Pt, end: Pt): SubCall {
  return { type: "sketch_line", params: { start, end } };
}

/** Chain a list of points into consecutive `sketch_line` sub-calls. When
 *  `close` is set, a final segment back to the first point is appended. */
function lineChain(points: Pt[], close: boolean): SubCall[] {
  const out: SubCall[] = [];
  for (let i = 0; i + 1 < points.length; i++) {
    out.push(line(points[i], points[i + 1]));
  }
  if (close && points.length > 2) {
    out.push(line(points[points.length - 1], points[0]));
  }
  return out;
}

function isFiniteNum(n: number): boolean {
  return Number.isFinite(n);
}

/* ───────────────────────── SVG ───────────────────────── */

/** Parse a `transform` attribute into a 2×3 affine matrix [a,b,c,d,e,f]
 *  mapping (x,y) → (a·x + c·y + e, b·x + d·y + f). Supports the full SVG
 *  transform list: translate, scale, matrix, rotate (with optional
 *  centre), skewX and skewY. Returns identity when empty/unparseable. */
type Mat = [number, number, number, number, number, number];
const IDENTITY: Mat = [1, 0, 0, 1, 0, 0];
/** SVG y-down → sketch y-up flip, applied as the outermost transform. */
const FLIP_Y: Mat = [1, 0, 0, -1, 0, 0];

function matMul(m: Mat, n: Mat): Mat {
  // m ∘ n  (apply n first, then m)
  return [
    m[0] * n[0] + m[2] * n[1],
    m[1] * n[0] + m[3] * n[1],
    m[0] * n[2] + m[2] * n[3],
    m[1] * n[2] + m[3] * n[3],
    m[0] * n[4] + m[2] * n[5] + m[4],
    m[1] * n[4] + m[3] * n[5] + m[5],
  ];
}

function applyMat(m: Mat, p: Pt): Pt {
  return [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]];
}

/** Flatness tolerance (in path units ≈ mm) for cubic → quad reduction. */
export const CUBIC_TOL = 0.05;
/** Hard recursion cap so a pathological cubic can't explode the segment
 *  count — at depth 6 a single cubic yields ≤ 64 quads, far past the
 *  tolerance for any realistic glyph/outline cubic. */
const CUBIC_MAX_DEPTH = 6;

/** Evaluate a cubic Bézier at parameter t. */
function cubicAt(p0: Pt, p1: Pt, p2: Pt, p3: Pt, t: number): Pt {
  const mt = 1 - t;
  const a = mt * mt * mt;
  const b = 3 * mt * mt * t;
  const c = 3 * mt * t * t;
  const d = t * t * t;
  return [
    a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
    a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
  ];
}

/** Best single-quad control point for a cubic span: the classic
 *  degree-reduction `q1 = (3·p1 + 3·p2 − p0 − p3) / 4`. */
function quadCtrlFromCubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt): Pt {
  return [
    (3 * p1[0] + 3 * p2[0] - p0[0] - p3[0]) / 4,
    (3 * p1[1] + 3 * p2[1] - p0[1] - p3[1]) / 4,
  ];
}

/** Max deviation of the fitted quad from the cubic, sampled at a few
 *  interior parameters (cheap, conservative flatness proxy). */
function cubicQuadError(p0: Pt, p1: Pt, p2: Pt, p3: Pt, q1: Pt): number {
  let max = 0;
  for (const t of [0.25, 0.5, 0.75]) {
    const c = cubicAt(p0, p1, p2, p3, t);
    const mt = 1 - t;
    const qx = mt * mt * p0[0] + 2 * mt * t * q1[0] + t * t * p3[0];
    const qy = mt * mt * p0[1] + 2 * mt * t * q1[1] + t * t * p3[1];
    max = Math.max(max, Math.hypot(c[0] - qx, c[1] - qy));
  }
  return max;
}

/** Degree-reduce a cubic Bézier into a chain of EXACT quad segments to
 *  within `tol`, by recursive subdivision (de Casteljau split at t=0.5).
 *  Each emitted quad uses the optimal degree-reduction control point.
 *  This preserves the curve AS curves (no polyline) while staying inside
 *  the kernel's quad-only `SketchSegment` vocabulary.
 *
 *  Exported so other vector producers (e.g. the F24 glyph→sketch_path
 *  text layout in `sketchText.ts`) reuse the SAME cubic→quad math rather
 *  than duplicating it. The emitted `to`/`ctrl` points are in the input
 *  points' own coordinate space — the caller transforms them. */
export function cubicToQuads(
  p0: Pt,
  p1: Pt,
  p2: Pt,
  p3: Pt,
  tol: number,
  depth = 0,
): PathSeg[] {
  const q1 = quadCtrlFromCubic(p0, p1, p2, p3);
  if (depth >= CUBIC_MAX_DEPTH || cubicQuadError(p0, p1, p2, p3, q1) <= tol) {
    return [{ kind: "quad", ctrl: q1, to: p3 }];
  }
  // de Casteljau split at t = 0.5.
  const p01: Pt = [(p0[0] + p1[0]) / 2, (p0[1] + p1[1]) / 2];
  const p12: Pt = [(p1[0] + p2[0]) / 2, (p1[1] + p2[1]) / 2];
  const p23: Pt = [(p2[0] + p3[0]) / 2, (p2[1] + p3[1]) / 2];
  const p012: Pt = [(p01[0] + p12[0]) / 2, (p01[1] + p12[1]) / 2];
  const p123: Pt = [(p12[0] + p23[0]) / 2, (p12[1] + p23[1]) / 2];
  const mid: Pt = [(p012[0] + p123[0]) / 2, (p012[1] + p123[1]) / 2];
  return [
    ...cubicToQuads(p0, p01, p012, mid, tol, depth + 1),
    ...cubicToQuads(mid, p123, p23, p3, tol, depth + 1),
  ];
}

/** Convert one SVG elliptical-arc command into quad segments by sampling
 *  the arc and fitting a quad per ~quarter-turn span (control point at
 *  the tangent intersection). All output points are in the arc's own
 *  (pre-transform) space; the caller transforms them. */
function arcToQuadSegs(
  start: Pt,
  rxIn: number,
  ryIn: number,
  phi: number,
  largeArc: boolean,
  sweep: boolean,
  end: Pt,
): PathSeg[] {
  let rx = Math.abs(rxIn);
  let ry = Math.abs(ryIn);
  if (rx === 0 || ry === 0) return [{ kind: "line", to: end }];
  const cosP = Math.cos(phi);
  const sinP = Math.sin(phi);
  const dx = (start[0] - end[0]) / 2;
  const dy = (start[1] - end[1]) / 2;
  const x1p = cosP * dx + sinP * dy;
  const y1p = -sinP * dx + cosP * dy;
  const lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
  if (lambda > 1) {
    const s = Math.sqrt(lambda);
    rx *= s;
    ry *= s;
  }
  const sign = largeArc !== sweep ? 1 : -1;
  const numr = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
  const den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
  const co = sign * Math.sqrt(Math.max(0, numr / den));
  const cxp = (co * rx * y1p) / ry;
  const cyp = (-co * ry * x1p) / rx;
  const cx = cosP * cxp - sinP * cyp + (start[0] + end[0]) / 2;
  const cy = sinP * cxp + cosP * cyp + (start[1] + end[1]) / 2;
  const ang = (ux: number, uy: number, vx: number, vy: number): number => {
    const dot = ux * vx + uy * vy;
    const len = Math.hypot(ux, uy) * Math.hypot(vx, vy);
    let a = Math.acos(Math.min(1, Math.max(-1, dot / len)));
    if (ux * vy - uy * vx < 0) a = -a;
    return a;
  };
  const theta1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry);
  let dTheta = ang(
    (x1p - cxp) / rx,
    (y1p - cyp) / ry,
    (-x1p - cxp) / rx,
    (-y1p - cyp) / ry,
  );
  if (!sweep && dTheta > 0) dTheta -= 2 * Math.PI;
  if (sweep && dTheta < 0) dTheta += 2 * Math.PI;
  const onArc = (t: number): Pt => [
    cosP * rx * Math.cos(t) - sinP * ry * Math.sin(t) + cx,
    sinP * rx * Math.cos(t) + cosP * ry * Math.sin(t) + cy,
  ];
  const tangent = (t: number): Pt => [
    -cosP * rx * Math.sin(t) - sinP * ry * Math.cos(t),
    -sinP * rx * Math.sin(t) + cosP * ry * Math.cos(t),
  ];
  // One quad per ≤90° span keeps the single-quad fit tight.
  const spans = Math.max(1, Math.ceil(Math.abs(dTheta) / (Math.PI / 2)));
  const segs: PathSeg[] = [];
  for (let i = 0; i < spans; i++) {
    const ta = theta1 + (dTheta * i) / spans;
    const tb = theta1 + (dTheta * (i + 1)) / spans;
    const a = onArc(ta);
    const b = onArc(tb);
    const da = tangent(ta);
    const db = tangent(tb);
    // Control = intersection of tangent lines a+s·da and b−u·db.
    const denom = da[0] * db[1] - da[1] * db[0];
    let ctrl: Pt;
    if (Math.abs(denom) < 1e-9) {
      ctrl = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    } else {
      const s = ((b[0] - a[0]) * db[1] - (b[1] - a[1]) * db[0]) / denom;
      ctrl = [a[0] + s * da[0], a[1] + s * da[1]];
    }
    segs.push({ kind: "quad", ctrl, to: b });
  }
  return segs;
}

/** Decompose the linear part of an affine matrix into rotation +
 *  axis-aligned scale via SVD of the 2×2 (Jacobi one-sided). Returns the
 *  semi-axis lengths a single radius `r` maps to and the rotation of the
 *  major axis. Used to map a circle/ellipse under a rotating/skewing
 *  transform to an `sketch_ellipse` (radii + rotation) rather than
 *  dropping the rotation. */
function decomposeLinear(m: Mat): {
  scaleX: number;
  scaleY: number;
  rotation: number;
} {
  // 2×2 linear part [[a, c], [b, d]] (column-major in our Mat layout).
  const a = m[0];
  const b = m[1];
  const c = m[2];
  const d = m[3];
  // Singular values via the M·Mᵀ symmetric eigen-decomposition.
  const e = (a * a + b * b + c * c + d * d) / 2;
  const f = (a * a + b * b - c * c - d * d) / 2;
  const g = a * c + b * d;
  const hyp = Math.sqrt(f * f + g * g);
  const sx = Math.sqrt(Math.max(0, e + hyp));
  const sy = Math.sqrt(Math.max(0, e - hyp));
  // Rotation of the major axis (eigenvector direction of M·Mᵀ).
  const rotation = f === 0 && g === 0 ? 0 : 0.5 * Math.atan2(g, f);
  return { scaleX: sx, scaleY: sy, rotation };
}

function parseTransform(attr: string | null): Mat {
  if (!attr) return IDENTITY;
  let m: Mat = IDENTITY;
  const re = /(\w+)\s*\(([^)]*)\)/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(attr)) !== null) {
    const op = match[1];
    const nums = match[2]
      .split(/[\s,]+/)
      .map(Number)
      .filter(isFiniteNum);
    let t: Mat | null = null;
    if (op === "translate") {
      t = [1, 0, 0, 1, nums[0] ?? 0, nums[1] ?? 0];
    } else if (op === "scale") {
      const sx = nums[0] ?? 1;
      const sy = nums[1] ?? sx;
      t = [sx, 0, 0, sy, 0, 0];
    } else if (op === "matrix" && nums.length === 6) {
      t = [nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]];
    } else if (op === "rotate") {
      // rotate(a) or rotate(a cx cy) — degrees. About a point =
      // translate(c) · rotate(a) · translate(−c).
      const a = ((nums[0] ?? 0) * Math.PI) / 180;
      const cos = Math.cos(a);
      const sin = Math.sin(a);
      const rot: Mat = [cos, sin, -sin, cos, 0, 0];
      if (nums.length >= 3) {
        const cx = nums[1];
        const cy = nums[2];
        t = matMul(matMul([1, 0, 0, 1, cx, cy], rot), [1, 0, 0, 1, -cx, -cy]);
      } else {
        t = rot;
      }
    } else if (op === "skewX") {
      const a = ((nums[0] ?? 0) * Math.PI) / 180;
      t = [1, 0, Math.tan(a), 1, 0, 0];
    } else if (op === "skewY") {
      const a = ((nums[0] ?? 0) * Math.PI) / 180;
      t = [1, Math.tan(a), 0, 1, 0, 0];
    }
    if (t) m = matMul(m, t);
  }
  return m;
}

function num(el: Element, name: string, dflt = 0): number {
  const v = Number(el.getAttribute(name));
  return isFiniteNum(v) ? v : dflt;
}

/** Parse the whitespace/comma separated point list of a polyline/polygon. */
function parsePointList(attr: string | null): Pt[] {
  if (!attr) return [];
  const nums = attr
    .split(/[\s,]+/)
    .map(Number)
    .filter(isFiniteNum);
  const pts: Pt[] = [];
  for (let i = 0; i + 1 < nums.length; i += 2) pts.push([nums[i], nums[i + 1]]);
  return pts;
}

interface SvgWalk {
  calls: SubCall[];
  skipped: number;
}

/** Emit an `sketch_ellipse` (or `sketch_circle`) for a circle/ellipse
 *  whose centre and radii live in the element's local space, transformed
 *  by `m` (which already folds in the y-flip + all ancestor transforms).
 *  The linear part of `m` is SVD-decomposed so a circle under a rotating
 *  or skewing transform becomes a correctly oriented ellipse instead of
 *  silently dropping the rotation. */
function emitTransformedEllipse(
  out: SvgWalk,
  m: Mat,
  cx: number,
  cy: number,
  rx: number,
  ry: number,
  steps: number,
): void {
  const center = applyMat(m, [cx, cy]);
  // Pre-scale the unit linear part by the element radii, then decompose:
  // map the ellipse basis vectors (rx,0) and (0,ry) through the linear
  // part and SVD the resulting 2×2.
  const lin: Mat = [m[0] * rx, m[1] * rx, m[2] * ry, m[3] * ry, 0, 0];
  const { scaleX, scaleY, rotation } = decomposeLinear(lin);
  if (!(scaleX > 0) || !(scaleY > 0)) {
    out.skipped++;
    return;
  }
  // A conformal transform (uniform scale + rotation, no skew) maps a
  // circle to a circle — keep it a `sketch_circle`. Only a genuinely
  // anisotropic transform yields an ellipse (with the right rotation).
  const rel = Math.abs(scaleX - scaleY) / Math.max(scaleX, scaleY);
  if (rel < 1e-4) {
    out.calls.push({
      type: "sketch_circle",
      params: { center, radius: (scaleX + scaleY) / 2 },
    });
    return;
  }
  out.calls.push({
    type: "sketch_ellipse",
    params: {
      center,
      radius_x: scaleX,
      radius_y: scaleY,
      rotation,
      segments: Math.max(16, steps * 2),
    },
  });
}

function walkSvg(el: Element, mat: Mat, steps: number, out: SvgWalk): void {
  const tag = el.tagName.toLowerCase();
  // Apply this element's own transform on top of the inherited matrix.
  // `mat` already carries the outermost y-flip, so curve control points
  // and angles all transform together in one space.
  const local = parseTransform(el.getAttribute("transform"));
  const m = matMul(mat, local);
  const xf = (p: Pt): Pt => applyMat(m, p);

  switch (tag) {
    case "svg":
    case "g": {
      for (const child of Array.from(el.children)) {
        walkSvg(child, m, steps, out);
      }
      return;
    }
    case "line": {
      out.calls.push(
        line(
          xf([num(el, "x1"), num(el, "y1")]),
          xf([num(el, "x2"), num(el, "y2")]),
        ),
      );
      return;
    }
    case "rect": {
      const x = num(el, "x");
      const y = num(el, "y");
      const w = num(el, "width");
      const h = num(el, "height");
      if (!(w > 0) || !(h > 0)) {
        out.skipped++;
        return;
      }
      // SVG rx/ry rules: if only one is present use it for both; clamp
      // rx ≤ w/2, ry ≤ h/2. rx/ry ≤ 0 or absent → sharp corners.
      const hasRx = el.hasAttribute("rx");
      const hasRy = el.hasAttribute("ry");
      let rx = hasRx ? num(el, "rx") : NaN;
      let ry = hasRy ? num(el, "ry") : NaN;
      if (!Number.isFinite(rx) && Number.isFinite(ry)) rx = ry;
      if (!Number.isFinite(ry) && Number.isFinite(rx)) ry = rx;
      rx = Number.isFinite(rx) ? Math.min(Math.max(rx, 0), w / 2) : 0;
      ry = Number.isFinite(ry) ? Math.min(Math.max(ry, 0), h / 2) : 0;
      if (!(rx > 0) || !(ry > 0)) {
        // Sharp rectangle: unchanged 4-line chain.
        const corners: Pt[] = (
          [
            [x, y],
            [x + w, y],
            [x + w, y + h],
            [x, y + h],
          ] as Pt[]
        ).map(xf);
        out.calls.push(...lineChain(corners, true));
        return;
      }
      // Rounded rectangle: ONE closed sketch_path contour of 4 straight
      // edges + 4 corner arcs. Corner arcs reuse arcToQuadSegs (the same
      // SVG-arc→quad helper as elliptical-arc commands) so fidelity
      // matches the rest of the importer. Build in local space, then
      // transform every emitted point via `xf` so rotate/skew/scale
      // carry through (circular corners turn elliptical exactly like
      // <circle>/<ellipse>). SVG y-down: corners sweep clockwise (sweep=1).
      const xl = x;
      const xr = x + w;
      const yt = y;
      const yb = y + h;
      const xfSeg = (seg: PathSeg): PathSeg =>
        seg.kind === "quad"
          ? { kind: "quad", ctrl: xf(seg.ctrl), to: xf(seg.to) }
          : { kind: "line", to: xf(seg.to) };
      const corner = (from: Pt, to: Pt): PathSeg[] =>
        arcToQuadSegs(from, rx, ry, 0, false, true, to).map(xfSeg);
      const segments: PathSeg[] = [];
      // top edge → top-right corner
      segments.push({ kind: "line", to: xf([xr - rx, yt]) });
      segments.push(...corner([xr - rx, yt], [xr, yt + ry]));
      // right edge → bottom-right corner
      segments.push({ kind: "line", to: xf([xr, yb - ry]) });
      segments.push(...corner([xr, yb - ry], [xr - rx, yb]));
      // bottom edge → bottom-left corner
      segments.push({ kind: "line", to: xf([xl + rx, yb]) });
      segments.push(...corner([xl + rx, yb], [xl, yb - ry]));
      // left edge → top-left corner
      segments.push({ kind: "line", to: xf([xl, yt + ry]) });
      segments.push(...corner([xl, yt + ry], [xl + rx, yt]));
      out.calls.push({
        type: "sketch_path",
        params: {
          contours: [
            {
              start: xf([xl + rx, yt]),
              segments,
              closed: true,
            },
          ],
        },
      });
      return;
    }
    case "circle": {
      const cx = num(el, "cx");
      const cy = num(el, "cy");
      const r = num(el, "r");
      if (!(r > 0)) {
        out.skipped++;
        return;
      }
      // A uniformly-scaled circle stays a circle; under rotation/skew it
      // becomes an ellipse — emitTransformedEllipse handles both.
      emitTransformedEllipse(out, m, cx, cy, r, r, steps);
      return;
    }
    case "ellipse": {
      const cx = num(el, "cx");
      const cy = num(el, "cy");
      const rx = num(el, "rx");
      const ry = num(el, "ry");
      if (!(rx > 0) || !(ry > 0)) {
        out.skipped++;
        return;
      }
      emitTransformedEllipse(out, m, cx, cy, rx, ry, steps);
      return;
    }
    case "polyline": {
      const pts = parsePointList(el.getAttribute("points")).map(xf) as Pt[];
      out.calls.push(...lineChain(pts, false));
      return;
    }
    case "polygon": {
      const pts = parsePointList(el.getAttribute("points")).map(xf) as Pt[];
      out.calls.push(...lineChain(pts, true));
      return;
    }
    case "path": {
      const call = parsePathToSketchPath(el.getAttribute("d") ?? "", m);
      if (call) out.calls.push(call);
      return;
    }
    default:
      // Unknown / unsupported element (text, image, use, defs, …).
      out.skipped++;
      return;
  }
}

/** Parse an SVG path `d` string into ONE `sketch_path` sub-call whose
 *  contours preserve curves AS curves. Each subpath (M…) becomes a
 *  contour; Z marks it closed. Quads (Q/T) map 1:1 to an EXACT quad
 *  segment; cubics (C/S) are degree-reduced to quad segments
 *  ([`cubicToQuads`], tolerance [`CUBIC_TOL`]); elliptical arcs (A)
 *  become quad segments per ≤90° span; L/H/V become line segments.
 *
 *  All geometry is parsed in the path's RAW space, then every endpoint
 *  and control point is transformed by `m` (which already folds in the
 *  y-flip and ancestor transforms). Quadratic Béziers are affine-
 *  invariant, so transforming the control points yields the exact
 *  transformed curve — including correct angles under rotate/skew.
 *
 *  Returns `null` when no contour with at least one segment results. */
function parsePathToSketchPath(d: string, m: Mat): SubCall | null {
  const cmdRe = /([MmLlHhVvCcSsQqTtAaZz])([^MmLlHhVvCcSsQqTtAaZz]*)/g;
  const xf = (p: Pt): Pt => applyMat(m, p);
  const contours: PathContour[] = [];
  let cur: Pt = [0, 0]; // current point, RAW
  let sub: Pt = [0, 0]; // subpath start, RAW
  let prevCubicCtrl: Pt | null = null;
  let prevQuadCtrl: Pt | null = null;
  let active: PathContour | null = null; // current contour (transformed)

  const ensureContour = () => {
    if (!active) {
      active = { start: xf(cur), segments: [], closed: false };
      contours.push(active);
    }
  };
  const finish = () => {
    active = null;
  };

  let match: RegExpExecArray | null;
  while ((match = cmdRe.exec(d)) !== null) {
    const cmd = match[1];
    const rel = cmd === cmd.toLowerCase() && cmd !== cmd.toUpperCase();
    const args = match[2]
      .split(/[\s,]+/)
      .map(Number)
      .filter(isFiniteNum);
    const up = cmd.toUpperCase();
    let i = 0;

    if (up === "M") {
      if (args.length >= 2) {
        finish();
        cur = rel ? [cur[0] + args[0], cur[1] + args[1]] : [args[0], args[1]];
        sub = [cur[0], cur[1]];
        active = { start: xf(cur), segments: [], closed: false };
        contours.push(active);
        i = 2;
        for (; i + 1 < args.length; i += 2) {
          cur = rel
            ? [cur[0] + args[i], cur[1] + args[i + 1]]
            : [args[i], args[i + 1]];
          active.segments.push({ kind: "line", to: xf(cur) });
        }
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "L") {
      ensureContour();
      for (; i + 1 < args.length; i += 2) {
        cur = rel
          ? [cur[0] + args[i], cur[1] + args[i + 1]]
          : [args[i], args[i + 1]];
        active!.segments.push({ kind: "line", to: xf(cur) });
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "H") {
      ensureContour();
      for (; i < args.length; i++) {
        cur = rel ? [cur[0] + args[i], cur[1]] : [args[i], cur[1]];
        active!.segments.push({ kind: "line", to: xf(cur) });
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "V") {
      ensureContour();
      for (; i < args.length; i++) {
        cur = rel ? [cur[0], cur[1] + args[i]] : [cur[0], args[i]];
        active!.segments.push({ kind: "line", to: xf(cur) });
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "C") {
      ensureContour();
      for (; i + 5 < args.length; i += 6) {
        const p1: Pt = rel
          ? [cur[0] + args[i], cur[1] + args[i + 1]]
          : [args[i], args[i + 1]];
        const p2: Pt = rel
          ? [cur[0] + args[i + 2], cur[1] + args[i + 3]]
          : [args[i + 2], args[i + 3]];
        const p3: Pt = rel
          ? [cur[0] + args[i + 4], cur[1] + args[i + 5]]
          : [args[i + 4], args[i + 5]];
        for (const q of cubicToQuads(cur, p1, p2, p3, CUBIC_TOL)) {
          active!.segments.push(xfSeg(q, xf));
        }
        cur = p3;
        prevCubicCtrl = p2;
      }
      prevQuadCtrl = null;
    } else if (up === "S") {
      ensureContour();
      for (; i + 3 < args.length; i += 4) {
        const reflect: Pt = prevCubicCtrl
          ? [2 * cur[0] - prevCubicCtrl[0], 2 * cur[1] - prevCubicCtrl[1]]
          : [cur[0], cur[1]];
        const p2: Pt = rel
          ? [cur[0] + args[i], cur[1] + args[i + 1]]
          : [args[i], args[i + 1]];
        const p3: Pt = rel
          ? [cur[0] + args[i + 2], cur[1] + args[i + 3]]
          : [args[i + 2], args[i + 3]];
        for (const q of cubicToQuads(cur, reflect, p2, p3, CUBIC_TOL)) {
          active!.segments.push(xfSeg(q, xf));
        }
        cur = p3;
        prevCubicCtrl = p2;
      }
      prevQuadCtrl = null;
    } else if (up === "Q") {
      ensureContour();
      for (; i + 3 < args.length; i += 4) {
        const p1: Pt = rel
          ? [cur[0] + args[i], cur[1] + args[i + 1]]
          : [args[i], args[i + 1]];
        const p2: Pt = rel
          ? [cur[0] + args[i + 2], cur[1] + args[i + 3]]
          : [args[i + 2], args[i + 3]];
        active!.segments.push({ kind: "quad", ctrl: xf(p1), to: xf(p2) });
        cur = p2;
        prevQuadCtrl = p1;
      }
      prevCubicCtrl = null;
    } else if (up === "T") {
      ensureContour();
      for (; i + 1 < args.length; i += 2) {
        const p1: Pt = prevQuadCtrl
          ? [2 * cur[0] - prevQuadCtrl[0], 2 * cur[1] - prevQuadCtrl[1]]
          : [cur[0], cur[1]];
        const p2: Pt = rel
          ? [cur[0] + args[i], cur[1] + args[i + 1]]
          : [args[i], args[i + 1]];
        active!.segments.push({ kind: "quad", ctrl: xf(p1), to: xf(p2) });
        cur = p2;
        prevQuadCtrl = p1;
      }
      prevCubicCtrl = null;
    } else if (up === "A") {
      ensureContour();
      for (; i + 6 < args.length; i += 7) {
        const rx = args[i];
        const ry = args[i + 1];
        const xRot = (args[i + 2] * Math.PI) / 180;
        const largeArc = args[i + 3] !== 0;
        const sweep = args[i + 4] !== 0;
        const end: Pt = rel
          ? [cur[0] + args[i + 5], cur[1] + args[i + 6]]
          : [args[i + 5], args[i + 6]];
        for (const q of arcToQuadSegs(
          cur,
          rx,
          ry,
          xRot,
          largeArc,
          sweep,
          end,
        )) {
          active!.segments.push(xfSeg(q, xf));
        }
        cur = end;
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "Z") {
      if (active) active.closed = true;
      cur = [sub[0], sub[1]];
      // A subsequent command without an M re-opens from the close point.
      finish();
      prevCubicCtrl = prevQuadCtrl = null;
    }
  }

  const kept = contours.filter((c) => c.segments.length > 0);
  if (kept.length === 0) return null;
  return {
    type: "sketch_path",
    params: { contours: kept },
  };
}

/** Transform a raw-space path segment's points through `xf`. Quads are
 *  affine-invariant, so transforming the control + end point yields the
 *  exact transformed quad. */
function xfSeg(seg: PathSeg, xf: (p: Pt) => Pt): PathSeg {
  if (seg.kind === "line") return { kind: "line", to: xf(seg.to) };
  return { kind: "quad", ctrl: xf(seg.ctrl), to: xf(seg.to) };
}

/** Parse SVG text into sub-calls. Returns `[]` on a missing/garbage root.
 *  Coordinates are y-flipped (SVG y-down → sketch y-up). */
export function svgToSubCalls(
  svgText: string,
  opts: ImportOpts = {},
): SubCall[] {
  const steps = opts.curveSteps ?? CURVE_STEPS;
  let doc: Document;
  try {
    doc = new DOMParser().parseFromString(svgText, "image/svg+xml");
  } catch {
    return [];
  }
  const root = doc.documentElement;
  if (!root || root.tagName.toLowerCase() !== "svg") return [];
  // A parse error yields a <parsererror> element somewhere in the tree.
  if (doc.getElementsByTagName("parsererror").length > 0) return [];
  const out: SvgWalk = { calls: [], skipped: 0 };
  // Seed with the y-flip as the OUTERMOST transform so every point,
  // curve control point and angle transforms together into sketch space.
  walkSvg(root, FLIP_Y, steps, out);
  return out.calls;
}

/* ───────────────────────── DXF ───────────────────────── */

interface DxfPair {
  code: number;
  value: string;
}

/** Tokenise a DXF text stream into (group-code, value) pairs. DXF stores
 *  one code per line and its value on the next line. */
function dxfPairs(text: string): DxfPair[] {
  const lines = text.split(/\r\n|\r|\n/);
  const pairs: DxfPair[] = [];
  for (let i = 0; i + 1 < lines.length; i += 2) {
    const code = Number(lines[i].trim());
    if (!Number.isFinite(code)) continue;
    pairs.push({ code, value: lines[i + 1] });
  }
  return pairs;
}

interface DxfEntity {
  type: string;
  // group-code → list of values (a code can repeat, e.g. LWPOLYLINE 10/20)
  codes: Map<number, string[]>;
}

/** Split the ENTITIES section into individual entities. Each entity starts
 *  with a code-0 marker (its type) and runs until the next code-0. */
function dxfEntities(pairs: DxfPair[]): DxfEntity[] {
  // Find the ENTITIES section bounds.
  let start = -1;
  let end = pairs.length;
  for (let i = 0; i + 1 < pairs.length; i++) {
    if (pairs[i].code === 2 && pairs[i].value.trim() === "ENTITIES") {
      start = i + 1;
      break;
    }
  }
  if (start < 0) return [];
  for (let i = start; i < pairs.length; i++) {
    if (pairs[i].code === 0 && pairs[i].value.trim() === "ENDSEC") {
      end = i;
      break;
    }
  }

  const entities: DxfEntity[] = [];
  let cur: DxfEntity | null = null;
  for (let i = start; i < end; i++) {
    const { code, value } = pairs[i];
    if (code === 0) {
      if (cur) entities.push(cur);
      cur = { type: value.trim(), codes: new Map() };
    } else if (cur) {
      const arr = cur.codes.get(code);
      if (arr) arr.push(value);
      else cur.codes.set(code, [value]);
    }
  }
  if (cur) entities.push(cur);
  return entities;
}

function dxfNum(e: DxfEntity, code: number, idx = 0): number {
  const arr = e.codes.get(code);
  if (!arr || arr[idx] === undefined) return NaN;
  return Number(arr[idx]);
}

/** Parse DXF text into sub-calls. DXF is already y-UP — no flip. Unknown
 *  entities are skipped. Returns `[]` on empty / section-less input. */
export function dxfToSubCalls(
  dxfText: string,
  opts: ImportOpts = {},
): SubCall[] {
  const steps = opts.curveSteps ?? CURVE_STEPS;
  const pairs = dxfPairs(dxfText);
  if (pairs.length === 0) return [];
  const entities = dxfEntities(pairs);
  const out: SubCall[] = [];

  for (const e of entities) {
    switch (e.type) {
      case "LINE": {
        const sx = dxfNum(e, 10);
        const sy = dxfNum(e, 20);
        const ex = dxfNum(e, 11);
        const ey = dxfNum(e, 21);
        if ([sx, sy, ex, ey].every(isFiniteNum)) {
          out.push(line([sx, sy], [ex, ey]));
        }
        break;
      }
      case "CIRCLE": {
        const cx = dxfNum(e, 10);
        const cy = dxfNum(e, 20);
        const r = dxfNum(e, 40);
        if (isFiniteNum(cx) && isFiniteNum(cy) && r > 0) {
          out.push({
            type: "sketch_circle",
            params: { center: [cx, cy], radius: r },
          });
        }
        break;
      }
      case "ARC": {
        const cx = dxfNum(e, 10);
        const cy = dxfNum(e, 20);
        const r = dxfNum(e, 40);
        const startDeg = dxfNum(e, 50);
        const endDeg = dxfNum(e, 51);
        if (
          isFiniteNum(cx) &&
          isFiniteNum(cy) &&
          r > 0 &&
          isFiniteNum(startDeg) &&
          isFiniteNum(endDeg)
        ) {
          const start = (startDeg * Math.PI) / 180;
          let sweep = ((endDeg - startDeg) * Math.PI) / 180;
          // DXF arcs are CCW; normalise sweep into (0, 2π].
          if (sweep <= 0) sweep += 2 * Math.PI;
          out.push({
            type: "sketch_arc",
            params: {
              center: [cx, cy],
              radius: r,
              start_angle: start,
              sweep_angle: sweep,
            },
          });
        }
        break;
      }
      case "ELLIPSE": {
        const cx = dxfNum(e, 10);
        const cy = dxfNum(e, 20);
        const mx = dxfNum(e, 11);
        const my = dxfNum(e, 21);
        const ratio = dxfNum(e, 40);
        if ([cx, cy, mx, my, ratio].every(isFiniteNum)) {
          const radiusX = Math.hypot(mx, my);
          out.push({
            type: "sketch_ellipse",
            params: {
              center: [cx, cy],
              radius_x: radiusX,
              radius_y: ratio * radiusX,
              rotation: Math.atan2(my, mx),
              segments: Math.max(16, steps * 2),
            },
          });
        }
        break;
      }
      case "LWPOLYLINE": {
        const xs = e.codes.get(10) ?? [];
        const ys = e.codes.get(20) ?? [];
        const n = Math.min(xs.length, ys.length);
        const pts: Pt[] = [];
        for (let i = 0; i < n; i++) {
          const x = Number(xs[i]);
          const y = Number(ys[i]);
          if (isFiniteNum(x) && isFiniteNum(y)) pts.push([x, y]);
        }
        const closed = (dxfNum(e, 70) & 1) === 1;
        out.push(...lineChain(pts, closed));
        break;
      }
      case "POLYLINE": {
        // Legacy POLYLINE: vertices live in following VERTEX entities. We
        // collect them in the loop below via a lookahead flag; handled
        // separately because they're sibling entities, not sub-codes.
        // (Closed flag 70 bit1.) See post-loop pass.
        break;
      }
      case "SPLINE": {
        // Prefer fit points (11/21); fall back to control points (10/20).
        const fx = e.codes.get(11) ?? [];
        const fy = e.codes.get(21) ?? [];
        let xs = fx;
        let ys = fy;
        if (xs.length < 2) {
          xs = e.codes.get(10) ?? [];
          ys = e.codes.get(20) ?? [];
        }
        const n = Math.min(xs.length, ys.length);
        const pts: Pt[] = [];
        for (let i = 0; i < n; i++) {
          const x = Number(xs[i]);
          const y = Number(ys[i]);
          if (isFiniteNum(x) && isFiniteNum(y)) pts.push([x, y]);
        }
        out.push(...lineChain(pts, false));
        break;
      }
      default:
        // Unknown entity (TEXT, INSERT, …) and VERTEX/SEQEND handled below.
        break;
    }
  }

  // Legacy POLYLINE → VERTEX → SEQEND pass: walk the entity list and join
  // consecutive VERTEX entities that follow a POLYLINE into a line chain.
  for (let i = 0; i < entities.length; i++) {
    if (entities[i].type !== "POLYLINE") continue;
    const closed = (dxfNum(entities[i], 70) & 1) === 1;
    const pts: Pt[] = [];
    let j = i + 1;
    for (; j < entities.length && entities[j].type === "VERTEX"; j++) {
      const x = dxfNum(entities[j], 10);
      const y = dxfNum(entities[j], 20);
      if (isFiniteNum(x) && isFiniteNum(y)) pts.push([x, y]);
    }
    out.push(...lineChain(pts, closed));
  }

  return out;
}

/* ───────────────────────── Dispatcher ───────────────────────── */

/** Choose a parser by file extension, falling back to content sniffing
 *  (`<svg` → SVG; a DXF `SECTION` marker → DXF). Returns `[]` for
 *  anything unrecognised. */
export function importVectorToSubCalls(
  text: string,
  filename: string,
  opts: ImportOpts = {},
): SubCall[] {
  const ext = filename.toLowerCase().split(".").pop();
  if (ext === "svg") return svgToSubCalls(text, opts);
  if (ext === "dxf") return dxfToSubCalls(text, opts);
  // Sniff: SVG markup vs DXF group-code stream.
  if (/<svg[\s>]/i.test(text)) return svgToSubCalls(text, opts);
  if (/\bSECTION\b/.test(text) && /\bENTITIES\b/.test(text)) {
    return dxfToSubCalls(text, opts);
  }
  return [];
}
