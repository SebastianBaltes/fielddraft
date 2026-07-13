/**
 * Onshape query-string decoder (Onshape-Import Schritt 4).
 *
 * Onshape serialises its declarative geometry queries into the
 * `queryString` field of `BTMIndividualQuery` parameters:
 *
 *   query=qCompressed(1.0,"<payload>",id);
 *
 * The payload is Onshape's BTON dictionary text — either plain
 * (`%B5$Query…`) or zlib/base64 (`&<hexlen>$eJ…`). We tokenize it and
 * extract the fields the sketch-plane translation needs. The grammar is
 * only partially reverse-engineered (string back-references stay
 * opaque), but every DISCRIMINATING field of the plane queries appears
 * as a literal on first use — validated against the Kabelbox corpus in
 * `fixtures/`. Anything that doesn't match a known shape comes back as
 * `{kind: "unsupported"}` so the import dialog can fail VISIBLY — never
 * silently fall back to XY.
 *
 * Token vocabulary (empirical):
 *   `M<hex>` map · `A<hex>` array · `C<hex>` class back-ref (0-based)
 *   `S<hex>$s` string · `S<h1>.<h2>$s1s2` two-part string
 *   `S-<ref>.<h>$suffix` ref+suffix string (ref left unresolved)
 *   `B<hex>$Name` class/enum introduction · `R<hex>`/`E<hex>` back-refs
 *   `D-?<hex>` number · `T`/`F` booleans · `%` root marker
 */

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
      let ref: number | undefined;
      for (const len of lens) {
        if (len < 0) {
          ref = -len;
          continue;
        }
        parts.push(payload.slice(i, i + len));
        i += len;
      }
      if (ref !== undefined) {
        out.push({ t: "refstr", n: ref, p1: parts.join(".") });
      } else {
        out.push({
          t: c === "B" ? "enum" : "str",
          p1: parts[0] ?? "",
          p2: parts.slice(1).join("."),
        });
      }
      continue;
    }
    if (c === "R" || c === "E") {
      // `R<hex>` is a string back-reference. `E<hex>` appears where a VALUE
      // is expected and is likewise a back-reference (structure sharing) —
      // we do not resolve either, but a well-formed token we happen not to
      // interpret must never abort the decode: throwing here killed every
      // edge query of a document that used it.
      i += 1;
      out.push({ t: "ref", n: hex() });
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

  // Operation refs + sketch-entity origins, in stream order. A repeated
  // operation suffix travels as an opaque back-ref (`S11.-e$F4Ns…`), so a
  // BARE feature-id literal also counts as a wire marker for origin
  // association — in the corpus it always directly precedes its
  // sketch-entity id.
  const ops: { idx: number; op: OnshapeOpRef }[] = [];
  const wires: { idx: number; featureId: string }[] = [];
  const origins: { idx: number; origin: OnshapeEntityOrigin }[] = [];
  tokens.forEach((t, idx) => {
    const op = opRefOf(t);
    if (op) {
      ops.push({ idx, op });
      if (op.op === "wireOp") wires.push({ idx, featureId: op.featureId });
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
      return;
    }
    // A sketch-entity id is a two-part string following its wire marker.
    if (t.t === "str" && t.p1 && t.p2) {
      const wire = [...wires].reverse().find((w) => w.idx < idx);
      if (wire) {
        origins.push({
          idx,
          origin: {
            sketchFeatureId: wire.featureId,
            entityId: `${t.p1}.${t.p2}`,
          },
        });
      }
    }
  });

  const opsBy = (name: string) => ops.filter((o) => o.op.op === name);
  const extrudes = opsBy("opExtrude");
  const fillets = opsBy("opFillet");

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

  // SWEPT_FACE: wall of an extrude from a sketch entity; optionally
  // disambiguated by BLEND_FACE adjacency and a boolean SPLIT peer.
  if (has("SWEPT_FACE")) {
    if (extrudes.length === 0 || origins.length === 0) {
      return { kind: "unsupported", reason: "SWEPT_FACE without extrude op / origin entity" };
    }
    const primary = origins[0].origin;
    const out: OnshapePlaneRef = {
      kind: "swept",
      opFeatureId: extrudes[0].op.featureId,
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
          extrudes[0].op.featureId,
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

/** Convenience: decode + tokenize + extract in one step. Tokenizer or
 *  decode failures come back as `unsupported` with the error text. */
export async function decodePlaneQuery(queryString: string): Promise<OnshapePlaneRef> {
  try {
    const { bton, payload } = await decodeOnshapeQueryString(queryString);
    if (!bton) return { kind: "unsupported", reason: "not a qCompressed query" };
    return extractPlaneRef(tokenizeBton(payload));
  } catch (e) {
    return { kind: "unsupported", reason: e instanceof Error ? e.message : String(e) };
  }
}
