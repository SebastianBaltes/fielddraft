/**
 * Onshape MATE CONNECTORS — the implicit ones a sketch brings along as its own
 * sub-feature (#416), and the explicit ones that stand in the feature list.
 *
 * Onshape lets a sketch be placed on a mate connector instead of on a plane or
 * a face. The connector is then NOT a feature of its own in the tree: it rides
 * along inside the sketch feature's `subFeatures`, and the sketch's
 * `sketchPlane` parameter names it with an uncompressed FeatureScript query
 *
 *     query = qBodyType(qCreatedBy(id + "<featureId>", EntityType.BODY),
 *                       BodyType.MATE_CONNECTOR);
 *
 * That is why the gap matrix's feature-type census — which counts TOP-LEVEL
 * features — never saw them: three of the five sketches of #416 are placed this
 * way, and everything drawn on them (15 of 19 features) falls with them.
 *
 * A connector is a COORDINATE SYSTEM, and Onshape derives it from geometry:
 *
 *   * `originType: ON_ENTITY` + `originQuery` — the entity the connector sits
 *     on, named by the same creation-query grammar as any other reference.
 *   * `entityInferenceType` — WHICH point of that entity: `MID_POINT`,
 *     `CENTER` (of a circular edge), or `POINT` (an endpoint, named by
 *     `secondaryOriginQuery`).
 *   * `translationX/Y/Z` + `rotationType`/`rotation` + `flipPrimary` — the
 *     realignment applied afterwards, along the connector's OWN axes.
 *
 * FieldDraft's `mate_connector` feature is built on the same rules (+Z = an
 * edge's tangent or a face's normal, `t` along an edge, `flip`, `reorient` in
 * 90° steps, `offset` along the connector's own axes), so the translation is a
 * near-identity — see {@link onshapeImport}'s `mateConnectorCall`.
 *
 * This module does the DECODING only: feature payload → a structured
 * description, with every form it cannot read named rather than guessed.
 */

import {
  decodeOnshapeQueryString,
  extractQueryRefs,
  tokenizeBton,
  type BtonToken,
  type OnshapeEntityOrigin,
} from "./onshapeQuery";

/** Which point of the anchor entity the connector sits on. Onshape's
 *  `EntityInferenceType`, restricted to the values that name a POINT. */
export type MateInference = "POINT" | "MID_POINT" | "CENTER" | "TOP" | "BOTTOM";

/** What an anchor query names, decoded. `capEntity` is the shape that matters
 *  in practice: an entity of an EXTRUDE'S CAP, derived from the sketch entity
 *  that generated it — the cap face itself, or the cap edge that bounds it. */
export type OnshapeMateAnchor =
  | {
      kind: "sketchEntity";
      entityType: "EDGE" | "VERTEX" | "FACE";
      origin: OnshapeEntityOrigin;
    }
  | {
      kind: "capEntity";
      entityType: "EDGE" | "VERTEX" | "FACE";
      opFeatureId: string;
      /** Onshape's START cap is the sketch-plane side of the sweep. */
      isStart: boolean;
      origin: OnshapeEntityOrigin;
    }
  /** The part studio's ORIGIN point — Onshape's `Origin`/`pointOp` creation
   *  query. No geometry under it at all: this is what the FIRST connector of a
   *  document anchors on, since nothing has been built yet. Its CSYS is the
   *  world one, which the realignment then offsets from. */
  | { kind: "originPoint" }
  | { kind: "unsupported"; reason: string };

/** A decoded mate connector — everything the FieldDraft translation needs. */
export interface OnshapeMateConnector {
  /** The connector's own Onshape featureId (an implicit one still has one —
   *  it is what the sketch's plane query points at). */
  featureId: string;
  name: string;
  /** `ON_ENTITY`, `ON_PART_STUDIO`, … */
  originType: string;
  inference: MateInference | string;
  anchor: OnshapeMateAnchor;
  /** `secondaryOriginQuery` — with `POINT` inference this names WHICH endpoint
   *  of the anchor edge, so it decides the position along it. */
  secondary: OnshapeMateAnchor | null;
  /** `translationX/Y/Z` in MILLIMETRES, along the connector's own axes. */
  offsetMm: [number, number, number];
  /** `ABOUT_X` | `ABOUT_Y` | `ABOUT_Z`. */
  rotationType: string;
  /** `rotation` in DEGREES. */
  rotationDeg: number;
  flipPrimary: boolean;
  realign: boolean;
}

