/**
 * Onshape document import (Onshape-Import Schritt 4).
 *
 * Walks an Onshape Part Studio's feature list IN ORDER and rebuilds it
 * as FieldDraft features against a kernel: sketches through
 * `translateOnshapeSketch`, the solid features (extrude / shell /
 * fillet) through a minimal parameter translation. Sketch planes are
 * translated PARAMETRICALLY (decoded Onshape creation queries →
 * FieldDraft queries: `qCreatedBy(op, CAP_*)`, `qWall(op, entity)`,
 * `qAdjacentTo(...)`) and checked for UNIQUE resolution against the
 * kernel before they are stored — an unresolvable or ambiguous plane is
 * a visible import error, never a silent fall-back to XY.
 */

import {
  decodeEdgeQuery,
  decodeOnshapeQueryString,
  decodePlaneQuery,
  extractQueryRefs,
  tokenizeBton,
  type OnshapeEntityOrigin,
  type OnshapeFaceRef,
  type OnshapePlaneRef,
} from "./onshapeQuery";
import {
  anchorT,
  describeAnchor,
  implicitMateConnectors,
  mateConnectorPlaneId,
  readMateConnector,
  type OnshapeMateConnector,
} from "./onshapeMateConnector";
import { arcWorldSpan, translateOnshapeSketch, type SketchSubCall } from "./onshapeSketch";
import { decodeRegionQuery } from "./onshapeRegion";
import { resolveRegionFaces, type FaceMetrics } from "./onshapeRegionFaces";
import {
  describeDimension,
  quantityKind,
  quantityToDeg,
  quantityToMm,
  toFieldDraftFormula,
  tryEvalOnshapeExpression,
  type Quantity,
} from "./onshapeExpr";
import {
  type CollinearFitReport,
  type RobustFitReport,
  describeXform,
  inPlaneXform,
  measureCollinearSketchFrame,
  measureConcentricSketchOrigin,
  measureOnshapeSketchFrame,
  placeSketchFromThreeD,
  sketches3dByFeature,
  transformSketchFeature,
  type OnshapeSketch3d,
  type OnshapeSketches3d,
} from "./onshapeSketchFrame";
import {
  faceWorldPlanes,
  resolvePlaneFrame,
  type PlaneFrameKernel,
} from "../sketchPlaneFrame";
import { evalFormula, type VarKind } from "../variableInput";

/** The kernel surface the importer drives — matches `Kernel` in
 *  `web/src/kernel.ts`; narrow so tests can mock it. The geometry readers
 *  ({@link PlaneFrameKernel}) are optional: without them the end-condition
 *  translation degrades to a reported approximation instead of failing. */
export interface ImportKernel extends Partial<PlaneFrameKernel> {
  addSketch(planeQuery: string, subCalls: SketchSubCall[]): number;
  addFeatureCall(typeName: string, paramsJson: string): number;
  resolveQuery(query: string): { kind: string; idHash: string; query: string }[];
  /** World AABB of everything built so far — the extent a THROUGH_ALL cut
   *  has to clear. `{min, max, size}` in mm. */
  sceneAabb?(): { min: number[]; max: number[]; size: number[] };
  /** `qRegion(sketchId, i)` for the flood-fill region of `sketchId` that
   *  contains the sketch-space point `(u, v)` in mm, or `""` when the point
   *  is outside every region (ticket #412). Optional: a kernel double
   *  without it degrades to whole-sketch profiles (never wrong regions). */
  regionQueryAtPoint?(sketchId: number, u: number, v: number): string;
  /** JSON array of the sketch's atomic imprint faces (`{index, area, …}`),
   *  in the kernel's deterministic arrangement order (#412). Used to rank
   *  candidate faces by area when resolving an Onshape region selection.
   *  Optional: without it the resolver falls back to edge probing alone. */
  listSketchRegions?(sketchId: number, resolution: number): string;
  /** Toggle the synchronous Dual-Contouring mesh pass. The importer turns it
   *  OFF for the whole walk and back ON once at the end: mid-history query
   *  resolution needs the woven B-Reps (still produced), but NOT a per-feature
   *  display mesh — re-meshing all bodies after every one of N features is the
   *  O(n²) main-thread stall that froze the Onshape import for minutes (#410).
   *  Optional: a kernel double without it just meshes eagerly. */
  setSyncMeshing?(enabled: boolean): boolean | void;
  /** Upsert a DOCUMENT VARIABLE — the target of Onshape's `assignVariable`
   *  (#409/#415/#417). `value` is in FieldDraft's base units (mm for a length,
   *  DEGREES for an angle); `formula` is optional provenance text. Optional:
   *  a kernel double without it still gets every expression evaluated (the
   *  importer keeps its own table), the document just carries no variables. */
  setVariable?(name: string, value: number, kind: VarKind, formula?: string): void;
}

export interface OnshapeImportResult {
  /** FieldDraft feature ids created, in document order. */
  created: number[];
  /** Onshape featureId → FieldDraft feature id. */
  idMap: Record<string, number>;
  /** Non-fatal approximations (through-all depth, up-to-surface, …). */
  warnings: string[];
  /** Hard failures (unresolvable planes, unknown features) — the import
   *  dialog surfaces these; affected features were SKIPPED. */
  errors: string[];
}

const M_TO_MM = 1000;

type AnyMsg = Record<string, any>;

const msg = (x: any): AnyMsg => (x?.message ?? x) as AnyMsg;

function paramsOf(feat: AnyMsg): Map<string, AnyMsg> {
  const out = new Map<string, AnyMsg>();
  for (const p of feat.parameters ?? []) {
    const pm = msg(p);
    if (pm.parameterId) out.set(pm.parameterId, pm);
  }
  return out;
}

function queryStringsOf(pm: AnyMsg | undefined): string[] {
  if (!pm?.queries) return [];
  return pm.queries
    .map((q: any) => msg(q).queryString as string | undefined)
    .filter((s: string | undefined): s is string => !!s);
}

/** The Onshape sketch featureId a profile/entities query draws from:
 *  either `qSketchRegion(id + "<featureId>")` or the first `…wireOp`
 *  operation inside a compressed creation query. */
async function profileSketchId(qs: string): Promise<string | null> {
  const region = qs.match(/qSketchRegion\(id \+ "([^"]+)"/);
  if (region) return region[1];
  try {
    const { bton, payload } = await decodeOnshapeQueryString(qs);
    if (!bton) return null;
    for (const t of tokenizeBton(payload)) {
      if (t.t === "str" && t.p2 === "wireOp" && t.p1) return t.p1;
    }
  } catch {
    return null;
  }
  return null;
}

const RAD_TO_DEG = 180 / Math.PI;

/**
 * The expression context of one import walk: the document variables declared
 * so far (in document order — a later `assignVariable` may reference an
 * earlier one), the result the warnings go into, and the feature currently
 * being translated.
 *
 * `feature` is mutable and set ONCE per walk step, before the type dispatch,
 * so every parameter read below reports the right feature name without
 * threading it through a dozen signatures.
 */
interface ExprEnv {
  /** Onshape variable name (WITHOUT the `#`) → its evaluated quantity. */
  vars: Map<string, Quantity>;
  /** Onshape variable name → the FieldDraft variable it was stored as. */
  fdNames: Map<string, string>;
  /** Those of {@link fdNames} a FieldDraft formula may reference: length and
   *  plain-number variables. An ANGLE variable is deliberately excluded — see
   *  {@link toFieldDraftFormula}. */
  fdProjectable: Set<string>;
  /** Every FieldDraft variable name handed out, to keep them unique. */
  fdUsed: Set<string>;
  result: OnshapeImportResult;
  feature: string;
  /** Already-emitted warnings — one broken expression is usually read from
   *  several parameters of the same feature, and saying it once is enough. */
  seen: Set<string>;
}

function newExprEnv(result: OnshapeImportResult): ExprEnv {
  return {
    vars: new Map(),
    fdNames: new Map(),
    fdProjectable: new Set(),
    fdUsed: new Set(),
    result,
    feature: "?",
    seen: new Set(),
  };
}

/** Compact number for warning texts (no 12-digit float tails). */
const fmtNum = (n: number): string => String(Math.round(n * 1e6) / 1e6);

function warnOnce(env: ExprEnv, message: string): void {
  if (env.seen.has(message)) return;
  env.seen.add(message);
  env.result.warnings.push(message);
}

/**
 * Quantity parameter → mm (`want = "length"`) or degrees (`want = "angle"`).
 *
 * Onshape carries the AUTHORED TEXT (`"25 cm"`, `"3 mm+#HoleExtraGap"`,
 * `"#Brettstaerke"`) plus `value`, its SI evaluation — which is very often 0
 * in the `/features` payload, so the expression wins. The expression is
 * evaluated against the document variables ({@link ExprEnv.vars}) by
 * `onshapeExpr.ts`.
 *
 * The point of this function is what happens when that FAILS. Until #415 the
 * translation quietly fell through to a hard-coded default whenever the text
 * was not a bare literal — 153 expressions in the ticket corpus took that
 * path, and the resulting geometry looked flawless and was wrong. Every
 * failure now names itself: the expression, the parameter, the cause, and
 * which number was used instead (Onshape's SI evaluation if it has one, the
 * feature's default otherwise — and then it says outright that the dimension
 * is probably wrong).
 */
function quantityParam(
  pm: AnyMsg | undefined,
  fallback: number,
  env: ExprEnv,
  want: "length" | "angle",
): number {
  if (!pm) return fallback;
  const unit = want === "length" ? "mm" : "°";
  const si =
    typeof pm.value === "number" && pm.value !== 0
      ? pm.value * (want === "length" ? M_TO_MM : RAD_TO_DEG)
      : null;
  const expr = typeof pm.expression === "string" ? pm.expression.trim() : "";
  if (!expr) return si ?? fallback;

  const where = pm.parameterId ? ` (Parameter ${pm.parameterId})` : "";
  const used =
    si !== null
      ? `Onshapes ausgewerteter Wert ${fmtNum(si)} ${unit} wird verwendet`
      : `Vorgabe ${fmtNum(fallback)} ${unit} wird verwendet — das Maß ist damit ` +
        `mit hoher Wahrscheinlichkeit FALSCH`;

  const ev = tryEvalOnshapeExpression(expr, env.vars);
  if (!ev.ok) {
    warnOnce(
      env,
      `${env.feature}: Ausdruck "${expr}"${where} nicht auswertbar (${ev.error}) — ${used}`,
    );
    return si ?? fallback;
  }
  const n = want === "length" ? quantityToMm(ev.q) : quantityToDeg(ev.q);
  if (n === null) {
    warnOnce(
      env,
      `${env.feature}: Ausdruck "${expr}"${where} ergibt ${describeDimension(ev.q)}, erwartet war ` +
        `${want === "length" ? "eine Länge" : "ein Winkel"} — ${used}`,
    );
    return si ?? fallback;
  }
  return n;
}

/** Quantity parameter → mm. See {@link quantityParam}. */
function mmOf(pm: AnyMsg | undefined, fallback: number, env: ExprEnv): number {
  return quantityParam(pm, fallback, env, "length");
}

/** Angle parameter → degrees. See {@link quantityParam}; Onshape's SI
 *  evaluation of an angle is in RADIANS. */
function degOf(pm: AnyMsg | undefined, fallback: number, env: ExprEnv): number {
  return quantityParam(pm, fallback, env, "angle");
}

interface SketchRecord {
  fdId: number;
  entityIds: Record<string, number>;
  /** The FieldDraft plane query the sketch was placed on — the extrude's
   *  local +Z, and the reference plane every end condition measures from. */
  planeQuery: string;
  /** Onshape sketch-entity id → the entity's 2D POLYLINE in sketch-space mm
   *  (a line is two points, an arc/circle its flattened rim). The region
   *  resolution walks these to find which imprint face a query names (#412). */
  entityGeom: Map<string, [number, number][]>;
  /** Which of the sketch's entities are straight LINES (construction lines
   *  included), with their 2D endpoints — the only shape a revolve axis (#416)
   *  or a pattern direction (#409/#419) may name. */
  lineSegments: Map<string, SketchSegment>;
  /** HANDEDNESS of the 2D basis this sketch's geometry lives in, against
   *  Onshape's: `+1` same, `−1` mirrored. It is the sign that makes Onshape's
   *  per-edge `±1` flank readable in OUR coordinates (#442).
   *
   *  It is a RECHNUNG, not a measurement, and it always exists (#446): the
   *  basis is whatever {@link alignSketchFrame} wrote the payload in. Rewritten
   *  by the in-plane map, that is `SketchXform.det`; left alone — no 3D answer,
   *  too few support points, a plane that resolved skew — the payload still
   *  carries ONSHAPE'S OWN 2D coordinates, so the map is the identity and the
   *  handedness is `+1`. There is no third case, and therefore no sketch whose
   *  flank has to go unread. */
  basisDet: number;
}

/** A straight sketch entity's two endpoints in sketch-space mm, in Onshape's
 *  own start→end order. That ORDER is the direction a pattern runs in — the
 *  one thing a direction query carries and nothing else can supply (#419). */
type SketchSegment = [[number, number], [number, number]];

/** Flattening resolution of an arc/circle rim, in segments per full turn. */
const ARC_FLATTEN_SEGMENTS = 48;

/**
 * Every entity of the sketch whose geometry is a STRAIGHT LINE, with its two
 * endpoints in sketch-space mm — CONSTRUCTION LINES INCLUDED.
 *
 * Construction lines never reach the B-Rep, yet they are exactly what a revolve
 * axis (#416) and a pattern direction/axis (#409/#419) are usually drawn as, so
 * {@link sketchEntityVertices} (which drops them, being about regions) cannot
 * serve here. Two translations read this map:
 *
 *  - the revolve axis, which only needs to know that the named entity IS a line
 *    (`qSketchLine` is resolved by the kernel itself);
 *  - a pattern's direction, which needs the line's WORLD direction and thus its
 *    endpoints — in Onshape's own start→end order, because that order IS the
 *    direction Onshape counts `oppositeDirection` against.
 */
export function sketchLineSegments(feat: AnyMsg): Map<string, SketchSegment> {
  const out = new Map<string, SketchSegment>();
  for (const raw of (feat.geometry ?? feat.entities ?? []) as AnyMsg[]) {
    const e = msg(raw);
    const g = msg(e.geometry);
    const bt = String(g?.btType ?? "");
    if (!e.entityId || !bt.startsWith("BTCurveGeometryLine")) continue;
    const s = typeof e.startParam === "number" ? e.startParam : 0;
    const t = typeof e.endParam === "number" ? e.endParam : 0;
    out.set(e.entityId as string, [
      [(g.pntX + g.dirX * s) * M_TO_MM, (g.pntY + g.dirY * s) * M_TO_MM],
      [(g.pntX + g.dirX * t) * M_TO_MM, (g.pntY + g.dirY * t) * M_TO_MM],
    ]);
  }
  return out;
}

/** Extract every non-construction sketch entity as a 2D POLYLINE (mm), keyed by
 *  Onshape entity id. The same ×1000 metre→mm scale the sketch translation
 *  uses, so a point computed here is directly comparable to what the kernel
 *  stored and `regionQueryAtPoint` expects.
 *
 *  A curve becomes its flattened rim rather than a bounding box: the region
 *  resolution probes just off the curve for the face it borders, and a box
 *  diagonal borders nothing. */
export function sketchEntityVertices(feat: AnyMsg): Map<string, [number, number][]> {
  const out = new Map<string, [number, number][]>();
  const entities: AnyMsg[] = (feat.geometry ?? feat.entities ?? []).map(msg);
  for (const e of entities) {
    const id = e.entityId as string | undefined;
    if (!id || e.isConstruction === true) continue;
    const g = msg(e.geometry);
    const bt = typeof g?.btType === "string" ? g.btType : "";
    if (bt.startsWith("BTCurveGeometryLine")) {
      const s = typeof e.startParam === "number" ? e.startParam : 0;
      const t = typeof e.endParam === "number" ? e.endParam : 0;
      out.set(id, [
        [(g.pntX + g.dirX * s) * M_TO_MM, (g.pntY + g.dirY * s) * M_TO_MM],
        [(g.pntX + g.dirX * t) * M_TO_MM, (g.pntY + g.dirY * t) * M_TO_MM],
      ]);
    } else if (bt.startsWith("BTCurveGeometryCircle")) {
      const cx = g.xCenter * M_TO_MM;
      const cy = g.yCenter * M_TO_MM;
      const r = g.radius * M_TO_MM;
      // `BTMSketchCurve` is the unbounded form (a full circle); a
      // `BTMSketchCurveSegment` carries its arc span in start/endParam as
      // angles OF ITS OWN FRAME — `arcWorldSpan` turns them into sketch angles,
      // the same reading `translateOnshapeSketch` uses (#444).
      //
      // A full circle runs through the SAME reading, and that is not cosmetic
      // (#446): the polyline's DIRECTION is what „the region lies left of this
      // edge" is measured against, and a circle Onshape parametrises CLOCKWISE
      // (every mirrored copy is one) has its inside on the other flank. Read as
      // a fixed counter-clockwise rim, the two mirrored Ø 1.9 mm bores of
      // `Sketch 4` in the #419 document elected the material AROUND them.
      const unbounded = !String(e.btType ?? "").startsWith("BTMSketchCurveSegment");
      const span = unbounded
        ? arcWorldSpan(g, 0, Math.PI * 2)
        : arcWorldSpan(g, e.startParam ?? 0, e.endParam ?? 0);
      const a0 = span.startAngle;
      const sweep = span.sweep;
      const steps = Math.max(
        2,
        Math.ceil((ARC_FLATTEN_SEGMENTS * Math.abs(sweep)) / (Math.PI * 2)),
      );
      const pts: [number, number][] = [];
      for (let i = 0; i <= steps; i++) {
        const a = a0 + (sweep * i) / steps;
        pts.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
      }
      out.set(id, pts);
    } else if (
      bt.startsWith("BTCurveGeometryEllipse") &&
      !String(e.btType ?? "").startsWith("BTMSketchCurveSegment")
    ) {
      // Same reading as the circle above, one axis longer. Onshape's `radius`
      // is the semi-MAJOR axis along the ellipse's own `xDir`; `minorRadius`
      // is perpendicular. The rim is walked in the ellipse's OWN handedness
      // (`clockwise`), because that direction is what „the region lies left of
      // this edge" is measured against (#446) — a mirrored copy runs the other
      // way and has its inside on the other flank.
      const cx = g.xCenter * M_TO_MM;
      const cy = g.yCenter * M_TO_MM;
      const a = g.radius * M_TO_MM;
      const b = g.minorRadius * M_TO_MM;
      const ux = typeof g.xDir === "number" ? g.xDir : 1;
      const uy = typeof g.yDir === "number" ? g.yDir : 0;
      const hand = g.clockwise ? -1 : 1;
      // The minor axis, oriented so the pair (major, minor) turns the way the
      // parametrisation does.
      const vx = -uy * hand;
      const vy = ux * hand;
      const pts: [number, number][] = [];
      for (let i = 0; i <= ARC_FLATTEN_SEGMENTS; i++) {
        const t = (Math.PI * 2 * i) / ARC_FLATTEN_SEGMENTS;
        const ca = a * Math.cos(t);
        const sb = b * Math.sin(t);
        pts.push([cx + ca * ux + sb * vx, cy + ca * uy + sb * vy]);
      }
      out.set(id, pts);
    }
  }
  return out;
}

