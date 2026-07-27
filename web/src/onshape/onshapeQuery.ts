/**
 * Onshape query-string decoder (Onshape-Import Schritt 4).
 *
 * Onshape serialises its declarative geometry queries into the
 * `queryString` field of `BTMIndividualQuery` parameters:
 *
 *   query=qCompressed(1.0,"<payload>",id);
 *
 * The payload is Onshape's BTON dictionary text — either plain
 * (`%B5$Query…`) or zlib/base64 (`&<hexlen>$eJ…`). Two readings sit on the same
 * tokenizer:
 *
 *  - {@link parseBton} rebuilds the actual document tree, resolving the string
 *    back-references, so a value can be read AT ITS KEY. The EDGE translation
 *    (#425) uses it; it is the only reading that can tell a query's own fields
 *    from those of the ancestors its disambiguation nests.
 *  - {@link extractPlaneRef} scans the flat token stream for literals. Every
 *    DISCRIMINATING field of a PLANE query appears as a literal on first use, so
 *    the scan suffices there — validated against the Kabelbox corpus.
 *
 * Anything that doesn't match a known shape comes back as
 * `{kind: "unsupported"}` so the import dialog can fail VISIBLY — never
 * silently fall back to XY, never round a plausible-looking wrong edge.
 *
 * Token vocabulary (empirical):
 *   `M<hex>` map · `A<hex>` array · `C<hex>` class back-ref (0-based)
 *   `S<hex>$s` string · `S<h1>.<h2>$s1s2` two-part string
 *   `S-<ref>.<h>$suffix` table reference + literal suffix
 *   `B<hex>$Name` class/enum introduction · `R<hex>` string back-ref
 *   `E<hex>` shared SUB-STRUCTURE (a table we do not reconstruct)
 *   `D-?<hex>` number · `T`/`F` booleans · `%` root marker
 */

/** One component of a BTON string: either spelled out or a back-reference
 *  into the string table. `S12.6$F…wireOp` is two literal parts,
 *  `S-13.5$right` a table reference followed by a literal one. */
export type BtonPart = { lit: string } | { ref: number };

/** One BTON token. Literal strings carry their parts; back-references
 *  stay opaque (`ref`). */
export interface BtonToken {
  t: "str" | "enum" | "ref" | "map" | "arr" | "cls" | "num" | "bool" | "refstr";
  /** str/enum: first part; refstr: the suffix. */
  p1?: string;
  /** str: second part of a two-part string ("" for plain). */
  p2?: string;
  /** ref/cls/map/arr/num: the numeric payload; refstr: the base ref. */
  n?: number;
  /** bool value. */
  v?: boolean;
  /** str/enum/refstr: the parts as written, literal or referenced. The
   *  structured parser needs the distinction (only LITERAL parts enter the
   *  string table); the flat scans read `p1`/`p2` instead. */
  parts?: BtonPart[];
  /** ref: written as `E<hex>` — a shared SUB-STRUCTURE, not a string. */
  structural?: boolean;
}

/** Strip the `qCompressed` wrapper and inflate the payload when it is
 *  the `&<hexlen>$<base64-zlib>` form. Non-compressed wrappers (e.g.
 *  `qSketchRegion(...)`) are returned verbatim with `bton: false`. */
export async function decodeOnshapeQueryString(
  queryString: string,
): Promise<{ bton: boolean; payload: string }> {
  const m = queryString.match(/qCompressed\(1\.0,"([\s\S]*)"/);
  if (!m) return { bton: false, payload: queryString };
  let body = m[1];
  if (body.startsWith("&")) {
    const dollar = body.indexOf("$");
    if (dollar < 0) throw new Error("malformed compressed payload");
    body = await inflateBase64(body.slice(dollar + 1));
  }
  return { bton: true, payload: body };
}

async function inflateBase64(b64: string): Promise<string> {
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  // Plain ReadableStream instead of Blob.stream() — jsdom's Blob lacks it.
  const source = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    },
  });
  // Type-only cast: lib.dom types DecompressionStream over `BufferSource`,
  // which pipeThrough's `ReadableWritablePair<Uint8Array, …>` rejects.
  const inflated = source.pipeThrough(
    new DecompressionStream("deflate") as unknown as ReadableWritablePair<
      Uint8Array,
      Uint8Array
    >,
  );
  return await new Response(inflated).text();
}

/** Tokenize a BTON payload. Throws on unknown syntax — the caller maps
 *  that to a visible "unsupported" result. */
