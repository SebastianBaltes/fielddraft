/**
 * Onshape sketch → FieldDraft sketch sub-calls (import spike, ticket:
 * Onshape-Import, docs/ONSHAPE-IMPORT-PLAN.md).
 *
 * Onshape's `.../partstudios/.../features` endpoint returns the feature tree
 * in its internal BTM format. A sketch is a `BTMSketch-151` (featureType
 * `newSketch`) carrying two arrays: `geometry` (curves + points) and
 * `constraints`. This module translates ONE sketch feature into the flat
 * `sketch_*` sub-call list a FieldDraft sketch session consumes.
 *
 * Scope (spike): geometry — lines, full circles, arcs, standalone points —
 * plus the positional (value-free) constraints. Everything else is reported
 * in `diagnostics.unhandled` rather than silently dropped, so a run against a
 * real document tells us exactly what to add next.
 *
 * Units: Onshape sketch coordinates are METRES in the sketch's local 2D
 * frame; FieldDraft (u,v) is millimetres. Every length is scaled ×1000 here.
 *
 * Entity-id mapping: Onshape references entities by STRING id ("Line1",
 * "Line1.start"); FieldDraft references by the INTEGER id that
 * `buildDraftSketchState` allocates (origin = 0, then per sub-call in
 * emission order: point → [point]; line → [start, end, line]; circle →
 * [centre, circle]; arc → [centre, start, end, arc]). We emit standalone
 * points first, then curves, then constraints, tracking every Onshape id →
 * FieldDraft id so constraints resolve deterministically.
 */

export interface SketchSubCall {
  type: string;
  params: Record<string, unknown>;
}

/** Metres → millimetres. Onshape stores sketch geometry in metres. */
const M_TO_MM = 1000;

// ── Onshape BTM types (the subset we read) ─────────────────────────

interface OnshapeVec2Line {
  btType: string; // "BTCurveGeometryLine-117"
  pntX: number;
  pntY: number;
  dirX: number;
  dirY: number;
}

interface OnshapeCircleGeom {
  btType: string; // "BTCurveGeometryCircle-115"
  xCenter: number;
  yCenter: number;
  radius: number;
  /** The circle's OWN x axis in sketch coordinates (default +x). */
  xDir?: number;
  yDir?: number;
  /** Handedness of that frame: `true` means the parameter angle turns
   *  CLOCKWISE in the sketch plane. See {@link arcWorldSpan}. */
  clockwise?: boolean;
}

interface OnshapeEllipseGeom {
  btType: string; // "BTCurveGeometryEllipse-1189"
  xCenter: number;
  yCenter: number;
  /** Semi-MAJOR radius, along the ellipse's own x axis (`xDir`/`yDir`). */
  radius: number;
  /** Semi-MINOR radius, perpendicular to it. */
  minorRadius: number;
  xDir?: number;
  yDir?: number;
  clockwise?: boolean;
}

type OnshapeCurveGeom =
  | OnshapeVec2Line
  | OnshapeCircleGeom
  | OnshapeEllipseGeom
  | { btType: string };

interface OnshapeSketchEntity {
  btType: string; // "BTMSketchCurveSegment-155" | "BTMSketchCurve-4" | "BTMSketchPoint-158"
  entityId?: string;
  isConstruction?: boolean;
  // curve segment / curve
  startPointId?: string;
  endPointId?: string;
  centerId?: string;
  startParam?: number;
  endParam?: number;
  geometry?: OnshapeCurveGeom;
  // sketch point
  x?: number;
  y?: number;
}

interface OnshapeConstraintParam {
  btType: string; // "BTMParameterString-149" | "BTMParameterQuantity-147" | …
  parameterId?: string;
  /** BTMParameterString: the referenced entity id. BTMParameterEnum: the tag. */
  value?: string | number;
  /** BTMParameterQuantity: the authored value, e.g. `"2 mm"` / `"45 deg"`. */
  expression?: string;
}

interface OnshapeConstraint {
  btType: string; // "BTMSketchConstraint-2"
  constraintType?: string; // "COINCIDENT" | "HORIZONTAL" | …
  parameters?: OnshapeConstraintParam[];
  entityId?: string;
}