/** What a translated sweep needs to be referable later. */
interface SweepRecord {
  fdId: number;
  /** Did the extrude grow along the sketch normal (+) or against it (−)?
   *  It decides which CAP is the sketch-plane side: a FieldDraft slab is
   *  centred on `depth/2`, so a NEGATIVE depth puts CAP_TOP — not
   *  CAP_BOTTOM — flush with the sketch plane. Onshape names the caps
   *  relative to the extrude (`isStart`), so without the sign a cap query
   *  resolves to the face on the far end, or to nothing at all. */
  depthPositive: boolean;
}

/** The depth (mm) a FieldDraft extrude needs, plus how we got there. */
interface EndCondition {
  depthMm: number;
  /** Extrude symmetrically about the sketch plane (Onshape `symmetric`). */
  symmetric: boolean;
  /** Second-direction depth (Onshape `hasSecondDirection`). */
  depthBackMm: number;
  warnings: string[];
  error?: string;
}

/**
 * Translate Onshape's end condition into a FieldDraft depth.
 *
 * - `BLIND` — the authored distance, verbatim.
 * - `THROUGH_ALL` — a cut to infinity. FieldDraft has no infinite extrude, but
 *   a slab that clears the whole scene AABB produces the IDENTICAL solid (the
 *   surplus removes empty space), so we measure the scene instead of guessing
 *   a constant. Falls back to a reported 1000 mm only when the kernel exposes
 *   no AABB (a test double).
 * - `UP_TO_*` — resolve the target face and measure the sketch plane's
 *   distance to it along the extrude axis. Geometrically exact; the distance
 *   is BAKED, so it does not track a later move of the target face — that is
 *   reported, not hidden. The measurement is SIGNED along FieldDraft's own
 *   sketch normal, and that sign is the whole direction answer —
 *   `oppositeDirection` must NOT be applied on top of it (#440, see below).
 */
function endCondition(
  params: Map<string, AnyMsg>,
  kernel: ImportKernel,
  sketchPlaneQuery: string,
  name: string,
  targetFaceQuery: string | undefined,
  env: ExprEnv,
): EndCondition {
  const bound = params.get("endBound")?.value ?? "BLIND";
  const out: EndCondition = {
    // BLIND is the only bound that USES the authored distance — the others
    // overwrite it below. Reading it regardless would report expression
    // problems for a parameter that has no effect on the result.
    depthMm: bound === "BLIND" ? mmOf(params.get("depth"), 25, env) : 0,
    symmetric: params.get("symmetric")?.value === true,
    depthBackMm: 0,
    warnings: [],
  };

  // Onshape can begin the extrusion on a plane OFFSET from the sketch
  // (`startOffset` + `startOffsetDistance`), so the material spans
  // [offset, offset + depth] along the normal instead of [0, depth].
  // FieldDraft's extrude has no such field — `depth`/`depth_back` are both
  // measured FROM the profile plane — so translating this by dropping the
  // offset would put the body `startOffsetDistance` away from where Onshape
  // has it, and nothing would say so. #409's Extrude 7 (100 mm) is exactly
  // that case, and #419 carries seven more.
  if (params.get("startOffset")?.value === true) {
    const startBound = params.get("startOffsetBound")?.value ?? "BLIND";
    const offsetMm = mmOf(params.get("startOffsetDistance"), 0, env);
    if (startBound !== "BLIND" || Math.abs(offsetMm) > 1e-6) {
      out.error =
        `${name}: Start-Offset (${startBound}${
          startBound === "BLIND" ? `, ${offsetMm.toFixed(2)} mm` : ""
        }) — FieldDrafts Extrude beginnt immer auf der Skizzenebene; ` +
        `den Versatz zu verschweigen hieße, den Körper still zu versetzen`;
      return out;
    }
  }

  if (bound === "THROUGH_ALL") {
    const aabb = kernel.sceneAabb?.();
    if (aabb) {
      const [dx, dy, dz] = aabb.size;
      // Twice the diagonal from the sketch plane clears the scene from any
      // plane position/orientation — no direction bookkeeping needed.
      out.depthMm = 2 * Math.hypot(dx, dy, dz);
    } else {
      out.depthMm = 1000;
      out.warnings.push(`${name}: THROUGH_ALL ohne Szenen-AABB → 1000 mm Näherung`);
    }
  } else if (bound !== "BLIND") {
    const frame = frameOf(kernel, sketchPlaneQuery);
    const target = targetFaceQuery ? frameOf(kernel, targetFaceQuery) : null;
    if (!frame || !target) {
      out.error =
        `${name}: End-Condition ${bound} — Zielfläche nicht auflösbar, ` +
        `Tiefe nicht bestimmbar`;
      return out;
    }
    // Distance from the sketch plane to the target plane, measured along the
    // extrude axis (the sketch normal).
    const d =
      (target.origin[0] - frame.origin[0]) * frame.normal[0] +
      (target.origin[1] - frame.origin[1]) * frame.normal[1] +
      (target.origin[2] - frame.origin[2]) * frame.normal[2];
    if (Math.abs(d) < 1e-6) {
      out.error = `${name}: End-Condition ${bound} — Zielfläche liegt in der Sketch-Ebene`;
      return out;
    }
    // `d` is signed along FIELDDRAFT's sketch normal and thereby says, on its
    // own, on which side of the profile the target lies — which is exactly
    // what an extrude depth means here. Onshape's `oppositeDirection` states
    // the SAME fact a second time (it is the flag that sent Onshape looking
    // for the surface on the far side of the profile), so applying it on top
    // negates a direction that is already right and puts the body on the empty
    // side. #440's `Extrude 16` is that case: sketch plane at 62.53 mm, target
    // disc at 46.68 mm, `oppositeDirection: true` — measured d = +15.85 mm was
    // flipped to −15.85 and the body landed 15.85 mm PAST the sketch plane,
    // out of reach of the `Boolean 1` and `Thicken 6` that follow it.
    //
    // Ignoring the flag is also the ROBUST reading, not just the convenient
    // one: FieldDraft derives its sketch normal from the resolved face and may
    // hold it anti-parallel to Onshape's, in which case Onshape's flag refers
    // to an axis we do not have. The measurement is in our own frame and needs
    // no such correspondence.
    out.depthMm = d;
    out.warnings.push(
      `${name}: ${bound} als gemessene Tiefe (${d.toFixed(2)} mm) übernommen — ` +
        `geometrisch exakt, folgt der Zielfläche aber nicht mehr, wenn sie sich bewegt`,
    );
    applySecondDirection(params, out, name, env);
    if (params.get("hasDraft")?.value === true) {
      out.warnings.push(`${name}: Draft-Winkel wird nicht unterstützt und ignoriert`);
    }
    return out;
  }

  if (params.get("oppositeDirection")?.value === true) out.depthMm = -out.depthMm;
  applySecondDirection(params, out, name, env);
  if (params.get("hasDraft")?.value === true) {
    out.warnings.push(`${name}: Draft-Winkel wird nicht unterstützt und ignoriert`);
  }
  return out;
}

function applySecondDirection(
  params: Map<string, AnyMsg>,
  out: EndCondition,
  name: string,
  env: ExprEnv,
): void {
  if (params.get("hasSecondDirection")?.value !== true) return;
  const bound = params.get("secondDirectionBound")?.value ?? "BLIND";
  if (bound !== "BLIND") {
    out.warnings.push(
      `${name}: zweite Richtung mit ${bound} → als BLIND übernommen`,
    );
  }
  out.depthBackMm = Math.abs(mmOf(params.get("secondDirectionDepth"), 0, env));
}

/** The world frame of a face/datum query, via the shared resolver. */
function frameOf(kernel: ImportKernel, query: string) {
  if (typeof kernel.bodyCount !== "function") return null;
  return resolvePlaneFrame(query, kernel as PlaneFrameKernel);
}

/** „Dieselbe Ebene" für den Ebenen-Pfad: Normale parallel UND gleichgerichtet,
 *  Versatz gleich. Beides sind die Schwellen, mit denen dieselbe Datei den
 *  Sketch-Frame beurteilt — `alignSketchFrame` nennt eine Ebene ab
 *  |1 − n·n₀| > 1e-6 „schief" und ab |Δd| > 1e-3 mm „daneben". Gemessen an
 *  #419s vier Deckflächen (`qCreatedBy(39, CAP_TOP)`): Normalen bis auf die
 *  neunte Nachkommastelle identisch, Versatz-Streuung 3,4e-6 mm — also 293-fach
 *  unter der Offset-Schwelle und um Größenordnungen unter der Normalen-Schwelle.
 *  Antiparallel (n·n₀ ≈ −1) zählt NICHT: geometrisch dieselbe Ebene, aber ein
 *  gespiegelter Sketch-Frame. */
const PLANE_NORMAL_TOL = 1e-6;
const PLANE_OFFSET_TOL_MM = 1e-3;

/** Onshapes eigene Query für eine Sketch-Ebene ist oft ein `MERGE` mehrerer
 *  **koplanarer** Flächen desselben Extrudes (vier getrennte Kreise → vier
 *  Deckflächen bei derselben Höhe). FieldDraft verschmilzt Berührendes bewusst
 *  nicht, also bindet dieselbe Query bei uns N Flächen. Für eine EBENE ist das
 *  keine Mehrdeutigkeit: spannen alle Treffer dieselbe Ebene auf, ist die Ebene
 *  eindeutig bestimmt.
 *
 *  Zurück kommt dann die kanonische Element-Query EINER dieser Flächen — nicht
 *  die parametrische Rollen-Query. Der Grund ist Determinismus, nicht Bequem-
 *  lichkeit: der Sketch-Frame hat seinen URSPRUNG im Schwerpunkt der
 *  aufgelösten Fläche, und Kernel (`resolve_face_frame`, erste Fläche des
 *  ersten passenden Körpers) und Web-Spiegel (`resolvePlaneFrame`, erster
 *  Treffer über alle Körper) müssen dieselbe wählen, sonst läge der Sketch um
 *  den Abstand zweier Kappen-Schwerpunkte verschoben in seiner Ebene. Eine
 *  Fläche zu benennen macht die Wahl für beide Seiten identisch — dieselbe
 *  Form, die `thicken` ohnehin verlangt, und sie heilt über ihren Fingerprint.
 *
 *  `null`, wenn die Treffer NICHT koplanar sind oder die Geometrie-Leser des
 *  Kernels fehlen (Test-Double): dann bleibt es beim sichtbaren Fehler. */
function coplanarPlaneFold(
  matches: { kind: string; idHash: string; query: string }[],
  kernel: ImportKernel,
): { query: string; normalDev: number; offsetSpreadMm: number } | null {
  if (matches.length < 2 || matches.some((m) => m.kind !== "face")) return null;
  if (typeof kernel.bodyCount !== "function") return null;
  const planes = faceWorldPlanes(
    matches.map((m) => m.idHash),
    kernel as PlaneFrameKernel,
  );
  if (planes.size !== matches.length) return null;
  const ref = planes.get(matches[0].idHash)!;
  let normalDev = 0;
  let offsetMin = ref.offset;
  let offsetMax = ref.offset;
  for (const m of matches.slice(1)) {
    const p = planes.get(m.idHash)!;
    const dot =
      p.normal[0] * ref.normal[0] + p.normal[1] * ref.normal[1] + p.normal[2] * ref.normal[2];
    normalDev = Math.max(normalDev, Math.abs(1 - dot));
    offsetMin = Math.min(offsetMin, p.offset);
    offsetMax = Math.max(offsetMax, p.offset);
  }
  if (normalDev > PLANE_NORMAL_TOL || offsetMax - offsetMin > PLANE_OFFSET_TOL_MM) return null;
  return { query: matches[0].query, normalDev, offsetSpreadMm: offsetMax - offsetMin };
}

/** Build the FieldDraft plane query for a decoded Onshape plane ref.
 *  Returns `{query}` or `{error}` — the caller records errors visibly. */
function buildPlaneQuery(
  ref: OnshapePlaneRef,
  featureName: string,
  idMap: Record<string, number>,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): { query?: string; error?: string } {
  switch (ref.kind) {
    case "default": {
      const plane =
        ref.plane === "top" ? "xy" : ref.plane === "front" ? "xz" : "yz";
      return { query: `qCreatedBy(origin_plane_${plane})` };
    }
    case "cap":
      return capFaceQuery(ref.opFeatureId, ref.isStart, featureName, idMap, sweeps);
    case "swept": {
      const fd = idMap[ref.opFeatureId];
      if (fd === undefined)
        return { error: `${featureName}: Wand-Ebene referenziert nicht importiertes Feature ${ref.opFeatureId}` };
      const wall = wallQuery(ref.origin, fd, featureName, sketches, ref.offset === true);
      if (wall.error) return wall;
      let query = wall.query!;
      // Disambiguation: adjacency to the blend face (when the fillet
      // still exists) and/or to the split peer's wall.
      const terms: string[] = [];
      if (ref.blendFilletId && idMap[ref.blendFilletId] !== undefined) {
        terms.push(`qCreatedBy(${idMap[ref.blendFilletId]}, BLEND)`);
      }
      if (ref.splitPeer) {
        const peerFd = idMap[ref.splitPeer.opFeatureId];
        if (peerFd !== undefined) {
          const peerWall = wallQuery(ref.splitPeer.origin, peerFd, featureName, sketches);
          if (!peerWall.error) terms.push(peerWall.query!);
        }
      }
      for (const t of terms) {
        query = `qAdjacentTo(${query}, ${t})`;
      }
      return { query };
    }
    case "unsupported":
      return { error: `${featureName}: Ebenen-Query nicht übersetzbar (${ref.reason})` };
  }
}

/** The FieldDraft query for an extrude's cap FACE. Onshape's START cap is the
 *  SKETCH-PLANE side; which FieldDraft cap that is depends on the extrude's
 *  direction: growing along +z the sketch plane is CAP_BOTTOM, growing the
 *  other way it is CAP_TOP. Shared by the sketch-plane translation and the
 *  mate-connector one, which name the same face for different purposes. */
function capFaceQuery(
  opFeatureId: string,
  isStart: boolean,
  featureName: string,
  idMap: Record<string, number>,
  sweeps: Map<string, SweepRecord>,
): { query?: string; error?: string } {
  const fd = idMap[opFeatureId];
  if (fd === undefined)
    return { error: `${featureName}: Cap-Ebene referenziert nicht importiertes Feature ${opFeatureId}` };
  const positive = sweeps.get(opFeatureId)?.depthPositive ?? true;
  const sketchSide = positive ? "CAP_BOTTOM" : "CAP_TOP";
  const farSide = positive ? "CAP_TOP" : "CAP_BOTTOM";
  return { query: `qCreatedBy(${fd}, ${isStart ? sketchSide : farSide})` };
}

function wallQuery(
  origin: { sketchFeatureId: string; entityId: string },
  fdOp: number,
  featureName: string,
  sketches: Map<string, SketchRecord>,
  inner = false,
): { query?: string; error?: string } {
  const rec = sketches.get(origin.sketchFeatureId);
  if (!rec)
    return { error: `${featureName}: Herkunfts-Sketch ${origin.sketchFeatureId} nicht importiert` };
  const ord = rec.entityIds[origin.entityId];
  if (ord === undefined)
    return { error: `${featureName}: Sketch-Entity ${origin.entityId} nicht in der Übersetzung` };
  // Onshape's OFFSET_FACE is the shell's inward counterpart of that wall.
  return { query: inner ? `qWall(${fdOp}, ${ord}, INNER)` : `qWall(${fdOp}, ${ord})` };
}

/**
 * The two readers [`resolveRegionFaces`] ranks candidate imprint faces with:
 * which atomic face covers a sketch point, and each face's area/perimeter.
 *
 * ONE definition, because there are now two callers with the same need and no
 * reason to disagree — an extrude PROFILE (#412) and a mate-connector ANCHOR
 * (#447) both take an Onshape region query and have to land on the same face
 * for it. `null` when the kernel has no sketch reading at all (a small
 * unit-test double); that is a test environment, not a finding.
 */
function regionReaders(
  kernel: ImportKernel,
  rec: SketchRecord,
): {
  faceAt: (u: number, v: number) => number | null;
  metrics: (i: number) => FaceMetrics | null;
  /** How many atomic faces the sketch's imprint arrangement has at all. */
  faceCount: number;
} | null {
  if (
    typeof kernel.regionQueryAtPoint !== "function" ||
    typeof kernel.listSketchRegions !== "function"
  ) {
    return null;
  }
  const faceAt = (u: number, v: number): number | null => {
    const q = kernel.regionQueryAtPoint!(rec.fdId, u, v);
    const m = q.match(/^qRegion\(\s*\d+\s*,\s*(\d+)\s*\)$/);
    return m ? Number(m[1]) : null;
  };
  const faceMetrics = new Map<number, FaceMetrics>();
  try {
    const rows = JSON.parse(kernel.listSketchRegions!(rec.fdId, 128)) as {
      index: number;
      area: number;
      perimeter: number;
    }[];
    for (const r of rows) faceMetrics.set(r.index, { area: r.area, perimeter: r.perimeter });
  } catch {
    faceMetrics.clear();
  }
  return {
    faceAt,
    metrics: (i: number) => faceMetrics.get(i) ?? null,
    faceCount: faceMetrics.size,
  };
}

/**
 * The FieldDraft ANCHOR query for a decoded mate-connector anchor, plus the
 * position `t` along it. FieldDraft's `mate_connector` derives its +Z the same
 * way Onshape does — an edge's tangent, a face's normal — so the translation is
 * a choice of the right entity, not a re-derivation of the frame.
 *
 * `CENTER` inference on a circular CAP EDGE is deliberately mapped to the cap
 * FACE: the centre of a cap's circular edge IS that face's centre, and the
 * face's normal is the axis Onshape puts the connector's +Z on. Naming the face
 * is therefore the SAME coordinate system, expressed in the form FieldDraft
 * resolves exactly — not an approximation of it.
 */