export function tokenizeBton(payload: string): BtonToken[] {
  const out: BtonToken[] = [];
  let i = 0;
  const hex = (): number => {
    let neg = false;
    if (payload[i] === "-") {
      neg = true;
      i += 1;
    }
    const start = i;
    while (i < payload.length && /[0-9a-f]/.test(payload[i])) i += 1;
    if (i === start) throw new Error(`expected hex at ${start}`);
    return (neg ? -1 : 1) * parseInt(payload.slice(start, i), 16);
  };
  while (i < payload.length) {
    const c = payload[i];
    if (c === "%") {
      i += 1;
      continue;
    }
    if (c === "S" || c === "B") {
      i += 1;
      // A string is a dot-separated list of PART lengths; a negative
      // length is a back-reference into the string table (kept opaque),
      // a positive one a literal consumed from the stream. Multi-part
      // entity ids join with "." (`Sc.1.9$5II1ZIpSJtA41filletArc` →
      // `5II1ZIpSJtA4.1.filletArc`, exactly the geometry's entityId).
      const lens: number[] = [hex()];
      while (payload[i] === ".") {
        i += 1;
        lens.push(hex());
      }
      if (payload[i] !== "$") throw new Error(`expected $ at ${i}`);
      i += 1;
      const parts: string[] = [];
      const raw: BtonPart[] = [];
      let ref: number | undefined;
      for (const len of lens) {
        if (len < 0) {
          ref = -len;
          raw.push({ ref: -len });
          continue;
        }
        const lit = payload.slice(i, i + len);
        parts.push(lit);
        raw.push({ lit });
        i += len;
      }
      if (ref !== undefined) {
        out.push({ t: "refstr", n: ref, p1: parts.join("."), parts: raw });
      } else {
        out.push({
          t: c === "B" ? "enum" : "str",
          p1: parts[0] ?? "",
          p2: parts.slice(1).join("."),
          parts: raw,
        });
      }
      continue;
    }
    if (c === "R" || c === "E") {
      // `R<hex>` is a STRING back-reference (into the table of parts every
      // literal string introduces). `E<hex>` appears where a VALUE is
      // expected and shares a whole previously-emitted SUB-STRUCTURE — a
      // different table, which we do not reconstruct. Keeping the two apart
      // matters: resolving an `E` against the string table yields a
      // plausible-looking but WRONG string, and that string then lands in a
      // map-key slot. A well-formed token we happen not to interpret must
      // never abort the decode either — throwing here killed every edge
      // query of a document that used it.
      i += 1;
      out.push({ t: "ref", n: hex(), structural: c === "E" });
      continue;
    }
    if (c === "M" || c === "A" || c === "C") {
      i += 1;
      out.push({ t: c === "M" ? "map" : c === "A" ? "arr" : "cls", n: hex() });
      continue;
    }
    if (c === "D") {
      i += 1;
      out.push({ t: "num", n: hex() });
      continue;
    }
    if (c === "T" || c === "F") {
      i += 1;
      out.push({ t: "bool", v: c === "T" });
      continue;
    }
    throw new Error(`unknown BTON syntax '${c}' at ${i}`);
  }
  return out;
}

/** A BTON document as a plain JS value. Maps become objects (their keys are
 *  always strings), arrays arrays, class wrappers are transparent — a class
 *  intro only names the value that follows. */
export type BtonValue =
  | string
  | number
  | boolean
  | BtonValue[]
  | { [k: string]: BtonValue };

/** A back-reference the string tables could not resolve. It is a VALUE, not a
 *  string, so every consumer that expects text rejects it instead of guessing.
 *  (Onshape shares whole sub-structures too; those refs live in a table we do
 *  not reconstruct, and the fields they carry are disambiguation metadata we
 *  do not read.) */
export interface BtonUnresolved {
  unresolvedRef: number;
}

export function isUnresolved(v: BtonValue | undefined): v is BtonValue & BtonUnresolved {
  return typeof v === "object" && v !== null && !Array.isArray(v) && "unresolvedRef" in v;
}

/**
 * The two string tables a BTON payload builds up as it is read. Empirically
 * calibrated against the #419 fillet/chamfer corpus (103 payloads), where every
 * reading is cross-checked by a hard constraint: a map KEY must come out as one
 * of Onshape's field names, and a class ref (`C<n>`) as a class name.
 *
 *  - `all` — every string INTRODUCED, in order: class names (`B…`), each
 *    literal part of a multi-part string, and then the joined string itself.
 *    `C<n>` and the negative part-lengths inside a string index this table.
 *  - `parts` — the same list WITHOUT class names and WITHOUT the joined
 *    composites. `R<n>` indexes this one.
 *
 * That the two tables differ is not a guess: `C12` resolves to `Id` only with
 * the class names counted, and a replayed map key `R13` resolves to
 * `sketchEntityId` only with them left out. Both hold across the corpus.
 */
