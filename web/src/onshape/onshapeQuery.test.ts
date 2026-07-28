import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  decodeEdgeQuery,
  decodePlaneQuery,
  decodeOnshapeQueryString,
  parseBton,
  parseQueryDocument,
  tokenizeBton,
  type OnshapeEdgeRef,
} from "./onshapeQuery";

/** Load a live-captured feature JSON, unwrapped to the feature messages. */
function fixtureFeatures(file: string): any[] {
  const raw = readFileSync(join(__dirname, "fixtures", file), "utf8");
  return (JSON.parse(raw).features ?? []).map((f: any) => f.message ?? f);
}

/** The Kabelbox corpus — the one every extraction rule was calibrated against. */
function kabelbox(): any[] {
  return fixtureFeatures("kabelbox-features.json");
}

function planeQueryOf(features: any[], name: string): string {
  const feat = features.find((m) => m.name === name);
  expect(feat, `feature ${name}`).toBeTruthy();
  const param = feat.parameters
    .map((p: any) => p.message ?? p)
    .find((p: any) => p.parameterId === "sketchPlane");
  const q = param.queries[0];
  return (q.message ?? q).queryString;
}

describe("decodePlaneQuery — Kabelbox corpus (live-captured)", () => {
  const feats = kabelbox();

  it("Sketch 1 + 3 sit on the default Top plane", async () => {
    for (const name of ["Sketch 1", "Sketch 3"]) {
      const ref = await decodePlaneQuery(planeQueryOf(feats, name));
      expect(ref).toEqual({ kind: "default", plane: "top" });
    }
  });

  it("Sketch 2 sits on Extrude 1's START cap", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 2"));
    expect(ref).toEqual({
      kind: "cap",
      opFeatureId: "Fz9Ll1Nhy8Iyyah_0",
      isStart: true,
    });
  });

  it("Sketch 6 sits on Extrude 1's END cap", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 6"));
    expect(ref).toEqual({
      kind: "cap",
      opFeatureId: "Fz9Ll1Nhy8Iyyah_0",
      isStart: false,
    });
  });

  it("Sketch 5 sits on the wall swept from Sketch 1's LEFT line", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 5"));
    expect(ref).toEqual({
      kind: "swept",
      opFeatureId: "Fz9Ll1Nhy8Iyyah_0",
      origin: {
        sketchFeatureId: "F6VjtplfD7vN1DM_0",
        entityId: "zTyZhgNVqaMx.left",
      },
    });
  });

  it("Sketch 4 disambiguates via BLEND_FACE adjacency + boolean SPLIT", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 4"));
    expect(ref.kind).toBe("swept");
    if (ref.kind !== "swept") return;
    expect(ref.opFeatureId).toBe("Fz9Ll1Nhy8Iyyah_0");
    expect(ref.origin).toEqual({
      sketchFeatureId: "F6VjtplfD7vN1DM_0",
      entityId: "zTyZhgNVqaMx.bottom",
    });
    expect(ref.blendFilletId).toBe("FH63Kd1qUroHFAm_1");
    expect(ref.splitPeer).toEqual({
      opFeatureId: "FFvPoOzuN3l2ZlB_1",
      origin: {
        sketchFeatureId: "F4NsMPM3ginTKFA_1",
        entityId: "CuIfANXbpd5x.left",
      },
    });
  });

  it("Sketch 8 mirrors Sketch 4's disambiguation shape", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 8"));
    expect(ref.kind).toBe("swept");
    if (ref.kind !== "swept") return;
    expect(ref.opFeatureId).toBe("Fz9Ll1Nhy8Iyyah_0");
    expect(ref.origin.entityId).toBe("zTyZhgNVqaMx.bottom");
    expect(ref.blendFilletId).toBe("FH63Kd1qUroHFAm_1");
  });

  it("every sketch plane in the corpus decodes to a supported shape", async () => {
    for (const feat of feats) {
      if (feat.featureType !== "newSketch") continue;
      const ref = await decodePlaneQuery(planeQueryOf(feats, feat.name));
      expect(ref.kind, `${feat.name}: ${JSON.stringify(ref)}`).not.toBe("unsupported");
    }
  });

  it("tokenizes every queryString in the corpus without errors", async () => {
    let count = 0;
    for (const feat of feats) {
      for (const p of feat.parameters ?? []) {
        const pm = p.message ?? p;
        for (const q of pm.queries ?? []) {
          const qs = (q.message ?? q).queryString;
          if (!qs) continue;
          const { bton, payload } = await decodeOnshapeQueryString(qs);
          if (!bton) continue; // qSketchRegion etc.
          expect(() => tokenizeBton(payload)).not.toThrow();
          count += 1;
        }
      }
    }
    expect(count).toBeGreaterThan(10);
  });
});

