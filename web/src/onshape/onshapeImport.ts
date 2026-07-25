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
  tokenizeBton,
  type OnshapePlaneRef,
} from "./onshapeQuery";
import { translateOnshapeSketch, type SketchSubCall } from "./onshapeSketch";
import { decodeRegionQuery } from "./onshapeRegion";
import { resolvePlaneFrame, type PlaneFrameKernel } from "../sketchPlaneFrame";

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

/** Quantity parameter → mm. Onshape carries the authored expression
 *  (`"25 cm"`, `"2.5 mm"`); `value` is the SI evaluation but often 0 in
 *  the features payload, so the expression wins. */
const UNIT_TO_MM: Record<string, number> = {
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

function mmOf(pm: AnyMsg | undefined, fallback = 0): number {
  const expr = typeof pm?.expression === "string" ? pm.expression : "";
  const m = expr.match(/^\s*(-?[\d.]+)\s*\*?\s*([a-zA-Z]+)\s*$/);
  if (m && UNIT_TO_MM[m[2]] !== undefined) {
    return parseFloat(m[1]) * UNIT_TO_MM[m[2]];
  }
  if (typeof pm?.value === "number" && pm.value !== 0) return pm.value * M_TO_MM;
  return fallback;
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
}

/** Flattening resolution of an arc/circle rim, in segments per full turn. */
const ARC_FLATTEN_SEGMENTS = 48;

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
): EndCondition {
  const out: EndCondition = {
    depthMm: mmOf(params.get("depth"), 25),
    symmetric: params.get("symmetric")?.value === true,
    depthBackMm: 0,
    warnings: [],
  };
  const bound = params.get("endBound")?.value ?? "BLIND";

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
    applySecondDirection(params, out, name);
    if (params.get("hasDraft")?.value === true) {
      out.warnings.push(`${name}: Draft-Winkel wird nicht unterstützt und ignoriert`);
    }
    return out;
  }

  if (params.get("oppositeDirection")?.value === true) out.depthMm = -out.depthMm;
  applySecondDirection(params, out, name);
  if (params.get("hasDraft")?.value === true) {
    out.warnings.push(`${name}: Draft-Winkel wird nicht unterstützt und ignoriert`);
  }
  return out;
}