interface BtonStrings {
  all: string[];
  parts: string[];
}

/** Read one string token, registering the parts it introduces. */
function readBtonString(tok: BtonToken, st: BtonStrings, isClass: boolean): string {
  const resolved: string[] = [];
  for (const part of tok.parts ?? []) {
    if ("lit" in part) {
      resolved.push(part.lit);
      st.all.push(part.lit);
      if (!isClass) st.parts.push(part.lit);
    } else {
      const s = st.all[part.ref];
      if (s === undefined) throw new Error(`BTON: string ref ${part.ref} out of range`);
      resolved.push(s);
    }
  }
  const joined = resolved.join(".");
  // A multi-part string also registers the joined form — that is the entry a
  // later `<featureId>.<op>` reference points at.
  if ((tok.parts?.length ?? 0) > 1) st.all.push(joined);
  return joined;
}

/**
 * Parse a tokenized BTON payload into a plain JS value.
 *
 * The grammar is small: a class intro (`B<len>$Name`) or class ref (`C<n>`)
 * merely NAMES the value that follows, `M<n>` is a map of n key/value pairs,
 * `A<n>` an array of n values, and everything else is a leaf. Unlike the flat
 * marker scans this yields the actual field structure, so a value can be read
 * AT ITS KEY (`sketchEntityId`) instead of by its shape — the mistake that
 * cost #419 six sketch planes.
 *
 * Throws on a malformed stream (unbalanced containers, a string ref pointing
 * past the table); the callers turn that into a visible "unsupported" result.
 */
export function parseBton(tokens: BtonToken[]): BtonValue {
  const st: BtonStrings = { all: [], parts: [] };
  const cur = { i: 0 };
  const value = parseBtonValue(tokens, cur, st);
  if (cur.i !== tokens.length) {
    throw new Error(`BTON: ${tokens.length - cur.i} trailing token(s)`);
  }
  return value;
}

function parseBtonValue(tokens: BtonToken[], cur: { i: number }, st: BtonStrings): BtonValue {
  const tok = tokens[cur.i];
  if (!tok) throw new Error("BTON: unexpected end of stream");
  cur.i += 1;
  switch (tok.t) {
    case "enum":
      // Class introduction: name it, then read the value it wraps.
      readBtonString(tok, st, true);
      return parseBtonValue(tokens, cur, st);
    case "cls":
      // Class back-reference — same, but the name is already known.
      return parseBtonValue(tokens, cur, st);
    case "map": {
      const out: { [k: string]: BtonValue } = {};
      for (let k = 0; k < (tok.n ?? 0); k += 1) {
        const key = parseBtonValue(tokens, cur, st);
        const val = parseBtonValue(tokens, cur, st);
        // A key we cannot name gets a synthetic one rather than aborting the
        // parse. Onshape shares whole sub-structures through a second table we
        // do not reconstruct, and those shared branches are TOPOLOGY
        // disambiguation metadata — nothing the translation reads. Throwing
        // here threw the well-formed ROOT away with them (9 of #416's 22 edge
        // queries). The `?` prefix cannot collide with an Onshape field name,
        // so a lost key can never masquerade as one.
        out[typeof key === "string" ? key : `?${k}`] = val;
      }
      return out;
    }
    case "arr": {
      const out: BtonValue[] = [];
      for (let k = 0; k < (tok.n ?? 0); k += 1) out.push(parseBtonValue(tokens, cur, st));
      return out;
    }
    case "str":
    case "refstr":
      return readBtonString(tok, st, false);
    case "ref": {
      const s = tok.structural ? undefined : st.parts[tok.n ?? -1];
      return s === undefined ? { unresolvedRef: tok.n ?? -1 } : s;
    }
    case "num":
      return tok.n ?? 0;
    case "bool":
      return tok.v === true;
  }
}

/** Decode + tokenize + parse in one step. */
export async function parseQueryDocument(queryString: string): Promise<BtonValue | null> {
  const { bton, payload } = await decodeOnshapeQueryString(queryString);
  if (!bton) return null;
  return parseBton(tokenizeBton(payload));
}

/** An operation reference extracted from the token stream: the Onshape
 *  feature id plus the operation suffix (`wireOp`, `opExtrude`,
 *  `opFillet`, `opBoolean`, `opShell`, …). */
export interface OnshapeOpRef {
  featureId: string;
  op: string;
}

/** A sketch-entity origin: which sketch's wire, and the entity id
 *  within it (`<wireEntityBase>.<suffix>` joined — the same string the
 *  sketch geometry's `entityId` field carries). */