function mateAnchorQuery(
  mc: OnshapeMateConnector,
  featureName: string,
  idMap: Record<string, number>,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  kernel: ImportKernel,
): { query?: string; t?: number; error?: string; sketchLine?: boolean } {
  const a = mc.anchor;
  if (a.kind === "unsupported") {
    return { error: `${featureName}: Mate-Connector-Anker nicht lesbar (${a.reason})` };
  }

  // An IMPRINT FACE — a sketch REGION. It is resolved by the SAME reading an
  // extrude profile gets (`resolveRegionFaces`), because it is the same
  // question with the same answer: Onshape names the region by its fence, and
  // the fence elects one atomic face of our imprint arrangement. The kernel
  // then reads that face's centroid and the sketch plane's normal as the
  // connector's origin and +Z (#447).
  if (a.kind === "region") {
    const rec = sketches.get(a.sketchFeatureId);
    if (!rec) {
      return {
        error: `${featureName}: Mate-Connector-Anker liegt auf nicht importiertem Sketch ${a.sketchFeatureId}`,
      };
    }
    const readers = regionReaders(kernel, rec);
    if (!readers) {
      return { error: `${featureName}: Mate-Connector-Anker ist eine Sketch-Region, die dieser Kernel nicht auflösen kann` };
    }
    const seed = resolveRegionFaces(
      a.edges,
      rec.entityGeom,
      readers.faceAt,
      readers.metrics,
      rec.basisDet,
    ).seed;
    if (seed === null) {
      return {
        error:
          `${featureName}: Mate-Connector-Anker ist eine Sketch-Region von ${a.sketchFeatureId}, ` +
          `deren ${a.edges.length} Randkurven keine Fläche benennen`,
      };
    }
    // A region binds no B-Rep element, so the uniqueness gate in
    // `emitMateConnector` must skip it — the same reason a raw sketch line does.
    return { query: `qRegion(${rec.fdId}, ${seed})`, sketchLine: true };
  }

  // The part studio ORIGIN. Onshape's origin point carries no frame of its own,
  // so the connector's CSYS is the world one — which is exactly what the Top
  // datum plane resolves to (origin 0, +Z normal). The realignment offsets
  // travel from there as they do for any other anchor.
  if (a.kind === "originPoint") {
    return { query: "qCreatedBy(origin_plane_xy)" };
  }

  if (a.kind === "capEntity") {
    const cap = capFaceQuery(a.opFeatureId, a.isStart, featureName, idMap, sweeps);
    if (cap.error) return { error: cap.error };
    // The cap face itself, or the centre of the circular edge bounding it —
    // both are the cap's own coordinate system.
    if (a.entityType === "FACE" || mc.inference === "CENTER") return { query: cap.query };
    if (a.entityType === "EDGE") {
      const wall = wallQuery(a.origin, idMap[a.opFeatureId], featureName, sketches);
      if (wall.error) return { error: wall.error };
      const t = anchorT(mc);
      if (t === null) {
        return {
          error:
            `${featureName}: Mate-Connector-Inferenz "${mc.inference}" auf einer Kante ` +
            `nicht übersetzbar — FieldDraft kennt nur eine Position t entlang der Kante`,
        };
      }
      return { query: `qCommonEdge(${cap.query}, ${wall.query})`, t };
    }
    return {
      error: `${featureName}: Mate-Connector auf einem ${a.entityType} einer Deckfläche wird nicht übersetzt`,
    };
  }

  // A RAW sketch entity: it never enters the B-Rep, so only a sketch-line
  // anchor has a FieldDraft form at all.
  const rec = sketches.get(a.origin.sketchFeatureId);
  if (!rec) {
    return {
      error: `${featureName}: Mate-Connector-Anker liegt auf nicht importiertem Sketch ${a.origin.sketchFeatureId}`,
    };
  }
  const ord = rec.entityIds[a.origin.entityId];
  if (ord === undefined) {
    return {
      error: `${featureName}: Mate-Connector-Anker ${a.origin.entityId} nicht in der Sketch-Übersetzung`,
    };
  }
  if (a.entityType !== "EDGE" || !rec.lineSegments.has(a.origin.entityId)) {
    return {
      error:
        `${featureName}: Mate-Connector-Anker ${describeAnchor(a)} ist keine gerade Sketch-Linie — ` +
        `FieldDraft adressiert rohe Sketch-Geometrie nur als qSketchLine`,
    };
  }
  // Straightness is established above, so a CENTROID anchor has a `t` here that
  // it would not have on a curved edge.
  const t = anchorT(mc, true);
  if (t === null) {
    return {
      error:
        `${featureName}: Mate-Connector-Inferenz "${mc.inference}" auf einer Sketch-Linie ` +
        `nicht übersetzbar — der Endpunkt ist aus secondaryOriginQuery nicht bestimmbar`,
    };
  }
  // Raw sketch geometry. The kernel resolves it (`resolve_mate_anchor` knows
  // `qSketchLine` since #409), and `sketchLine` stays on the result because the
  // UNIQUENESS gate in `emitMateConnector` must not run on it: a sketch line
  // binds no B-Rep entity, so `resolveQuery` reports nothing about it.
  return { query: `qSketchLine(${rec.fdId}, ${ord})`, t, sketchLine: true };
}

/**
 * Translate one Onshape mate connector into a FieldDraft `mate_connector`
 * feature call, or report why it cannot be placed.
 *
 * The realignment travels verbatim: Onshape's `translationX/Y/Z` are millimetre
 * offsets along the connector's OWN axes, which is exactly what FieldDraft's
 * `offset` means, and `flipPrimary` is `flip`. A rotation about +Z survives
 * only in FieldDraft's 90° steps (`reorient`); any other angle is reported
 * rather than rounded, because a silently squared-off connector would take
 * every sketch on it with it.
 */
function mateConnectorParams(
  mc: OnshapeMateConnector,
  featureName: string,
  idMap: Record<string, number>,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  kernel: ImportKernel,
): { params?: Record<string, any>; error?: string; warning?: string } {
  if (mc.originType !== "ON_ENTITY") {
    return {
      error: `${featureName}: Mate-Connector-Ursprung "${mc.originType}" wird nicht importiert (nur ON_ENTITY)`,
    };
  }
  const anchor = mateAnchorQuery(mc, featureName, idMap, sketches, sweeps, kernel);
  if (anchor.error || !anchor.query) {
    return { error: anchor.error ?? `${featureName}: Mate-Connector-Anker nicht übersetzbar` };
  }
  let warning: string | undefined;
  let reorient = 0;
  if (Math.abs(mc.rotationDeg) > 1e-6) {
    if (mc.rotationType !== "ABOUT_Z" || Math.abs(mc.rotationDeg % 90) > 1e-6) {
      warning =
        `${featureName}: Mate-Connector-Drehung ${mc.rotationDeg.toFixed(1)}° um ${mc.rotationType} ` +
        `nicht übernommen — FieldDraft dreht nur in 90°-Schritten um +Z`;
    } else {
      reorient = Math.round(mc.rotationDeg / 90) % 4;
    }
  }

  return {
    warning,
    params: {
      anchor: anchor.query,
      t: anchor.t ?? 0.5,
      flip: mc.flipPrimary,
      reorient,
      offset: mc.offsetMm,
    },
  };
}

/**
 * Resolve a sweep's profile selection (extrude / revolve) to a FieldDraft
 * `profiles` MultiQuery (tickets #412, #440).
 *
 * Onshape supplies ONE query per selected sketch region; each decodes to that
 * region's BOUNDARY. [`resolveRegionFaces`] maps each boundary to the atomic
 * imprint faces it covers — one region may well be SEVERAL of our faces, since
 * our arrangement need not cut where Onshape's does — and their union becomes
 * one MultiQuery. A multi-region selection extrudes exactly those regions, not
 * the whole sketch.
 *
 * Queries that DON'T resolve are dropped, not guessed: the remaining ones still
 * produce their (correct) regions and a visible warning reports the shortfall.
 * Extruding three right plates and missing a fourth beats extruding the whole
 * sheet, and both beat inventing a region nobody selected.
 *
 * The SEED of a region — its best-ranked face — outranks any face another
 * region merely GREW into. Two regions may not both claim a face on inference
 * alone; where they would, the face stays with the region that is anchored on
 * it and the warning names the doubt. Guessing wide is how a profile silently
 * swallows its neighbour (#412).
 *
 * A profile need not be a sketch region at all: Onshape extrudes a WALL or a CAP
 * of an earlier feature just as readily, and says so with a `SWEPT_FACE` /
 * `CAP_FACE` query instead of an `IMPRINT` one (#442). Those translate through
 * the same face vocabulary the sketch planes and fillet edges use
 * ({@link faceQueryOf}) and are handed to the kernel's FACE-extrude path.
 * Treating them as unresolvable regions is what made the #416 document extrude
 * its whole first sketch 32 mm deep where Onshape grows a small tab.
 *
 * Falls back to the whole sketch (`qCreatedBy(sketch)`) — WITH a warning where
 * a selection was actually made — when the kernel exposes no region API (a test
 * double), when no query is a region descriptor at all, or when not a single
 * one of them resolves.
 */
async function resolveProfiles(
  profileQss: string[],
  rec: SketchRecord,
  kernel: ImportKernel,
  name: string,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): Promise<{ query: string; planeQuery: string }> {
  const whole = { query: `qCreatedBy(${rec.fdId})`, planeQuery: rec.planeQuery };

  // Only a FACE query carries a specific selection; a plain whole-sketch
  // reference is not compressed BTON at all and decodes to null.
  const decoded = await Promise.all(profileQss.map((qs) => decodeRegionQuery(qs)));

  // A face of an existing SOLID — decided before the sketch-region path,
  // because it has no sketch reading at all.
  const faceQueries: string[] = [];
  for (const d of decoded) {
    if (!d?.faceRef) continue;
    if ("reason" in d.faceRef) {
      result.errors.push(`${name}: Flächen-Profil nicht übersetzbar (${d.faceRef.reason})`);
      continue;
    }
    const built = faceQueryOf(d.faceRef, name, result.idMap, sketches, sweeps);
    if (!built.query) {
      if (built.error) result.errors.push(built.error);
      continue;
    }
    // A PROFILE must name exactly one face. Onshape folds coplanar caps into
    // ONE face and we do not (#443), so `qCreatedBy(op, CAP_TOP)` can bind four
    // — and unioning them extrudes an area Onshape never selected. Where the
    // name is ambiguous the face path is refused outright rather than narrowed
    // by a guess; the sketch-region reading below still applies.
    const bound = kernel.resolveQuery(built.query).filter((r) => r.kind === "face").length;
    if (bound !== 1) {
      result.errors.push(
        `${name}: Flächen-Profil "${built.query}" bindet ${bound} Flächen statt einer — ` +
          `nicht als Profil übernommen`,
      );
      continue;
    }
    if (!faceQueries.includes(built.query)) faceQueries.push(built.query);
  }
  if (faceQueries.length > 0) {
    const sketchRegions = decoded.filter((d) => d?.edges.length).length;
    if (sketchRegions > 0) {
      result.warnings.push(
        `${name}: Profil mischt ${faceQueries.length} Körperfläche(n) mit ${sketchRegions} ` +
          `Sketch-Region(en) — FieldDrafts Extrude nimmt entweder das eine oder das andere; ` +
          `die Flächen wurden übernommen`,
      );
    }
    // The extrude's own plane is the picked FACE, not the sketch the face's
    // curves came from — an UP_TO_* depth is measured from there.
    return { query: faceQueries.join(", "), planeQuery: faceQueries[0] };
  }

  if (
    typeof kernel.regionQueryAtPoint !== "function" ||
    typeof kernel.listSketchRegions !== "function"
  ) {
    return whole;
  }
  const regionQueries = decoded.filter(
    (d): d is NonNullable<typeof d> => !!d && d.isRegionQuery,
  );
  if (regionQueries.length === 0) return whole;

  const readers = regionReaders(kernel, rec)!;
  const resolutions = regionQueries.map((d) =>
    resolveRegionFaces(d.edges, rec.entityGeom, readers.faceAt, readers.metrics, rec.basisDet),
  );
  const seeds = resolutions.map((r) => r.seed).filter((s): s is number => s !== null);
  const seedSet = new Set(seeds);

  const indices = new Set<number>(seeds);
  /** Faces more than one region grew into — real doubt about who owns them. */
  const contested = new Set<number>();
  const claimedBy = new Map<number, number>();
  resolutions.forEach((r, q) => {
    for (const face of r.faces) {
      if (face !== r.seed) {
        // A face another region is ANCHORED on is that region's, never inferred
        // away from it.
        if (seedSet.has(face)) continue;
        const first = claimedBy.get(face);
        if (first !== undefined && first !== q) contested.add(face);
        else claimedBy.set(face, q);
      }
      indices.add(face);
    }
  });

  const unresolved = resolutions.filter((r) => r.seed === null).length;
  if (indices.size === 0) {
    result.warnings.push(
      `${name}: Region-Auswahl (${regionQueries.length} Onshape-Region(en)) nicht auflösbar — ` +
        `ganzer Sketch extrudiert`,
    );
    return whole;
  }
  // Every way the mapping can be less than exact, named by count. Counting
  // faces against queries would miss all of them: one Onshape region legitimately
  // becomes several of our faces, so the totals can match while a region was
  // lost and another doubled (#440).
  const collapsed = seeds.length - seedSet.size;
  const conflicted = resolutions.filter((r) => r.conflict).length;
  const reasons: string[] = [];
  if (unresolved > 0) reasons.push(`${unresolved} ohne auflösbare Fläche`);
  // A dead curve id is Onshape's own bookkeeping, not a decode failure — the
  // remaining edges still rank the region (#442). Worth a line, not a refusal.
  const curves = resolutions.filter((r) => r.unknownCurve).length;
  if (curves > 0) {
    reasons.push(`${curves} nennen Sketch-Kurven, die dieser Sketch nicht enthält`);
  }
  if (collapsed > 0) reasons.push(`${collapsed} fielen auf dieselbe atomare Face wie eine andere`);
  if (conflicted > 0) {
    reasons.push(`${conflicted} zerfallen widersprüchlich — nur die Kernfläche übernommen`);
  }
  if (contested.size > 0) {
    reasons.push(`${contested.size} Face(s) von zwei Regionen beansprucht`);
  }
  if (reasons.length > 0) {
    result.warnings.push(
      `${name}: Onshape wählte ${regionQueries.length} Regionen, FieldDraft bildet sie auf ` +
        `${indices.size} atomare Face(s) ab — ${reasons.join("; ")} ` +
        `(siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md)`,
    );
  }
  // Selecting the ONE face an undivided sketch has IS the whole sketch — say so.
  // The two profiles bound the same area, but only the whole-sketch one carries
  // the sketch's entity provenance; a region-restricted profile falls back to
  // positional wall numbering (docs/SKETCH-REGION-IMPRINT-PLAN.md §5), which
  // silently breaks the `qWall(...)` plane queries of every LATER feature.
  if (readers.faceCount === 1 && indices.size === 1) return whole;

  const sorted = [...indices].sort((a, b) => a - b);
  return {
    query:
      sorted.length === 1
        ? `qRegion(${rec.fdId}, ${sorted[0]})`
        : `qRegions(${rec.fdId}, [${sorted.join(", ")}])`,
    planeQuery: rec.planeQuery,
  };
}

/** Onshape's `operationType` → the FieldDraft boolean op. Shared by every
 *  body-producing feature (extrude / revolve / loft). */
function operationOf(params: Map<string, AnyMsg>): string {
  switch (params.get("operationType")?.value ?? "NEW") {
    case "NEW":
      return "new";
    case "REMOVE":
      return "remove";
    case "INTERSECT":
      return "intersect";
    default:
      return "add";
  }
}

/** Onshape's `booleanScope` → an explicit FieldDraft `targets` query
 *  (Feature-Scope-Direktive: a boolean never merges implicitly). Empty for a
 *  NEW body and whenever the scope names an operation we did not import. */
async function booleanTargets(
  params: Map<string, AnyMsg>,
  operation: string,
  result: OnshapeImportResult,
): Promise<string> {
  if (operation === "new") return "";
  const scopeQs = queryStringsOf(params.get("booleanScope"))[0];
  if (!scopeQs) return "";
  const ref = await decodeAnyOpRef(scopeQs);
  const fd = ref ? result.idMap[ref] : undefined;
  return fd === undefined ? "" : `qCreatedBy(${fd})`;
}

/** Zusatzdaten neben dem Feature-Baum, die der Import mitziehen kann. */
export interface OnshapeImportSources {
  /** Onshapes eigene 3D-Antwort für die Sketches des Part Studios
   *  (`GET /partstudios/…/sketches?includeGeometry=true&output3D=true`). Sie
   *  ist die einzige exakte Quelle für Onshapes Sketch-FRAME auf einer
   *  Körperfläche (#415) — fehlt sie, werden flächengebundene Sketches wie
   *  bisher wörtlich übernommen und das EINMAL gemeldet. Als Parameter (statt
   *  als Fetch im Importer), damit Tests sie aus Fixtures speisen. */
  sketches3d?: OnshapeSketches3d | null;
}

/** Import a fetched Onshape feature list into the kernel. */
export async function importOnshapeFeatures(
  featuresJson: { features?: any[] },
  kernel: ImportKernel,
  sources: OnshapeImportSources = {},
): Promise<OnshapeImportResult> {
  const result: OnshapeImportResult = { created: [], idMap: {}, warnings: [], errors: [] };
  const sketches = new Map<string, SketchRecord>();
  const sweeps = new Map<string, SweepRecord>();
  const patterns = new Map<string, PatternRecord>();
  const env = newExprEnv(result);
  const frames3d = sketches3dByFeature(sources.sketches3d);

  // Batch mode (#410): defer the display mesh for the whole walk and rebuild it
  // ONCE at the end. Every feature still regenerates (so its plane/edge queries
  // resolve against the mid-history B-Rep), but re-meshing all bodies after each
  // of N features is O(n²) main-thread work that froze the import for minutes.
  kernel.setSyncMeshing?.(false);
  try {
    await importFeatures(featuresJson, kernel, result, sketches, sweeps, patterns, env, frames3d);
  } finally {
    kernel.setSyncMeshing?.(true); // one final mesh of the finished document
  }
  return result;
}

