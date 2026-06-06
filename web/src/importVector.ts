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
 * ── Curve tessellation ─────────────────────────────────────────────────
 *  There is no Bézier / spline sub-call, so every cubic / quadratic Bézier,
 *  smooth-curve and SVG elliptical-arc segment is flattened into a chain of
 *  short `sketch_line` segments. We use a FIXED step count
 *  (`CURVE_STEPS = 16`) per curve segment — simple and predictable; fine for
 *  import where the user can refine afterwards. SPLINEs in DXF connect their
 *  fit points (or, lacking those, control points) directly with lines.
 */

export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

export interface ImportOpts {
  /** Bézier / elliptical-arc tessellation steps per segment. Default 16. */
  curveSteps?: number;
}

/** Default tessellation step count per curve segment. */
export const CURVE_STEPS = 16;

type Pt = [number, number];

/** Negate y to convert SVG (y-down) into sketch (y-up) space. */
function flipY(p: Pt): Pt {
  return [p[0], -p[1]];
}

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

/** Sample a cubic Bézier P0→P3 (controls P1, P2) into `steps` line pts,
 *  excluding the start point (caller already has it). */
function cubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt, steps: number): Pt[] {
  const out: Pt[] = [];
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    const mt = 1 - t;
    const a = mt * mt * mt;
    const b = 3 * mt * mt * t;
    const c = 3 * mt * t * t;
    const d = t * t * t;
    out.push([
      a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
      a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
    ]);
  }
  return out;
}

/** Sample a quadratic Bézier P0→P2 (control P1) into `steps` line pts,
 *  excluding the start point. */
function quad(p0: Pt, p1: Pt, p2: Pt, steps: number): Pt[] {
  const out: Pt[] = [];
  for (let i = 1; i <= steps; i++) {
    const t = i / steps;
    const mt = 1 - t;
    out.push([
      mt * mt * p0[0] + 2 * mt * t * p1[0] + t * t * p2[0],
      mt * mt * p0[1] + 2 * mt * t * p1[1] + t * t * p2[1],
    ]);
  }
  return out;
}

function isFiniteNum(n: number): boolean {
  return Number.isFinite(n);
}

/* ───────────────────────── SVG ───────────────────────── */

/** Parse a `transform` attribute into a 2×3 affine matrix [a,b,c,d,e,f]
 *  mapping (x,y) → (a·x + c·y + e, b·x + d·y + f). Supports translate,
 *  scale, and matrix; unsupported ops (rotate/skew) are skipped — a
 *  documented v1 limitation. Returns identity when empty/unparseable. */
type Mat = [number, number, number, number, number, number];
const IDENTITY: Mat = [1, 0, 0, 1, 0, 0];

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
    }
    // rotate / skewX / skewY: documented limitation — ignored.
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
  const nums = attr.split(/[\s,]+/).map(Number).filter(isFiniteNum);
  const pts: Pt[] = [];
  for (let i = 0; i + 1 < nums.length; i += 2) pts.push([nums[i], nums[i + 1]]);
  return pts;
}

interface SvgWalk {
  calls: SubCall[];
  skipped: number;
}

function walkSvg(el: Element, mat: Mat, steps: number, out: SvgWalk): void {
  const tag = el.tagName.toLowerCase();
  // Apply this element's own transform on top of the inherited matrix.
  const local = parseTransform(el.getAttribute("transform"));
  const m = matMul(mat, local);
  const xf = (p: Pt): Pt => flipY(applyMat(m, p));

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
      // Rounded rects (rx/ry) are flattened to sharp corners in v1.
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
    case "circle": {
      const cx = num(el, "cx");
      const cy = num(el, "cy");
      const r = num(el, "r");
      if (!(r > 0)) {
        out.skipped++;
        return;
      }
      out.calls.push({
        type: "sketch_circle",
        params: { center: xf([cx, cy]), radius: r },
      });
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
      out.calls.push({
        type: "sketch_ellipse",
        params: {
          center: xf([cx, cy]),
          radius_x: rx,
          radius_y: ry,
          // rotation negated: SVG y-down flip mirrors the angle.
          rotation: 0,
          segments: Math.max(16, steps * 2),
        },
      });
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
      out.calls.push(...parsePath(el.getAttribute("d") ?? "", xf, steps));
      return;
    }
    default:
      // Unknown / unsupported element (text, image, use, defs, …).
      out.skipped++;
      return;
  }
}

/** Tokenise + walk an SVG path `d` string. Emits lines for L/H/V, line
 *  chains for tessellated C/S/Q/T and A, and closes on Z. The `xf`
 *  transform is applied (matrix + y-flip) to every output point; curve
 *  math runs in the path's own coordinate space first so tessellation
 *  is correct, then each sampled point goes through `xf`. */