export interface OnshapeEntityOrigin {
  /** featureId of the OWNING SKETCH (from the closest preceding wireOp). */
  sketchFeatureId: string;
  /** The full sketch-entity id, e.g. `zTyZhgNVqaMx.bottom`. */
  entityId: string;
}

/** What a decoded sketch-plane query means. */
export type OnshapePlaneRef =
  | { kind: "default"; plane: "top" | "front" | "right" }
  | { kind: "cap"; opFeatureId: string; isStart: boolean }
  | {
      kind: "swept";
      opFeatureId: string;
      origin: OnshapeEntityOrigin;
      /** Onshape `OFFSET_FACE`: not the wall itself but its SHELL
       *  counterpart — the inward-offset face of the hollow. */
      offset?: boolean;
      /** Fillet whose BLEND_FACE disambiguates the fragment (split faces). */
      blendFilletId?: string;
      /** The split peer's swept origin (boolean SPLIT disambiguation). */
      splitPeer?: { opFeatureId: string; origin: OnshapeEntityOrigin };
    }
  | { kind: "unsupported"; reason: string };

const DEFAULT_PLANES: Record<string, "top" | "front" | "right"> = {
  Top: "top",
  Front: "front",
  Right: "right",
};

/** Every operation reference in a token stream, in order — `opExtrude`,
 *  `opShell`, `opFillet`, `wireOp`, … A query that names a face of a
 *  SHELLED sweep, for instance, carries both the shell op and the sweep
 *  op that actually owns the face. */
export function extractOpRefs(tokens: BtonToken[]): OnshapeOpRef[] {
  return tokens.map(opRefOf).filter((o): o is OnshapeOpRef => o !== null);
}

/** Does a two-part string look like `<featureId><operation>`? Onshape
 *  feature ids are `F…_<n>` (deterministic per document). */
function opRefOf(tok: BtonToken): OnshapeOpRef | null {
  if (tok.t !== "str" || !tok.p1 || !tok.p2) return null;
  if (!/^F[A-Za-z0-9]+_\d+$/.test(tok.p1)) return null;
  if (!/^(wireOp|op[A-Z][A-Za-z]*)$/.test(tok.p2)) return null;
  return { featureId: tok.p1, op: tok.p2 };
}

/** The structural landmarks of a decoded query: operation references, wire
 *  (sketch) markers and the sketch-entity origins associated with them. Shared
 *  by the plane translation and the EDGE translation (#414), which read the
 *  same stream for different markers. */
export interface OnshapeQueryRefs {
  ops: { idx: number; op: OnshapeOpRef }[];
  wires: { idx: number; featureId: string }[];
  origins: { idx: number; origin: OnshapeEntityOrigin }[];
}

/**
 * Scan a token stream for operation refs + sketch-entity origins, in stream
 * order. A repeated operation suffix travels as an opaque back-ref
 * (`S11.-e$F4Ns…`), so a BARE feature-id literal also counts as a wire marker
 * for origin association — in the corpus it always directly precedes its
 * sketch-entity id.
 *
 * An entity id is read from the MAP KEY `sketchEntityId`, not from its shape.
 * Both forms occur: a sub-piece of a construction primitive carries a suffix
 * (`pozkE6lurT5A.right`, `mckFvNyxoF2z.0`), a plainly drawn line does not
 * (`pfTtMVvYeZsa`). Keying off the two-part shape alone — as this did until
 * #419 — silently drops every undotted id, and a `SWEPT_FACE` plane whose
 * origin went missing reads as "unsupported query shape", which is a lie: the
 * shape was understood, one value fell through the sieve.
 */
export function extractQueryRefs(tokens: BtonToken[]): OnshapeQueryRefs {
  const ops: { idx: number; op: OnshapeOpRef }[] = [];
  const wires: { idx: number; featureId: string }[] = [];
  const origins: { idx: number; origin: OnshapeEntityOrigin }[] = [];
  // Set by the literal key, consumed by the very next string token — BTON is
  // a flat key/value stream, so the value is whatever string follows.
  let entityIdExpected = false;
  tokens.forEach((t, idx) => {
    const op = opRefOf(t);
    if (op) {
      ops.push({ idx, op });
      if (op.op === "wireOp") wires.push({ idx, featureId: op.featureId });
      entityIdExpected = false;
      return;
    }
    if (
      (t.t === "str" || t.t === "refstr") &&
      t.p1 &&
      !t.p2 &&
      /^F[A-Za-z0-9]+_\d+$/.test(t.p1)
    ) {
      // Bare feature id, literal or with an opaque ref suffix (a repeated
      // operation name compressed away) — a wire/operation marker.
      wires.push({ idx, featureId: t.p1 });
      entityIdExpected = false;
      return;
    }
    // A back-referenced value CONSUMES the key slot: its string cannot be
    // reconstructed here, and letting the flag survive would pin the key to
    // the next unrelated literal.
    if (t.t === "refstr") {
      entityIdExpected = false;
      return;
    }
    if (t.t !== "str" || !t.p1) return;
    if (t.p1 === "sketchEntityId" && !t.p2) {
      entityIdExpected = true;
      return;
    }
    // A sketch-entity id: the value of `sketchEntityId`, or a two-part string
    // following its wire marker (the corpus form the decoder was built on).
    if (t.p2 || entityIdExpected) {
      entityIdExpected = false;
      const wire = [...wires].reverse().find((w) => w.idx < idx);
      if (wire) {
        origins.push({
          idx,
          origin: {
            sketchFeatureId: wire.featureId,
            entityId: t.p2 ? `${t.p1}.${t.p2}` : t.p1,
          },
        });
      }
    }
  });
  return { ops, wires, origins };
}

