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
  decodeOnshapeQueryString,
  decodePlaneQuery,
  extractOpRefs,
  extractPlaneRef,
  extractQueryRefs,
  tokenizeBton,
  type OnshapeEntityOrigin,
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
import { translateOnshapeSketch, type SketchSubCall } from "./onshapeSketch";
import { decodeRegionQuery } from "./onshapeRegion";
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
  describeXform,
  inPlaneXform,
  measureOnshapeSketchFrame,
  sketches3dByFeature,
  transformSketchFeature,
  type OnshapeSketch3d,
  type OnshapeSketches3d,
} from "./onshapeSketchFrame";
import { resolvePlaneFrame, type PlaneFrameKernel } from "../sketchPlaneFrame";
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
   *  included) — the only shape a revolve axis may name (#416). */
  lineEntityIds: Set<string>;
}

/** Flattening resolution of an arc/circle rim, in segments per full turn. */
const ARC_FLATTEN_SEGMENTS = 48;

/** Every entity id of the sketch whose geometry is a STRAIGHT LINE, including
 *  construction lines — those never reach the B-Rep but are exactly what a
 *  revolve axis is usually drawn as (#416). Lets the axis translation reject a
 *  query that names an arc or a point instead of silently emitting a
 *  `qSketchLine` the kernel then cannot resolve. */
export function sketchLineEntityIds(feat: AnyMsg): Set<string> {
  const out = new Set<string>();
  for (const raw of (feat.geometry ?? feat.entities ?? []) as AnyMsg[]) {
    const e = msg(raw);
    const bt = String(msg(e.geometry)?.btType ?? "");
    if (e.entityId && bt.startsWith("BTCurveGeometryLine")) out.add(e.entityId as string);
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
      // angles, running CCW — the same convention `translateOnshapeSketch` uses.
      const unbounded = !String(e.btType ?? "").startsWith("BTMSketchCurveSegment");
      const a0 = unbounded ? 0 : (e.startParam ?? 0);
      let sweep = unbounded ? Math.PI * 2 : (e.endParam ?? 0) - a0;
      while (sweep <= 0) sweep += Math.PI * 2;
      const steps = Math.max(2, Math.ceil((ARC_FLATTEN_SEGMENTS * sweep) / (Math.PI * 2)));
      const pts: [number, number][] = [];
      for (let i = 0; i <= steps; i++) {
        const a = a0 + (sweep * i) / steps;
        pts.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
      }
      out.set(id, pts);
    }
  }
  return out;
}

/** How many stations to probe along one named curve. The stations are spread
 *  by ARC LENGTH, so a long edge is sampled as thoroughly as a short one and
 *  the measured coverage is a length, not a vertex count. */
const EDGE_STATIONS = 32;
/** Absolute probe-offset clamp (mm) — a long edge still probes close enough to
 *  stay inside the thin plate it bounds (#412 plates are 22 mm thick). */
const EDGE_PROBE_MAX_MM = 4;

/** What `listSketchRegions` reports about one atomic imprint face. */
export interface FaceMetrics {
  /** Total boundary length (outer loop + hole loops) in mm. */
  perimeter: number;
  /** Face area in mm². */
  area: number;
}

/**
 * Resolve ONE Onshape region query to the FieldDraft imprint face it names, by
 * BOUNDARY COVERAGE (#412).
 *
 * A region query names a few of the curves that bound ITS region. Walking such
 * a curve and probing both sides tells us which faces it separates — but that
 * alone does not identify the region: the big ambient face around the plates
 * borders every one of those curves too, and so does any large rectangle the
 * curve happens to run along. What identifies the region is a two-sided fit:
 *
 *   score = covered length / face perimeter   ×   named curves touching / named
 *
 * The first factor is how much of the FACE the query accounts for — half the
 * plate's perimeter, a few percent of the surrounding sheet's. The second is
 * how much of the QUERY the face accounts for — the neighbouring block that a
 * long shared edge also runs along is bounded by that one curve and no other.
 * Both are fractions in [0, 1]; the highest product wins, smaller area breaks
 * a tie.
 *
 * Strictness that the ticket paid for: a named entity id that the imported
 * sketch does NOT carry makes the whole query unresolvable. Substituting a
 * same-prefix sibling (`R.left` for a missing `R.bottom`) is a guess, and in
 * the #412 document it guessed a 604×1500 block that Onshape never extruded —
 * silent wrong geometry. An honest `null` lets the caller warn and drop the
 * region instead.
 *
 * Returns `null` when an id is unknown, when every probe lands outside all
 * faces, or when no candidate face has a usable perimeter.
 */