const M_TO_MM = 1000;

const msg = (x: any): Record<string, any> => (x?.message ?? x) as Record<string, any>;

function paramMap(feat: Record<string, any>): Map<string, Record<string, any>> {
  const out = new Map<string, Record<string, any>>();
  for (const p of feat.parameters ?? []) {
    const pm = msg(p);
    if (pm.parameterId) out.set(pm.parameterId, pm);
  }
  return out;
}

function firstQueryString(pm: Record<string, any> | undefined): string | null {
  const q = pm?.queries?.[0];
  const s = q ? msg(q).queryString : undefined;
  return typeof s === "string" && s.length > 0 ? s : null;
}

/**
 * Is this sketch placed on a mate connector? Onshape writes that plane query
 * UNCOMPRESSED, as a FeatureScript call naming the connector's featureId.
 * Returns that id, or `null` for any other plane form.
 */
export function mateConnectorPlaneId(planeQueryString: string): string | null {
  if (!/BodyType\.MATE_CONNECTOR/.test(planeQueryString)) return null;
  const m = planeQueryString.match(/qCreatedBy\(\s*id\s*\+\s*"([^"]+)"/);
  return m ? m[1] : null;
}

/** The implicit mate connector a sketch carries as its own sub-feature, keyed
 *  by featureId. A sketch on a plane or a face has none. */
export function implicitMateConnectors(
  sketchFeature: Record<string, any>,
): Map<string, Record<string, any>> {
  const out = new Map<string, Record<string, any>>();
  for (const raw of sketchFeature.subFeatures ?? []) {
    const sub = msg(raw);
    if (sub.featureType === "mateConnector" && sub.featureId) {
      out.set(sub.featureId as string, sub);
    }
  }
  return out;
}

/** Literal string values of a token stream, in order (back-references stay
 *  opaque — every discriminating field appears as a literal on first use). */
const literals = (tokens: BtonToken[]): BtonToken[] => tokens.filter((t) => t.t === "str");

/**
 * Decode a mate-connector ANCHOR query (`originQuery` / `secondaryOriginQuery`).
 *
 * The grammar is the ordinary creation-query one, read for different markers
 * than a plane query needs: the ENTITY TYPE of the anchor, the sketch entity it
 * derives from, and — when the anchor is an entity of an extrude's cap — the
 * sweep operation plus the `isStart` side.
 */