/**
 * Extract the plane semantics from a tokenized sketch-plane query.
 * Order-based literal scan, validated against the Kabelbox corpus:
 * every plane query states its discriminating fields as literals on
 * first use; repeats travel as opaque back-refs.
 */
export function extractPlaneRef(tokens: BtonToken[]): OnshapePlaneRef {
  // Literal string values seen, in order.
  const strs = tokens.filter((t) => t.t === "str");
  const has = (v: string) => strs.some((t) => t.p1 === v && !t.p2);

  // Default datum plane: `<Top|Front|Right>planeOp` + queryType DUMMY.
  for (const t of strs) {
    if (t.p2 === "planeOp" && t.p1 && DEFAULT_PLANES[t.p1] && has("DUMMY")) {
      return { kind: "default", plane: DEFAULT_PLANES[t.p1] };
    }
  }

  const { ops, wires, origins } = extractQueryRefs(tokens);

  const opsBy = (name: string) => ops.filter((o) => o.op.op === name);
  const extrudes = opsBy("opExtrude");
  const fillets = opsBy("opFillet");
  // A lateral wall is a wall whichever op swept it: FieldDraft names it
  // `qWall(op, entity)` from the PROFILE-EDGE ordinal, and a revolve builds
  // its mantle from the same profile edges an extrude does (#419 — six of its
  // sketches sit on the mantle of a revolve). A cap is NOT widened the same
  // way: `capFaceQuery` needs the sweep direction to know which FieldDraft cap
  // Onshape's `isStart` names, and only an extrude records one.
  const sweptOps = [...extrudes, ...opsBy("opRevolve")].sort((a, b) => a.idx - b.idx);

  // CAP_FACE: cap of an extrude, side from the isStart flag.
  if (has("CAP_FACE") && !has("SWEPT_FACE")) {
    const isStartIdx = tokens.findIndex((t) => t.t === "str" && t.p1 === "isStart");
    const boolTok = isStartIdx >= 0 ? tokens.slice(isStartIdx + 1).find((t) => t.t === "bool") : undefined;
    if (extrudes.length >= 1 && boolTok) {
      return {
        kind: "cap",
        opFeatureId: extrudes[0].op.featureId,
        isStart: boolTok.v === true,
      };
    }
    return { kind: "unsupported", reason: "CAP_FACE without extrude op / isStart flag" };
  }

  // SWEPT_FACE: wall of a sweep (extrude or revolve) from a sketch entity;
  // optionally disambiguated by BLEND_FACE adjacency and a boolean SPLIT peer.
  if (has("SWEPT_FACE")) {
    if (sweptOps.length === 0 || origins.length === 0) {
      return { kind: "unsupported", reason: "SWEPT_FACE without sweep op / origin entity" };
    }
    const primary = origins[0].origin;
    const out: OnshapePlaneRef = {
      kind: "swept",
      opFeatureId: sweptOps[0].op.featureId,
      origin: primary,
    };
    // A shelled body's inner wall is a face of the SAME sweep, offset inward.
    if (has("OFFSET_FACE")) out.offset = true;
    if (has("BLEND_FACE") && fillets.length >= 1) {
      out.blendFilletId = fillets[0].op.featureId;
    }
    if (has("SPLIT")) {
      // The split peer: a second origin entity, followed by the feature
      // id of the sweep that owns the splitting wall (its operation
      // suffix travels as an opaque ref, so match the FIRST feature
      // marker after the peer origin that isn't the fillet, the primary
      // op, or a sketch wire already accounted for).
      const peerOrigin = origins.find((o) => o.origin.entityId !== primary.entityId);
      if (peerOrigin) {
        const known = new Set<string>([
          sweptOps[0].op.featureId,
          out.blendFilletId ?? "",
          ...origins.map((o) => o.origin.sketchFeatureId),
        ]);
        const peer = wires.find((w) => w.idx > peerOrigin.idx && !known.has(w.featureId));
        if (peer) {
          out.splitPeer = { opFeatureId: peer.featureId, origin: peerOrigin.origin };
        }
      }
    }
    return out;
  }

  return { kind: "unsupported", reason: "no known plane query shape" };
}

