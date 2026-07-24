/**
 * Onshape extrude-profile REGION resolution (ticket #412).
 *
 * An Onshape extrude selects its profile through one `BTMIndividualQuery`
 * per chosen sketch REGION (face). Each query is a compressed
 * `SKETCH_ENTITY` topology descriptor that names the sketch curve
 * entities bounding that region — e.g. `c5e93azXj2ID.right`,
 * `oPgfZ6ZnMtct.bottom`. The importer used to read only the FIRST query
 * and then extrude the WHOLE sketch (`qCreatedBy(sketch)`), turning a
 * multi-region selection into one solid block — silent wrong geometry.
 *
 * This module decodes those queries down to the set of referenced sketch
 * entity ids so the importer can map them to a FieldDraft region.
 *
 * IMPORTANT MODEL MISMATCH (documented for the caller): Onshape identifies
 * a region by IMPRINT — overlapping/nested sketch curves subdivide the
 * plane into many small faces, each independently selectable. FieldDraft's
 * region model is an SDF-UNION flood-fill, so nested/overlapping closed
 * profiles MERGE into one region and imprint sub-faces have NO FieldDraft
 * counterpart. Where that happens the caller must warn and fall back to the
 * whole sketch — never silently emit the wrong regions. See
 * docs/ONSHAPE-IMPORT-GAP-MATRIX.md.
 */

import { decodeOnshapeQueryString, tokenizeBton, extractOpRefs } from "./onshapeQuery";

/** The structural keys of an Onshape topology query — everything that is
 *  NOT a sketch entity id. A str token whose first part is one of these is
 *  grammar, not geometry. */
const QUERY_KEYWORDS = new Set([
  "Query",
  "disambiguationData",
  "disambiguationType",
  "TOPOLOGY",
  "SILHOUETTE",
  "entities",
  "derivedFrom",
  "entityType",
  "EntityType",
  "EDGE",
  "VERTEX",
  "FACE",
  "BODY",
  "historyType",
  "CREATION",
  "IMPRINT",
  "imprint",
  "operationId",
  "Id",
  "queryType",
  "SKETCH_ENTITY",
  "sketchEntityId",
  "INTERSECT",
  "UNION",
  "SUBTRACTION",
]);

/** Onshape feature ids are `F…_<n>`; an operation ref (`…wireOp`,
 *  `…opExtrude`) carries one as its first string part. */
function isFeatureId(s: string): boolean {
  return /^F[A-Za-z0-9]+_\d+/.test(s);
}

export interface DecodedRegionQuery {
  /** featureId of the owning SKETCH (from the query's `wireOp`), or null. */
  sketchFeatureId: string | null;
  /** The distinct sketch-entity ids the region query references, in first
   *  appearance order. Back-referenced repeats and structural keywords are
   *  dropped; a base id whose first mention is a literal is always
   *  captured, so the SET of bounding rectangles/curves is complete even
   *  when individual `.top`/`.bottom` sides travel as opaque back-refs. */
  entityIds: string[];
  /** True when the query is a `SKETCH_ENTITY` region descriptor (as opposed
   *  to a whole-sketch `qCreatedBy`/`qSketchRegion`, which carries none). */
  isRegionQuery: boolean;
}

/**
 * Decode ONE extrude-profile query. Returns the owning sketch and the
 * referenced sketch-entity ids, or `null` when the string is not a
 * compressed BTON query (a plain `qSketchRegion(...)` / `qCreatedBy(...)`).
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
  try {
    tokens = tokenizeBton(payload);
  } catch {
    return null;
  }

  const wire = extractOpRefs(tokens).find((o) => o.op === "wireOp");
  const isRegionQuery = tokens.some((t) => t.t === "str" && t.p1 === "SKETCH_ENTITY");

  const entityIds: string[] = [];
  for (const t of tokens) {
    if (t.t !== "str" || !t.p1) continue;
    if (QUERY_KEYWORDS.has(t.p1)) continue;
    if (isFeatureId(t.p1)) continue; // op / wire marker
    const id = t.p2 ? `${t.p1}.${t.p2}` : t.p1;
    if (!entityIds.includes(id)) entityIds.push(id);
  }

  return { sketchFeatureId: wire?.featureId ?? null, entityIds, isRegionQuery };
}