/**
 * Import one Onshape mate connector as a FieldDraft `mate_connector` feature.
 * Returns its FieldDraft id, or `null` when it was skipped (with the reason
 * recorded).
 *
 * The anchor is gated exactly like a sketch plane: it must bind to EXACTLY ONE
 * entity right now (mid-history resolution). A connector on a query that binds
 * nothing would regenerate as an error, and one that binds several would sit on
 * whichever the kernel happened to pick first — neither is a connector the
 * document ever had. A DATUM-plane anchor is exempt for the same reason a datum
 * sketch plane is: it names no B-Rep entity, so `resolveQuery` reports nothing
 * about it — there is no ambiguity for the gate to catch.
 */
async function emitMateConnector(
  feature: AnyMsg,
  featureName: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  env: ExprEnv,
): Promise<{ fdId: number | null; mc: OnshapeMateConnector }> {
  // The realignment offsets are ordinary quantity parameters and go through
  // the same evaluator every other dimension does. `translationZ` of `#Hoehe-3cm`
  // is a length; Onshape's `value` field reports 0 for it, and reading that
  // literally put the whole shelf at the origin instead of 1.8 m above it.
  const mc = await readMateConnector(feature, (pm, fallback, want) =>
    quantityParam(pm, fallback, env, want),
  );
  const built = mateConnectorParams(mc, featureName, result.idMap, sketches, sweeps, kernel);
  if (built.warning) result.warnings.push(built.warning);
  if (built.error || !built.params) {
    result.errors.push(built.error ?? `${featureName}: Mate-Connector nicht übersetzbar`);
    return { fdId: null, mc };
  }
  const anchor = built.params.anchor as string;
  // Exempt: everything that names no B-Rep entity at all, so `resolveQuery`
  // reports nothing about it and there is no ambiguity for the gate to catch —
  // a datum plane, a raw sketch LINE, and a sketch REGION (#447). Failing them
  // on „0 Elemente" would be the gate answering a question it was never asked.
  const namesNoTopology =
    anchor.startsWith("qCreatedBy(origin_plane_") ||
    anchor.startsWith("qSketchLine(") ||
    anchor.startsWith("qRegion(");
  if (!namesNoTopology) {
    let matches = 0;
    try {
      matches = kernel.resolveQuery(anchor).length;
    } catch {
      matches = 0;
    }
    if (matches !== 1) {
      result.errors.push(
        `${featureName}: Mate-Connector-Anker "${anchor}" löst auf ${matches} Elemente auf (erwartet 1) — übersprungen`,
      );
      return { fdId: null, mc };
    }
  }
  const fdId = kernel.addFeatureCall("mate_connector", JSON.stringify(built.params));
  if (mc.featureId) result.idMap[mc.featureId] = fdId;
  result.created.push(fdId);
  return { fdId, mc };
}

/**
 * A sketch placed on a MATE CONNECTOR (#416). The connector rides inside the
 * sketch's own `subFeatures`, so it is imported here — it is reference geometry
 * in its own right, and the anchor the sketch then hangs on.
 *
 * Returns the FieldDraft plane query for the sketch (`qCreatedBy(<connector>)`)
 * or `null` when the connector itself could not be translated; in that case the
 * reason is already in `result.errors` and the caller skips the sketch.
 *
 * Until #409 the sketch was ALWAYS skipped, because `resolve_plane_frame` took
 * a plane query to exactly two things — an origin datum plane or a resolved
 * B-Rep face — and a mate connector is neither. That was one unresolved plane
 * taking out eleven downstream features in the reference part: every extrude on
 * the sketch, then the booleans and mirrors on those extrudes. The kernel now
 * resolves a connector's CSYS as a plane frame, so the sketch lands on the
 * connector's own origin and rotation rather than on a guess.
 */
async function importMateConnectorPlane(
  mcFeatureId: string,
  sketchFeature: AnyMsg,
  featureName: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  env: ExprEnv,
): Promise<string | null> {
  const sub = implicitMateConnectors(sketchFeature).get(mcFeatureId);
  if (!sub) {
    result.errors.push(
      `${featureName}: Ebene nennt den Mate-Connector ${mcFeatureId}, der aber weder als Sub-Feature noch im Dokument steht`,
    );
    return null;
  }
  const { fdId, mc } = await emitMateConnector(
    sub,
    `${featureName} · ${String(sub.name ?? "Mate connector")}`,
    kernel,
    result,
    sketches,
    sweeps,
    env,
  );
  if (fdId === null) {
    result.errors.push(
      `${featureName}: Sketch liegt auf dem Mate-Connector „${mc.name}“ ` +
        `(${describeAnchor(mc.anchor)}), der nicht übersetzt werden konnte — Sketch übersprungen`,
    );
    return null;
  }
  return `qCreatedBy(${fdId})`;
}

/**
 * Onshapes 2D-Sketch-Koordinaten in FieldDrafts Sketch-Basis drehen (#415).
 *
 * FieldDraft leitet die u/v-Basis einer flächengebundenen Ebene aus der
 * Flächennormalen ab, Onshape aus der Parametrisierung der Fläche — beides
 * deterministisch, beides verschieden. Statt an einer der beiden Konventionen
 * zu rühren (die FieldDraft-Seite ist ein Konventions-Lock mit dem Kernel),
 * rechnet der Importer um: `B_os` wird an Onshapes eigener 3D-Antwort GEMESSEN,
 * `B_fd` kommt aus dem gemeinsamen Resolver, und die Differenz ist eine exakte
 * 2D-Drehung/Spiegelung. Für gleiche Frames (jede Ursprungsebene) ist sie die
 * Identität und der Payload bleibt unangetastet.
 *
 * Fehlt die 3D-Antwort oder ist der Frame aus ihr nicht bestimmbar, wird der
 * Sketch wie bisher wörtlich übernommen — und genau das gemeldet.
 *
 * Zurück kommt neben dem Payload die HÄNDIGKEIT der Basis, in der er nun steht
 * (`det`, #446). Sie ist keine Messung, sondern eine Buchhaltung über die
 * eigene Arbeit: umgeschrieben, ist es `SketchXform.det`; wörtlich übernommen,
 * stehen dort Onshapes EIGENE 2D-Zahlen, die Abbildung ist die Identität und
 * die Händigkeit `+1`. Ein dritter Fall existiert nicht — auch eine Ebene, die
 * 90° schief aufgelöst hat, ändert an den 2D-Zahlen nichts, sie hängt sie nur
 * woanders in den Raum. Deshalb ist Onshapes Flanken-Angabe für JEDEN Sketch
 * lesbar, nicht nur für den mit messbarem Frame.
 *
 * Zwei Befunde, die früher EINER waren (#427): „die Ebene steht schief" und
 * „die Ebene liegt parallel daneben" sind verschiedene Fehler, und nur der
 * erste macht die Umrechnung sinnlos. Steht die Normale, ist die In-Plane-
 * Abbildung EXAKT bestimmt, ganz gleich wie weit die Ebenen auseinanderliegen;
 * sie dann wegzuwerfen verhindert keinen Fehler, sondern erzeugt einen viel
 * größeren: Onshapes 2D-Zahlen landen auf FieldDrafts anders orientierten
 * Achsen. Im #409-Regal war das der Unterschied zwischen 50 mm Versatz und
 * einer um 90° verdrehten Ebene 1072,5 mm daneben — genau der Betrag, den
 * Sketch 11 als 2D-v-Koordinate trägt. Der Versatz bleibt ein FEHLER (die
 * Query hat eine andere Fläche erwischt), nur wird er jetzt beziffert statt
 * verstärkt.
 */
function alignSketchFrame(
  feat: AnyMsg,
  name: string,
  planeQuery: string,
  kernel: ImportKernel,
  three: OnshapeSketch3d | undefined,
  result: OnshapeImportResult,
): { feature: AnyMsg; det: number } {
  // Wörtlich übernommen heißt: der Payload steht in ONSHAPES 2D-Basis, die
  // Abbildung ist die Identität, die Händigkeit ist +1 (#446).
  const LITERAL = { feature: feat, det: 1 };
  // Ein Kernel-Double ohne Geometrie-Leser kann keinen Frame liefern; das ist
  // kein Befund, sondern eine Testumgebung.
  if (typeof kernel.bodyCount !== "function") return LITERAL;
  const fd = frameOf(kernel, planeQuery);
  if (!fd) {
    result.warnings.push(
      `${name}: FieldDrafts Frame für die Ebene "${planeQuery}" ist nicht ` +
        `auflösbar — Onshapes 2D-Koordinaten wörtlich übernommen`,
    );
    return LITERAL;
  }
  // Reicht die Korrespondenz nicht für Drehung UND Verschiebung, kann sie
  // trotzdem für die Verschiebung allein reichen — wenn die Drehung an diesem
  // Sketch nichts bewegt (#442).
  //
  // Dritte Stufe (#452): liegen ALLE Stützpunkte auf einer Geraden, ist die
  // Anpassung nur ohne die Ebene entartet. Mit ihr bestimmen eine Richtung und
  // ein Punkt den Frame — bis auf die Händigkeit, die dabei die an 26 von 29
  // Sketches gemessene Regel `det = +1` ist und nicht eine Messung an DIESEM
  // Sketch. Das wird gemeldet, nicht verschwiegen.
  const collinear: CollinearFitReport = { residualMm: NaN, samples: 0 };
  // #454: verwirft die Ausgleichsrechnung Ausreisser, um den Rest exakt zu
  // treffen, dann sind die verworfenen Punkte ein Befund über das DOKUMENT und
  // gehören genannt — nicht die Toleranz aufgeweicht, sondern die Ausreisser
  // benannt.
  const robust: RobustFitReport = { dropped: [], total: 0 };
  const os =
    measureOnshapeSketchFrame(feat, three, robust) ??
    measureConcentricSketchOrigin(feat, three, fd) ??
    measureCollinearSketchFrame(feat, three, fd, collinear);
  if (!os) {
    // #452, letzte Stufe: ohne messbaren Frame kann die Geometrie trotzdem
    // exakt platziert werden, wenn Onshapes 3D-Antwort sie Entity für Entity
    // meldet. Keine Abbildung, kein Residuum — projizieren.
    const placed = placeSketchFromThreeD(feat, three, fd);
    if (placed) {
      result.warnings.push(
        `${name}: kein Frame messbar — ${placed.lines} Linie(n) direkt aus Onshapes ` +
          `3D-Antwort in die aufgelöste Ebene projiziert (exakt); die Händigkeit für ` +
          `Onshapes Flanken-Angabe folgt der Regel „beide Systeme bauen rechtshändig ` +
          `auf dieselbe Flächennormale"`,
      );
      return { feature: placed.feature, det: 1 };
    }
    result.warnings.push(
      !three
        ? `${name}: fehlt in Onshapes 3D-Sketch-Antwort — Frame ungeprüft, ` +
          `2D-Koordinaten wörtlich übernommen`
        : collinear.samples >= 2
          ? // Der Grund ist hier BEZIFFERT: kollinear allein wäre auflösbar, es
            // scheitert an Onshape selbst. Eine starre Abbildung zwischen
            // 2D-Payload und 3D-Antwort gibt es nur, wenn beide dieselben
            // Abstände melden — dieser Sketch tut es nicht.
            `${name}: Onshapes 3D-Antwort ist mit seinem eigenen 2D-Payload nicht starr ` +
            `vereinbar (${collinear.residualMm.toFixed(4)} mm über ${collinear.samples} ` +
            `Stützpunkte, alle kollinear) — 2D-Koordinaten wörtlich übernommen`
          : `${name}: Onshapes Sketch-Frame ist aus seiner 3D-Antwort nicht bestimmbar ` +
            `(zu wenige oder kollineare Stützpunkte) — 2D-Koordinaten wörtlich übernommen; ` +
            `liegt der Sketch auf einer Körperfläche, kann er verdreht sein`,
    );
    return LITERAL;
  }
  if (robust.dropped.length > 0) {
    const worst = robust.dropped.reduce((a, b) => (b.errMm > a.errMm ? b : a));
    result.warnings.push(
      `${name}: Sketch-Frame über ${robust.total - robust.dropped.length} von ` +
        `${robust.total} Stützpunkten gemessen — ${robust.dropped.length} verworfen, ` +
        `weil Onshapes 3D-Antwort sie nicht dort meldet, wo sein eigener 2D-Payload ` +
        `sie hat (grösste Abweichung ${worst.errMm.toFixed(3)} mm bei "${worst.id}")`,
    );
  }
  if (os.samples === collinear.samples && Number.isFinite(collinear.residualMm)) {
    result.warnings.push(
      `${name}: Sketch-Frame aus ${os.samples} KOLLINEAREN Stützpunkten gelegt — Lage und ` +
        `Drehung sind damit bestimmt, die Händigkeit nicht; sie folgt der Regel „beide ` +
        `Systeme bauen rechtshändig auf dieselbe Flächennormale"`,
    );
  }

  const x = inPlaneXform(os, fd);
  // Steht die Normale schief, ist die In-Plane-Abbildung gegenstandslos: die
  // beiden Ebenen teilen keine Richtung, in der man drehen könnte — die Query
  // hat eine Fläche in einer ANDEREN Lage erwischt. Der Sketch wird trotzdem
  // gebaut (die #419-Fixture allein hat acht solcher Ebenen, und sie tragen dort
  // fünf Körper), aber der Winkel steht jetzt in der Meldung: das ist die Zahl,
  // an der die Ursache hängt. #442 hat den #416-Fall vermessen — `Sketch 6`
  // sitzt auf einem Mate-Connector, dessen Anker `qWall(3, 13)` auf eine
  // positionell benannte Wand fällt, also auf die falsche Kante.
  if (Math.abs(Math.abs(x.normalDot) - 1) > 1e-6) {
    const deg = (Math.acos(Math.min(1, Math.abs(x.normalDot))) * 180) / Math.PI;
    result.errors.push(
      `${name}: Onshapes Sketch-Ebene steht ${deg.toFixed(1)}° schief zur aufgelösten ` +
        `FieldDraft-Ebene "${planeQuery}" — Koordinaten NICHT umgerechnet`,
    );
    return LITERAL;
  }
  // Parallel, aber daneben: die Query hat eine andere Fläche erwischt — ein
  // Fehler, der genannt gehört. Die Basis stimmt trotzdem, also wird IN der
  // Ebene umgerechnet und nur der Versatz bleibt stehen.
  if (Math.abs(x.outOfPlaneMm) > 1e-3) {
    result.errors.push(
      `${name}: die aufgelöste FieldDraft-Ebene "${planeQuery}" liegt ` +
        `${x.outOfPlaneMm.toFixed(4)} mm neben Onshapes Sketch-Ebene — Koordinaten IN der ` +
        `Ebene umgerechnet, der Versatz bleibt`,
    );
  }
  // Eine SPIEGELUNG ist ein Befund über die EBENE, nicht über die Konvention
  // (#446). Beide Systeme bauen ihre 2D-Basis rechtshändig auf die Normale der
  // Ebene, die sie meinen; `det = n_os · (x_fd × y_fd)` ist deshalb `+1`,
  // sobald beide dieselbe Fläche in derselben Orientierung meinen. Gemessen an
  // den fünf Dokumenten mit 3D-Antwort: 29 Sketches mit auflösbarem Frame, 26×
  // `+1`, zweimal gespiegelt, einer 90° schief. Die beiden Ausreißer sind genau
  // die zwei bekannten Fehlauflösungen: eine
  // koinzidente Fläche mit umgekehrter Normalen (die andere Seite einer 0,1 mm
  // dicken Schale in #419) und ein Mate-Connector, dessen Z verdreht steht
  // (#416, offen als #447). Umgerechnet wird trotzdem exakt — die Abbildung ist
  // richtig, nur die Fläche darunter ist es nicht.
  if (x.det < 0) {
    result.warnings.push(
      `${name}: die aufgelöste FieldDraft-Ebene "${planeQuery}" ist ENTGEGENGESETZT ` +
        `orientiert zu Onshapes Sketch-Ebene (${describeXform(x)}) — die 2D-Umrechnung ` +
        `bleibt exakt, aber gemeint war fast sicher eine andere Fläche`,
    );
  }
  if (x.identity) return { feature: feat, det: x.det };

  const t = transformSketchFeature(feat, x);
  if (t.droppedAxisConstraints > 0) {
    result.warnings.push(
      `${name}: ${t.droppedAxisConstraints}× HORIZONTAL/VERTICAL verworfen — ` +
        `Onshapes Sketch-Achsen stehen ${describeXform(x)} zu FieldDrafts, ` +
        `dafür gibt es keine treue Entsprechung`,
    );
  }
  if (t.droppedOriginConstraints > 0) {
    result.warnings.push(
      `${name}: ${t.droppedOriginConstraints}× Bedingung auf Onshapes ` +
        `Sketch-Ursprung verworfen — er liegt nicht auf FieldDrafts Ursprung ` +
        `(${describeXform(x)})`,
    );
  }
  return { feature: t.feature, det: x.det };
}

/** The feature walk itself — see {@link importOnshapeFeatures}, which wraps this
 *  in the batch meshing deferral. */