export function decodeMateAnchor(tokens: BtonToken[]): OnshapeMateAnchor {
  const strs = literals(tokens);
  const has = (v: string) => strs.some((t) => t.p1 === v && !t.p2);

  // The part studio's ORIGIN point, which Onshape names as an operation
  // called `Origin` of type `pointOp`. It carries no `sketchEntityId` and no
  // feature id, so every marker the rest of this decoder looks for is absent —
  // it has to be recognised before them, not after. `extractQueryRefs` does
  // not see it either: `opRefOf` requires an `F…_n` feature id.
  if (strs.some((t) => t.p1 === "Origin" && t.p2 === "pointOp")) {
    return { kind: "originPoint" };
  }

  // Anchor entity type: the FIRST `entityType` value in the stream. A cap
  // query states the derived entity first and the cap's own type after it.
  const typeIdx = tokens.findIndex((t) => t.t === "str" && t.p1 === "entityType" && !t.p2);
  const entityType = tokens
    .slice(typeIdx + 1)
    .find(
      (t) => t.t === "str" && !t.p2 && (t.p1 === "EDGE" || t.p1 === "VERTEX" || t.p1 === "FACE"),
    )?.p1 as "EDGE" | "VERTEX" | "FACE" | undefined;
  if (typeIdx < 0 || !entityType) {
    return { kind: "unsupported", reason: "Anker-Query nennt keinen Entity-Typ" };
  }

  // The sketch entity the anchor derives from: the string token right after
  // the `sketchEntityId` key. Single-part ids (`EDMomUfcnXmW`) and multi-part
  // ones (`HhKEnMRHLJLB.right`) both appear.
  const idIdx = tokens.findIndex((t) => t.t === "str" && t.p1 === "sketchEntityId" && !t.p2);
  if (idIdx < 0) {
    return { kind: "unsupported", reason: "Anker-Query nennt keine Sketch-Entity" };
  }
  const idTok = tokens.slice(idIdx + 1).find((t) => t.t === "str" || t.t === "refstr");
  const entityId = idTok ? [idTok.p1, idTok.p2].filter(Boolean).join(".") : "";
  if (!entityId) {
    return { kind: "unsupported", reason: "Sketch-Entity-Id der Anker-Query nicht lesbar" };
  }

  // Which sketch owns it: the closest preceding wire marker.
  const { wires, ops } = extractQueryRefs(tokens);
  const wire = [...wires].reverse().find((w) => w.idx < idIdx);
  if (!wire) {
    return { kind: "unsupported", reason: `Anker-Sketch von "${entityId}" nicht bestimmbar` };
  }
  const origin: OnshapeEntityOrigin = { sketchFeatureId: wire.featureId, entityId };

  // A cap anchor: the entity belongs to an extrude's CAP, not to the raw wire.
  if (has("CAP_FACE") || has("CAP_EDGE")) {
    const extrude = ops.find((o) => o.op.op === "opExtrude");
    if (!extrude) {
      return { kind: "unsupported", reason: "Cap-Anker ohne Extrude-Operation" };
    }
    const isStartIdx = tokens.findIndex((t) => t.t === "str" && t.p1 === "isStart");
    const boolTok =
      isStartIdx >= 0 ? tokens.slice(isStartIdx + 1).find((t) => t.t === "bool") : undefined;
    if (!boolTok) {
      return { kind: "unsupported", reason: "Cap-Anker ohne isStart-Flag" };
    }
    return {
      kind: "capEntity",
      entityType,
      opFeatureId: extrude.op.featureId,
      isStart: boolTok.v === true,
      origin,
    };
  }

  return { kind: "sketchEntity", entityType, origin };
}

/** Decode + tokenize in one step; decode failures come back as `unsupported`. */
export async function decodeMateAnchorQuery(queryString: string): Promise<OnshapeMateAnchor> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(queryString);
    if (!bton) return { kind: "unsupported", reason: "Anker ist keine qCompressed-Query" };
    return decodeMateAnchor(tokenizeBton(payload));
  } catch (e) {
    return { kind: "unsupported", reason: e instanceof Error ? e.message : String(e) };
  }
}

function quantityValue(pm: Record<string, any> | undefined): number {
  const v = pm?.value;
  return typeof v === "number" ? v : 0;
}

/**
 * How to turn a quantity parameter into mm / degrees.
 *
 * This module decodes; it does not evaluate Onshape expressions, and it must
 * not — the evaluator needs the document's variables, which only the import
 * walk has. So the caller supplies the reader, and `readMateConnector` falls
 * back to Onshape's own SI `value` when none is given (which is what the unit
 * tests use).
 *
 * That fallback is NOT harmless in production: Onshape's `/features` payload
 * very often reports `value: 0` for a parameter whose `expression` is the real
 * content — a connector offset of `#Hoehe-3cm` reads as 0 mm, and the sketch on
 * that connector then sits at the origin instead of 1.8 m above it, with no
 * complaint anywhere. The importer therefore always passes a real reader.
 */
export type QuantityReader = (
  pm: Record<string, any> | undefined,
  fallback: number,
  want: "length" | "angle",
) => number;