export interface OnshapeSketchFeature {
  btType?: string; // "BTMSketch-151"
  featureType?: string; // "newSketch"
  featureId?: string;
  name?: string;
  /** Solved sketch geometry (the endpoint returns this under `geometry` on
   *  the sketch, or the caller passes the sketch's `entities`). */
  geometry?: OnshapeSketchEntity[];
  entities?: OnshapeSketchEntity[];
  constraints?: OnshapeConstraint[];
}

export interface SketchTranslation {
  subCalls: SketchSubCall[];
  /** Onshape entityId → FieldDraft solver entity id, for every entity the
   *  translation emitted (curve ids included). The plane translator uses
   *  this to turn an Onshape `SWEPT_FACE` origin (`<wire>.<suffix>`) into
   *  the FieldDraft wall ordinal (`qWall(op, <curve id>)`). */
  entityIds: Record<string, number>;
  diagnostics: {
    /** Onshape btType / constraintType strings we didn't translate, with a
     *  count each — the to-do list a real document surfaces. */
    unhandled: Record<string, number>;
    /** Human-readable notes (skipped degenerate entities, origin remaps, …). */
    notes: string[];
  };
}

// ── helpers ────────────────────────────────────────────────────────

function isLineGeom(g: OnshapeCurveGeom | undefined): g is OnshapeVec2Line {
  return !!g && g.btType.startsWith("BTCurveGeometryLine");
}
function isCircleGeom(g: OnshapeCurveGeom | undefined): g is OnshapeCircleGeom {
  return !!g && g.btType.startsWith("BTCurveGeometryCircle");
}
function isEllipseGeom(g: OnshapeCurveGeom | undefined): g is OnshapeEllipseGeom {
  return !!g && g.btType.startsWith("BTCurveGeometryEllipse");
}

/**
 * The SKETCH-space angular span of an Onshape circle segment (#444).
 *
 * `startParam` / `endParam` are angles in the CIRCLE'S OWN frame, not in the
 * sketch: `xDir`/`yDir` carry that frame's x axis and `clockwise` its
 * HANDEDNESS. Reading the parameters as sketch angles is right only for the
 * default frame (+x, counter-clockwise) — and every arc that came out of a
 * mirror carries a REFLECTED one, where the same parameters run the other way.
 *
 * What that cost, measured on #419 `Sketch 4`: the sketch draws four identical
 * keyhole slots, one drawn and three mirrored copies. The three copies' arcs
 * landed on the mirror image of their true position, so their tangent lines no
 * longer met them — the closed slot outlines fell open, and the imprint
 * arrangement lost the faces they bound (11 atomic faces where the symmetric
 * sketch has 15, two of them 0.03 mm² slivers cut by an arc that crossed a line
 * it should have ended on). `Extrude 20` then covered the whole plate instead
 * of its frame, and the walls `Thicken 7` names on the inner rectangle never
 * existed (`qWall(31, 29)` / `qWall(31, 11)` bound nothing).
 *
 * The parameters are INVARIANT under the sketch-frame alignment
 * (`transformSketchFeature`) — that rigid map moves `xDir`/`yDir` and, when it
 * reflects, flips `clockwise`. Local angles stay local.
 */
export function arcWorldSpan(
  g: { xDir?: number; yDir?: number; clockwise?: boolean },
  startParam: number,
  endParam: number,
): { startAngle: number; sweep: number } {
  const phi = Math.atan2(g.yDir ?? 0, g.xDir ?? 1);
  const hand = g.clockwise ? -1 : 1;
  // Onshape stores the parameters ascending; a span that comes out ≤ 0 crossed
  // the ±π seam and is the same normalisation the CCW-only reading did.
  let span = endParam - startParam;
  while (span <= 0) span += Math.PI * 2;
  return { startAngle: phi + hand * startParam, sweep: hand * span };
}

// Scale to mm and round to 1e-9 mm (picometre) — kills the float noise a bare
// `0.01 * 1000` leaves (→ 10.000000000000002) without touching CAD precision.
const mm = (v: number): number => Math.round(v * M_TO_MM * 1e9) / 1e9;

