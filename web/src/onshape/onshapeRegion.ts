/**
 * Onshape extrude-profile REGION resolution (tickets #412, #440).
 *
 * An Onshape extrude selects its profile through one `BTMIndividualQuery`
 * per chosen sketch REGION (face). Each query is a compressed topology
 * descriptor. The importer used to read only the FIRST query and then
 * extrude the WHOLE sketch (`qCreatedBy(sketch)`), turning a multi-region
 * selection into one solid block — silent wrong geometry.
 *
 * This module decodes those queries into the region's BOUNDARY: the list of
 * imprint edges that fence it in, each one a named sketch curve plus the
 * imprint vertices that trim it to a piece. #412 read only a flat scan of
 * the referenced sketch-entity ids, which cost two things #440 needed back:
 *
 *  - the flat scan cannot tell a BOUNDING curve from one that merely appears
 *    in a nested vertex disambiguation, and it swept up structural values
 *    (`CAP_EDGE`) as if they were entity ids — every one of which made the
 *    whole query unresolvable;
 *  - it drops the TRIM, and the trim is the only thing that distinguishes two
 *    regions bounded by the same curves. Onshape's `Extrude 4` in the #419
 *    document selects three strips of one rectangle whose flat id sets are
 *    literally identical; only the vertex pairs tell them apart.
 *
 * IMPORTANT MODEL MISMATCH (documented for the caller): Onshape identifies
 * a region by IMPRINT — overlapping/nested sketch curves subdivide the
 * plane into many small faces, each independently selectable. FieldDraft's
 * arrangement need not cut in the same places, so ONE Onshape region can
 * cover SEVERAL FieldDraft faces (and, where a region is fenced by geometry
 * our sketch does not carry, several Onshape regions can share one of ours).
 * Mapping the boundary onto our faces is `resolveRegionFaces`
 * (`onshapeRegionFaces.ts`); see docs/ONSHAPE-IMPORT-GAP-MATRIX.md.
 */

import {
  decodeFaceRef,
  decodeOnshapeQueryString,
  tokenizeBton,
  extractOpRefs,
  parseBton,
  type BtonValue,
  type OnshapeFaceRef,
} from "./onshapeQuery";

/** One imprint edge of a region's boundary. */
export interface RegionBoundaryEdge {
  /** The sketch curve this edge is a piece of. */
  entityId: string;
  /** The sketch curves meeting at each end of the piece — an imprint vertex
   *  is an INTERSECTion, so naming the crossing curves locates it. Two ends
   *  trim the curve; ANY other count means the whole curve is the edge (0) or
   *  the trim is not decidable (1), and the caller must not trim. */
  ends: string[][];
  /** Onshape's `±1`: on WHICH FLANK of this edge the region lies. `null` when
   *  the query states none. */
  side?: number | null;
}

export interface DecodedRegionQuery {
  /** featureId of the owning SKETCH (from the query's `wireOp`), or null. */
  sketchFeatureId: string | null;
  /** The region's boundary, in the order Onshape lists it. Empty when the
   *  query is a FACE query of some other kind (a swept face, a cap) whose
   *  profile has no sketch-region reading at all. */
  edges: RegionBoundaryEdge[];
  /** True when the query names a FACE — the shape a region selection has.
   *  A whole-sketch `qCreatedBy`/`qSketchRegion` carries no BTON at all. */
  isRegionQuery: boolean;
  /** The face a NON-imprint profile query names — a wall or a cap of an
   *  earlier feature, i.e. an extrude that grows out of a SOLID rather than a
   *  sketch (#442). `null` for an imprint region (which has `edges` instead)
   *  and for anything that is no face query at all. */
  faceRef: OnshapeFaceRef | { reason: string } | null;
}

type Obj = { [k: string]: BtonValue };

const isObj = (v: BtonValue | undefined): v is Obj =>
  !!v && typeof v === "object" && !Array.isArray(v);

/** The TOPOLOGY disambiguation of a query: the `[entity, side]` pairs that
 *  fence a face in, or that trim an edge. Onshape also carries
 *  `ORIGINAL_DEPENDENCY` blocks, which name provenance rather than
 *  neighbours — those say nothing about a boundary. */