async function importFeatures(
  featuresJson: { features?: any[] },
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  patterns: Map<string, PatternRecord>,
  env: ExprEnv,
  frames3d: Map<string, OnshapeSketch3d>,
): Promise<void> {
  for (const raw of featuresJson.features ?? []) {
    const feat = msg(raw);
    const name: string = feat.name ?? feat.featureId ?? "?";
    if (feat.suppressed) {
      result.warnings.push(`${name}: unterdrückt — übersprungen`);
      continue;
    }
    const params = paramsOf(feat);
    // Every parameter read below reports under THIS feature's name.
    env.feature = name;

    if (feat.featureType === "newSketch") {
      const planeQs = queryStringsOf(params.get("sketchPlane"))[0];
      if (!planeQs) {
        result.errors.push(`${name}: keine Ebenen-Query`);
        continue;
      }
      // Placed on a MATE CONNECTOR? Then the connector travels inside this
      // sketch's own `subFeatures` (#416) — import it and put the sketch ON
      // it (#409). No uniqueness gate: a connector is datum geometry, it
      // binds no face, and it is addressed by its own feature id.
      const mcId = mateConnectorPlaneId(planeQs);
      let planeQuery: string;
      if (mcId) {
        const onConnector = await importMateConnectorPlane(
          mcId, feat, name, kernel, result, sketches, sweeps, env,
        );
        if (!onConnector) continue;
        planeQuery = onConnector;
      } else {
        const ref = await decodePlaneQuery(planeQs);
        const built = buildPlaneQuery(ref, name, result.idMap, sketches, sweeps);
        if (built.error || !built.query) {
          result.errors.push(built.error ?? `${name}: Ebene nicht übersetzbar`);
          continue;
        }
        // Uniqueness gate for face-level planes: the stored query must
        // bind exactly ONE PLANE right now (mid-history resolution). Several
        // faces are an ambiguity only when they lie in DIFFERENT planes;
        // coplanar hits are Onshape's `MERGE` seen through our decomposition
        // (see {@link coplanarPlaneFold}).
        if (!built.query.startsWith("qCreatedBy(origin_plane_")) {
          const matches = kernel.resolveQuery(built.query);
          const fold = matches.length === 1 ? null : coplanarPlaneFold(matches, kernel);
          if (matches.length !== 1 && !fold) {
            result.errors.push(
              `${name}: Ebenen-Query "${built.query}" löst auf ${matches.length} Flächen auf (erwartet 1) — Sketch übersprungen.` +
                (built.query.includes("qWall(") && matches.length === 0
                  ? " Eine Wand trägt den Namen der Sketch-Entity, die sie erzeugt hat — löst sie nicht auf, gehört die genannte Entity nicht zum Rand des extrudierten Profils (siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md, Klasse A)."
                  : ""),
            );
            continue;
          }
          if (fold) {
            result.warnings.push(
              `${name}: Ebenen-Query "${built.query}" bindet ${matches.length} koplanare Flächen ` +
                `(Normalen-Abweichung ${fold.normalDev.toExponential(1)}, Versatz-Streuung ` +
                `${fold.offsetSpreadMm.toExponential(1)} mm) — Onshapes MERGE in unserer ` +
                `Zerlegung; die Ebene ist eindeutig, der Sketch sitzt auf einer davon`,
            );
            built.query = fold.query;
          }
        }
        planeQuery = built.query;
      }
      // Onshapes 2D-Koordinaten in FieldDrafts Sketch-Basis drehen (#415).
      // Umgeschrieben wird der PAYLOAD, damit Übersetzung, Region-Innenpunkte
      // und jeder spätere 2D-Leser dieselben Zahlen sehen. Ohne die 3D-Quelle
      // (`sources.sketches3d`) bleibt alles wie bisher — dass sie fehlt, meldet
      // die Stelle, die sie holen sollte, nicht jeder einzelne Sketch.
      const aligned =
        frames3d.size === 0
          ? { feature: feat, det: 1 }
          : alignSketchFrame(
              feat,
              name,
              planeQuery,
              kernel,
              frames3d.get(feat.featureId),
              result,
            );
      const translation = translateOnshapeSketch(aligned.feature);
      for (const [k, v] of Object.entries(translation.diagnostics.unhandled)) {
        result.warnings.push(`${name}: ${v}× nicht übersetzt: ${k}`);
      }
      const fdId = kernel.addSketch(planeQuery, translation.subCalls);
      sketches.set(feat.featureId, {
        fdId,
        entityIds: translation.entityIds,
        planeQuery,
        entityGeom: sketchEntityVertices(aligned.feature),
        lineSegments: sketchLineSegments(aligned.feature),
        basisDet: aligned.det,
      });
      result.idMap[feat.featureId] = fdId;
      result.created.push(fdId);
      continue;
    }

    // A mate connector STANDING IN THE TREE — the same feature the implicit
    // ones are, just not hidden inside a sketch. Reference geometry: it builds
    // no body, it gives later features (and the user) a frame to hang on.
    if (feat.featureType === "mateConnector") {
      await emitMateConnector(feat, name, kernel, result, sketches, sweeps, env);
      continue;
    }

    if (feat.featureType === "extrude") {
      const profileQss = queryStringsOf(params.get("entities"));
      const profileQs = profileQss[0];
      const sketchFid = profileQs ? await profileSketchId(profileQs) : null;
      const rec = sketchFid ? sketches.get(sketchFid) : undefined;
      if (!rec) {
        result.errors.push(`${name}: Profil-Sketch nicht auflösbar — übersprungen`);
        continue;
      }
      // Which sketch REGIONS did Onshape select? One profile query per chosen
      // region — resolve each to a FieldDraft `qRegion`; fall back visibly to
      // the whole sketch when the selection can't be honoured (#412).
      const profiles = await resolveProfiles(
        profileQss,
        rec,
        kernel,
        name,
        result,
        sketches,
        sweeps,
      );
      // End condition: BLIND / THROUGH_ALL / UP_TO_* (see `endCondition`).
      const upToQs = queryStringsOf(params.get("endBoundEntityFace"))[0];
      let upToQuery: string | undefined;
      if (upToQs) {
        const upToRef = await decodePlaneQuery(upToQs);
        const built = buildPlaneQuery(upToRef, name, result.idMap, sketches, sweeps);
        if (built.query) upToQuery = built.query;
      }
      const end = endCondition(params, kernel, profiles.planeQuery, name, upToQuery, env);
      result.warnings.push(...end.warnings);
      if (end.error) {
        result.errors.push(end.error);
        continue;
      }
      const depthMm = end.depthMm;
      const operation = operationOf(params);
      const targets = await booleanTargets(params, operation, result);
      const fdId = kernel.addFeatureCall(
        "extrude",
        JSON.stringify({
          display_name: name,
          profiles: profiles.query,
          depth: { value: depthMm, unit: "mm" },
          operation,
          ...(end.symmetric ? { symmetric: true } : {}),
          ...(end.depthBackMm
            ? { depth_back: { value: end.depthBackMm, unit: "mm" } }
            : {}),
          ...(targets ? { targets } : {}),
        }),
      );
      result.idMap[feat.featureId] = fdId;
      sweeps.set(feat.featureId, { fdId, depthPositive: depthMm >= 0 });
      result.created.push(fdId);
      continue;
    }

    if (feat.featureType === "loft") {
      // FieldDraft lofts SOLID bodies only; an Onshape SURFACE loft has no
      // FieldDraft counterpart, so report and skip rather than fake a solid.
      const bodyType = params.get("bodyType")?.value ?? "SOLID";
      if (bodyType !== "SOLID") {
        result.warnings.push(
          `${name}: Loft-Bodytyp ${bodyType} (kein SOLID) wird nicht importiert — übersprungen`,
        );
        continue;
      }
      // Each item of sheetProfilesArray is ONE cross-section; its
      // `sheetProfileEntities` query points at the sketch region to loft.
      const arr = params.get("sheetProfilesArray");
      const items: AnyMsg[] = (arr?.items ?? []).map(msg);
      const profiles: string[] = [];
      for (const item of items) {
        const ip = paramsOf(item);
        const profileQs = queryStringsOf(ip.get("sheetProfileEntities"))[0];
        const sketchFid = profileQs ? await profileSketchId(profileQs) : null;
        const rec = sketchFid ? sketches.get(sketchFid) : undefined;
        if (!rec) {
          result.warnings.push(
            `${name}: ein Loft-Querschnitt ist nicht auflösbar — übersprungen`,
          );
          continue;
        }
        profiles.push(`qCreatedBy(${rec.fdId})`);
      }
      if (profiles.length < 2) {
        result.warnings.push(
          `${name}: weniger als 2 auflösbare Loft-Querschnitte (${profiles.length}) — Feature übersprungen`,
        );
        continue;
      }
      // v1 lofts by straight cross-section morph only: guide curves and
      // start/end tangency (derivative) conditions are not yet honoured.
      const guides: AnyMsg[] = (params.get("guidesArray")?.items ?? []).map(msg);
      if (guides.length > 0 || params.get("addGuides")?.value === true) {
        result.warnings.push(
          `${name}: Führungskurven (guidesArray) werden in v1 ignoriert`,
        );
      }
      const startCond = params.get("startCondition")?.value ?? "DEFAULT";
      const endCond = params.get("endCondition")?.value ?? "DEFAULT";
      if (startCond !== "DEFAULT" || endCond !== "DEFAULT") {
        result.warnings.push(
          `${name}: Loft-Randableitung (${startCond}/${endCond}) wird in v1 ignoriert`,
        );
      }
      const operation = operationOf(params);
      const targets = await booleanTargets(params, operation, result);
      const fdId = kernel.addFeatureCall(
        "loft",
        JSON.stringify({
          display_name: name,
          profiles: profiles.join(", "),
          operation,
          ...(targets ? { targets } : {}),
        }),
      );
      result.idMap[feat.featureId] = fdId;
      result.created.push(fdId);
      continue;
    }

    if (feat.featureType === "shell") {
      const faceQs = queryStringsOf(params.get("entities"));
      const openFaces: string[] = [];
      for (const qs of faceQs) {
        const ref = await decodePlaneQuery(qs);
        const built = buildPlaneQuery(ref, name, result.idMap, sketches, sweeps);
        if (built.query) openFaces.push(built.query);
        else result.errors.push(built.error ?? `${name}: Open-Face nicht übersetzbar`);
      }
      if (openFaces.length !== faceQs.length) continue;
      const fdId = kernel.addFeatureCall(
        "shell",
        JSON.stringify({
          display_name: name,
          open_faces: openFaces.join(", "),
          thickness: { value: mmOf(params.get("thickness"), 2, env), unit: "mm" },
        }),
      );
      result.idMap[feat.featureId] = fdId;
      result.created.push(fdId);
      continue;
    }

    if (feat.featureType === "fillet") {
      const filletType = params.get("filletType")?.value ?? "EDGE";
      if (filletType !== "EDGE") {
        result.errors.push(`${name}: Fillet-Typ ${filletType} wird nicht importiert`);
        continue;
      }
      const control = params.get("blendControlType")?.value ?? "RADIUS";
      if (control !== "RADIUS") {
        result.warnings.push(`${name}: ${control}-Steuerung → als Radius übernommen`);
      }
      const section = params.get("crossSection")?.value ?? "CIRCULAR";
      if (section !== "CIRCULAR") {
        result.warnings.push(`${name}: ${section}-Querschnitt → kreisförmig übernommen`);
      }
      const radiusMm = mmOf(params.get("radius"), 2, env);
      const edgeQs = queryStringsOf(params.get("entities"));

      // Sibling edges can decode to the same query (the un-resolved BTON
      // back-references, see `buildFilletEdgeQuery`) — filleting the same
      // edge twice would stack two blends on it, so de-duplicate and say so.
      const queries: string[] = [];
      for (const qs of edgeQs) {
        const edgeQuery = await buildEdgeQuery(qs, name, result, sketches, sweeps);
        if (edgeQuery && !queries.includes(edgeQuery)) queries.push(edgeQuery);
      }
      if (queries.length && queries.length < edgeQs.length) {
        result.warnings.push(
          `${name}: ${edgeQs.length} Onshape-Kanten → ${queries.length} unterscheidbare Query(s); ` +
            `Geschwister-Kanten sind (noch) nicht auseinanderzuhalten`,
        );
      }

      let made = 0;
      for (const edgeQuery of queries) {
        const matches = kernel.resolveQuery(edgeQuery);
        if (matches.length < 1) {
          result.errors.push(
            `${name}: Kanten-Query "${edgeQuery}" löst nichts auf — übersprungen`,
          );
          continue;
        }
        const fdId = kernel.addFeatureCall(
          "fillet_edge",
          JSON.stringify({
            display_name: name,
            edge: edgeQuery,
            blend_radius: radiusMm,
          }),
        );
        if (made === 0) result.idMap[feat.featureId] = fdId;
        result.created.push(fdId);
        made += 1;
      }
      continue;
    }

    if (feat.featureType === "revolve") {
      await importRevolve(feat, params, name, kernel, result, sketches, sweeps, env);
      continue;
    }

    if (feat.featureType === "chamfer") {
      await importChamfer(feat, params, name, kernel, result, sketches, sweeps, env);
      continue;
    }

    if (feat.featureType === "hole") {
      await importHole(feat, params, name, kernel, result, sketches, env);
      continue;
    }

    if (feat.featureType === "thicken") {
      await importThicken(feat, params, name, kernel, result, sketches, sweeps, env);
      continue;
    }

    if (feat.featureType === "assignVariable") {
      importAssignVariable(params, name, kernel, env);
      continue;
    }

    if (feat.featureType === "booleanBodies") {
      await importBooleanBodies(feat, params, name, kernel, result, env);
      continue;
    }

    if (feat.featureType === "linearPattern") {
      await importLinearPattern(feat, params, name, kernel, result, sketches, patterns, env);
      continue;
    }

    if (feat.featureType === "circularPattern") {
      await importCircularPattern(feat, params, name, kernel, result, sketches, patterns, env);
      continue;
    }

    if (feat.featureType === "mirror") {
      await importMirror(feat, params, name, kernel, result, sketches, sweeps, patterns, env);
      continue;
    }

    result.errors.push(`${name}: Feature-Typ "${feat.featureType}" wird noch nicht importiert`);
  }
}

/** Characters a FieldDraft identifier may hold (the kernel lexer's rule:
 *  ASCII word characters plus everything non-ASCII, so `Seitenlänge` passes). */
const FD_NAME_FORBIDDEN = /[^A-Za-z0-9_\u0080-\uFFFF]/g;

/** A unique, lexer-legal FieldDraft name for an Onshape variable. */
function fieldDraftVariableName(raw: string, used: Set<string>): string {
  let base = raw.replace(FD_NAME_FORBIDDEN, "_");
  if (!base || /^[0-9]/.test(base)) base = `v${base}`;
  let out = base;
  for (let n = 2; used.has(out); n += 1) out = `${base}_${n}`;
  used.add(out);
  return out;
}

/**
 * Onshape `assignVariable` → a FieldDraft DOCUMENT VARIABLE (#409/#415/#417).
 *
 * Onshape declares variables as features, so they arrive in document order and
 * a later one may reference an earlier one (`#Gesamtlaenge = #Klingenlaenge +
 * #Grifflaenge`). The walk therefore evaluates each declaration against the
 * table built so far and appends its result — which is also the table every
 * later quantity parameter is read against ({@link quantityParam}). That
 * ordering is the whole point: without it, `#Brettstaerke` in an extrude depth
 * has nothing to resolve to.
 *
 * Two Onshape concepts do NOT lower:
 *  - `mode: MEASURED` — the value comes out of the geometry (Onshape writes
 *    `"?"` in the feature list, the number lives only inside its own
 *    evaluation). FieldDraft has no measured variables; the declaration is
 *    reported and the name stays undefined, so every expression that uses it
 *    says so on its own.
 *  - a dimension FieldDraft has no variable kind for (an area, a ratio of
 *    lengths with a leftover exponent). The value still enters the importer's
 *    own table — dependent expressions keep evaluating correctly — it just
 *    does not become a document variable.
 *
 * The stored FieldDraft variable additionally carries a FORMULA whenever the
 * Onshape expression projects faithfully onto FieldDraft's dimensionless,
 * mm-based formula language ({@link toFieldDraftFormula}) AND FieldDraft's own
 * evaluator reproduces the number this module computed. That cross-check is
 * what makes the projection safe to store: a formula that would drift is
 * dropped, and the variable keeps its plain baked value.
 */
function importAssignVariable(
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  env: ExprEnv,
): void {
  const result = env.result;
  const osName = String(params.get("name")?.value ?? "").trim();
  if (!osName) {
    result.errors.push(`${name}: assignVariable ohne Variablennamen — übersprungen`);
    return;
  }
  const mode = String(params.get("mode")?.value ?? "ASSIGNED");
  if (mode !== "ASSIGNED") {
    result.warnings.push(
      `${name}: Variable "#${osName}" wird in Onshape aus der Geometrie GEMESSEN (${mode}) — ` +
        `FieldDraft kennt keine gemessenen Variablen; sie bleibt undefiniert, und jeder Ausdruck, ` +
        `der sie benutzt, meldet sich einzeln`,
    );
    return;
  }
  // `value` mirrors whichever of lengthValue/angleValue/numberValue/anyValue
  // the chosen `variableType` uses, and is the only one that is right for ALL
  // types (a NUMBER variable's lengthValue still reads `0.0*m`).
  const byType: Record<string, string> = {
    LENGTH: "lengthValue",
    ANGLE: "angleValue",
    NUMBER: "numberValue",
    ANY: "anyValue",
  };
  const varType = String(params.get("variableType")?.value ?? "ANY");
  const pm = params.get("value") ?? params.get(byType[varType] ?? "anyValue");
  const expr = typeof pm?.expression === "string" ? pm.expression.trim() : "";
  if (!expr) {
    result.errors.push(`${name}: Variable "#${osName}" hat keinen Wert — übersprungen`);
    return;
  }
  const ev = tryEvalOnshapeExpression(expr, env.vars);
  if (!ev.ok) {
    result.errors.push(
      `${name}: Variable "#${osName}" = "${expr}" nicht auswertbar (${ev.error}) — ` +
        `Variable nicht angelegt`,
    );
    return;
  }
  // Known to the importer from here on, whatever FieldDraft can store.
  env.vars.set(osName, ev.q);

  const kind = quantityKind(ev.q);
  if (kind === null) {
    result.warnings.push(
      `${name}: Variable "#${osName}" ist ${describeDimension(ev.q)} — dafür hat FieldDraft ` +
        `keine Variablenart; sie wird für die Ausdrücke dieses Imports benutzt, aber nicht ` +
        `als Dokument-Variable angelegt`,
    );
    return;
  }
  const value = kind === "angle" ? (quantityToDeg(ev.q) as number) : ev.q.v;

  const existing = env.fdNames.get(osName);
  const fdName = existing ?? fieldDraftVariableName(osName, env.fdUsed);
  if (!existing && fdName !== osName) {
    result.warnings.push(
      `${name}: Variablenname "#${osName}" ist in FieldDraft nicht zulässig — als "${fdName}" importiert`,
    );
  }

  // Parametric provenance, but only when it demonstrably computes the same
  // number in FieldDraft's own (dimensionless, mm/degree-based) evaluator.
  let formula: string | undefined;
  if (expr.includes("#") && kind !== "angle") {
    const rendered = toFieldDraftFormula(expr, (v) =>
      env.fdProjectable.has(v) ? env.fdNames.get(v) : undefined,
    );
    if (rendered) {
      const scope: Record<string, number> = {};
      for (const os of env.fdProjectable) {
        const fd = env.fdNames.get(os);
        const q = env.vars.get(os);
        if (fd && q) scope[fd] = q.v;
      }
      try {
        const check = evalFormula(rendered, scope);
        if (Math.abs(check - value) <= 1e-6 * Math.max(1, Math.abs(value))) formula = rendered;
      } catch {
        // FieldDraft cannot evaluate it — keep the baked number, drop the text.
      }
    }
  }

  env.fdNames.set(osName, fdName);
  if (kind === "length" || kind === "number") env.fdProjectable.add(osName);

  try {
    kernel.setVariable?.(fdName, value, kind, formula);
  } catch (e) {
    result.errors.push(
      `${name}: Variable "${fdName}" konnte nicht angelegt werden (${String(e)})`,
    );
  }
}