/** Onshape sketch origin point ids that map to FieldDraft's origin (id 0). */
const ORIGIN_IDS = new Set(["origin", "Origin", "sketchOrigin"]);

// ── translation ────────────────────────────────────────────────────

/**
 * Translate one Onshape sketch feature into FieldDraft sub-calls.
 *
 * Pure and dependency-free: feed it the sketch feature JSON from the
 * `features` endpoint, get back the sub-calls a session would push plus a
 * diagnostics report of everything left untranslated.
 */
export function translateOnshapeSketch(
  sketch: OnshapeSketchFeature,
): SketchTranslation {
  const entities = sketch.geometry ?? sketch.entities ?? [];
  const constraints = sketch.constraints ?? [];
  const unhandled: Record<string, number> = {};
  const notes: string[] = [];
  const bump = (key: string) => {
    unhandled[key] = (unhandled[key] ?? 0) + 1;
  };

  // Onshape id (entityId / pointId) → FieldDraft integer entity id.
  const idMap = new Map<string, number>();
  for (const oid of ORIGIN_IDS) idMap.set(oid, 0);
  // line entity id → its two endpoint entity ids. A LENGTH dimension names the
  // LINE, but the kernel measures a distance between two POINTS.
  const endpointsOf = new Map<number, [number, number]>();

  const subCalls: SketchSubCall[] = [];
  // FieldDraft allocates ids in emission order starting at 1 (origin = 0).
  let nextId = 1;

  // Point ids consumed as a curve's start/end — those points come WITH their
  // curve in FieldDraft, so we never emit a standalone sketch_point for them.
  const curvePointIds = new Set<string>();
  for (const e of entities) {
    if (e.startPointId) curvePointIds.add(e.startPointId);
    if (e.endPointId) curvePointIds.add(e.endPointId);
  }

  const point2 = (e: OnshapeSketchEntity): [number, number] | null => {
    if (typeof e.x === "number" && typeof e.y === "number") {
      return [mm(e.x), mm(e.y)];
    }
    return null;
  };

  // Pass 1: standalone points (not curve endpoints, not the sketch origin).
  for (const e of entities) {
    if (!e.btType.startsWith("BTMSketchPoint")) continue;
    const id = e.entityId;
    if (id && (curvePointIds.has(id) || ORIGIN_IDS.has(id))) continue;
    const at = point2(e);
    if (!at) {
      bump("BTMSketchPoint(no-coords)");
      continue;
    }
    subCalls.push({
      type: "sketch_point",
      params: withConstruction({ at }, e),
    });
    if (id) idMap.set(id, nextId);
    nextId += 1; // point
  }

  // Pass 2: curves.
  for (const e of entities) {
    if (e.btType.startsWith("BTMSketchPoint")) continue;
    const isSegment = e.btType.startsWith("BTMSketchCurveSegment");
    const isCurve = e.btType.startsWith("BTMSketchCurve"); // unbounded (full circle)

    if (!isSegment && !isCurve) {
      bump(e.btType);
      continue;
    }

    if (isLineGeom(e.geometry)) {
      const g = e.geometry;
      const s = e.startParam ?? 0;
      const t = e.endParam ?? 0;
      const start: [number, number] = [mm(g.pntX + g.dirX * s), mm(g.pntY + g.dirY * s)];
      const end: [number, number] = [mm(g.pntX + g.dirX * t), mm(g.pntY + g.dirY * t)];
      subCalls.push({ type: "sketch_line", params: withConstruction({ start, end }, e) });
      if (e.startPointId) idMap.set(e.startPointId, nextId);
      if (e.endPointId) idMap.set(e.endPointId, nextId + 1);
      if (e.entityId) {
        idMap.set(e.entityId, nextId + 2);
        idMap.set(`${e.entityId}.start`, nextId);
        idMap.set(`${e.entityId}.end`, nextId + 1);
        endpointsOf.set(nextId + 2, [nextId, nextId + 1]);
      }
      nextId += 3; // start, end, line
      continue;
    }

    if (isCircleGeom(e.geometry)) {
      const g = e.geometry;
      const center: [number, number] = [mm(g.xCenter), mm(g.yCenter)];
      const radius = mm(g.radius);
      if (isCurve && !isSegment) {
        // Full circle. The kernel allocates centre, then the circle.
        subCalls.push({ type: "sketch_circle", params: withConstruction({ center, radius }, e) });
        if (e.centerId) idMap.set(e.centerId, nextId);
        if (e.entityId) {
          idMap.set(e.entityId, nextId + 1);
          // Onshape addresses a curve's own points as `<entityId>.center`.
          idMap.set(`${e.entityId}.center`, nextId);
        }
        nextId += 2; // centre, circle
        continue;
      }
      // Bounded → arc. The DIRECTION is the circle frame's handedness in both
      // readings below: an arc is not „always CCW", it runs the way its own
      // frame does (#444).
      const hand = g.clockwise ? -1 : 1;
      // Prefer angles from the referenced start/end POINT coordinates
      // (robust); fall back to startParam/endParam, which are angles in that
      // same frame.
      const sp = findPointCoords(entities, e.startPointId);
      const ep = findPointCoords(entities, e.endPointId);
      let startAngle: number;
      let sweep: number;
      if (sp && ep) {
        const a0 = Math.atan2(sp[1] - g.yCenter, sp[0] - g.xCenter);
        const a1 = Math.atan2(ep[1] - g.yCenter, ep[0] - g.xCenter);
        startAngle = a0;
        sweep = a1 - a0;
        if (hand > 0) while (sweep <= 0) sweep += Math.PI * 2;
        else while (sweep >= 0) sweep -= Math.PI * 2;
      } else {
        const span = arcWorldSpan(g, e.startParam ?? 0, e.endParam ?? 0);
        startAngle = span.startAngle;
        sweep = span.sweep;
      }
      subCalls.push({
        type: "sketch_arc",
        params: withConstruction(
          { center, radius, start_angle: startAngle, sweep_angle: sweep },
          e,
        ),
      });
      // The kernel allocates centre, start, end, arc — in that order. Register
      // all four: Onshape's fillet constraints tie the ARC'S ENDPOINTS to the
      // adjoining lines (`<arc>.start` ↔ `<line>.end`), so an arc that only
      // registers its curve id loses every one of those coincidences and the
      // rounded corner falls apart under the solver.
      if (e.centerId) idMap.set(e.centerId, nextId);
      if (e.startPointId) idMap.set(e.startPointId, nextId + 1);
      if (e.endPointId) idMap.set(e.endPointId, nextId + 2);
      if (e.entityId) {
        idMap.set(e.entityId, nextId + 3);
        idMap.set(`${e.entityId}.center`, nextId);
        idMap.set(`${e.entityId}.start`, nextId + 1);
        idMap.set(`${e.entityId}.end`, nextId + 2);
      }
      nextId += 4; // centre, start, end, arc
      continue;
    }

    if (isEllipseGeom(e.geometry)) {
      const g = e.geometry;
      // FieldDraft has no elliptical ARC: `sketch_ellipse` is the whole,
      // closed curve. A bounded segment is reported rather than silently
      // closed into a full ellipse, which would invent area nobody drew.
      if (isSegment) {
        bump(`${g.btType}(arc)`);
        continue;
      }
      // Onshape's `radius` is the semi-MAJOR axis and lies along the ellipse's
      // own x axis (`xDir`/`yDir`); `minorRadius` is perpendicular to it. That
      // is exactly FieldDraft's `(radius_x, radius_y, rotation)` frame, so the
      // mapping is a rename plus one `atan2` — no swap, no half-turn.
      //
      // `clockwise` says which way the PARAMETER runs and cannot change the
      // shape of a closed curve, so it is not read here. It does matter to the
      // region FLANK, which is measured off the flattened rim in
      // `sketchEntityVertices` (#446) — that reader honours it.
      subCalls.push({
        type: "sketch_ellipse",
        params: withConstruction(
          {
            center: [mm(g.xCenter), mm(g.yCenter)],
            radius_x: mm(g.radius),
            radius_y: mm(g.minorRadius),
            rotation: Math.atan2(g.yDir ?? 0, g.xDir ?? 1),
          },
          e,
        ),
      });
      // The kernel allocates centre, FOCUS, ellipse — PlaneGCS parameterises an
      // ellipse by centre + one focus + semi-minor radius, and that focus is a
      // real entity in the shared id space. Onshape names no focus, so only the
      // centre and the curve itself get an id here.
      if (e.centerId) idMap.set(e.centerId, nextId);
      if (e.entityId) {
        idMap.set(e.entityId, nextId + 2);
        idMap.set(`${e.entityId}.center`, nextId);
      }
      nextId += 3; // centre, focus, ellipse
      continue;
    }

    // Splines, conics, interpolated curves — not in the spike yet.
    bump(e.geometry?.btType ?? `${e.btType}(no-geometry)`);
  }

  // Pass 3: constraints.
  for (const c of constraints) {
    const call = translateConstraint(c, idMap, endpointsOf, notes);
    if (call) subCalls.push(call);
    else bump(`constraint:${c.constraintType ?? c.btType}`);
  }

  return {
    subCalls,
    entityIds: Object.fromEntries(idMap),
    diagnostics: { unhandled, notes },
  };
}

