/**
 * Ticket matf — rigid translation of a *whole* geometry sub-call.
 *
 * The Move/Copy tool (and now a plain Select-mode drag) translates the
 * current selection by a (dx, dy) offset. The original transform path went
 * through `curveOfSubCall` → `translateCurve`, which only understands lines,
 * circles and arcs — so rectangles, polygons, splines, paths and free points
 * were silently dropped from any move/mirror.
 *
 * This module translates EVERY coordinate field of a geometry sub-call
 * directly on its params, so the full set of sketch primitives moves as one
 * rigid body. It is intentionally DOM/kernel-free so Vitest can pin every
 * kind. Constraint sub-calls (and any kind without coordinates) are returned
 * unchanged — the live solver then pulls the translated geometry back onto
 * whatever constraints forbid the motion (ticket matf: "…solange sie nicht
 * durch Constraints daran gehindert werden").
 */

export type SubCall = { type: string; params: Record<string, unknown> };

/** Offset a `[x, y]` pair if it really is one, else return it untouched. */
function shiftXy(value: unknown, dx: number, dy: number): unknown {
  if (!Array.isArray(value) || value.length !== 2) return value;
  const x = Number(value[0]);
  const y = Number(value[1]);
  if (!Number.isFinite(x) || !Number.isFinite(y)) return value;
  return [x + dx, y + dy];
}

/** Offset a flat `[x0,y0,x1,y1,…]` coordinate array. Returns the input
 *  untouched if it isn't a finite, even-length number array. */
function shiftFlat(value: unknown, dx: number, dy: number): unknown {
  if (!Array.isArray(value) || value.length < 2 || value.length % 2 !== 0) {
    return value;
  }
  const out = value.slice();
  for (let i = 0; i + 1 < out.length; i += 2) {
    const x = Number(out[i]);
    const y = Number(out[i + 1]);
    if (!Number.isFinite(x) || !Number.isFinite(y)) return value;
    out[i] = x + dx;
    out[i + 1] = y + dy;
  }
  return out;
}

/**
 * Return a copy of `call` with every positional field translated by
 * `(dx, dy)`. Angles, radii and the construction flag are invariant under a
 * pure translation, so they carry across untouched. A sub-call kind with no
 * coordinates (a constraint, or an unrecognised geometry kind) is returned
 * structurally unchanged — callers can translate a mixed list uniformly.
 */
export function translateSubCall(
  call: SubCall,
  dx: number,
  dy: number,
): SubCall {
  const p = call.params;
  const replace = (params: Record<string, unknown>): SubCall => ({
    ...call,
    params: { ...p, ...params },
  });
  switch (call.type) {
    case "sketch_point":
      return replace({ at: shiftXy(p.at, dx, dy) });
    case "sketch_line":
      return replace({
        start: shiftXy(p.start, dx, dy),
        end: shiftXy(p.end, dx, dy),
      });
    case "sketch_circle":
    case "sketch_arc":
      // Centre moves; radius / angles are translation-invariant.
      return replace({ center: shiftXy(p.center, dx, dy) });
    case "sketch_rectangle":
      return replace({
        corner_a: shiftXy(p.corner_a, dx, dy),
        corner_b: shiftXy(p.corner_b, dx, dy),
      });
    case "sketch_ellipse":
      // Centre + focus move; the semi-minor radius is invariant.
      return replace({
        center: shiftXy(p.center, dx, dy),
        focus: shiftXy(p.focus, dx, dy),
      });
    case "sketch_polygon":
      return replace({ vertices: shiftFlat(p.vertices, dx, dy) });
    case "sketch_spline":
      return replace({ points: shiftFlat(p.points, dx, dy) });
    case "sketch_path":
      return replace({ contours: translateContours(p.contours, dx, dy) });
    default:
      // Constraints and unknown kinds carry no geometry — leave them be.
      return call;
  }
}

/** Translate every on-/off-curve vertex of a `sketch_path`'s contour list. */
function translateContours(value: unknown, dx: number, dy: number): unknown {
  if (!Array.isArray(value)) return value;
  return value.map((c) => {
    if (typeof c !== "object" || c === null) return c;
    const cc = c as { start?: unknown; segments?: unknown };
    const next: Record<string, unknown> = { ...(c as object) };
    next.start = shiftXy(cc.start, dx, dy);
    if (Array.isArray(cc.segments)) {
      next.segments = cc.segments.map((seg) => {
        if (typeof seg !== "object" || seg === null) return seg;
        const s = seg as Record<string, unknown>;
        const ns: Record<string, unknown> = { ...s };
        if (s.ctrl !== undefined) ns.ctrl = shiftXy(s.ctrl, dx, dy);
        if (s.to !== undefined) ns.to = shiftXy(s.to, dx, dy);
        return ns;
      });
    }
    return next;
  });
}

/** Does this sub-call carry any translatable geometry? Mirrors the kinds
 *  {@link translateSubCall} actually moves — used to skip constraint /
 *  unknown calls when deciding whether a selection can be dragged. */
export function isTranslatableSubCall(call: SubCall): boolean {
  switch (call.type) {
    case "sketch_point":
    case "sketch_line":
    case "sketch_circle":
    case "sketch_arc":
    case "sketch_rectangle":
    case "sketch_ellipse":
    case "sketch_polygon":
    case "sketch_spline":
    case "sketch_path":
      return true;
    default:
      return false;
  }
}
