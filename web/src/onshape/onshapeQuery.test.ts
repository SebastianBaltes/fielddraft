import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { decodePlaneQuery, decodeOnshapeQueryString, tokenizeBton } from "./onshapeQuery";

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

describe("tokenizeBton — unknown-but-well-formed tokens (Ticket #339)", () => {
  it("consumes the E<hex> back-reference instead of aborting the decode", () => {
    // A real document used `E<hex>` where a VALUE was expected. The tokenizer
    // threw on it, which killed the WHOLE query — every fillet edge of that
    // document failed to decode. A token we merely do not interpret must not
    // be a hard stop.
    const toks = tokenizeBton("%Sc$instanceNameE22R9S9$queryTypeS4$FACE");
    expect(() => toks).not.toThrow();
    const literals = toks.filter((t) => t.t === "str").map((t) => t.p1);
    expect(literals).toContain("instanceName");
    expect(literals).toContain("FACE"); // the scan survived past the E token
  });

  it("still rejects genuinely unknown syntax — visibly", () => {
    expect(() => tokenizeBton("%Z9$nope")).toThrow();
  });
});