function withConstruction(
  params: Record<string, unknown>,
  e: OnshapeSketchEntity,
): Record<string, unknown> {
  return e.isConstruction ? { ...params, construction: true } : params;
}

function findPointCoords(
  entities: OnshapeSketchEntity[],
  pointId: string | undefined,
): [number, number] | null {
  if (!pointId) return null;
  const p = entities.find((e) => e.entityId === pointId && typeof e.x === "number");
  return p && typeof p.x === "number" && typeof p.y === "number" ? [p.x, p.y] : null;
}

/** Onshape quantity expression → millimetres (`"2 mm"`, `"0.5 in"`, `"1 cm"`). */
const LEN_TO_MM: Record<string, number> = {
  mm: 1,
  millimeter: 1,
  cm: 10,
  centimeter: 10,
  m: 1000,
  meter: 1000,
  in: 25.4,
  inch: 25.4,
  ft: 304.8,
};

function quantityMm(expr: string | undefined): number | null {
  const m = expr?.match(/^\s*(-?[\d.]+)\s*\*?\s*([a-zA-Z]+)\s*$/);
  if (!m || LEN_TO_MM[m[2]] === undefined) return null;
  return parseFloat(m[1]) * LEN_TO_MM[m[2]];
}

/** Onshape angle expression → degrees (`"45 deg"`, `"0.5 rad"`). */
function quantityDeg(expr: string | undefined): number | null {
  const m = expr?.match(/^\s*(-?[\d.]+)\s*\*?\s*([a-zA-Z]+)\s*$/);
  if (!m) return null;
  const v = parseFloat(m[1]);
  if (/^deg/.test(m[2])) return v;
  if (/^rad/.test(m[2])) return (v * 180) / Math.PI;
  return null;
}