/**
 * The #419 document (a long rail assembly) broke two assumptions the Kabelbox
 * corpus never exercised, and both failed the same misleading way: the plane
 * came back "unsupported", which reads as "we don't understand this query" when
 * in truth the query was understood and one value fell out of the sieve.
 */
describe("decodePlaneQuery — #419 rail assembly", () => {
  const feats = fixtureFeatures("onshape-419-features.json");

  it("reads an entity id that carries NO dot suffix", async () => {
    // A sub-piece of a construction primitive is `<base>.<suffix>`
    // (`pozkE6lurT5A.right`); a plainly drawn line is just `pfTtMVvYeZsa`.
    // Keying off the dotted SHAPE dropped every undotted id on the floor.
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 4"));
    expect(ref).toEqual({
      kind: "swept",
      opFeatureId: "FDuA7djDoDV41hj_7",
      origin: { sketchFeatureId: "FLZMozrL9HQb6CN_2", entityId: "pfTtMVvYeZsa" },
    });
  });

  it("accepts the wall of a REVOLVE, not just of an extrude", async () => {
    // FieldDraft names a lateral wall `qWall(op, entity)` from the profile-edge
    // ordinal, and a revolve sweeps its mantle from the same profile edges.
    for (const name of ["Sketch 6", "Sketch 13"]) {
      const ref = await decodePlaneQuery(planeQueryOf(feats, name));
      expect(ref, name).toEqual({
        kind: "swept",
        opFeatureId: "FMmPnhPvNpoB96B_3",
        origin: { sketchFeatureId: "FLZMozrL9HQb6CN_2", entityId: "46y5P1bc7KXF" },
      });
    }
  });

  it("still reports the cap of a THICKEN as unsupported rather than guessing", async () => {
    // `capFaceQuery` needs the sweep DIRECTION to know which FieldDraft cap
    // Onshape's `isStart` names, and only an extrude records one. Guessing
    // would put the sketch on the far side of the wall — plausible and wrong.
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 17"));
    expect(ref.kind).toBe("unsupported");
  });
});

/**
 * #427 stood on the claim that the plane decoder picks the WRONG `SWEPT_FACE`
 * origin entity for the #409 shelf — that it names a wall Onshape does not mean.
 * It does not, and this is the measurement that says so: the entity it names is
 * checked against Onshape's OWN world geometry (the `sketches?output3D=true`
 * answer), which is the only source that can settle it. The wall Onshape means
 * is the one swept from an edge lying IN the plane the dependent sketch sits on.
 */