/** mm for a length, degrees for an angle — from Onshape's SI `value` alone. */
const siQuantity: QuantityReader = (pm, fallback, want) => {
  if (!pm) return fallback;
  const v = quantityValue(pm);
  return want === "length" ? v * M_TO_MM : (v * 180) / Math.PI;
};

/**
 * Read a `mateConnector` feature (implicit sub-feature or top-level) into its
 * structured form. Every query is decoded; nothing is inferred from names.
 */
export async function readMateConnector(
  feature: Record<string, any>,
  quantity: QuantityReader = siQuantity,
): Promise<OnshapeMateConnector> {
  const feat = msg(feature);
  const params = paramMap(feat);
  const originQs = firstQueryString(params.get("originQuery"));
  const secondaryQs = firstQueryString(params.get("secondaryOriginQuery"));

  const anchor: OnshapeMateAnchor = originQs
    ? await decodeMateAnchorQuery(originQs)
    : { kind: "unsupported", reason: "kein originQuery" };
  const secondary = secondaryQs ? await decodeMateAnchorQuery(secondaryQs) : null;

  // Onshape reports quantities in SI; a translation is a length in metres.
  return {
    featureId: String(feat.featureId ?? ""),
    name: String(feat.name ?? "Mate connector"),
    originType: String(params.get("originType")?.value ?? ""),
    inference: String(params.get("entityInferenceType")?.value ?? ""),
    anchor,
    secondary,
    offsetMm: [
      quantity(params.get("translationX"), 0, "length"),
      quantity(params.get("translationY"), 0, "length"),
      quantity(params.get("translationZ"), 0, "length"),
    ],
    rotationType: String(params.get("rotationType")?.value ?? "ABOUT_Z"),
    rotationDeg: quantity(params.get("rotation"), 0, "angle"),
    flipPrimary: params.get("flipPrimary")?.value === true,
    realign: params.get("realign")?.value === true,
  };
}

/**
 * Where along the anchor EDGE the connector sits, as FieldDraft's `t ∈ [0, 1]`.
 *
 * `MID_POINT` is the midpoint. `POINT` names an ENDPOINT, and which one is only
 * legible from the `secondaryOriginQuery`: Onshape suffixes a sketch line's
 * vertices `.start` / `.end`, and FieldDraft's `t` runs the same way (the
 * sketch line's own direction). `CENTER` is not a position along an edge at all
 * — it is the centre of a circular one, which FieldDraft reaches through the
 * FACE the edge bounds, so it has no `t`.
 */
export function anchorT(mc: OnshapeMateConnector, straightEdge = false): number | null {
  if (mc.inference === "MID_POINT") return 0.5;
  // `CENTROID` is the mean position of the entity, which lies ON the edge only
  // when the edge is STRAIGHT — there it is the midpoint. On a circular rim the
  // centroid is the rim's centre, a point the edge never passes through, so the
  // caller has to vouch for straightness (#419).
  if (mc.inference === "CENTROID" && straightEdge) return 0.5;
  if (mc.inference !== "POINT") return null;
  const sec = mc.secondary;
  // Only an anchor that NAMES a sketch entity can say which end it means; the
  // origin point and an unreadable query both leave `t` undetermined.
  if (!sec || sec.kind === "unsupported" || sec.kind === "originPoint") return null;
  if (sec.origin.entityId.endsWith(".start")) return 0.0;
  if (sec.origin.entityId.endsWith(".end")) return 1.0;
  return null;
}

/** Human-readable form of an anchor, for import messages. */
export function describeAnchor(a: OnshapeMateAnchor): string {
  switch (a.kind) {
    case "sketchEntity":
      return `Sketch-Entity ${a.origin.entityId} (${a.entityType})`;
    case "capEntity":
      return `${a.entityType} der ${a.isStart ? "Start" : "End"}-Deckfläche von ${a.opFeatureId} (aus ${a.origin.entityId})`;
    case "originPoint":
      return "Ursprung des Part Studios";
    case "unsupported":
      return `nicht lesbar (${a.reason})`;
  }
}