/** A B-Rep FACE an Onshape query names, in the terms FieldDraft can express. */
export type OnshapeFaceRef =
  /** A sweep's cap. `isStart` is Onshape's sketch-plane side. */
  | { kind: "cap"; opFeatureId: string; isStart: boolean }
  /** The lateral wall swept from one sketch entity. */
  | { kind: "wall"; opFeatureId: string; origin: OnshapeEntityOrigin }
  /** A shelled body's inner skin (Onshape `OFFSET_FACE`), wall side. */
  | { kind: "innerWall"; opFeatureId: string }
  /** A shelled body's inner skin, cap side. */
  | { kind: "innerCap"; opFeatureId: string; isStart: boolean };

/**
 * What a decoded fillet/chamfer EDGE query names. FieldDraft addresses a B-Rep
 * edge as `qCommonEdge(faceA, faceB)` — the edge where two named faces meet —
 * so every supported shape lowers to a PAIR of faces. A shape that does not is
 * refused by name rather than approximated: rounding the wrong edge is a silent
 * geometry defect, a refusal is a line in the import report.
 */
export type OnshapeEdgeRef =
  | { kind: "faces"; a: OnshapeFaceRef; b: OnshapeFaceRef }
  | { kind: "unsupported"; reason: string };

const asMap = (v: BtonValue | undefined): { [k: string]: BtonValue } | null =>
  typeof v === "object" && v !== null && !Array.isArray(v) && !("unresolvedRef" in v)
    ? (v as { [k: string]: BtonValue })
    : null;

const asText = (v: BtonValue | undefined): string | null => (typeof v === "string" ? v : null);

/**
 * A `queryType`, but only if it LOOKS like one. Onshape's query types are
 * SCREAMING_CASE tokens; a string table drifts by one where a shared
 * sub-structure was skipped, and then a `queryType` slot reads `bottom`,
 * `opRevolve` or `1` — a value from a neighbouring slot. Those are not query
 * types, and reporting „Flächenform bottom" would blame Onshape for our own
 * decode. Anything that fails this test is refused as UNDECODED.
 */
function queryTypeOf(v: BtonValue | undefined): string | null {
  const s = asText(v);
  return s && /^[A-Z][A-Z_0-9]*$/.test(s) ? s : null;
}

/** Onshape ids travel as `Id[["<featureId>.<op>"]]` — one array level around a
 *  dotted string. Split it, and only accept the full form: a bare `opExtrude`
 *  is a back-reference whose owning feature we cannot recover. */
function opRefOfValue(v: BtonValue | undefined): OnshapeOpRef | null {
  const s = asText(Array.isArray(v) ? v[0] : v);
  if (!s) return null;
  const dot = s.indexOf(".");
  if (dot < 0) return null;
  const featureId = s.slice(0, dot);
  if (!/^F[A-Za-z0-9]+_\d+$/.test(featureId)) return null;
  return { featureId, op: s.slice(dot + 1) };
}

/**
 * The SKETCH ENTITIES an entity descends from, read out of its
 * `ORIGINAL_DEPENDENCY` disambiguation — at the KEYS `operationId` and
 * `sketchEntityId`, never by the shape of a string.
 *
 * Onshape replays a repeated `operationId` as a back-reference to a string PART
 * (`"wireOp"`), which on its own no longer names a sketch. Such an entry
 * inherits the sketch of the entry BEFORE it — that is what a back-reference to
 * the preceding value means, and it is not left as an assumption: the caller
 * looks the entity id up in exactly that sketch's entity table, and a 12-random-
 * character id that is not in it fails the translation visibly. In the one
 * corpus query where two sketches meet (#419 `Fillet 7`), the inherited sketch
 * is the one whose entity ids the following originals actually carry.
 */