export function regionFaceIndex(
  entityIds: string[],
  geom: Map<string, [number, number][]>,
  faceAt: (u: number, v: number) => number | null,
  metrics: (index: number) => FaceMetrics | null,
): number | null {
  if (entityIds.length === 0) return null;
  const polylines: [number, number][][] = [];
  for (const id of entityIds) {
    const pts = geom.get(id);
    if (!pts || pts.length < 2) return null; // names a curve we do not have
    polylines.push(pts);
  }

  /** face index → length of named curve running along its boundary. */
  const covered = new Map<number, number>();
  /** face index → which named entities touch it (indices into `polylines`). */
  const touched = new Map<number, Set<number>>();

  polylines.forEach((pts, entity) => {
    const lens: number[] = [];
    let total = 0;
    for (let i = 0; i + 1 < pts.length; i++) {
      const l = Math.hypot(pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]);
      lens.push(l);
      total += l;
    }
    if (total < 1e-6) return;
    const weight = total / EDGE_STATIONS;
    // Probe half a station spacing off the curve: far enough to be unambiguously
    // on one side, close enough not to jump over a thin neighbouring face.
    const off = Math.min(weight / 2, EDGE_PROBE_MAX_MM);
    for (let k = 0; k < EDGE_STATIONS; k++) {
      // Station at arc length s, offset by half a spacing so neither endpoint
      // (where the neighbouring faces change) is probed.
      let s = (total * (k + 0.5)) / EDGE_STATIONS;
      let i = 0;
      while (i < lens.length - 1 && s > lens[i]) {
        s -= lens[i];
        i += 1;
      }
      if (lens[i] < 1e-9) continue;
      const t = s / lens[i];
      const dx = pts[i + 1][0] - pts[i][0];
      const dy = pts[i + 1][1] - pts[i][1];
      const nx = -dy / lens[i];
      const ny = dx / lens[i];
      const px = pts[i][0] + dx * t;
      const py = pts[i][1] + dy * t;
      const a = faceAt(px + off * nx, py + off * ny);
      const b = faceAt(px - off * nx, py - off * ny);
      // Same face on both sides: here the curve runs THROUGH that face rather
      // than bounding it (a dangling trim line) — it says nothing about it.
      if (a === b) continue;
      for (const f of [a, b]) {
        if (f === null) continue;
        covered.set(f, (covered.get(f) ?? 0) + weight);
        let who = touched.get(f);
        if (!who) touched.set(f, (who = new Set()));
        who.add(entity);
      }
    }
  });

  let best: number | null = null;
  let bestScore = 0;
  let bestArea = Infinity;
  for (const [face, len] of covered) {
    const m = metrics(face);
    if (!m || !(m.perimeter > 0)) continue;
    const share = (touched.get(face)?.size ?? 0) / polylines.length;
    const score = (len / m.perimeter) * share;
    if (score > bestScore + 1e-9 || (score > bestScore - 1e-9 && m.area < bestArea)) {
      best = face;
      bestScore = score;
      bestArea = m.area;
    }
  }
  return best;
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
 *   reported, not hidden.
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
    out.depthMm = Math.abs(d);
    // The measured distance already carries the direction; the caller's
    // oppositeDirection flip must not double it.
    out.warnings.push(
      `${name}: ${bound} als gemessene Tiefe (${out.depthMm.toFixed(2)} mm) übernommen — ` +
        `geometrisch exakt, folgt der Zielfläche aber nicht mehr, wenn sie sich bewegt`,
    );
    if (d < 0) out.depthMm = -out.depthMm;
    if (params.get("oppositeDirection")?.value === true) out.depthMm = -out.depthMm;
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
): { query?: string; t?: number; error?: string; sketchLine?: boolean } {
  const a = mc.anchor;
  if (a.kind === "unsupported") {
    return { error: `${featureName}: Mate-Connector-Anker nicht lesbar (${a.reason})` };
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
  if (a.entityType !== "EDGE" || !rec.lineEntityIds.has(a.origin.entityId)) {
    return {
      error:
        `${featureName}: Mate-Connector-Anker ${describeAnchor(a)} ist keine gerade Sketch-Linie — ` +
        `FieldDraft adressiert rohe Sketch-Geometrie nur als qSketchLine`,
    };
  }
  const t = anchorT(mc);
  if (t === null) {
    return {
      error:
        `${featureName}: Mate-Connector-Inferenz "${mc.inference}" auf einer Sketch-Linie ` +
        `nicht übersetzbar — der Endpunkt ist aus secondaryOriginQuery nicht bestimmbar`,
    };
  }
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
): { params?: Record<string, any>; error?: string; warning?: string } {
  if (mc.originType !== "ON_ENTITY") {
    return {
      error: `${featureName}: Mate-Connector-Ursprung "${mc.originType}" wird nicht importiert (nur ON_ENTITY)`,
    };
  }
  const anchor = mateAnchorQuery(mc, featureName, idMap, sketches, sweeps);
  if (anchor.error || !anchor.query) {
    return { error: anchor.error ?? `${featureName}: Mate-Connector-Anker nicht übersetzbar` };
  }
  if (anchor.sketchLine) {
    // The query is CORRECT and names exactly the line Onshape names — the
    // kernel just cannot anchor a connector to raw sketch geometry yet, so
    // emitting it would only add a feature that fails to regenerate.
    return {
      error:
        `${featureName}: Mate-Connector sitzt auf der rohen Sketch-Linie ${describeAnchor(mc.anchor)} ` +
        `(${anchor.query}) — FieldDraft verankert Mate-Connectors nur an B-Rep-Geometrie ` +
        `(Kernel: resolve_mate_anchor kennt qFace/qEdge/qVertex/qSketchPoint, aber kein qSketchLine)`,
    };
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
 * `profiles` MultiQuery (ticket #412).
 *
 * Onshape supplies ONE query per selected sketch region; each decodes to the
 * sketch-entity ids bounding that region. [`regionFaceIndex`] maps each to the
 * atomic imprint face it names, and the DISTINCT faces join into one MultiQuery
 * — so a multi-region selection extrudes only those regions, not the whole
 * sketch.
 *
 * Queries that DON'T resolve are dropped, not guessed: the remaining ones still
 * produce their (correct) regions and a visible warning reports the shortfall.
 * Extruding three right plates and missing a fourth beats extruding the whole
 * sheet, and both beat inventing a region nobody selected.
 *
 * Falls back to the whole sketch (`qCreatedBy(sketch)`) — WITH a warning where
 * a selection was actually made — when the kernel exposes no region API (a test
 * double), when no query carries region entities at all, or when not a single
 * one of them resolves.
 */
async function resolveProfiles(
  profileQss: string[],
  rec: SketchRecord,
  kernel: ImportKernel,
  name: string,
  result: OnshapeImportResult,
): Promise<string> {
  const whole = `qCreatedBy(${rec.fdId})`;
  if (
    typeof kernel.regionQueryAtPoint !== "function" ||
    typeof kernel.listSketchRegions !== "function"
  ) {
    return whole;
  }

  // Only the SKETCH_ENTITY region descriptors carry a specific selection; a
  // plain whole-sketch reference decodes to no region entities.
  const decoded = await Promise.all(profileQss.map((qs) => decodeRegionQuery(qs)));
  const regionQueries = decoded.filter(
    (d): d is NonNullable<typeof d> => !!d && d.isRegionQuery && d.entityIds.length > 0,
  );
  if (regionQueries.length === 0) return whole;

  // Each resolved query is a `qRegion(sketch, i)`; collect the DISTINCT atomic
  // face indices the imprint arrangement (#412) assigns.
  const faceAt = (u: number, v: number): number | null => {
    const q = kernel.regionQueryAtPoint!(rec.fdId, u, v);
    const m = q.match(/^qRegion\(\s*\d+\s*,\s*(\d+)\s*\)$/);
    return m ? Number(m[1]) : null;
  };
  // Perimeter + area rank the candidate faces (see `regionFaceIndex`).
  const faceMetrics = new Map<number, FaceMetrics>();
  try {
    const rows = JSON.parse(kernel.listSketchRegions(rec.fdId, 128)) as {
      index: number;
      area: number;
      perimeter: number;
    }[];
    for (const r of rows) faceMetrics.set(r.index, { area: r.area, perimeter: r.perimeter });
  } catch {
    faceMetrics.clear();
  }
  const metrics = (i: number) => faceMetrics.get(i) ?? null;

  const indices = new Set<number>();
  let unresolved = 0;
  for (const d of regionQueries) {
    const face = regionFaceIndex(d.entityIds, rec.entityGeom, faceAt, metrics);
    if (face !== null) indices.add(face);
    else unresolved += 1;
  }

  if (indices.size === 0) {
    result.warnings.push(
      `${name}: Region-Auswahl (${regionQueries.length} Onshape-Region(en)) nicht auflösbar — ` +
        `ganzer Sketch extrudiert`,
    );
    return whole;
  }
  if (indices.size < regionQueries.length) {
    // Two ways to lose a region, and the warning names both by count. Either a
    // query mentions a sketch curve this document does not carry (Onshape's own
    // selection can be stale — a trimmed-away rectangle it no longer extrudes
    // either), or two queries landed on the SAME atomic face. Both mean the
    // extrude covers something other than Onshape's, so say so, don't guess.
    const collapsed = regionQueries.length - unresolved - indices.size;
    const reasons: string[] = [];
    if (unresolved > 0) {
      reasons.push(`${unresolved} nennen Sketch-Kurven, die dieser Sketch nicht enthält`);
    }
    if (collapsed > 0) {
      reasons.push(`${collapsed} fielen mit einer anderen Auswahl auf dieselbe atomare Face`);
    }
    result.warnings.push(
      `${name}: Onshape wählte ${regionQueries.length} Regionen, FieldDraft findet dafür nur ` +
        `${indices.size} — ${reasons.join("; ")} ` +
        `(siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md)`,
    );
  }
  // Selecting the ONE face an undivided sketch has IS the whole sketch — say so.
  // The two profiles bound the same area, but only the whole-sketch one carries
  // the sketch's entity provenance; a region-restricted profile falls back to
  // positional wall numbering (docs/SKETCH-REGION-IMPRINT-PLAN.md §5), which
  // silently breaks the `qWall(...)` plane queries of every LATER feature.
  if (faceMetrics.size === 1 && indices.size === 1) return whole;

  const sorted = [...indices].sort((a, b) => a - b);
  return sorted.length === 1
    ? `qRegion(${rec.fdId}, ${sorted[0]})`
    : `qRegions(${rec.fdId}, [${sorted.join(", ")}])`;
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
  const env = newExprEnv(result);
  const frames3d = sketches3dByFeature(sources.sketches3d);

  // Batch mode (#410): defer the display mesh for the whole walk and rebuild it
  // ONCE at the end. Every feature still regenerates (so its plane/edge queries
  // resolve against the mid-history B-Rep), but re-meshing all bodies after each
  // of N features is O(n²) main-thread work that froze the import for minutes.
  kernel.setSyncMeshing?.(false);
  try {
    await importFeatures(featuresJson, kernel, result, sketches, sweeps, env, frames3d);
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
 * document ever had.
 */
async function emitMateConnector(
  feature: AnyMsg,
  featureName: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): Promise<{ fdId: number | null; mc: OnshapeMateConnector }> {
  const mc = await readMateConnector(feature);
  const built = mateConnectorParams(mc, featureName, result.idMap, sketches, sweeps);
  if (built.warning) result.warnings.push(built.warning);
  if (built.error || !built.params) {
    result.errors.push(built.error ?? `${featureName}: Mate-Connector nicht übersetzbar`);
    return { fdId: null, mc };
  }
  let matches = 0;
  try {
    matches = kernel.resolveQuery(built.params.anchor as string).length;
  } catch {
    matches = 0;
  }
  if (matches !== 1) {
    result.errors.push(
      `${featureName}: Mate-Connector-Anker "${built.params.anchor}" löst auf ${matches} Elemente auf (erwartet 1) — übersprungen`,
    );
    return { fdId: null, mc };
  }
  const fdId = kernel.addFeatureCall("mate_connector", JSON.stringify(built.params));
  if (mc.featureId) result.idMap[mc.featureId] = fdId;
  result.created.push(fdId);
  return { fdId, mc };
}

/**
 * A sketch placed on a MATE CONNECTOR (#416). The connector rides inside the
 * sketch's own `subFeatures`, so it is imported here — it is reference geometry
 * in its own right, and the anchor a user can re-attach the sketch to by hand.
 *
 * The SKETCH itself is skipped: FieldDraft's `resolve_plane_frame` takes a
 * plane query to exactly two things, one of the three origin datum planes or a
 * resolved B-Rep face, and a mate connector is neither. That gap is named in
 * full rather than papered over — a sketch dropped onto a guessed plane takes
 * every feature drawn on it to the wrong place.
 */
async function importMateConnectorPlane(
  mcFeatureId: string,
  sketchFeature: AnyMsg,
  featureName: string,
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): Promise<void> {
  const sub = implicitMateConnectors(sketchFeature).get(mcFeatureId);
  if (!sub) {
    result.errors.push(
      `${featureName}: Ebene nennt den Mate-Connector ${mcFeatureId}, der aber weder als Sub-Feature noch im Dokument steht`,
    );
    return;
  }
  const { fdId, mc } = await emitMateConnector(
    sub,
    `${featureName} · ${String(sub.name ?? "Mate connector")}`,
    kernel,
    result,
    sketches,
    sweeps,
  );
  result.errors.push(
    `${featureName}: Sketch liegt auf dem Mate-Connector „${mc.name}“ (${describeAnchor(mc.anchor)}) — ` +
      `FieldDraft kann eine Sketch-Ebene noch nicht auf einem Mate-Connector aufsetzen ` +
      `(Kernel: resolve_plane_frame kennt nur Datum-Ebenen und B-Rep-Flächen, ` +
      `siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md)` +
      (fdId === null ? "" : ` — der Mate-Connector selbst ist als Feature ${fdId} importiert`),
  );
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
 */
function alignSketchFrame(
  feat: AnyMsg,
  name: string,
  planeQuery: string,
  kernel: ImportKernel,
  three: OnshapeSketch3d | undefined,
  result: OnshapeImportResult,
): AnyMsg {
  // Ein Kernel-Double ohne Geometrie-Leser kann keinen Frame liefern; das ist
  // kein Befund, sondern eine Testumgebung.
  if (typeof kernel.bodyCount !== "function") return feat;
  const fd = frameOf(kernel, planeQuery);
  if (!fd) {
    result.warnings.push(
      `${name}: FieldDrafts Frame für die Ebene "${planeQuery}" ist nicht ` +
        `auflösbar — Onshapes 2D-Koordinaten wörtlich übernommen`,
    );
    return feat;
  }
  const os = measureOnshapeSketchFrame(feat, three);
  if (!os) {
    result.warnings.push(
      three
        ? `${name}: Onshapes Sketch-Frame ist aus seiner 3D-Antwort nicht bestimmbar ` +
          `(zu wenige oder kollineare Stützpunkte) — 2D-Koordinaten wörtlich übernommen; ` +
          `liegt der Sketch auf einer Körperfläche, kann er verdreht sein`
        : `${name}: fehlt in Onshapes 3D-Sketch-Antwort — Frame ungeprüft, ` +
          `2D-Koordinaten wörtlich übernommen`,
    );
    return feat;
  }

  const x = inPlaneXform(os, fd);
  // Erst die Ebene, dann die Basis: weicht die NORMALE ab oder liegt Onshapes
  // Ursprung neben der Ebene, hat die FieldDraft-Query eine andere Fläche
  // erwischt. Das umzurechnen hieße, einen Fehlgriff zu kaschieren.
  if (Math.abs(Math.abs(x.normalDot) - 1) > 1e-6 || Math.abs(x.outOfPlaneMm) > 1e-3) {
    result.errors.push(
      `${name}: Onshapes Sketch-Ebene und die aufgelöste FieldDraft-Ebene ` +
        `"${planeQuery}" sind nicht dieselbe Ebene ` +
        `(Normalen-Skalarprodukt ${x.normalDot.toFixed(6)}, Abstand ` +
        `${x.outOfPlaneMm.toFixed(4)} mm) — Koordinaten NICHT umgerechnet`,
    );
    return feat;
  }
  if (x.identity) return feat;

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
  return t.feature;
}

/** The feature walk itself — see {@link importOnshapeFeatures}, which wraps this
 *  in the batch meshing deferral. */
async function importFeatures(
  featuresJson: { features?: any[] },
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
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
      // sketch's own `subFeatures` (#416) — import it, and report the plane.
      const mcId = mateConnectorPlaneId(planeQs);
      if (mcId) {
        await importMateConnectorPlane(mcId, feat, name, kernel, result, sketches, sweeps);
        continue;
      }
      const ref = await decodePlaneQuery(planeQs);
      const built = buildPlaneQuery(ref, name, result.idMap, sketches, sweeps);
      if (built.error || !built.query) {
        result.errors.push(built.error ?? `${name}: Ebene nicht übersetzbar`);
        continue;
      }
      // Uniqueness gate for face-level planes: the stored query must
      // bind exactly ONE face right now (mid-history resolution).
      if (!built.query.startsWith("qCreatedBy(origin_plane_")) {
        const matches = kernel.resolveQuery(built.query);
        if (matches.length !== 1) {
          result.errors.push(
            `${name}: Ebenen-Query "${built.query}" löst auf ${matches.length} Flächen auf (erwartet 1) — Sketch übersprungen.` +
              (built.query.includes("qWall(") && matches.length === 0
                ? " Ursache ist meist eine tangential verschmolzene Mantelfläche: FieldDraft trennt Flächen nur an Knicken, ein abgerundetes Rechteck hat daher EINE Mantelfläche statt vier Seiten + vier Rundungen (siehe docs/ONSHAPE-IMPORT-STATE.md)."
                : ""),
          );
          continue;
        }
      }
      // Onshapes 2D-Koordinaten in FieldDrafts Sketch-Basis drehen (#415).
      // Umgeschrieben wird der PAYLOAD, damit Übersetzung, Region-Innenpunkte
      // und jeder spätere 2D-Leser dieselben Zahlen sehen. Ohne die 3D-Quelle
      // (`sources.sketches3d`) bleibt alles wie bisher — dass sie fehlt, meldet
      // die Stelle, die sie holen sollte, nicht jeder einzelne Sketch.
      const aligned =
        frames3d.size === 0
          ? feat
          : alignSketchFrame(
              feat,
              name,
              built.query,
              kernel,
              frames3d.get(feat.featureId),
              result,
            );
      const translation = translateOnshapeSketch(aligned);
      for (const [k, v] of Object.entries(translation.diagnostics.unhandled)) {
        result.warnings.push(`${name}: ${v}× nicht übersetzt: ${k}`);
      }
      const fdId = kernel.addSketch(built.query, translation.subCalls);
      sketches.set(feat.featureId, {
        fdId,
        entityIds: translation.entityIds,
        planeQuery: built.query,
        entityGeom: sketchEntityVertices(aligned),
        lineEntityIds: sketchLineEntityIds(aligned),
      });
      result.idMap[feat.featureId] = fdId;
      result.created.push(fdId);
      continue;
    }

    // A mate connector STANDING IN THE TREE — the same feature the implicit
    // ones are, just not hidden inside a sketch. Reference geometry: it builds
    // no body, it gives later features (and the user) a frame to hang on.
    if (feat.featureType === "mateConnector") {
      await emitMateConnector(feat, name, kernel, result, sketches, sweeps);
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
      const profiles = await resolveProfiles(profileQss, rec, kernel, name, result);
      // End condition: BLIND / THROUGH_ALL / UP_TO_* (see `endCondition`).
      const upToQs = queryStringsOf(params.get("endBoundEntityFace"))[0];
      let upToQuery: string | undefined;
      if (upToQs) {
        const upToRef = await decodePlaneQuery(upToQs);
        const built = buildPlaneQuery(upToRef, name, result.idMap, sketches, sweeps);
        if (built.query) upToQuery = built.query;
      }
      const end = endCondition(params, kernel, rec.planeQuery, name, upToQuery, env);
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
          profiles,
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
      await importRevolve(feat, params, name, kernel, result, sketches, env);
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
  const profiles = await resolveProfiles(profileQss, rec, kernel, name, result);

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
  if (!axisRec.lineEntityIds.has(axisRef.entityId)) {
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
      profiles,
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
 * An Onshape edge query → a FieldDraft `qCommonEdge(faceA, faceB)`. Shared by
 * the `fillet` and `chamfer` translations (#414): both select EDGES, and an
 * edge is named by the two faces that meet at it, so the translation is driven
 * by the markers the query carries:
 *  - `CAP_FACE` ∩ `SWEPT_FACE` — a cap rim, named by its two FACES.
 *  - `CAP_EDGE` — the SAME cap rim, but named as an edge in its own right
 *    (which is what Onshape emits when the user clicks the edge instead of
 *    rolling it up from faces). The payload still carries the extrude, the
 *    `isStart` cap flag and the sketch entity whose wall the cap meets, so it
 *    lowers to the identical FieldDraft form.
 *  - `OFFSET_EDGE` / `OFFSET_FACE` — the rim of a SHELLED body: the cap meets
 *    the shell's inner wall (`OFFSET_*` is Onshape's name for the shell's
 *    offset surface). The inner wall keeps its outer face's feature id, so
 *    the FieldDraft form is `qCommonEdge(cap, INNER_WALL)`.
 *
 * Known limit: Onshape compresses repeated strings into back-references
 * whose index scheme we do not resolve yet, so two SIBLING edges of one
 * fillet (e.g. the two long rim edges) decode to the SAME markers and
 * collapse into one query. The caller de-duplicates and reports the
 * collapse rather than filleting the same edge twice.
 */
async function buildEdgeQuery(
  qs: string,
  name: string,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): Promise<string | null> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(qs);
    if (!bton) return null;
    const tokens = tokenizeBton(payload);
    const ref = extractPlaneRef(tokens);
    const markers = new Set(
      tokens.filter((t) => t.t === "str" && t.p1 && !t.p2).map((t) => t.p1 as string),
    );
    // The cap side, from the isStart flag. Onshape's START cap is the
    // SKETCH-PLANE side; which FieldDraft cap that is depends on the extrude's
    // direction, because a FieldDraft slab is centred on `depth/2` — a NEGATIVE
    // depth puts CAP_TOP, not CAP_BOTTOM, flush with the sketch plane (the same
    // bookkeeping `buildPlaneQuery` does for cap PLANES).
    const isStartIdx = tokens.findIndex((t) => t.t === "str" && t.p1 === "isStart");
    const boolTok =
      isStartIdx >= 0 ? tokens.slice(isStartIdx + 1).find((t) => t.t === "bool") : undefined;
    const capRoleOf = (opFeatureId: string): string => {
      const positive = sweeps.get(opFeatureId)?.depthPositive ?? true;
      const sketchSide = positive ? "CAP_BOTTOM" : "CAP_TOP";
      const farSide = positive ? "CAP_TOP" : "CAP_BOTTOM";
      return boolTok?.v === true ? sketchSide : farSide;
    };

    // Inside a SHELLED body: the hollow's floor/ceiling meets its side — the
    // inner cap against the inner wall. Both faces belong to the SWEEP (an
    // inner face keeps its outer face's feature id); the shell op only
    // hollowed it, so take the sweep op out of the stream rather than the
    // query's outermost op.
    if (markers.has("OFFSET_EDGE") || markers.has("OFFSET_FACE")) {
      const sweep = extractOpRefs(tokens).find((o) => /^op(Extrude|Revolve|Sweep)$/.test(o.op));
      const fd = sweep ? result.idMap[sweep.featureId] : undefined;
      if (fd === undefined) {
        result.errors.push(`${name}: Shell-Kante — der geschalte Sweep ist nicht importiert`);
        return null;
      }
      const innerCap =
        capRoleOf(sweep!.featureId) === "CAP_TOP" ? "INNER_CAP_TOP" : "INNER_CAP_BOTTOM";
      return `qCommonEdge(qCreatedBy(${fd}, ${innerCap}), qCreatedBy(${fd}, INNER_WALL))`;
    }

    // Cap rim: cap ∩ the wall swept from a named sketch entity.
    if (ref.kind === "swept" && markers.has("CAP_FACE")) {
      const fd = result.idMap[ref.opFeatureId];
      if (fd === undefined) {
        result.errors.push(`${name}: Kanten-Query referenziert nicht importiertes Feature`);
        return null;
      }
      const wall = wallQuery(ref.origin, fd, name, sketches);
      if (!wall.query) {
        result.errors.push(wall.error ?? `${name}: Kante nicht übersetzbar`);
        return null;
      }
      return `qCommonEdge(qCreatedBy(${fd}, ${capRoleOf(ref.opFeatureId)}), ${wall.query})`;
    }

    // The same cap rim, named directly as an EDGE. `SWEPT_FACE` alongside means
    // the query mixes in face-level dependencies we cannot order reliably yet,
    // so restrict this to the pure edge form rather than guess which origin is
    // the rim's.
    if (markers.has("CAP_EDGE") && !markers.has("SWEPT_FACE")) {
      const refs = extractQueryRefs(tokens);
      const extrude = refs.ops.find((o) => o.op.op === "opExtrude");
      const origin = refs.origins[0]?.origin;
      const fd = extrude ? result.idMap[extrude.op.featureId] : undefined;
      if (fd === undefined || !origin) {
        result.errors.push(
          `${name}: CAP_EDGE-Kante — ${extrude ? "Herkunfts-Sketch-Entity" : "Extrude"} nicht auflösbar`,
        );
        return null;
      }
      const wall = wallQuery(origin, fd, name, sketches);
      if (!wall.query) {
        result.errors.push(wall.error ?? `${name}: Kante nicht übersetzbar`);
        return null;
      }
      return `qCommonEdge(qCreatedBy(${fd}, ${capRoleOf(extrude!.op.featureId)}), ${wall.query})`;
    }

    result.errors.push(
      `${name}: Kanten-Form nicht übersetzbar (Marker: ${[...markers]
        .filter((m) => /FACE|EDGE/.test(m))
        .join(", ") || "keine"})`,
    );
    return null;
  } catch (e) {
    result.errors.push(`${name}: Kante nicht dekodierbar (${e instanceof Error ? e.message : e})`);
    return null;
  }
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