describe("decodePlaneQuery — #409 shelf, SWEPT_FACE origin (Ticket #427)", () => {
  const feats = fixtureFeatures("onshape-409-features.json");
  const three = (JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-409-sketches3d.json"), "utf8"),
  ).sketches ?? []) as any[];

  /** Every world point of a sketch entity, in mm (the payload reports metres). */
  const worldPointsMm = (featureId: string, entityId: string): number[][] => {
    const s = three.find((x) => x.featureId === featureId);
    const e = (s?.geomEntities ?? []).find((g: any) => g.id === entityId);
    const pts = [e?.point, e?.startPoint, e?.endPoint].filter(Boolean) as number[][];
    return pts.map((p) => p.map((v) => v * 1000));
  };

  it("Sketch 6 names the edge that sweeps into the plane it sits on", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 6"));
    expect(ref).toEqual({
      kind: "swept",
      opFeatureId: "Fki57tHha8xZm97_14", // Extrude 10
      origin: {
        sketchFeatureId: "FU9cdmO9DPaHZuT_14", // Sketch 11
        entityId: "OUhPEGp5z47e.bottom",
      },
    });
    if (ref.kind !== "swept") return;

    // The named edge runs at z = 965 …
    const edge = worldPointsMm(ref.origin.sketchFeatureId, ref.origin.entityId);
    expect(edge.length).toBeGreaterThan(0);
    for (const p of edge) expect(p[2]).toBeCloseTo(965, 6);
    // … and Sketch 6 itself lies at z = 965. Extruding that edge sweeps exactly
    // this plane: the decoder names the face Onshape names.
    const sketch6 = (three.find((x) => x.featureId === "FpQFfaAB8RCjfQh_9")?.geomEntities ?? [])
      .filter((g: any) => g.point)
      .map((g: any) => g.point[2] * 1000);
    expect(sketch6.length).toBeGreaterThan(0);
    for (const z of sketch6) expect(z).toBeCloseTo(965, 6);
  });

  it("Sketch 11 likewise names the edge in its own plane", async () => {
    const ref = await decodePlaneQuery(planeQueryOf(feats, "Sketch 11"));
    expect(ref).toEqual({
      kind: "swept",
      opFeatureId: "FBH24UYdTchhs9i_1", // Extrude 2
      origin: {
        sketchFeatureId: "Fmgukhe1CDBKQR7_1", // Sketch 2
        entityId: "qn3WXLQ0bKS1.bottom",
      },
    });
    if (ref.kind !== "swept") return;
    // y = −550 in Onshape's world — the plane Sketch 11 sits on.
    for (const p of worldPointsMm(ref.origin.sketchFeatureId, ref.origin.entityId)) {
      expect(p[1]).toBeCloseTo(-550, 6);
    }
    for (const g of three.find((x) => x.featureId === "FU9cdmO9DPaHZuT_14")?.geomEntities ?? []) {
      if (g.point) expect(g.point[1] * 1000).toBeCloseTo(-550, 6);
    }
  });
});

describe("decodePlaneQuery — failure visibility", () => {
  it("unknown wrappers come back unsupported, never a silent default", async () => {
    const ref = await decodePlaneQuery('query = qEverything(id + "X");');
    expect(ref.kind).toBe("unsupported");
  });

  it("garbage payloads surface the tokenizer error", async () => {
    const ref = await decodePlaneQuery('query=qCompressed(1.0,"%Z9$nope",id);');
    expect(ref.kind).toBe("unsupported");
  });
});

/** The `entities` query strings of one fillet/chamfer feature, in order. */
function edgeQueriesOf(features: any[], name: string): string[] {
  const feat = features.find((m) => m.name === name);
  expect(feat, `feature ${name}`).toBeTruthy();
  const param = feat.parameters
    .map((p: any) => p.message ?? p)
    .find((p: any) => p.parameterId === "entities");
  return (param.queries ?? []).map((q: any) => (q.message ?? q).queryString as string);
}

/**
 * The BTON back-reference tables — the piece of the format that had to be
 * reverse-engineered, and the piece that decides whether a value is read at its
 * KEY or at a position.
 *
 * A payload builds up TWO tables as it is read, and they hold different things:
 * the string POOL (every string spelled out, class names and joined composites
 * included), indexed by `C<n>`, `E<n>` and the negative part-lengths inside a
 * string — and the VALUE table (every string, number, map and array the
 * document produces, appended when COMPLETE), indexed by `R<n>`. Get either
 * wrong by one and the decode still "works": it just returns a neighbouring
 * entry, which is how a map key comes out as `bottom` and a query type as
 * `opRevolve`.
 */