/**
 * Translate one Onshape constraint.
 *
 * Entity references come from the `BTMParameterString` params
 * (`localFirst`/`localSecond`/… → an entityId) and resolve through `idMap`;
 * a DIMENSION additionally carries its value in a `BTMParameterQuantity`
 * (`length`/`angle`). Returns `null` for unmapped ids or types with no
 * FieldDraft counterpart — the caller counts those as unhandled.
 */
function translateConstraint(
  c: OnshapeConstraint,
  idMap: Map<string, number>,
  endpointsOf: Map<number, [number, number]>,
  notes: string[],
): SketchSubCall | null {
  const refs = new Map<string, number>();
  const quantities = new Map<string, string>();
  for (const p of c.parameters ?? []) {
    if (!p.parameterId) continue;
    if (p.btType.startsWith("BTMParameterQuantity")) {
      if (typeof p.expression === "string") quantities.set(p.parameterId, p.expression);
      continue;
    }
    if (!p.btType.startsWith("BTMParameterString")) continue;
    if (typeof p.value !== "string") continue;
    // `sketchToolType` and friends are enums, not refs — only the *local*
    // reference slots address entities.
    if (!/^(local|external)/.test(p.parameterId)) continue;
    const fid = idMap.get(p.value);
    if (fid === undefined) {
      notes.push(`constraint ${c.constraintType}: unresolved ref ${p.value}`);
      return null;
    }
    refs.set(p.parameterId, fid);
  }
  const first = refs.get("localFirst");
  const second = refs.get("localSecond");
  const single = refs.get("localFirst") ?? [...refs.values()][0];

  switch (c.constraintType) {
    case "COINCIDENT":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_coincident", params: { a: first, b: second } };
      return null;
    case "HORIZONTAL":
      if (single !== undefined)
        return { type: "sketch_constraint_horizontal", params: { line: single } };
      return null;
    case "VERTICAL":
      if (single !== undefined)
        return { type: "sketch_constraint_vertical", params: { line: single } };
      return null;
    case "PARALLEL":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_parallel", params: { line_a: first, line_b: second } };
      return null;
    case "PERPENDICULAR":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_perpendicular", params: { line_a: first, line_b: second } };
      return null;
    case "CONCENTRIC":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_concentric", params: { a: first, b: second } };
      return null;
    case "EQUAL":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_equal", params: { entity_a: first, entity_b: second } };
      return null;
    case "TANGENT":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_tangent", params: { line: first, curve: second } };
      return null;
    case "MIDPOINT":
      if (first !== undefined && second !== undefined)
        return { type: "sketch_constraint_midpoint", params: { point: first, line: second } };
      return null;
    case "FIX":
      if (single !== undefined)
        return { type: "sketch_constraint_fix", params: { point: single } };
      return null;
    case "SILHOUETTED":
    case "POINT_ON_OBJECT":
      if (first !== undefined && second !== undefined)
        return {
          type: "sketch_constraint_point_on_object",
          params: { point: first, curve: second },
        };
      return null;

    // ── dimensions: the value rides in a BTMParameterQuantity ──────────
    case "DISTANCE": {
      const mmv = quantityMm(quantities.get("length"));
      if (first === undefined || second === undefined || mmv === null) return null;
      return {
        type: "sketch_constraint_distance",
        params: { entity_a: first, entity_b: second, value_mm: mmv },
      };
    }
    case "LENGTH": {
      // Onshape dimensions the LINE; the kernel measures point-to-point, so
      // resolve the line to the two endpoints it allocated.
      const mmv = quantityMm(quantities.get("length"));
      const ends = single !== undefined ? endpointsOf.get(single) : undefined;
      if (!ends || mmv === null) {
        if (!ends && single !== undefined) {
          notes.push(`constraint LENGTH: entity ${single} is not a line with endpoints`);
        }
        return null;
      }
      return {
        type: "sketch_constraint_distance",
        params: { entity_a: ends[0], entity_b: ends[1], value_mm: mmv },
      };
    }
    case "ANGLE": {
      const deg = quantityDeg(quantities.get("angle")) ?? quantityDeg(quantities.get("length"));
      if (first === undefined || second === undefined || deg === null) return null;
      return {
        type: "sketch_constraint_angle",
        params: { line_a: first, line_b: second, value_deg: deg },
      };
    }
    case "RADIUS": {
      const mmv = quantityMm(quantities.get("length"));
      if (single === undefined || mmv === null) return null;
      return { type: "sketch_constraint_radius", params: { entity: single, value_mm: mmv } };
    }
    case "DIAMETER": {
      const mmv = quantityMm(quantities.get("length"));
      if (single === undefined || mmv === null) return null;
      return { type: "sketch_constraint_diameter", params: { entity: single, value_mm: mmv } };
    }

    default:
      // Still open: MIRROR (Onshape mirrors ABOUT AN EXTERNAL LINE — FieldDraft's
      // `symmetric` needs an in-sketch axis entity, which an origin axis is not),
      // PROJECTED (ties a sketch curve to an external body edge; the geometry is
      // already imported, only the associativity is lost), SYMMETRIC.
      return null;
  }
}
