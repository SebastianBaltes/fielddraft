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

/** Import a fetched Onshape feature list into the kernel. */
export async function importOnshapeFeatures(
  featuresJson: { features?: any[] },
  kernel: ImportKernel,
): Promise<OnshapeImportResult> {
  const result: OnshapeImportResult = { created: [], idMap: {}, warnings: [], errors: [] };
  const sketches = new Map<string, SketchRecord>();
  const sweeps = new Map<string, SweepRecord>();

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
      });
      result.idMap[feat.featureId] = fdId;
      result.created.push(fdId);
      continue;
    }

    if (feat.featureType === "extrude") {
      const profileQs = queryStringsOf(params.get("entities"))[0];
      const sketchFid = profileQs ? await profileSketchId(profileQs) : null;
      const rec = sketchFid ? sketches.get(sketchFid) : undefined;
      if (!rec) {
        result.errors.push(`${name}: Profil-Sketch nicht auflösbar — übersprungen`);
        continue;
      }
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
          profiles: `qCreatedBy(${rec.fdId})`,
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

  return result;
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