/** The FieldDraft feature ids owning the bodies a boolean operand names, or
 *  `null` when one of them points at an Onshape operation this import did not
 *  produce — in which case the caller SKIPS the feature instead of guessing an
 *  operand (a Boolean on the wrong body silently deletes material). */
async function booleanOperands(
  pm: AnyMsg | undefined,
  name: string,
  label: string,
  result: OnshapeImportResult,
): Promise<number[] | null> {
  const out: number[] = [];
  for (const qs of queryStringsOf(pm)) {
    const ref = await decodeAnyOpRef(qs);
    const fd = ref === null ? undefined : result.idMap[ref];
    if (fd === undefined) {
      result.errors.push(
        `${name}: ${label} nicht auflösbar ` +
          `(${ref ? `Onshape-Operation ${ref} wurde nicht importiert` : "Query benennt keine importierte Operation"}) — ` +
          `Feature übersprungen`,
      );
      return null;
    }
    if (!out.includes(fd)) out.push(fd);
  }
  return out;
}

/**
 * Onshape `booleanBodies` → a FieldDraft `boolean` feature (#409/#415/…/#420).
 *
 * `operationType` maps 1:1 onto FieldDraft's three Boolean tabs
 * (UNION → Union, SUBTRACTION → Subtract, INTERSECTION → Intersect). Both
 * operand lists are body queries; each is decoded to the Onshape OPERATION
 * that created the body and looked up in the id map, exactly as
 * {@link booleanTargets} does for a feature's own boolean scope — the
 * FieldDraft Boolean accepts `qCreatedBy(<feature>)` and nothing else.
 *
 * Scope is never implicit (Feature-Scope-Direktive): a Subtract or Intersect
 * without resolvable targets is SKIPPED with an error, not turned into a
 * free-floating tool that happens to overlap something.
 *
 * The id map entry for the Boolean itself points at the feature that OWNS the
 * resulting body — the first target for a Subtract, the first tool otherwise.
 * A later Onshape query naming "the body the boolean produced" therefore lands
 * on the FieldDraft feature that actually carries that geometry (the Boolean
 * feature itself creates none; it re-routes the ops of its operands).
 */
async function importBooleanBodies(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  env: ExprEnv,
): Promise<void> {
  const opType = String(params.get("operationType")?.value ?? "UNION");
  const operation = { UNION: "Union", SUBTRACTION: "Subtract", INTERSECTION: "Intersect" }[
    opType
  ];
  if (!operation) {
    result.errors.push(
      `${name}: Boolean-Operation "${opType}" hat kein FieldDraft-Gegenstück — übersprungen`,
    );
    return;
  }

  const tools = await booleanOperands(params.get("tools"), name, "Werkzeug-Körper", result);
  if (!tools) return;
  const targets = await booleanOperands(params.get("targets"), name, "Ziel-Körper", result);
  if (!targets) return;

  // Onshape's UNION puts every body in `tools`; FieldDraft's Union does the
  // same, so a stray target simply joins the tool list.
  const toolIds = operation === "Union" ? [...new Set([...tools, ...targets])] : tools;
  const targetIds = operation === "Union" ? [] : targets;
  if (toolIds.length === 0) {
    result.errors.push(`${name}: Boolean ohne Werkzeug-Körper — übersprungen`);
    return;
  }
  if (operation === "Union" && toolIds.length < 2) {
    result.errors.push(
      `${name}: Union mit nur einem auflösbaren Körper — übersprungen (nichts zu vereinigen)`,
    );
    return;
  }
  if (operation !== "Union" && targetIds.length === 0) {
    result.errors.push(
      `${name}: ${operation === "Subtract" ? "Subtraktion" : "Verschneidung"} ohne Ziel-Körper — ` +
        `übersprungen (FieldDraft verrechnet nie implizit mit einem zufällig überlappenden Körper)`,
    );
    return;
  }

  if (params.get("offset")?.value === true) {
    const offsetMm = mmOf(params.get("offsetDistance"), 0, env);
    if (offsetMm !== 0) {
      result.warnings.push(
        `${name}: Werkzeug-Aufmaß (${fmtNum(offsetMm)} mm Luft) wird nicht nachgebildet — ` +
          `es wird ohne Spiel verrechnet`,
      );
    }
  }
  if (params.get("reFillet")?.value === true) {
    result.warnings.push(`${name}: „Verrundungen wiederherstellen" wird nicht nachgebildet`);
  }

  const asQuery = (ids: number[]) => ids.map((i) => `qCreatedBy(${i})`).join(", ");
  const fdId = kernel.addFeatureCall(
    "boolean",
    JSON.stringify({
      display_name: name,
      operation,
      tools: asQuery(toolIds),
      ...(targetIds.length ? { targets: asQuery(targetIds) } : {}),
      ...(params.get("keepTools")?.value === true ? { keep_tools: true } : {}),
    }),
  );
  result.created.push(fdId);
  result.idMap[feat.featureId] = operation === "Subtract" ? targetIds[0] : toolIds[0];
}

/**
 * Onshape `revolve` → a FieldDraft `revolve` feature (#416).
 *
 * What lowers EXACTLY: a SOLID full turn. Profile selection goes through the
 * same region resolution as the extrude ({@link resolveProfiles}), the boolean
 * op and its scope through the same {@link operationOf} / {@link booleanTargets}
 * as every other body-producing feature.
 *
 * The AXIS is the one genuinely new piece. Onshape names it with a query on a
 * single edge; in practice that is a (usually construction) SKETCH LINE, which
 * FieldDraft addresses as `qSketchLine(sketch, entity)` — the kernel lifts its
 * 2D endpoints through the sketch frame and rotates the profile onto it. A
 * query that names anything else (a B-Rep edge, a datum axis, an arc) has no
 * FieldDraft counterpart today: `revolve.axis` accepts `qEdge`/`qSketchLine`
 * only, and our edge translation produces `qCommonEdge`, which the axis
 * resolver rejects. Such a revolve is SKIPPED with an error naming the reason
 * rather than spun around a guessed axis.
 *
 * PARTIAL turns have no counterpart either: `RevolveDef` carries no angle, it
 * always sweeps the full 360°. A `ONE_SIDED`/`TWO_SIDED`/`SYMMETRIC` revolve is
 * therefore only imported when its angles ADD UP to a full turn (Onshape writes
 * exactly that for a `TWO_SIDED` 30°/330° pair), and skipped with a warning
 * naming type and angle otherwise — a 90° sector silently closed into a ring is
 * wrong geometry, not an import.
 */
async function importRevolve(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  env: ExprEnv,
): Promise<void> {
  const bodyType = String(params.get("bodyType")?.value ?? "SOLID");
  if (bodyType !== "SOLID") {
    result.warnings.push(
      `${name}: Revolve-Bodytyp ${bodyType} (kein SOLID) wird nicht importiert — übersprungen`,
    );
    return;
  }

  // Sweep angle: FieldDraft revolves a full turn, nothing else.
  const revolveType = String(params.get("revolveType")?.value ?? "FULL");
  const angleDeg = degOf(params.get("angle"), 0, env);
  const angleBackDeg = degOf(params.get("angleBack"), 0, env);
  const sweepDeg =
    revolveType === "FULL"
      ? 360
      : revolveType === "TWO_SIDED"
        ? angleDeg + angleBackDeg
        : angleDeg;
  if (Math.abs(sweepDeg - 360) > 1e-3) {
    result.warnings.push(
      `${name}: Teil-Umdrehung (${revolveType}, ${sweepDeg}°) — FieldDraft dreht ein Profil ` +
        `immer volle 360°; Feature übersprungen`,
    );
    return;
  }
  if (revolveType !== "FULL") {
    result.warnings.push(
      `${name}: Revolve-Typ ${revolveType} über insgesamt 360° — als volle Umdrehung ` +
        `übernommen (deckungsgleich)`,
    );
  }

  // Profile: same region resolution as the extrude (#412).
  const profileQss = queryStringsOf(params.get("entities"));
  const sketchFid = profileQss[0] ? await profileSketchId(profileQss[0]) : null;
  const rec = sketchFid ? sketches.get(sketchFid) : undefined;
  if (!rec) {
    result.errors.push(`${name}: Profil-Sketch nicht auflösbar — übersprungen`);
    return;
  }
  const profiles = await resolveProfiles(
    profileQss,
    rec,
    kernel,
    name,
    result,
    sketches,
    sweeps,
  );

  // Axis: a straight sketch line, construction included.
  const axisQs = queryStringsOf(params.get("axis"))[0];
  const axisRef = axisQs ? await decodeSketchEntityRef(axisQs) : null;
  const axisRec = axisRef ? sketches.get(axisRef.sketchFeatureId) : undefined;
  const axisOrd = axisRef && axisRec ? axisRec.entityIds[axisRef.entityId] : undefined;
  if (!axisRef || !axisRec || axisOrd === undefined) {
    result.errors.push(
      `${name}: Revolve-Achse nicht auflösbar ` +
        `(${axisRef ? `Sketch-Entity ${axisRef.entityId}` : "keine Sketch-Linie benannt"}) — ` +
        `FieldDraft nimmt als Achse nur eine Sketch-Linie (auch Konstruktionslinie); Feature übersprungen`,
    );
    return;
  }
  if (!axisRec.lineSegments.has(axisRef.entityId)) {
    result.errors.push(
      `${name}: Revolve-Achse ${axisRef.entityId} ist keine gerade Sketch-Linie — ` +
        `Feature übersprungen`,
    );
    return;
  }

  if (queryStringsOf(params.get("wallShape")).length > 0) {
    result.warnings.push(
      `${name}: dünnwandiger Revolve (Wandstärke ${mmOf(params.get("thickness1"), 0, env)} mm) ` +
        `wird nicht übernommen — importiert wird der volle Körper`,
    );
  }

  const operation = operationOf(params);
  const targets = await booleanTargets(params, operation, result);
  const fdId = kernel.addFeatureCall(
    "revolve",
    JSON.stringify({
      display_name: name,
      profiles: profiles.query,
      axis: `qSketchLine(${axisRec.fdId}, ${axisOrd})`,
      operation,
      ...(targets ? { targets } : {}),
    }),
  );
  result.idMap[feat.featureId] = fdId;
  result.created.push(fdId);
}

/**
 * Onshape `chamfer` → one `chamfer_edge` feature per resolved edge (#414).
 *
 * FieldDraft's chamfer is EQUAL_OFFSETS only: a symmetric bevel that sets back
 * `distance` on BOTH adjacent faces. Two Onshape forms lower to it exactly:
 *  - `EQUAL_OFFSETS` — verbatim, `width` is the leg length.
 *  - `OFFSET_ANGLE` at exactly 45° — the second leg is `width1 · tan(45°)`,
 *    i.e. equal to the first WHEN the two faces meet at a right angle. That
 *    caveat is reported, because it is the one case where the translation is
 *    an approximation rather than an identity.
 * Everything else (`TWO_OFFSETS`, `OPPOSITE_OFFSETS`, a non-45° angle) has no
 * FieldDraft counterpart and is SKIPPED with a warning naming the type — an
 * asymmetric bevel silently squared off is wrong geometry, not an import.
 */
async function importChamfer(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  env: ExprEnv,
): Promise<void> {
  const chamferType = String(params.get("chamferType")?.value ?? "EQUAL_OFFSETS");
  let distanceMm: number;
  if (chamferType === "EQUAL_OFFSETS") {
    distanceMm = mmOf(params.get("width"), 0, env);
  } else if (chamferType === "OFFSET_ANGLE") {
    const angleDeg = degOf(params.get("angle"), 0, env);
    if (Math.abs(angleDeg - 45) > 1e-3) {
      result.warnings.push(
        `${name}: Fasen-Typ OFFSET_ANGLE mit ${angleDeg}° — FieldDraft kann nur die ` +
          `symmetrische 45°-Fase; Feature übersprungen`,
      );
      return;
    }
    distanceMm = mmOf(params.get("width1"), 0, env);
    result.warnings.push(
      `${name}: OFFSET_ANGLE 45° als symmetrische Fase (${distanceMm} mm) übernommen — ` +
        `deckungsgleich, solange die beiden Flächen rechtwinklig zusammenstoßen`,
    );
  } else {
    result.warnings.push(
      `${name}: Fasen-Typ ${chamferType} (ungleiche Schenkel) wird nicht unterstützt — ` +
        `Feature übersprungen`,
    );
    return;
  }
  if (!(distanceMm > 0)) {
    result.errors.push(`${name}: Fasen-Maß ist 0 — Feature übersprungen`);
    return;
  }
  const method = String(params.get("chamferMethod")?.value ?? "FACE_OFFSET");
  if (method !== "FACE_OFFSET") {
    result.warnings.push(
      `${name}: Fasen-Methode ${method} wird als Flächen-Offset übernommen`,
    );
  }
  if (params.get("tangentPropagation")?.value === true) {
    result.warnings.push(
      `${name}: Tangenten-Fortpflanzung wird nicht nachgezogen — nur die benannten Kanten werden gefast`,
    );
  }

  // Same edge translation (and the same sibling-collapse caveat) as fillet.
  const edgeQs = queryStringsOf(params.get("entities"));
  const queries: string[] = [];
  for (const qs of edgeQs) {
    const edgeQuery = await buildEdgeQuery(qs, name, result, sketches, sweeps);
    if (edgeQuery && !queries.includes(edgeQuery)) queries.push(edgeQuery);
  }
  if (queries.length && queries.length < edgeQs.length) {
    result.warnings.push(
      `${name}: ${edgeQs.length} Onshape-Kanten → ${queries.length} unterscheidbare Query(s); ` +
        `Geschwister-Kanten sind (noch) nicht auseinanderzuhalten`,
    );
  }

  let made = 0;
  for (const edgeQuery of queries) {
    if (kernel.resolveQuery(edgeQuery).length < 1) {
      result.errors.push(`${name}: Kanten-Query "${edgeQuery}" löst nichts auf — übersprungen`);
      continue;
    }
    const fdId = kernel.addFeatureCall(
      "chamfer_edge",
      JSON.stringify({
        display_name: name,
        edge: edgeQuery,
        distance: { value: distanceMm, unit: "mm" },
      }),
    );
    if (made === 0) result.idMap[feat.featureId] = fdId;
    result.created.push(fdId);
    made += 1;
  }
}

/**
 * Onshape `hole` → a FieldDraft `hole` feature (#414, hole P1).
 *
 * What lowers EXACTLY: the bore itself — style `SIMPLE`, blind depth or
 * through-all, N sketch-point locations in one feature, the drill diameter
 * Onshape computed (for a tapped hole that is the tap-drill Ø, which is the
 * only volume Onshape cuts either — its thread is a callout, not geometry).
 *
 * What is reported and dropped: counterbore/countersink steps, the drill-tip
 * cone, an explicit start plane, and the `up to next` / `up to entity` end
 * conditions (hole P3/P4/P5). Each names the aspect it could not honour, so a
 * user reading the import report knows exactly what the bore is missing —
 * as opposed to a hole that quietly came out cylindrical.
 */