describe("parseBton — the two back-reference tables", () => {
  // #409 „Chamfer 1", verbatim: the smallest real SWEPT_EDGE in the corpus.
  const CHAMFER_1_409 =
    "%B5$QueryM5S12$disambiguationDataA1M2S12$disambiguationTypeS13$ORIGINAL_DEPENDENCY" +
    "S9$originalsA2C0M5Sa$entityTypeBa$EntityTypeS4$EDGESb$historyTypeS8$CREATION" +
    "Sb$operationIdB2$IdA1S12.6$Fu6o1ZFH8h4gKml_18wireOpS9$queryTypeSd$SKETCH_ENTITY" +
    "Se$sketchEntityIdSc.6$dE9VSfqolyPgbottomC0M5R4R5R6R7R8RaRbRcRdSc$exVl27xLp2IA" +
    "R4R5R6R7R8CbA1S12.9$FJaU8k5c579SOul_18opExtrudeRbSa$SWEPT_EDGE";

  it("replays a compressed map with its FIELD NAMES, not with neighbours", () => {
    // The second `originals` entry is written as nine back-references plus one
    // literal. Only the literal (`exVl27xLp2IA`) is spelled out — everything
    // else, INCLUDING the key `sketchEntityId`, comes out of the table.
    //
    // `Ra` in that entry is the value slot of `operationId`, and it is the
    // sharpest test the corpus offers of what `R<n>` names: read as a string
    // table it lands on the PART `wireOp` (index 10 of the literal parts), read
    // as the value table on the whole `Id[["Fu6o1ZFH8h4gKml_18.wireOp"]]`
    // (value 10 — the composite counts once, the array closes right after it).
    // Both are well-formed; only the second says which sketch the entry means.
    expect(parseBton(tokenizeBton(CHAMFER_1_409))).toEqual({
      disambiguationData: [
        {
          disambiguationType: "ORIGINAL_DEPENDENCY",
          originals: [
            {
              entityType: "EDGE",
              historyType: "CREATION",
              operationId: ["Fu6o1ZFH8h4gKml_18.wireOp"],
              queryType: "SKETCH_ENTITY",
              sketchEntityId: "dE9VSfqolyPg.bottom",
            },
            {
              entityType: "EDGE",
              historyType: "CREATION",
              // The replayed operation id is the SAME `Id` value as above, so
              // this entry names its sketch outright. It used to come out as
              // the bare suffix `wireOp`, and the sketch had to be inherited
              // from the entry before it.
              operationId: ["Fu6o1ZFH8h4gKml_18.wireOp"],
              queryType: "SKETCH_ENTITY",
              sketchEntityId: "exVl27xLp2IA",
            },
          ],
        },
      ],
      entityType: "EDGE",
      historyType: "CREATION",
      operationId: ["FJaU8k5c579SOul_18.opExtrude"],
      queryType: "SWEPT_EDGE",
    });
  });

  /**
   * #409 „Extrude 5", `booleanScope`, verbatim: a pattern COPY of a pattern
   * COPY. It is the payload that settled #430 — the shortest one whose
   * back-references simply do not exist in any string table.
   *
   * `R1a` (26) stands in TWO `entityType` value slots. The literal parts of
   * this payload run out at 30 entries and put `BODY` at 20, so as a string
   * table the reference is either out of range or points at the feature id
   * `FzRCW3jQWfL6H0k_9`. As the VALUE table it is exactly `BODY`: the four
   * `originals` maps, the two arrays and the two enclosing maps that closed in
   * between each took a slot. Same for `R1f`/`R20` (`instanceName` and its
   * value) and `R23` (`COPY`).
   */
  const EXTRUDE_5_409 =
    "%B5$QueryM6Sb$derivedFromC0M6R0C0M5S12$disambiguationDataA1M2S12$disambiguationTypeS13$O" +
    "RIGINAL_DEPENDENCYS9$originalsA4C0M5Sa$entityTypeBa$EntityTypeS4$EDGESb$historyTypeS8$CR" +
    "EATIONSb$operationIdB2$IdA1S11.6$FhBUvJiYcqBpE1M_0wireOpS9$queryTypeSd$SKETCH_ENTITYSe$s" +
    "ketchEntityIdSc.6$ytjnFzrj1CKNbottomC0M5R5R6R7R8R9RbRcRdReS-13.3$topC0M5R5R6R7R8R9RbRcRd" +
    "ReS-13.4$leftC0M5R5R6R7R8R9RbRcRdReS-13.5$rightR5C7S4$BODYR7R8R9CcA1S11.9$Fiz0Mmct8ouL6F" +
    "G_0opExtrudeRcSa$SWEPT_BODYR5R1aR7R8Sc$instanceNameS1$1R9CcA1S11.9$FzRCW3jQWfL6H0k_9opPa" +
    "tternRcS4$COPYR5R1aR7R8R1fR20R9CcA1S11.-24$F3PepExHlLDpHaC_9RcR23";

  it("shares a value the string tables never held (#430)", () => {
    const doc: any = parseBton(tokenizeBton(EXTRUDE_5_409));
    // Both COPY levels and the swept body they descend from say `BODY` — the
    // reference the old string-table reading could not resolve at all.
    expect(doc.entityType).toBe("BODY");
    expect(doc.derivedFrom.entityType).toBe("BODY");
    expect(doc.derivedFrom.derivedFrom.entityType).toBe("BODY");
    // The outer instance repeats the inner one's `instanceName` key AND value.
    expect(doc.instanceName).toBe("1");
    expect(doc.derivedFrom.instanceName).toBe("1");
    // The pool reference `S11.-24$…` splices the earlier `opPattern` onto a new
    // feature id — the pool and the value table indexing the same payload with
    // different numbers is the whole point.
    expect(doc.operationId).toEqual(["F3PepExHlLDpHaC_9.opPattern"]);
    expect(doc.derivedFrom.operationId).toEqual(["FzRCW3jQWfL6H0k_9.opPattern"]);
    expect(doc.queryType).toBe("COPY");
    expect(doc.derivedFrom.queryType).toBe("COPY");
    expect(doc.derivedFrom.derivedFrom.queryType).toBe("SWEPT_BODY");
    // Four sketch edges, each named in full — the last three are nine
    // back-references and a suffix apiece.
    expect(
      doc.derivedFrom.derivedFrom.disambiguationData[0].originals.map(
        (o: any) => o.sketchEntityId,
      ),
    ).toEqual([
      "ytjnFzrj1CKN.bottom",
      "ytjnFzrj1CKN.top",
      "ytjnFzrj1CKN.left",
      "ytjnFzrj1CKN.right",
    ]);
  });

  it("reads `E<n>` out of the string POOL, not the value table (#430)", async () => {
    // Eight payloads in the corpus write the `instanceName` value as `E<hex>`.
    // It is the only token that indexes the pool from a VALUE position: here
    // `E22` is pool entry 0x22, the `1` of the pattern instance. Against the
    // value table the same index is an array, and an array in a string slot is
    // how a decode starts inventing shapes.
    const doc: any = await parseQueryDocument(
      edgeQueriesOf(fixtureFeatures("ticket339-features.json"), "Fillet 1")[1],
    );
    expect(doc.derivedFrom[0].derivedFrom.instanceName).toBe("1");
    expect(doc.derivedFrom[0].derivedFrom.queryType).toBe("COPY");
  });

  it("resolves a string that is written as `<table entry> + suffix`", async () => {
    // #419 „Fillet 15": the second entity travels as `S-13.5$right` — a
    // reference to `pozkE6lurT5A` plus the literal `right`. Reading that ref
    // one slot off yields `bottom.right`, which looks like an entity id and is
    // not one.
    const feats = fixtureFeatures("onshape-419-features.json");
    const doc: any = await parseQueryDocument(edgeQueriesOf(feats, "Fillet 15")[1]);
    const ids = doc.disambiguationData[0].originals.map((o: any) => o.sketchEntityId);
    expect(ids).toEqual(["RRAmtdnwAfCb.bottom", "RRAmtdnwAfCb.left"]);
  });

  it("parses every fillet/chamfer edge query of the corpus", async () => {
    // 206 payloads over eight fixtures. The parse is the floor the translation
    // stands on: it may report a shape as unsupported, it may not fall over.
    let n = 0;
    for (const file of [
      "onshape-419-features.json",
      "onshape-409-features.json",
      "onshape-414-features.json",
      "onshape-415-features.json",
      "onshape-416-features.json",
      "kabelbox-features.json",
      "ticket339-features.json",
    ]) {
      for (const feat of fixtureFeatures(file)) {
        if (feat.featureType !== "fillet" && feat.featureType !== "chamfer") continue;
        for (const qs of edgeQueriesOf(fixtureFeatures(file), feat.name)) {
          const doc = await parseQueryDocument(qs);
          expect(doc, `${file} ${feat.name}`).toBeTruthy();
          n += 1;
        }
      }
    }
    expect(n).toBe(206);
  });

  /**
   * The whole-corpus gate (#430). Every BTON query of every fixture — sketch
   * planes, boolean scopes, fillet edges, extrude boundaries, the lot — is
   * decoded and held against the hard constraint the tables were calibrated on:
   *
   *  - no map key may be synthetic. The parser gives a key it cannot name a
   *    `?<n>` (which no Onshape field can collide with), so a single drifted
   *    index shows up here as a failure instead of as a plausible wrong shape.
   *  - no value may be an unresolved back-reference.
   *  - every key must be a field name Onshape actually uses.
   *
   * Before #430 this was 656 of 697 payloads. A regression drops the number,
   * and this test says by how much and where.
   */
  it("decodes every BTON query of every fixture — all 697", async () => {
    const FIELDS = new Set([
      "blendedFrom", "blendedInto", "derivedFrom", "disambiguationData",
      "disambiguationType", "entities", "entityType", "faceName", "fromTools",
      "historyType", "instanceName", "isStart", "operationId", "order",
      "originals", "owners", "queryType", "side", "sketchEntityId",
    ]);
    const bad: string[] = [];
    const check = (v: any, where: string) => {
      if (Array.isArray(v)) return v.forEach((x, i) => check(x, `${where}[${i}]`));
      if (typeof v !== "object" || v === null) return;
      if ("unresolvedRef" in v) return void bad.push(`${where}: unresolved ${v.unresolvedRef}`);
      for (const [k, val] of Object.entries(v)) {
        if (!FIELDS.has(k)) bad.push(`${where}.${k}: not an Onshape field name`);
        check(val, `${where}.${k}`);
      }
    };
    let n = 0;
    for (const file of [
      "onshape-409-features.json", "onshape-412-features.json",
      "onshape-414-features.json", "onshape-415-features.json",
      "onshape-416-features.json", "onshape-419-features.json",
      "kabelbox-features.json", "ticket339-features.json",
    ]) {
      for (const feat of fixtureFeatures(file)) {
        for (const p of feat.parameters ?? []) {
          for (const q of (p.message ?? p).queries ?? []) {
            const qs = (q.message ?? q).queryString;
            if (!qs) continue;
            const doc = await parseQueryDocument(qs);
            if (doc === null) continue; // qSketchRegion & co. — not BTON
            n += 1;
            check(doc, `${file} ${feat.name} ${(p.message ?? p).parameterId}`);
          }
        }
      }
    }
    expect(n).toBe(697);
    expect(bad.slice(0, 12)).toEqual([]);
  });
});