function sketchOriginals(root: { [k: string]: BtonValue }): OnshapeEntityOrigin[] {
  const out: OnshapeEntityOrigin[] = [];
  const dd = root.disambiguationData;
  if (!Array.isArray(dd)) return out;
  for (const entry of dd) {
    const em = asMap(entry);
    if (!em || asText(em.disambiguationType) !== "ORIGINAL_DEPENDENCY") continue;
    const originals = Array.isArray(em.originals) ? em.originals : [];
    let sketchFeatureId: string | null = null;
    for (const o of originals) {
      const om = asMap(o);
      if (!om || queryTypeOf(om.queryType) !== "SKETCH_ENTITY") continue;
      const op = opRefOfValue(om.operationId);
      if (op) sketchFeatureId = op.op === "wireOp" ? op.featureId : null;
      const entityId = asText(om.sketchEntityId);
      if (!entityId || !sketchFeatureId) continue;
      out.push({ sketchFeatureId, entityId });
    }
  }
  return out;
}

/** One face of an edge's `derivedFrom` pair. */
function faceRefOf(v: BtonValue): OnshapeFaceRef | { reason: string } {
  const m = asMap(v);
  if (!m) return { reason: "Fläche als geteilte Rück-Referenz (nicht auflösbar)" };
  const qt = queryTypeOf(m.queryType);
  const op = opRefOfValue(m.operationId);
  if (!qt) return { reason: "Flächen-Query, deren queryType nicht dekodierbar ist" };
  if (!op) return { reason: `Flächen-Query ${qt} ohne lesbare Operations-Id` };
  if (qt === "CAP_FACE") {
    if (typeof m.isStart !== "boolean") return { reason: "CAP_FACE ohne isStart-Flag" };
    return { kind: "cap", opFeatureId: op.featureId, isStart: m.isStart };
  }
  if (qt === "SWEPT_FACE") {
    const origins = sketchOriginals(m);
    if (origins.length !== 1) {
      return {
        reason: `SWEPT_FACE mit ${origins.length} Herkunfts-Skizzenelementen (genau eines erwartet)`,
      };
    }
    return { kind: "wall", opFeatureId: op.featureId, origin: origins[0] };
  }
  if (qt === "OFFSET_FACE") return { kind: "innerWall", opFeatureId: op.featureId };
  return { reason: `Flächenform ${qt}` };
}

/**
 * Translate a parsed fillet/chamfer edge query into the face pair FieldDraft
 * names it by.
 *
 * The discriminator is the ROOT `queryType` — not the set of markers anywhere in
 * the document, which was the earlier rule and is provably wrong: a query's
 * disambiguation nests the FULL queries of the entities it descends from, so a
 * boolean-derived edge carries `CAP_FACE`, `SWEPT_EDGE` and `SKETCH_ENTITY`
 * markers that belong to its ancestors, not to itself.
 */