async function importHole(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  rawName: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  env: ExprEnv,
): Promise<void> {
  // Onshape's auto-name is the literal template `#featureName` in the feature
  // list — useless as a label, so fall back to the bore's own description.
  const metric = String(params.get("unitsSystem")?.value ?? "METRIC") === "METRIC";
  const table = msg(params.get(metric ? "isoHoleTableEx" : "ansiHoleTableEx")?.value ?? {});
  const holeKind = String(table.holeType ?? "Drilled");
  const tapped = /tapped/i.test(holeKind);
  const diameterMm = tapped
    ? mmOf(params.get("tapDrillDiameterV2") ?? params.get("tapDrillDiameter"), 0, env)
    : mmOf(params.get("holeDiameterV2") ?? params.get("holeDiameter"), 0, env);
  const name = rawName.startsWith("#") ? `Bohrung Ø${diameterMm} mm` : rawName;
  if (!(diameterMm > 0)) {
    result.errors.push(`${name}: Bohrungsdurchmesser ist 0 — Feature übersprungen`);
    return;
  }

  // Locations: one bore per selected sketch point, all sharing the parameters.
  const locations: string[] = [];
  const locQs = queryStringsOf(params.get("locations"));
  for (const qs of locQs) {
    const ref = await decodeSketchEntityRef(qs);
    const rec = ref ? sketches.get(ref.sketchFeatureId) : undefined;
    const ord = ref && rec ? rec.entityIds[ref.entityId] : undefined;
    if (!ref || !rec || ord === undefined) {
      result.errors.push(
        `${name}: eine Loch-Position ist nicht auflösbar ` +
          `(${ref ? `Sketch-Entity ${ref.entityId}` : "kein Sketch-Punkt"}) — Bohrung ausgelassen`,
      );
      continue;
    }
    const q = `qSketchPoint(${rec.fdId}, ${ord})`;
    if (!locations.includes(q)) locations.push(q);
  }
  if (locations.length === 0) {
    result.errors.push(`${name}: keine auflösbare Loch-Position — Feature übersprungen`);
    return;
  }

  // Merge scope → explicit targets (Feature-Scope-Direktive). A hole without a
  // scope drills nothing, so an unresolvable scope is an error, not a silent
  // no-op feature.
  const targets: string[] = [];
  for (const qs of queryStringsOf(params.get("scope"))) {
    const opRef = await decodeAnyOpRef(qs);
    const fd = opRef ? result.idMap[opRef] : undefined;
    if (fd === undefined) continue;
    const q = `qCreatedBy(${fd})`;
    if (!targets.includes(q)) targets.push(q);
  }
  if (targets.length === 0) {
    result.errors.push(`${name}: Bohr-Scope nicht auflösbar — Feature übersprungen`);
    return;
  }

  // End condition. Only blind + through-all exist in hole P1; `up to next` /
  // `up to entity` are approximated by through-all AND said out loud, because
  // that removes at least as much material as Onshape did, never less.
  const endStyle = String(params.get("endStyleV2")?.value ?? params.get("endStyle")?.value ?? "BLIND");
  let endType = "blind";
  let depthMm = mmOf(params.get("holeDepth"), 0, env);
  if (endStyle === "THROUGH" || endStyle === "THROUGH_ALL") {
    endType = "through_all";
  } else if (endStyle !== "BLIND") {
    endType = "through_all";
    result.warnings.push(
      `${name}: End-Bedingung ${endStyle} wird noch nicht unterstützt — als „durch alles" gebohrt`,
    );
  } else if (!(depthMm > 0)) {
    endType = "through_all";
    result.warnings.push(`${name}: Blind-Tiefe fehlt — als „durch alles" gebohrt`);
  }
  if (endType === "through_all") depthMm = 0;

  // Everything the P1 bore cannot represent, named individually.
  const style = String(params.get("styleV2")?.value ?? params.get("style")?.value ?? "SIMPLE");
  if (style === "C_BORE") {
    result.warnings.push(
      `${name}: Senkung (Counterbore Ø${mmOf(params.get("cBoreDiameter"), 0, env)} mm × ` +
        `${mmOf(params.get("cBoreDepth"), 0, env)} mm) wird nicht modelliert — als einfache Bohrung importiert`,
    );
  } else if (style === "C_SINK") {
    result.warnings.push(
      `${name}: Senkung (Countersink Ø${mmOf(params.get("cSinkDiameter"), 0, env)} mm × ` +
        `${degOf(params.get("cSinkAngle"), 0, env)}°) wird nicht modelliert — als einfache Bohrung importiert`,
    );
  } else if (style !== "SIMPLE") {
    result.warnings.push(`${name}: Loch-Stil ${style} wird als einfache Bohrung importiert`);
  }
  if (tapped) {
    result.warnings.push(
      `${name}: Gewinde ist kosmetisch (wie in Onshape) — gebohrt wird das Kernloch Ø${diameterMm} mm`,
    );
  }
  const tip = String(params.get("tipAngleStyle")?.value ?? "FLAT");
  if (endType === "blind" && tip !== "FLAT") {
    result.warnings.push(
      `${name}: Bohrspitze (${degOf(params.get("tipAngle"), 0, env)}°) wird nicht modelliert — flacher Bohrgrund`,
    );
  }
  if (queryStringsOf(params.get("startBoundEntity")).length > 0) {
    result.warnings.push(
      `${name}: Start-Referenz (${params.get("startStyle")?.value ?? "?"}) wird ignoriert — ` +
        `gebohrt wird ab dem Zielpunkt`,
    );
  }
  if (params.get("offset")?.value === true) {
    result.warnings.push(`${name}: Endversatz (offset) wird ignoriert`);
  }

  const fdId = kernel.addFeatureCall(
    "hole",
    JSON.stringify({
      display_name: name,
      style: "simple",
      hole_type: "drilled",
      // The bore diameter is taken VERBATIM from Onshape rather than looked up
      // in FieldDraft's ISO table: the document is the ground truth, and a
      // standard-size round-trip could silently shift the hole by a tenth.
      standard_size: "custom",
      diameter: { value: diameterMm, unit: "mm" },
      end_type: endType,
      depth: { value: depthMm, unit: "mm" },
      opposite_direction: params.get("oppositeDirection")?.value === true,
      locations: locations.join(", "),
      targets: targets.join(", "),
    }),
  );
  result.idMap[feat.featureId] = fdId;
  result.created.push(fdId);
}

/**
 * Onshape `thicken` → a FieldDraft `thicken` feature (#415/#417/#418/#419).
 *
 * Onshape's Thicken offsets the SURFACES it is given into a solid wall. When
 * those surfaces are faces of a solid (the whole corpus: 25 occurrences, all
 * face selections) that is precisely FieldDraft's `thicken`, which was built
 * against this Onshape feature: `thickness1` grows outward along the face
 * normal, `thickness2` inward, the direction arrow swaps the two. A planar face
 * reduces to the same prismatic slab a face-Extrude builds (#402), a curved
 * analytic one to concentric offset surfaces — so the mapping is an identity,
 * not an approximation. That is why this is `thicken` and not a face-Extrude:
 * an Extrude would need a single planar profile and could not carry a curved
 * wall at all.
 *
 * The one translation step with teeth is the FACE REFERENCE. Onshape names each
 * face with the same creation query a sketch plane uses, so the decoding is
 * {@link decodePlaneQuery} + {@link buildPlaneQuery} — but the kernel's thicken
 * accepts ONLY the id-hash form `qFace(<hash> @ <fingerprint>)`. Every
 * parametric query is therefore resolved against the mid-history B-Rep and
 * stored as the canonical element reference it binds to right now. A query that
 * binds to no face, or to several, is DROPPED with an error naming the count:
 * thickening a wall Onshape never picked adds material where the document has
 * none, and that is the failure mode this whole importer is built to avoid.
 */
async function importThicken(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  env: ExprEnv,
): Promise<void> {
  const entityQss = queryStringsOf(params.get("entities"));
  const faces: string[] = [];
  for (const qs of entityQss) {
    const ref = await decodePlaneQuery(qs);
    const built = buildPlaneQuery(ref, name, result.idMap, sketches, sweeps);
    if (!built.query) {
      result.errors.push(built.error ?? `${name}: Fläche nicht übersetzbar`);
      continue;
    }
    // A datum plane has no material to offset — Onshape would not accept it
    // either, but our plane translation happily produces one, so say why.
    if (built.query.startsWith("qCreatedBy(origin_plane_")) {
      result.errors.push(
        `${name}: "${built.query}" ist eine Ursprungsebene, keine Körperfläche — nicht aufdickbar`,
      );
      continue;
    }
    const matches = kernel.resolveQuery(built.query).filter((m) => m.kind === "face");
    if (matches.length !== 1) {
      result.errors.push(
        `${name}: Flächen-Query "${built.query}" löst auf ${matches.length} Flächen auf ` +
          `(erwartet 1) — Fläche ausgelassen`,
      );
      continue;
    }
    // The fingerprint-carrying canonical form — the only one `thicken` takes.
    if (!faces.includes(matches[0].query)) faces.push(matches[0].query);
  }
  if (faces.length === 0) {
    result.errors.push(`${name}: keine auflösbare Fläche — Feature übersprungen`);
    return;
  }
  if (faces.length < entityQss.length) {
    result.warnings.push(
      `${name}: Onshape dickt ${entityQss.length} Flächen auf, FieldDraft ${faces.length} — ` +
        `die fehlenden sind oben einzeln genannt`,
    );
  }

  let thicknessMm = mmOf(params.get("thickness1"), 0, env);
  let backMm = mmOf(params.get("thickness2"), 0, env);
  let reverse = params.get("oppositeDirection")?.value === true;
  if (!(thicknessMm > 0) && backMm > 0) {
    // Onshape's first direction is empty: the wall lies entirely on the other
    // side. Same solid, written in the only form FieldDraft has — a positive
    // primary thickness plus the flipped growth direction.
    thicknessMm = backMm;
    backMm = 0;
    reverse = !reverse;
  }
  if (!(thicknessMm > 0)) {
    result.errors.push(`${name}: Wandstärke ist 0 — Feature übersprungen`);
    return;
  }

  if (params.get("keepTools")?.value === true) {
    result.warnings.push(
      `${name}: „Werkzeuge behalten" betrifft nur Onshapes Flächenkörper — FieldDraft dickt ` +
        `Flächen eines Volumenkörpers auf, die dabei ohnehin erhalten bleiben`,
    );
  }

  const operation = operationOf(params);
  const targets = await booleanTargets(params, operation, result);
  if (operation !== "new" && !targets) {
    result.warnings.push(
      `${name}: Boolean-Scope nicht auflösbar — die Wand wird mit dem Körper verrechnet, ` +
        `dem die aufgedickte Fläche gehört`,
    );
  }

  const fdId = kernel.addFeatureCall(
    "thicken",
    JSON.stringify({
      display_name: name,
      faces: faces.join(", "),
      thickness: { value: thicknessMm, unit: "mm" },
      ...(backMm > 0 ? { thickness_back: { value: backMm, unit: "mm" } } : {}),
      ...(reverse ? { reverse: true } : {}),
      operation,
      ...(targets ? { targets } : {}),
    }),
  );
  result.idMap[feat.featureId] = fdId;
  result.created.push(fdId);
}

// ─── P5: Muster und Spiegelung (#409 / #419) ──────────────────────────────
//
// Onshape's three replication features share a shape: a set of BODIES to copy,
// a rule for where the copies go, and a count. FieldDraft has all three as
// kernel features, so the work is translation, not construction — with one
// sharp exception per type, each recorded where it bites.

/** A world-space axis: a unit direction and a point on it. */
interface WorldAxis {
  dir: [number, number, number];
  /** A point ON the axis, in mm — a circular pattern's pivot. */
  point: [number, number, number];
}

/** Which Onshape features were imported as a PATTERN, and what they replicate.
 *  A pattern OF a pattern has no FieldDraft counterpart (the kernel's source
 *  lookup finds the pattern feature itself, not its clones), so the second one
 *  has to say that instead of quietly replicating only the original. */
interface PatternRecord {
  fdId: number;
  sourceIds: number[];
}

const vlen = (v: number[]): number => Math.hypot(v[0], v[1], v[2]);

/**
 * The world direction (and a point on it) of a straight SKETCH LINE.
 *
 * The line's 2D endpoints are lifted through the sketch's own FieldDraft frame
 * — the same frame the kernel builds its geometry on, so the axis is where the
 * bodies actually are and not merely where Onshape put them. Onshape's
 * start→end order is preserved, because that order IS the direction its
 * `oppositeDirection` flag is counted against (verified against Onshape's own
 * part boxes for #419: the axis line runs +X→−X and the copies land at −X).
 */
function sketchLineAxis(
  rec: SketchRecord,
  entityId: string,
  kernel: ImportKernel,
): WorldAxis | null {
  const seg = rec.lineSegments.get(entityId);
  if (!seg) return null;
  const f = frameOf(kernel, rec.planeQuery);
  if (!f) return null;
  const at = (p: [number, number]): [number, number, number] => [
    f.origin[0] + f.xAxis[0] * p[0] + f.yAxis[0] * p[1],
    f.origin[1] + f.xAxis[1] * p[0] + f.yAxis[1] * p[1],
    f.origin[2] + f.xAxis[2] * p[0] + f.yAxis[2] * p[1],
  ];
  const a = at(seg[0]);
  const b = at(seg[1]);
  const d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
  const len = vlen(d);
  if (len < 1e-9) return null;
  return { dir: [d[0] / len, d[1] / len, d[2] / len], point: a };
}

/**
 * An Onshape pattern DIRECTION / AXIS query → a world axis.
 *
 * Exactly one form lowers exactly: a straight SKETCH LINE (construction lines
 * included). Everything else is reported with the concrete reason.
 *
 * The one that hurts is a **swept B-Rep edge** (`SWEPT_EDGE`, #409): its LINE is
 * known — a lateral edge of an extrude is parallel to that extrude's sweep, i.e.
 * to its profile sketch's normal — but its ORIENTATION is not. Onshape counts
 * `oppositeDirection` against the edge's own parametrisation, which lives in
 * Onshape's B-Rep and appears nowhere in the feature payload. Measured against
 * Onshape's own part boxes for #409, that orientation is the OPPOSITE of the
 * sweep (the extrude grows +Z, the edge runs −Z), so "use the sweep direction"
 * would be a coin flip dressed as a rule — and a pattern with a flipped sign
 * puts every copy on the wrong side of the part. It is skipped instead, and the
 * gap-matrix records the way to close it: pull the edge orientation from
 * Onshape's own answer, the way `sources.sketches3d` already does for frames.
 */
async function patternAxis(
  qs: string | undefined,
  name: string,
  label: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
): Promise<WorldAxis | null> {
  if (!qs) {
    result.errors.push(`${name}: ${label} fehlt — Feature übersprungen`);
    return null;
  }
  const ref = await decodeSketchEntityRef(qs);
  if (ref) {
    const rec = sketches.get(ref.sketchFeatureId);
    if (!rec) {
      result.errors.push(
        `${name}: ${label} liegt in Sketch ${ref.sketchFeatureId}, der nicht importiert wurde — Feature übersprungen`,
      );
      return null;
    }
    if (!rec.lineSegments.has(ref.entityId)) {
      result.errors.push(
        `${name}: ${label} nennt die Sketch-Entity ${ref.entityId}, die keine gerade Linie ist — ` +
          `Feature übersprungen`,
      );
      return null;
    }
    const axis = sketchLineAxis(rec, ref.entityId, kernel);
    if (!axis) {
      result.errors.push(
        `${name}: ${label} (Sketch-Linie ${ref.entityId}) ist nicht in Weltkoordinaten auflösbar — ` +
          `Feature übersprungen`,
      );
      return null;
    }
    return axis;
  }

  // Not a sketch entity. Name what it is, so the gap is legible.
  let markers: string[] = [];
  try {
    const { bton, payload } = await decodeOnshapeQueryString(qs);
    if (bton) {
      markers = [
        ...new Set(
          tokenizeBton(payload)
            .filter((t) => t.t === "str" && t.p1 && !t.p2 && /EDGE|FACE|VERTEX/.test(t.p1))
            .map((t) => t.p1 as string),
        ),
      ];
    }
  } catch {
    markers = [];
  }
  if (markers.includes("SWEPT_EDGE")) {
    result.errors.push(
      `${name}: ${label} ist eine Mantelkante eines Extrudes. Ihre GERADE ist bekannt ` +
        `(parallel zur Extrusionsrichtung), ihre ORIENTIERUNG nicht: Onshape zählt ` +
        `„oppositeDirection" gegen die Parametrisierung dieser Kante, die nur in Onshapes ` +
        `B-Rep steht und im Feature-Payload nicht vorkommt (am #409-Dokument nachgemessen: ` +
        `sie läuft der Extrusionsrichtung ENTGEGEN). Ein geratenes Vorzeichen setzt jede ` +
        `Kopie auf die falsche Seite — Feature übersprungen ` +
        `(siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md)`,
    );
    return null;
  }
  result.errors.push(
    `${name}: ${label} ist keine Sketch-Linie (${markers.join(", ") || "unbekannte Query-Form"}) — ` +
      `FieldDraft nimmt als Muster-Richtung nur eine gerade Sketch-Linie; Feature übersprungen`,
  );
  return null;
}

/**
 * The bodies a pattern/mirror replicates → the FieldDraft features that own
 * them. Same decoding as {@link booleanOperands}: an Onshape body query names
 * the OPERATION that created the body, and `qCreatedBy(<feature>)` is the only
 * source form the kernel's pattern features accept.
 *
 * The extra check here is CHAINING: `#409` patterns the output of an earlier
 * pattern. FieldDraft cannot do that — its pattern clones are children of the
 * pattern feature, and a second pattern naming it would clone the pattern
 * feature rather than its copies (`patterns.rs`, "Pattern-of-pattern isn't
 * supported"). Rather than silently replicating only the original, that case
 * is refused by name.
 */
async function patternSources(
  pm: AnyMsg | undefined,
  name: string,
  result: OnshapeImportResult,
  patterns: Map<string, PatternRecord>,
): Promise<number[] | null> {
  const out: number[] = [];
  const qss = queryStringsOf(pm);
  if (qss.length === 0) {
    result.errors.push(`${name}: keine Körper ausgewählt — Feature übersprungen`);
    return null;
  }
  for (const qs of qss) {
    const ref = await decodeAnyOpRef(qs);
    if (ref !== null && patterns.has(ref)) {
      result.errors.push(
        `${name}: der zu vervielfältigende Körper stammt aus dem Muster ${ref} — ` +
          `FieldDraft kann kein Muster eines Musters (die Kopien sind Kinder des ` +
          `Muster-Features, nicht eigene Körper); Feature übersprungen`,
      );
      return null;
    }
    const fd = ref === null ? undefined : result.idMap[ref];
    if (fd === undefined) {
      result.errors.push(
        `${name}: zu vervielfältigender Körper nicht auflösbar ` +
          `(${ref ? `Onshape-Operation ${ref} wurde nicht importiert` : "Query benennt keine importierte Operation"}) — ` +
          `Feature übersprungen`,
      );
      return null;
    }
    if (!out.includes(fd)) out.push(fd);
  }
  return out;
}

/** A COUNT parameter (`instanceCount`) → a positive integer. Onshape writes it
 *  as a dimensionless quantity expression, so it goes through the same
 *  variable-aware evaluator as every dimension — `#Faecher` is a legal count. */
function countOf(pm: AnyMsg | undefined, name: string, label: string, env: ExprEnv): number | null {
  if (!pm) return null;
  const expr = typeof pm.expression === "string" ? pm.expression.trim() : "";
  if (!expr) {
    return typeof pm.value === "number" && pm.value > 0 ? Math.round(pm.value) : null;
  }
  const ev = tryEvalOnshapeExpression(expr, env.vars);
  if (!ev.ok || quantityKind(ev.q) !== "number") {
    warnOnce(
      env,
      `${name}: ${label} „${expr}" ist keine reine Zahl ` +
        `(${ev.ok ? describeDimension(ev.q) : ev.error}) — Feature übersprungen`,
    );
    return null;
  }
  return Math.round(ev.q.v);
}

/** Onshape's pattern `operationType` against FieldDraft's pattern features,
 *  which carry NO boolean op of their own: every clone inherits the boolean op
 *  of the feature it copies. For `NEW` that is exactly Onshape's behaviour;
 *  anything else is reported rather than approximated. */
function notePatternOperation(
  params: Map<string, AnyMsg>,
  name: string,
  result: OnshapeImportResult,
): void {
  const op = String(params.get("operationType")?.value ?? "NEW");
  if (op === "NEW") return;
  result.warnings.push(
    `${name}: Onshape-Operation ${op} — FieldDrafts Muster kennt keine eigene ` +
      `Boolean-Operation; jede Kopie erbt die des Quell-Features`,
  );
}

/** `skippedInstances` has no FieldDraft counterpart — a pattern is a count, not
 *  a list. Say so instead of shipping the instances Onshape left out. */
function noteSkippedInstances(
  params: Map<string, AnyMsg>,
  name: string,
  result: OnshapeImportResult,
): void {
  const n = (params.get("skippedInstances")?.items ?? []).length;
  if (n > 0) {
    result.warnings.push(
      `${name}: ${n} ausgelassene Instanz(en) werden nicht nachgebildet — ` +
        `FieldDrafts Muster ist eine Anzahl, keine Liste; die Kopien werden vollzählig gesetzt`,
    );
  }
}