function applySecondDirection(
  params: Map<string, AnyMsg>,
  out: EndCondition,
  name: string,
): void {
  if (params.get("hasSecondDirection")?.value !== true) return;
  const bound = params.get("secondDirectionBound")?.value ?? "BLIND";
  if (bound !== "BLIND") {
    out.warnings.push(
      `${name}: zweite Richtung mit ${bound} → als BLIND übernommen`,
    );
  }
  out.depthBackMm = Math.abs(mmOf(params.get("secondDirectionDepth"), 0));
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
    case "cap": {
      const sweep = sweeps.get(ref.opFeatureId);
      const fd = idMap[ref.opFeatureId];
      if (fd === undefined)
        return { error: `${featureName}: Cap-Ebene referenziert nicht importiertes Feature ${ref.opFeatureId}` };
      // Onshape's START cap is the SKETCH-PLANE side. Which FieldDraft cap
      // that is depends on the extrude's direction: growing along +z the
      // sketch plane is CAP_BOTTOM, growing the other way it is CAP_TOP.
      const positive = sweep?.depthPositive ?? true;
      const sketchSide = positive ? "CAP_BOTTOM" : "CAP_TOP";
      const farSide = positive ? "CAP_TOP" : "CAP_BOTTOM";
      return { query: `qCreatedBy(${fd}, ${ref.isStart ? sketchSide : farSide})` };
    }
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
 * Resolve an extrude's profile selection to a FieldDraft `profiles`
 * MultiQuery (ticket #412).
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
async function resolveExtrudeProfiles(
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

/** Import a fetched Onshape feature list into the kernel. */
export async function importOnshapeFeatures(
  featuresJson: { features?: any[] },
  kernel: ImportKernel,
): Promise<OnshapeImportResult> {
  const result: OnshapeImportResult = { created: [], idMap: {}, warnings: [], errors: [] };
  const sketches = new Map<string, SketchRecord>();
  const sweeps = new Map<string, SweepRecord>();

  // Batch mode (#410): defer the display mesh for the whole walk and rebuild it
  // ONCE at the end. Every feature still regenerates (so its plane/edge queries
  // resolve against the mid-history B-Rep), but re-meshing all bodies after each
  // of N features is O(n²) main-thread work that froze the import for minutes.
  kernel.setSyncMeshing?.(false);
  try {
    await importFeatures(featuresJson, kernel, result, sketches, sweeps);
  } finally {
    kernel.setSyncMeshing?.(true); // one final mesh of the finished document
  }
  return result;
}

/** The feature walk itself — see {@link importOnshapeFeatures}, which wraps this
 *  in the batch meshing deferral. */
async function importFeatures(
  featuresJson: { features?: any[] },
  kernel: ImportKernel,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
  sweeps: Map<string, SweepRecord>,
): Promise<void> {
  for (const raw of featuresJson.features ?? []) {
    const feat = msg(raw);
    const name: string = feat.name ?? feat.featureId ?? "?";
    if (feat.suppressed) {
      result.warnings.push(`${name}: unterdrückt — übersprungen`);
      continue;
    }
    const params = paramsOf(feat);

    if (feat.featureType === "newSketch") {
      const planeQs = queryStringsOf(params.get("sketchPlane"))[0];
      if (!planeQs) {
        result.errors.push(`${name}: keine Ebenen-Query`);
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
      const translation = translateOnshapeSketch(feat);
      for (const [k, v] of Object.entries(translation.diagnostics.unhandled)) {
        result.warnings.push(`${name}: ${v}× nicht übersetzt: ${k}`);
      }
      const fdId = kernel.addSketch(built.query, translation.subCalls);
      sketches.set(feat.featureId, {
        fdId,
        entityIds: translation.entityIds,
        planeQuery: built.query,
        entityGeom: sketchEntityVertices(feat),
      });
      result.idMap[feat.featureId] = fdId;
      result.created.push(fdId);
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
      const profiles = await resolveExtrudeProfiles(profileQss, rec, kernel, name, result);
      // End condition: BLIND / THROUGH_ALL / UP_TO_* (see `endCondition`).
      const upToQs = queryStringsOf(params.get("endBoundEntityFace"))[0];
      let upToQuery: string | undefined;
      if (upToQs) {
        const upToRef = await decodePlaneQuery(upToQs);
        const built = buildPlaneQuery(upToRef, name, result.idMap, sketches, sweeps);
        if (built.query) upToQuery = built.query;
      }
      const end = endCondition(params, kernel, rec.planeQuery, name, upToQuery);
      result.warnings.push(...end.warnings);
      if (end.error) {
        result.errors.push(end.error);
        continue;
      }
      const depthMm = end.depthMm;
      const opType = params.get("operationType")?.value ?? "NEW";
      const operation =
        opType === "NEW" ? "new" : opType === "REMOVE" ? "remove" : opType === "INTERSECT" ? "intersect" : "add";
      // Boolean scope → explicit targets (Feature-Scope-Direktive).
      let targets = "";
      const scopeQs = queryStringsOf(params.get("booleanScope"))[0];
      if (scopeQs && operation !== "new") {
        const scopeRef = await decodeAnyOpRef(scopeQs);
        if (scopeRef && result.idMap[scopeRef] !== undefined) {
          targets = `qCreatedBy(${result.idMap[scopeRef]})`;
        }
      }
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
      const opType = params.get("operationType")?.value ?? "NEW";
      const operation =
        opType === "NEW" ? "new" : opType === "REMOVE" ? "remove" : opType === "INTERSECT" ? "intersect" : "add";
      // Boolean scope → explicit targets (Feature-Scope-Direktive), same as extrude.
      let targets = "";
      const scopeQs = queryStringsOf(params.get("booleanScope"))[0];
      if (scopeQs && operation !== "new") {
        const scopeRef = await decodeAnyOpRef(scopeQs);
        if (scopeRef && result.idMap[scopeRef] !== undefined) {
          targets = `qCreatedBy(${result.idMap[scopeRef]})`;
        }
      }
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
          thickness: { value: mmOf(params.get("thickness"), 2), unit: "mm" },
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
      const radiusMm = mmOf(params.get("radius"), 2);
      const edgeQs = queryStringsOf(params.get("entities"));

      // Sibling edges can decode to the same query (the un-resolved BTON
      // back-references, see `buildFilletEdgeQuery`) — filleting the same
      // edge twice would stack two blends on it, so de-duplicate and say so.
      const queries: string[] = [];
      for (const qs of edgeQs) {
        const edgeQuery = await buildFilletEdgeQuery(qs, name, result, sketches);
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

    result.errors.push(`${name}: Feature-Typ "${feat.featureType}" wird noch nicht importiert`);
  }
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
 * An Onshape fillet edge query → a FieldDraft `qCommonEdge(faceA, faceB)`.
 *
 * An edge is named by the two faces that meet at it, so the translation is
 * driven by the face markers the query carries:
 *  - `CAP_FACE` ∩ `SWEPT_FACE` — a cap rim (the box's top/bottom edge).
 *  - `CAP_EDGE` / `OFFSET_EDGE` — the rim of a SHELLED body: the cap meets
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
async function buildFilletEdgeQuery(
  qs: string,
  name: string,
  result: OnshapeImportResult,
  sketches: Map<string, SketchRecord>,
): Promise<string | null> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(qs);
    if (!bton) return null;
    const tokens = tokenizeBton(payload);
    const ref = extractPlaneRef(tokens);
    const markers = new Set(
      tokens.filter((t) => t.t === "str" && t.p1 && !t.p2).map((t) => t.p1 as string),
    );
    // The cap side, from the isStart flag (Onshape's START cap = the sketch
    // plane = FieldDraft CAP_BOTTOM).
    const isStartIdx = tokens.findIndex((t) => t.t === "str" && t.p1 === "isStart");
    const boolTok =
      isStartIdx >= 0 ? tokens.slice(isStartIdx + 1).find((t) => t.t === "bool") : undefined;
    const capRole = boolTok?.v === true ? "CAP_BOTTOM" : "CAP_TOP";

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
      const innerCap = capRole === "CAP_TOP" ? "INNER_CAP_TOP" : "INNER_CAP_BOTTOM";
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
        result.errors.push(wall.error ?? `${name}: Fillet-Kante nicht übersetzbar`);
        return null;
      }
      return `qCommonEdge(qCreatedBy(${fd}, ${capRole}), ${wall.query})`;
    }

    result.errors.push(
      `${name}: Fillet-Kanten-Form nicht übersetzbar (Marker: ${[...markers]
        .filter((m) => /FACE|EDGE/.test(m))
        .join(", ") || "keine"})`,
    );
    return null;
  } catch (e) {
    result.errors.push(`${name}: Fillet-Kante nicht dekodierbar (${e instanceof Error ? e.message : e})`);
    return null;
  }
}
