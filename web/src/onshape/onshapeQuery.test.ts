import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { decodePlaneQuery, decodeOnshapeQueryString, tokenizeBton } from "./onshapeQuery";

/** Load the live-captured Kabelbox feature JSON (the corpus every
 *  extraction rule was calibrated against). */
function kabelbox(): any[] {
  const raw = readFileSync(
    join(__dirname, "fixtures", "kabelbox-features.json"),
    "utf8",
  );
  return (JSON.parse(raw).features ?? []).map((f: any) => f.message ?? f);
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