/**
 * Onshape `linearPattern` (PART) → a FieldDraft `linear_pattern` (#409/#419).
 *
 * The mapping is an identity where it lands: N copies of the same bodies at
 * `i · distance` along a direction, the original included in the count. Proven
 * against the real kernel — a sketch extrude patterned along +X at 50 mm gives
 * bodies at exactly +0/+50/+100 mm.
 *
 * What is refused rather than approximated:
 *  - `patternType` FACE / FEATURE — FieldDraft replicates whole BODIES; a face
 *    pattern has no counterpart at all, and a feature pattern would need the
 *    feature's inputs re-evaluated at each station.
 *  - a SECOND direction (a 2-D grid): that is a pattern OF a pattern, which the
 *    kernel does not do (see {@link patternSources}).
 *  - a direction whose orientation is not derivable (see {@link patternAxis}).
 */
async function importLinearPattern(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  patterns: Map<string, PatternRecord>,
  env: ExprEnv,
): Promise<void> {
  const kind = String(params.get("patternType")?.value ?? "PART");
  if (kind !== "PART") {
    result.errors.push(
      `${name}: Muster-Typ ${kind} — FieldDraft vervielfältigt ganze Körper, ` +
        `${kind === "FACE" ? "kein Flächen-Muster" : "kein Feature-Muster"}; Feature übersprungen`,
    );
    return;
  }

  const sourceIds = await patternSources(params.get("entities"), name, result, patterns);
  if (!sourceIds) return;

  const axis = await patternAxis(
    queryStringsOf(params.get("directionOne"))[0],
    name,
    "Muster-Richtung",
    kernel,
    result,
    sketches,
  );
  if (!axis) return;

  const count = countOf(params.get("instanceCount"), name, "Instanz-Anzahl", env);
  if (count === null) return;
  const spacingMm = mmOf(params.get("distance"), 0, env);
  if (count < 2) {
    result.warnings.push(`${name}: ${count} Instanz(en) — nichts zu vervielfältigen, übersprungen`);
    return;
  }
  if (!(spacingMm > 0)) {
    result.errors.push(
      `${name}: Abstand ${fmtNum(spacingMm)} mm ist nicht positiv — Feature übersprungen`,
    );
    return;
  }

  // A second direction is a GRID, i.e. a pattern of a pattern.
  const countTwo = countOf(params.get("instanceCountTwo"), name, "Instanz-Anzahl 2", env) ?? 1;
  if (params.get("hasSecondDirection")?.value === true && countTwo > 1) {
    result.warnings.push(
      `${name}: zweite Richtung (${countTwo} Instanzen) wird nicht übernommen — ` +
        `ein 2D-Raster ist ein Muster eines Musters, das FieldDraft nicht kann; ` +
        `importiert wird nur die erste Richtung`,
    );
  }
  notePatternOperation(params, name, result);
  noteSkippedInstances(params, name, result);

  const flip = params.get("oppositeDirection")?.value === true ? -1 : 1;
  const fdId = kernel.addFeatureCall(
    "linear_pattern",
    JSON.stringify({
      display_name: name,
      source: sourceIds.map((i) => `qCreatedBy(${i})`).join(", "),
      direction: [axis.dir[0] * flip, axis.dir[1] * flip, axis.dir[2] * flip],
      count,
      spacing: { value: spacingMm, unit: "mm" },
    }),
  );
  result.created.push(fdId);
  patterns.set(feat.featureId, { fdId, sourceIds });
  // Like the Boolean (see `importBooleanBodies`): the id map points at the
  // feature that OWNS the geometry a later Onshape query would name. The
  // pattern feature itself creates no body of its own — it clones the source's.
  result.idMap[feat.featureId] = sourceIds[0];
}

/**
 * Onshape `circularPattern` (PART) → a FieldDraft `circular_pattern` (#419).
 *
 * FieldDraft places instance `i` at `(i / count) · angle_deg` around the axis,
 * Onshape at `i · angle` when `equalSpace` is off and evenly across a full turn
 * when it is on. Two of those coincide exactly:
 *
 *  - `equalSpace` + 360° — the same ring, `angle_deg = 360`. (Proven against the
 *    real kernel: 4 instances land on the quarter turns.)
 *  - no `equalSpace` — `angle` is the step, so `angle_deg = angle · count`
 *    reproduces every station exactly.
 *
 * `equalSpace` over a PARTIAL angle is the one that is refused: whether Onshape
 * divides that arc by `count` or by `count − 1` decides where every copy but the
 * first sits, and the corpus carries no partial equal-spaced pattern to measure
 * it against. Guessing would be an invisible half-step error on every instance.
 */
async function importCircularPattern(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  patterns: Map<string, PatternRecord>,
  env: ExprEnv,
): Promise<void> {
  const kind = String(params.get("patternType")?.value ?? "PART");
  if (kind !== "PART") {
    result.errors.push(
      `${name}: Muster-Typ ${kind} — FieldDraft vervielfältigt ganze Körper, ` +
        `${kind === "FACE" ? "kein Flächen-Muster" : "kein Feature-Muster"}; Feature übersprungen`,
    );
    return;
  }

  const sourceIds = await patternSources(params.get("entities"), name, result, patterns);
  if (!sourceIds) return;

  const axis = await patternAxis(
    queryStringsOf(params.get("axis"))[0],
    name,
    "Rotationsachse",
    kernel,
    result,
    sketches,
  );
  if (!axis) return;

  const count = countOf(params.get("instanceCount"), name, "Instanz-Anzahl", env);
  if (count === null) return;
  if (count < 2) {
    result.warnings.push(`${name}: ${count} Instanz(en) — nichts zu vervielfältigen, übersprungen`);
    return;
  }
  const angleDeg = degOf(params.get("angle"), 360, env);
  const equalSpace = params.get("equalSpace")?.value === true;
  let totalDeg: number;
  if (equalSpace && Math.abs(angleDeg - 360) < 1e-6) {
    totalDeg = 360;
  } else if (!equalSpace) {
    totalDeg = angleDeg * count;
  } else {
    result.errors.push(
      `${name}: gleichmäßige Verteilung über eine TEIL-Umdrehung (${fmtNum(angleDeg)}°, ` +
        `${count} Instanzen) — ob Onshape den Bogen durch ${count} oder ${count - 1} teilt, ` +
        `entscheidet über die Lage jeder Kopie und ist am Korpus nicht nachmessbar; ` +
        `Feature übersprungen (siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md)`,
    );
    return;
  }
  notePatternOperation(params, name, result);
  noteSkippedInstances(params, name, result);

  const flip = params.get("oppositeDirection")?.value === true ? -1 : 1;
  const fdId = kernel.addFeatureCall(
    "circular_pattern",
    JSON.stringify({
      display_name: name,
      source: sourceIds.map((i) => `qCreatedBy(${i})`).join(", "),
      axis: [axis.dir[0] * flip, axis.dir[1] * flip, axis.dir[2] * flip],
      center: axis.point,
      count,
      angle_deg: totalDeg,
    }),
  );
  result.created.push(fdId);
  patterns.set(feat.featureId, { fdId, sourceIds });
  result.idMap[feat.featureId] = sourceIds[0];
}

/**
 * Onshape `mirror` (#409) — and why nothing is emitted for it.
 *
 * FieldDraft HAS a `mirror` feature, and it is not the one Onshape means.
 * `MirrorFeature::execute` (crates/kernel/src/features/patterns.rs) reflects the
 * source feature's `transform.translate` through the plane and drops an
 * otherwise VERBATIM copy there: it RELOCATES a copy, it does not REFLECT the
 * geometry. That is exact for a symmetric primitive whose transform origin is
 * its centre — the toolbar's use case — and wrong for everything else.
 *
 * Measured against the real kernel rather than read off the source: a sketch
 * extrude occupying x ∈ [10, 30] mm, mirrored through the YZ datum, produces a
 * second body at x ∈ [10, 30] mm — a copy sitting exactly ON the original,
 * where Onshape has x ∈ [−30, −10]. A sketch-built body carries
 * `translate = [0, 0, 0]`, so the reflection has nothing to move.
 *
 * The condition for the two to agree is sharp: the source body must ALREADY be
 * symmetric about the plane parallel to the mirror plane through its own
 * transform origin — which is precisely the case in which nobody would draw a
 * mirror. Emitting the feature anyway would put a silent duplicate into every
 * document that has one, so the mirror is skipped and the gap named.
 *
 * The plane is still decoded, because it is half of the diagnosis: all three
 * mirrors of #409 sit on an implicit MATE CONNECTOR, which `resolve_plane_frame`
 * cannot turn into a plane either (the second, independent blocker — the same
 * kernel gap that keeps #416's sketches out). The connector is imported as
 * reference geometry so the plane at least exists in the document.
 */
/**
 * Onshape `mirror` (PART) → a FieldDraft `mirror` (#409 / #419).
 *
 * The kernel feature reflects each source body's PLACEMENT through the plane
 * (`crates/kernel/src/features/patterns.rs`), so the translation is the plane
 * query plus the source list. It did not always: until the reflection was made
 * real, `MirrorFeature` moved only the source's origin and left the body
 * unturned, which put a sketch-based body — origin at the world zero — exactly
 * on top of its source. This translator refused the feature outright for that
 * reason; the refusal is gone, the plane-query gaps below are what is left.
 */
async function importMirror(
  feat: AnyMsg,
  params: Map<string, AnyMsg>,
  name: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
  patterns: Map<string, PatternRecord>,
  env: ExprEnv,
): Promise<void> {
  const kind = String(params.get("patternType")?.value ?? "PART");
  if (kind !== "PART") {
    result.errors.push(
      `${name}: Spiegel-Typ ${kind} — FieldDraft spiegelt ganze Körper; Feature übersprungen`,
    );
    return;
  }

  const sourceIds = await patternSources(params.get("entities"), name, result, patterns);
  if (!sourceIds) return;

  const planeQs = queryStringsOf(params.get("mirrorPlane"))[0];
  if (!planeQs) {
    result.errors.push(`${name}: keine Spiegelebene benannt — Feature übersprungen`);
    return;
  }

  // A mate connector as the mirror plane. Since #409 `resolve_plane_frame`
  // lowers a connector's CSYS to a plane frame, so the mirror runs on the
  // connector itself — imported here from the mirror's own `subFeatures`, the
  // same way a sketch's implicit connector is (#416).
  let planeQuery: string;
  const mcId = mateConnectorPlaneId(planeQs);
  if (mcId) {
    const sub = implicitMateConnectors(feat).get(mcId);
    const built = sub
      ? await emitMateConnector(
          sub,
          `${name} · ${String(sub.name ?? "Mate connector")}`,
          kernel,
          result,
          sketches,
          sweeps,
          env,
        )
      : null;
    if (built?.fdId == null) {
      result.errors.push(
        `${name}: die Spiegelebene ist ein Mate-Connector, der nicht übersetzt werden ` +
          `konnte — Feature übersprungen`,
      );
      return;
    }
    planeQuery = `qCreatedBy(${built.fdId})`;
  } else {
    const plane = buildPlaneQuery(
      await decodePlaneQuery(planeQs),
      name,
      result.idMap,
      sketches,
      sweeps,
    );
    if (!plane.query) {
      result.errors.push(
        `${name}: Spiegelebene nicht übersetzbar (${plane.error ?? "unbekannte Form"}) — ` +
          `Feature übersprungen`,
      );
      return;
    }
    planeQuery = plane.query;
  }

  notePatternOperation(params, name, result);

  const fdId = kernel.addFeatureCall(
    "mirror",
    JSON.stringify({
      display_name: name,
      source: sourceIds.map((i) => `qCreatedBy(${i})`).join(", "),
      plane: planeQuery,
    }),
  );
  result.created.push(fdId);
  patterns.set(feat.featureId, { fdId, sourceIds });
  // Like the patterns and the Boolean: a later Onshape query naming this
  // feature means the geometry, which the SOURCE owns — the mirror only clones.
  result.idMap[feat.featureId] = sourceIds[0];
}

/** First feature-operation id inside a compressed query (`…opExtrude`
 *  etc.) — used for boolean-scope (`SWEPT_BODY of X`) resolution. */
async function decodeAnyOpRef(qs: string): Promise<string | null> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(qs);
    if (!bton) return null;
    for (const t of tokenizeBton(payload)) {
      if (t.t === "str" && t.p1 && /^op[A-Z]/.test(t.p2 ?? "") && /^F[A-Za-z0-9]+_\d+$/.test(t.p1)) {
        return t.p1;
      }
    }
  } catch {
    return null;
  }
  return null;
}

/**
 * One face of an edge, in FieldDraft's query language.
 *
 * Onshape's START cap is the SKETCH-PLANE side; which FieldDraft cap that is
 * depends on the extrude's DIRECTION, because a FieldDraft slab is centred on
 * `depth/2` — a negative depth puts CAP_TOP, not CAP_BOTTOM, flush with the
 * sketch plane (the same bookkeeping `buildPlaneQuery` does for cap planes).
 *
 * Recursive, because {@link OnshapeFaceRef} is: Onshape's boolean shapes are
 * combinations of named faces, and each combinator has one FieldDraft form —
 * `MERGE` → `qUnion`, `SPLIT` → `qAdjacentTo` (#429/E3). Nothing here invents a
 * name; a sub-face that has none propagates its error out.
 */
function faceQueryOf(
  face: OnshapeFaceRef,
  name: string,
  idMap: Record<string, number>,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): { query?: string; error?: string } {
  const capRole = (opFeatureId: string, isStart: boolean, inner: boolean): string => {
    const positive = sweeps.get(opFeatureId)?.depthPositive ?? true;
    const top = isStart ? !positive : positive;
    return `${inner ? "INNER_" : ""}CAP_${top ? "TOP" : "BOTTOM"}`;
  };
  const recurse = (f: OnshapeFaceRef) => faceQueryOf(f, name, idMap, sketches, sweeps);

  // The combinators carry no operation of their own — resolve the parts first.
  if (face.kind === "union") {
    const parts: string[] = [];
    for (const p of face.parts) {
      const q = recurse(p);
      if (!q.query) return q;
      if (!parts.includes(q.query)) parts.push(q.query);
    }
    // Left fold: `qUnion` is binary, an n-way merge nests. Two partners that
    // translate to the SAME FieldDraft face collapse to one — FieldDraft may
    // already keep as one face what Onshape merged.
    return { query: parts.reduce((acc, q) => `qUnion(${acc}, ${q})`) };
  }
  if (face.kind === "adjacent") {
    const base = recurse(face.base);
    if (!base.query) return base;
    let query = base.query;
    for (const n of face.neighbours) {
      const q = recurse(n);
      if (!q.query) return q;
      query = `qAdjacentTo(${query}, ${q.query})`;
    }
    return { query };
  }

  const fd = idMap[face.opFeatureId];
  if (fd === undefined) {
    return { error: `${name}: Kanten-Query nennt das nicht importierte Feature ${face.opFeatureId}` };
  }
  switch (face.kind) {
    case "cap":
      return { query: `qCreatedBy(${fd}, ${capRole(face.opFeatureId, face.isStart, false)})` };
    case "innerCap":
      return { query: `qCreatedBy(${fd}, ${capRole(face.opFeatureId, face.isStart, true)})` };
    case "innerWall":
      return { query: `qCreatedBy(${fd}, INNER_WALL)` };
    case "blend":
      return { query: `qCreatedBy(${fd}, BLEND)` };
    case "wall": {
      const wall = wallQuery(face.origin, fd, name, sketches);
      return wall.query ? { query: wall.query } : { error: wall.error };
    }
  }
}

/**
 * An Onshape edge query → a FieldDraft `qCommonEdge(faceA, faceB)`. Shared by
 * the `fillet` and `chamfer` translations (#414/#425): both select EDGES, and
 * FieldDraft names an edge by the two faces that meet at it.
 *
 * The shape recognition lives in {@link decodeEdgeQuery}, which reads the
 * PARSED query document (root `queryType` + the fields at their keys). This
 * function only lowers the decoded face pair — cap, wall, shell skin — into
 * kernel query text and reports, per query, why one could not be built.
 *
 * Known limit (#425, measured on the #419 corpus): an edge whose history runs
 * through a boolean, a pattern copy or an earlier blend is REFUSED by name. Two
 * SIBLING edges of one fillet can still decode to the same face pair, which the
 * caller de-duplicates rather than filleting the same edge twice.
 */
async function buildEdgeQuery(
  qs: string,
  name: string,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): Promise<string | null> {
  const ref = await decodeEdgeQuery(qs);
  if (ref.kind === "unsupported") {
    result.errors.push(`${name}: ${ref.reason}`);
    return null;
  }
  const a = faceQueryOf(ref.a, name, result.idMap, sketches, sweeps);
  const b = faceQueryOf(ref.b, name, result.idMap, sketches, sweeps);
  if (!a.query || !b.query) {
    result.errors.push(a.error ?? b.error ?? `${name}: Kante nicht übersetzbar`);
    return null;
  }
  return `qCommonEdge(${a.query}, ${b.query})`;
}

/**
 * An Onshape query that names ONE sketch entity directly → that entity.
 *
 * Two translations need this shape: a hole LOCATION (the sketch point it is
 * drilled at, #414) and a revolve AXIS (the sketch line it spins around, #416).
 * Both travel as `queryType: SKETCH_ENTITY` + a `sketchEntityId` whose owning
 * sketch is the preceding `wireOp` marker. Unlike a wall/edge origin the id is
 * usually a SINGLE-part string (a free point or a construction line has no
 * `.top`/`.left` suffix), so the generic two-part origin scan does not see it —
 * read it positionally instead.
 *
 * Returns `null` for anything that is not a sketch entity (mate connectors,
 * B-Rep vertices and edges, datum axes): the caller reports that honestly
 * instead of guessing a position or an axis.
 */
async function decodeSketchEntityRef(qs: string): Promise<OnshapeEntityOrigin | null> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(qs);
    if (!bton) return null;
    const tokens = tokenizeBton(payload);
    const idx = tokens.findIndex((t) => t.t === "str" && t.p1 === "sketchEntityId" && !t.p2);
    if (idx < 0) return null;
    const idTok = tokens.slice(idx + 1).find((t) => t.t === "str");
    if (!idTok?.p1) return null;
    const { wires } = extractQueryRefs(tokens);
    const wire = [...wires].reverse().find((w) => w.idx < idx);
    if (!wire) return null;
    return {
      sketchFeatureId: wire.featureId,
      entityId: idTok.p2 ? `${idTok.p1}.${idTok.p2}` : idTok.p1,
    };
  } catch {
    return null;
  }
}