/**
 * Ticket #425 — the EDGE shapes of `fillet` and `chamfer`.
 *
 * FieldDraft addresses an edge as `qCommonEdge(faceA, faceB)`, so a shape is
 * either lowered to a pair of faces or REFUSED by name. The refusals matter as
 * much as the translations: the decoder this replaced discriminated on the set
 * of markers found ANYWHERE in the payload, and a query's disambiguation nests
 * the full queries of its ancestors — so a boolean-derived edge carried
 * `CAP_FACE` and `SWEPT_FACE` markers belonging to faces that are not its own,
 * and got translated into an edge of the pattern ORIGINAL. Two different #414
 * fillets came out with the identical query that way.
 */
describe("decodeEdgeQuery — Ticket #425", () => {
  const faceKinds = (ref: OnshapeEdgeRef) =>
    ref.kind === "faces" ? [ref.a.kind, ref.b.kind] : [ref.kind];

  it("#409 Chamfer 1: SWEPT_EDGE = the corner where two swept walls meet", async () => {
    const feats = fixtureFeatures("onshape-409-features.json");
    const ref = await decodeEdgeQuery(edgeQueriesOf(feats, "Chamfer 1")[0]);
    expect(ref).toEqual({
      kind: "faces",
      a: {
        kind: "wall",
        opFeatureId: "FJaU8k5c579SOul_18",
        origin: { sketchFeatureId: "Fu6o1ZFH8h4gKml_18", entityId: "dE9VSfqolyPg.bottom" },
      },
      b: {
        kind: "wall",
        opFeatureId: "FJaU8k5c579SOul_18",
        // The sketch is INHERITED from the entry before it (the operation id was
        // replayed as a back-reference) — and it is checked, not assumed: the
        // importer looks this id up in exactly that sketch's entity table.
        origin: { sketchFeatureId: "Fu6o1ZFH8h4gKml_18", entityId: "exVl27xLp2IA" },
      },
    });
  });

  it("#419 Chamfer 11: both SWEPT_EDGEs name their own pair of walls", async () => {
    const feats = fixtureFeatures("onshape-419-features.json");
    const ids = [];
    for (const qs of edgeQueriesOf(feats, "Chamfer 11")) {
      const ref = await decodeEdgeQuery(qs);
      expect(faceKinds(ref)).toEqual(["wall", "wall"]);
      if (ref.kind !== "faces" || ref.a.kind !== "wall" || ref.b.kind !== "wall") return;
      ids.push([ref.a.origin.entityId, ref.b.origin.entityId]);
    }
    // Two DIFFERENT edges — the old decoder collapsed sibling edges into one.
    expect(ids).toEqual([
      ["D3GaT2WoTJJE", "DpzrxFGDJsRl"],
      ["DpzrxFGDJsRl", "I2Ckp3kppzV1"],
    ]);
  });

  it("#419 Chamfer 5: CAP_EDGE = the cap against the wall it descends from", async () => {
    const feats = fixtureFeatures("onshape-419-features.json");
    const ref = await decodeEdgeQuery(edgeQueriesOf(feats, "Chamfer 5")[0]);
    expect(ref).toEqual({
      kind: "faces",
      a: { kind: "cap", opFeatureId: "FuEM5hJm4ZF6lHu_11", isStart: false },
      b: {
        kind: "wall",
        opFeatureId: "FuEM5hJm4ZF6lHu_11",
        origin: { sketchFeatureId: "F2ZztNtd6WHnwmB_11", entityId: "mwr4t9YMihpG.bottom" },
      },
    });
  });

  it("Kabelbox Fillet 1: OFFSET_EDGE = the SHELLED body's inner rim", async () => {
    // The shell op only hollowed the body; both faces still belong to the
    // sweep, which the offset edge names through its TRUE_DEPENDENCY.
    const ref = await decodeEdgeQuery(edgeQueriesOf(kabelbox(), "Fillet 1")[0]);
    expect(ref).toEqual({
      kind: "faces",
      a: { kind: "innerCap", opFeatureId: "Fz9Ll1Nhy8Iyyah_0", isStart: true },
      b: { kind: "innerWall", opFeatureId: "Fz9Ll1Nhy8Iyyah_0" },
    });
  });

  it("#414 Fillet 3: INTERSECT names its two faces — across TWO sweeps", async () => {
    // The wall belongs to one extrude and the cap to another. The marker-based
    // decoder used the first operation it found for BOTH.
    const ref = await decodeEdgeQuery(edgeQueriesOf(fixtureFeatures("onshape-414-features.json"), "Fillet 3")[0]);
    expect(ref.kind).toBe("faces");
    if (ref.kind !== "faces") return;
    expect(ref.a).toEqual({
      kind: "wall",
      opFeatureId: "FdtzO9AArGbv004_1",
      origin: { sketchFeatureId: "FGnfhqBaFuIRIZp_0", entityId: "F2cYeD5tmibR" },
    });
    expect(ref.b).toEqual({ kind: "cap", opFeatureId: "F3WyOgYA1MVUHhI_1", isStart: true });
  });

  it("refuses the shapes with no FieldDraft counterpart, by name", async () => {
    const f419 = fixtureFeatures("onshape-419-features.json");
    const cases: [string, string, number, RegExp][] = [
      // A boolean merged two edges into one — which of them FieldDraft should
      // name is not a question the query answers.
      ["MERGE", "Fillet 5", 0, /MERGE/],
      // The edge of a pattern INSTANCE; FieldDraft has no name for one.
      ["COPY", "Fillet 6", 0, /COPY/],
      // A fragment of an edge a boolean cut in two.
      ["SPLIT", "Fillet 14", 0, /SPLIT/],
      // An edge of an EARLIER chamfer's blend face.
      ["BLEND_EDGE", "Fillet 12", 0, /BLEND_EDGE/],
      // Three profile curves meet there — an edge is the seam of exactly two
      // faces, so which two is not determined.
      ["SWEPT_EDGE ×3", "Fillet 7", 4, /SWEPT_EDGE aus 3/],
    ];
    for (const [label, feat, index, pattern] of cases) {
      const ref = await decodeEdgeQuery(edgeQueriesOf(f419, feat)[index]);
      expect(ref.kind, `${label} (${feat})`).toBe("unsupported");
      if (ref.kind !== "unsupported") return;
      expect(ref.reason, `${label} (${feat})`).toMatch(pattern);
    }
  });

  it("never reports a drifted string as if it were an Onshape shape", async () => {
    // Where a payload shares a whole sub-structure, our string table falls
    // behind Onshape's and a `queryType` slot reads a neighbouring value
    // (`bottom`, `opRevolve`, `1`). Those must come back as UNDECODED, not as
    // „Flächenform bottom" — that would blame Onshape for our own gap.
    const f419 = fixtureFeatures("onshape-419-features.json");
    const reasons: string[] = [];
    for (const feat of f419) {
      if (feat.featureType !== "fillet" && feat.featureType !== "chamfer") continue;
      for (const qs of edgeQueriesOf(f419, feat.name)) {
        const ref = await decodeEdgeQuery(qs);
        if (ref.kind === "unsupported") reasons.push(ref.reason);
      }
    }
    expect(reasons.length).toBeGreaterThan(0);
    for (const r of reasons) {
      const shape = /Flächenform (\S+)/.exec(r)?.[1] ?? /Onshape-Kantenform (\S+)/.exec(r)?.[1];
      if (shape) expect(shape, r).toMatch(/^[A-Z][A-Z_0-9]*$/);
    }
  });
});

describe("tokenizeBton — the E<hex> token (Tickets #339, #430)", () => {
  it("keeps E apart from R — they index DIFFERENT tables", () => {
    // A real document used `E<hex>` where a VALUE was expected. The tokenizer
    // threw on it, which killed the WHOLE query (#339); then it read it as a
    // string back-reference, which silently produced the wrong string (#430).
    // It is a STRING-POOL reference, and only its own token type says so.
    const toks = tokenizeBton("%Sc$instanceNameE22R9S9$queryTypeS4$FACE");
    expect(toks.filter((t) => t.t === "poolref").map((t) => t.n)).toEqual([0x22]);
    expect(toks.filter((t) => t.t === "ref").map((t) => t.n)).toEqual([9]);
  });

  it("still rejects genuinely unknown syntax — visibly", () => {
    expect(() => tokenizeBton("%Z9$nope")).toThrow();
  });
});