export function extractEdgeRef(doc: BtonValue): OnshapeEdgeRef {
  const root = asMap(doc);
  if (!root) return { kind: "unsupported", reason: "Kanten-Query ist kein BTON-Objekt" };
  const qt = queryTypeOf(root.queryType);
  if (!qt) {
    return { kind: "unsupported", reason: "queryType der Kante nicht dekodierbar" };
  }
  const op = opRefOfValue(root.operationId);
  const unsupported = (reason: string): OnshapeEdgeRef => ({ kind: "unsupported", reason });

  // A LATERAL edge of a sweep: where the walls swept from two neighbouring
  // sketch entities meet. Onshape names it by those entities, FieldDraft by the
  // two walls — the same edge, said the same way.
  if (qt === "SWEPT_EDGE") {
    if (!op) return unsupported("SWEPT_EDGE ohne lesbare Operations-Id");
    const origins = sketchOriginals(root);
    if (origins.length !== 2) {
      return unsupported(
        `SWEPT_EDGE aus ${origins.length} Skizzenelementen — eine Kante ist der Schnitt von GENAU zwei Wänden`,
      );
    }
    return {
      kind: "faces",
      a: { kind: "wall", opFeatureId: op.featureId, origin: origins[0] },
      b: { kind: "wall", opFeatureId: op.featureId, origin: origins[1] },
    };
  }

  // A cap RIM, named as an edge in its own right: the cap meets the wall swept
  // from the one sketch entity the edge descends from.
  if (qt === "CAP_EDGE") {
    if (!op) return unsupported("CAP_EDGE ohne lesbare Operations-Id");
    if (typeof root.isStart !== "boolean") return unsupported("CAP_EDGE ohne isStart-Flag");
    const origins = sketchOriginals(root);
    if (origins.length !== 1) {
      return unsupported(
        `CAP_EDGE aus ${origins.length} Skizzenelementen — welches davon die Wand trägt, sagt die Query nicht`,
      );
    }
    return {
      kind: "faces",
      a: { kind: "cap", opFeatureId: op.featureId, isStart: root.isStart },
      b: { kind: "wall", opFeatureId: op.featureId, origin: origins[0] },
    };
  }

  // The same rim, named as the INTERSECTION of its two faces (what Onshape
  // emits when the user rolled the edge up from faces rather than clicking it).
  if (qt === "INTERSECT") {
    const df = Array.isArray(root.derivedFrom) ? root.derivedFrom : [];
    if (df.length !== 2) {
      return unsupported(`INTERSECT-Kante aus ${df.length} Flächen (genau zwei erwartet)`);
    }
    const a = faceRefOf(df[0]);
    const b = faceRefOf(df[1]);
    if ("reason" in a) return unsupported(`INTERSECT-Kante: ${a.reason}`);
    if ("reason" in b) return unsupported(`INTERSECT-Kante: ${b.reason}`);
    return { kind: "faces", a, b };
  }

  // A SHELLED body's inner rim: the hollow's floor meets its side. Both faces
  // belong to the SWEEP — an inner face keeps the outer one's provenance; the
  // shell only hollowed it — so the sweep comes out of the TRUE_DEPENDENCY the
  // offset edge carries, not out of the shell op the root names.
  if (qt === "OFFSET_EDGE") {
    const dd = Array.isArray(root.disambiguationData) ? root.disambiguationData : [];
    for (const entry of dd) {
      const em = asMap(entry);
      if (!em || asText(em.disambiguationType) !== "TRUE_DEPENDENCY") continue;
      const df = Array.isArray(em.derivedFrom) ? em.derivedFrom : [];
      for (const d of df) {
        const dm = asMap(d);
        const dop = dm ? opRefOfValue(dm.operationId) : null;
        if (!dm || !dop || asText(dm.queryType) !== "CAP_EDGE") continue;
        if (typeof dm.isStart !== "boolean") continue;
        return {
          kind: "faces",
          a: { kind: "innerCap", opFeatureId: dop.featureId, isStart: dm.isStart },
          b: { kind: "innerWall", opFeatureId: dop.featureId },
        };
      }
    }
    return unsupported("OFFSET_EDGE ohne den geschalten Sweep (TRUE_DEPENDENCY)");
  }

  // Everything else is a topological history FieldDraft has no name for. Say
  // WHICH one — „Kantenform X" is a gap-matrix line, „nicht übersetzbar" is not.
  const named: Record<string, string> = {
    MERGE: "aus einer Boolean-Vereinigung hervorgegangene Kante (MERGE)",
    COPY: "Kante einer Muster-Kopie (COPY)",
    SPLIT: "Bruchstück einer von einer Boolean geteilten Kante (SPLIT)",
    BLEND_EDGE: "Kante einer bereits verrundeten/gefasten Fläche (BLEND_EDGE)",
    BLEND_FACE: "Kante an einer Verrundungsfläche (BLEND_FACE)",
    CAP_FACE: "ganze Deckelfläche statt einer Kante (CAP_FACE)",
  };
  return unsupported(
    `${named[qt] ?? `Onshape-Kantenform ${qt}`} — FieldDraft benennt Kanten über zwei Flächen und hat dafür kein Gegenstück`,
  );
}

/** Decode + tokenize + parse + extract an EDGE query in one step. */
export async function decodeEdgeQuery(queryString: string): Promise<OnshapeEdgeRef> {
  try {
    const doc = await parseQueryDocument(queryString);
    if (doc === null) return { kind: "unsupported", reason: "keine qCompressed-Query" };
    return extractEdgeRef(doc);
  } catch (e) {
    return {
      kind: "unsupported",
      reason: `Kante nicht dekodierbar (${e instanceof Error ? e.message : String(e)})`,
    };
  }
}

/** Convenience: decode + tokenize + extract in one step. Tokenizer or
 *  decode failures come back as `unsupported` with the error text. */
export async function decodePlaneQuery(queryString: string): Promise<OnshapePlaneRef> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(queryString);
    if (!bton) {
      // An uncompressed FeatureScript query. The one that matters in the corpus
      // is a sketch placed on an implicit MATE CONNECTOR (a sub-feature of the
      // sketch itself, #416) — name it, because „not a qCompressed query" tells
      // a user nothing about which Onshape concept is missing.
      if (/BodyType\.MATE_CONNECTOR/.test(queryString)) {
        return {
          kind: "unsupported",
          reason:
            "Sketch liegt auf einem Mate-Connector — FieldDraft hat dafür kein Gegenstück " +
            "(Lücke „mateConnector“, siehe docs/ONSHAPE-IMPORT-GAP-MATRIX.md)",
        };
      }
      return { kind: "unsupported", reason: "not a qCompressed query" };
    }
    return extractPlaneRef(tokenizeBton(payload));
  } catch (e) {
    return { kind: "unsupported", reason: e instanceof Error ? e.message : String(e) };
  }
}