function topologyEntities(v: Obj): Obj[] {
  return topologyPairs(v).map(([e]) => e);
}

/** The TOPOLOGY entries WITH the SIDE Onshape states for each — `[entity, ±1]`.
 *  The side says on which flank of that edge the face lies, and it is the only
 *  thing in the query that distinguishes a region from the hole it is fenced
 *  around (#442). */
function topologyPairs(v: Obj): [Obj, number | null][] {
  const data = v.disambiguationData;
  if (!Array.isArray(data)) return [];
  const topo = data.find((d) => isObj(d) && d.disambiguationType === "TOPOLOGY");
  if (!isObj(topo) || !Array.isArray(topo.entities)) return [];
  const out: [Obj, number | null][] = [];
  for (const pair of topo.entities) {
    const e = Array.isArray(pair) ? pair[0] : pair;
    if (!isObj(e)) continue;
    const s = Array.isArray(pair) ? pair[1] : undefined;
    out.push([e, typeof s === "number" ? s : null]);
  }
  return out;
}

/** The sketch curve a query derives from, or null when it derives from
 *  something a sketch has no name for (a cap edge, a swept edge). */
function sketchEntityOf(v: Obj): string | null {
  const from = v.derivedFrom;
  return isObj(from) && typeof from.sketchEntityId === "string" ? from.sketchEntityId : null;
}

/** The sketch curves that meet at an imprint VERTEX. */
function vertexCurves(v: Obj): string[] {
  const from = v.derivedFrom;
  const list = Array.isArray(from) ? from : [from];
  const ids: string[] = [];
  for (const f of list) {
    if (isObj(f) && typeof f.sketchEntityId === "string" && !ids.includes(f.sketchEntityId)) {
      ids.push(f.sketchEntityId);
    }
  }
  return ids;
}

/** The boundary of a sketch-region query: every imprint edge that derives
 *  from a sketch curve, with the vertices trimming it.
 *
 *  Edges that derive from something else are DROPPED rather than refused: the
 *  region really is fenced by them, we just have no curve to walk. Losing one
 *  lowers how much of a candidate face the boundary accounts for, which is
 *  exactly the effect it should have on the ranking — while refusing the whole
 *  query would throw away a selection Onshape made. */
export function regionBoundary(doc: { [k: string]: BtonValue }): RegionBoundaryEdge[] {
  const out: RegionBoundaryEdge[] = [];
  for (const [edge, side] of topologyPairs(doc)) {
    const entityId = sketchEntityOf(edge);
    if (!entityId) continue;
    const ends = topologyEntities(edge)
      .filter((v) => v.entityType === "VERTEX")
      .map(vertexCurves)
      .filter((ids) => ids.length > 0);
    out.push({ entityId, ends, side });
  }
  return out;
}

/**
 * Decode ONE extrude-profile query. Returns the owning sketch and the region's
 * boundary, or `null` when the string is not a compressed BTON query (a plain
 * `qSketchRegion(...)` / `qCreatedBy(...)`).
 */
export async function decodeRegionQuery(
  queryString: string,
): Promise<DecodedRegionQuery | null> {
  let bton: boolean;
  let payload: string;
  try {
    ({ bton, payload } = await decodeOnshapeQueryString(queryString));
  } catch {
    return null;
  }
  if (!bton) return null;

  let tokens;
  let doc: BtonValue;
  try {
    tokens = tokenizeBton(payload);
    doc = parseBton(tokens);
  } catch {
    return null;
  }

  const wire = extractOpRefs(tokens).find((o) => o.op === "wireOp");
  const isRegionQuery = isObj(doc) && doc.entityType === "FACE";
  // Only an IMPRINT face is a sketch region. A `SWEPT_FACE` / `CAP_FACE` names
  // a face of a SOLID — a profile too, but one that reads as a FACE and not as
  // a boundary of sketch curves (#442).
  const imprint = isObj(doc) && doc.queryType === "IMPRINT";
  const edges = isObj(doc) && imprint ? regionBoundary(doc) : [];
  const faceRef = isRegionQuery && !imprint ? decodeFaceRef(doc) : null;

  return { sketchFeatureId: wire?.featureId ?? null, edges, isRegionQuery, faceRef };
}