function parsePath(d: string, xf: (p: Pt) => Pt, steps: number): SubCall[] {
  // Split into command tokens: a letter followed by its number run.
  const cmdRe = /([MmLlHhVvCcSsQqTtAaZz])([^MmLlHhVvCcSsQqTtAaZz]*)/g;
  const out: SubCall[] = [];
  let cur: Pt = [0, 0]; // current point, RAW (pre-xf) coordinates
  let sub: Pt = [0, 0]; // subpath start, RAW
  let prevCubicCtrl: Pt | null = null; // last cubic control reflection
  let prevQuadCtrl: Pt | null = null; // last quad control reflection
  let chain: Pt[] = []; // current subpath point chain (RAW)

  const flushChain = (close: boolean) => {
    if (chain.length >= 2) {
      out.push(...lineChain(chain.map(xf) as Pt[], close));
    }
    chain = [];
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

    const startChainIfEmpty = () => {
      if (chain.length === 0) chain.push([cur[0], cur[1]]);
    };

    if (up === "M") {
      // First pair = moveto; subsequent pairs are implicit linetos.
      if (args.length >= 2) {
        flushChain(false);
        cur = rel ? [cur[0] + args[0], cur[1] + args[1]] : [args[0], args[1]];
        sub = [cur[0], cur[1]];
        chain = [[cur[0], cur[1]]];
        i = 2;
        for (; i + 1 < args.length; i += 2) {
          cur = rel
            ? [cur[0] + args[i], cur[1] + args[i + 1]]
            : [args[i], args[i + 1]];
          chain.push([cur[0], cur[1]]);
        }
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "L") {
      startChainIfEmpty();
      for (; i + 1 < args.length; i += 2) {
        cur = rel
          ? [cur[0] + args[i], cur[1] + args[i + 1]]
          : [args[i], args[i + 1]];
        chain.push([cur[0], cur[1]]);
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "H") {
      startChainIfEmpty();
      for (; i < args.length; i++) {
        cur = rel ? [cur[0] + args[i], cur[1]] : [args[i], cur[1]];
        chain.push([cur[0], cur[1]]);
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "V") {
      startChainIfEmpty();
      for (; i < args.length; i++) {
        cur = rel ? [cur[0], cur[1] + args[i]] : [cur[0], args[i]];
        chain.push([cur[0], cur[1]]);
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "C") {
      startChainIfEmpty();
      for (; i + 5 < args.length; i += 6) {
        const p1: Pt = rel ? [cur[0] + args[i], cur[1] + args[i + 1]] : [args[i], args[i + 1]];
        const p2: Pt = rel ? [cur[0] + args[i + 2], cur[1] + args[i + 3]] : [args[i + 2], args[i + 3]];
        const p3: Pt = rel ? [cur[0] + args[i + 4], cur[1] + args[i + 5]] : [args[i + 4], args[i + 5]];
        chain.push(...cubic(cur, p1, p2, p3, steps));
        cur = p3;
        prevCubicCtrl = p2;
      }
      prevQuadCtrl = null;
    } else if (up === "S") {
      startChainIfEmpty();
      for (; i + 3 < args.length; i += 4) {
        const reflect: Pt = prevCubicCtrl
          ? [2 * cur[0] - prevCubicCtrl[0], 2 * cur[1] - prevCubicCtrl[1]]
          : [cur[0], cur[1]];
        const p2: Pt = rel ? [cur[0] + args[i], cur[1] + args[i + 1]] : [args[i], args[i + 1]];
        const p3: Pt = rel ? [cur[0] + args[i + 2], cur[1] + args[i + 3]] : [args[i + 2], args[i + 3]];
        chain.push(...cubic(cur, reflect, p2, p3, steps));
        cur = p3;
        prevCubicCtrl = p2;
      }
      prevQuadCtrl = null;
    } else if (up === "Q") {
      startChainIfEmpty();
      for (; i + 3 < args.length; i += 4) {
        const p1: Pt = rel ? [cur[0] + args[i], cur[1] + args[i + 1]] : [args[i], args[i + 1]];
        const p2: Pt = rel ? [cur[0] + args[i + 2], cur[1] + args[i + 3]] : [args[i + 2], args[i + 3]];
        chain.push(...quad(cur, p1, p2, steps));
        cur = p2;
        prevQuadCtrl = p1;
      }
      prevCubicCtrl = null;
    } else if (up === "T") {
      startChainIfEmpty();
      for (; i + 1 < args.length; i += 2) {
        const p1: Pt = prevQuadCtrl
          ? [2 * cur[0] - prevQuadCtrl[0], 2 * cur[1] - prevQuadCtrl[1]]
          : [cur[0], cur[1]];
        const p2: Pt = rel ? [cur[0] + args[i], cur[1] + args[i + 1]] : [args[i], args[i + 1]];
        chain.push(...quad(cur, p1, p2, steps));
        cur = p2;
        prevQuadCtrl = p1;
      }
      prevCubicCtrl = null;
    } else if (up === "A") {
      startChainIfEmpty();
      for (; i + 6 < args.length; i += 7) {
        const rx = args[i];
        const ry = args[i + 1];
        const xRot = (args[i + 2] * Math.PI) / 180;
        const largeArc = args[i + 3] !== 0;
        const sweep = args[i + 4] !== 0;
        const end: Pt = rel
          ? [cur[0] + args[i + 5], cur[1] + args[i + 6]]
          : [args[i + 5], args[i + 6]];
        chain.push(...arcToPoints(cur, rx, ry, xRot, largeArc, sweep, end, steps));
        cur = end;
      }
      prevCubicCtrl = prevQuadCtrl = null;
    } else if (up === "Z") {
      // Close: append segment back to subpath start, flush as closed.
      flushChain(true);
      cur = [sub[0], sub[1]];
      prevCubicCtrl = prevQuadCtrl = null;
    }
  }
  flushChain(false);
  return out;
}

/** Tessellate an SVG elliptical-arc (endpoint parameterisation) into
 *  sampled points, excluding the start point. Implements the SVG spec's
 *  endpoint→center conversion (w3.org/TR/SVG/implnote.html). */
function arcToPoints(
  start: Pt,
  rxIn: number,
  ryIn: number,
  phi: number,
  largeArc: boolean,
  sweep: boolean,
  end: Pt,
  steps: number,
): Pt[] {
  let rx = Math.abs(rxIn);
  let ry = Math.abs(ryIn);
  if (rx === 0 || ry === 0) return [end]; // degenerate → straight line
  const cosP = Math.cos(phi);
  const sinP = Math.sin(phi);
  const dx = (start[0] - end[0]) / 2;
  const dy = (start[1] - end[1]) / 2;
  const x1p = cosP * dx + sinP * dy;
  const y1p = -sinP * dx + cosP * dy;
  // Correct out-of-range radii.
  const lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
  if (lambda > 1) {
    const s = Math.sqrt(lambda);
    rx *= s;
    ry *= s;
  }
  const sign = largeArc !== sweep ? 1 : -1;
  const num =
    rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
  const den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
  const co = sign * Math.sqrt(Math.max(0, num / den));
  const cxp = (co * rx * y1p) / ry;
  const cyp = (-co * ry * x1p) / rx;
  const cx = cosP * cxp - sinP * cyp + (start[0] + end[0]) / 2;
  const cy = sinP * cxp + cosP * cyp + (start[1] + end[1]) / 2;

  const angle = (ux: number, uy: number, vx: number, vy: number): number => {
    const dot = ux * vx + uy * vy;
    const len = Math.hypot(ux, uy) * Math.hypot(vx, vy);
    let a = Math.acos(Math.min(1, Math.max(-1, dot / len)));
    if (ux * vy - uy * vx < 0) a = -a;
    return a;
  };
  const theta1 = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry);
  let dTheta = angle(
    (x1p - cxp) / rx,
    (y1p - cyp) / ry,
    (-x1p - cxp) / rx,
    (-y1p - cyp) / ry,
  );
  if (!sweep && dTheta > 0) dTheta -= 2 * Math.PI;
  if (sweep && dTheta < 0) dTheta += 2 * Math.PI;

  const out: Pt[] = [];
  for (let i = 1; i <= steps; i++) {
    const t = theta1 + (dTheta * i) / steps;
    const x = cosP * rx * Math.cos(t) - sinP * ry * Math.sin(t) + cx;
    const y = sinP * rx * Math.cos(t) + cosP * ry * Math.sin(t) + cy;
    out.push([x, y]);
  }
  return out;
}

/** Parse SVG text into sub-calls. Returns `[]` on a missing/garbage root.
 *  Coordinates are y-flipped (SVG y-down → sketch y-up). */
export function svgToSubCalls(svgText: string, opts: ImportOpts = {}): SubCall[] {
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
  walkSvg(root, IDENTITY, steps, out);
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
export function dxfToSubCalls(dxfText: string, opts: ImportOpts = {}): SubCall[] {
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
