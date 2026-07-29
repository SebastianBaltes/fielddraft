import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  anchorT,
  decodeMateAnchorQuery,
  describeAnchor,
  implicitMateConnectors,
  mateConnectorPlaneId,
  readMateConnector,
} from "./onshapeMateConnector";

/** The #416 document (Onshape `204cc669…`): three of its five sketches sit on
 *  IMPLICIT mate connectors, carried inside the sketch features themselves. */
function ticket416(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-416-features.json"), "utf8"),
  );
}

/**
 * Onshape's OWN answer for where those sketches lie:
 * `GET /partstudios/…/sketches?includeGeometry=true&output3D=true`, which
 * reports every sketch entity in WORLD coordinates. This is the ground truth
 * the frames below are measured against — our import is never its own witness
 * (the methodology lesson of #412).
 */
function sketches3d(): { sketches: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "onshape-416-sketches3d.json"), "utf8"),
  );
}

const feature = (doc: { features: any[] }, id: string): any =>
  doc.features.map((f: any) => f.message ?? f).find((f: any) => f.featureId === id);

const planeQueryString = (feat: any): string =>
  (feat.parameters.map((p: any) => p.message ?? p).find((p: any) => p.parameterId === "sketchPlane")
    .queries[0].message ?? feat.parameters
    .map((p: any) => p.message ?? p)
    .find((p: any) => p.parameterId === "sketchPlane").queries[0]).queryString;

/** The connector a sketch is placed on, read out of the sketch itself. */
async function connectorOf(doc: { features: any[] }, sketchId: string) {
  const sketch = feature(doc, sketchId);
  const mcId = mateConnectorPlaneId(planeQueryString(sketch));
  expect(mcId, `${sketchId} is not placed on a mate connector`).toBeTruthy();
  const sub = implicitMateConnectors(sketch).get(mcId!);
  expect(sub, `${sketchId} carries no implicit connector ${mcId}`).toBeTruthy();
  return await readMateConnector(sub!);
}

const SKETCH_2 = "FfRxz4hmBrm96Lm_0";
const SKETCH_4 = "FUYLAXOHGV4RBJ9_2";
const SKETCH_6 = "F2BP3BGeuInpe6G_1";

describe("implicit mate connectors — detection", () => {
  const doc = ticket416();

  it("recognises the uncompressed MATE_CONNECTOR plane query and names the connector", () => {
    const sketch = feature(doc, SKETCH_2);
    expect(mateConnectorPlaneId(planeQueryString(sketch))).toBe("FoeA2WjkTWLKRze");
  });

  it("leaves an ordinary plane query alone", () => {
    // Sketch 1 sits on a real datum plane — no connector anywhere.
    expect(mateConnectorPlaneId(planeQueryString(feature(doc, "FeniPjG8I3vr6X2_0")))).toBeNull();
  });

  it("finds exactly the three sketches the census missed", () => {
    const onConnectors = doc.features
      .map((f: any) => f.message ?? f)
      .filter((f: any) => f.featureType === "newSketch")
      .filter((f: any) => mateConnectorPlaneId(planeQueryString(f)) !== null)
      .map((f: any) => f.name);
    expect(onConnectors).toEqual(["Sketch 2", "Sketch 6", "Sketch 4"]);
  });

  it("carries the connector INSIDE the sketch, not in the feature list", () => {
    const topLevel = doc.features
      .map((f: any) => f.message ?? f)
      .filter((f: any) => f.featureType === "mateConnector");
    expect(topLevel).toEqual([]);
    expect(implicitMateConnectors(feature(doc, SKETCH_2)).size).toBe(1);
  });
});

describe("implicit mate connectors — decoding", () => {
  const doc = ticket416();

  it("reads Sketch 2's connector: an ENDPOINT of a raw sketch line", async () => {
    const mc = await connectorOf(doc, SKETCH_2);
    expect(mc.originType).toBe("ON_ENTITY");
    expect(mc.inference).toBe("POINT");
    expect(mc.anchor.kind).toBe("sketchEntity");
    if (mc.anchor.kind !== "sketchEntity") throw new Error("unreachable");
    expect(mc.anchor.entityType).toBe("EDGE");
    expect(mc.anchor.origin).toEqual({
      sketchFeatureId: "FeniPjG8I3vr6X2_0",
      entityId: "HhKEnMRHLJLB.right",
    });
    // WHICH endpoint is only legible from the secondary query.
    expect(mc.secondary?.kind).toBe("sketchEntity");
    expect(anchorT(mc)).toBe(1.0);
  });

  it("reads Sketch 6's connector: the MIDPOINT of an extrude's cap edge", async () => {
    const mc = await connectorOf(doc, SKETCH_6);
    expect(mc.inference).toBe("MID_POINT");
    expect(mc.anchor.kind).toBe("capEntity");
    if (mc.anchor.kind !== "capEntity") throw new Error("unreachable");
    expect(mc.anchor.entityType).toBe("EDGE");
    expect(mc.anchor.opFeatureId).toBe("FUwE53MM0gG2CH2_0");
    expect(mc.anchor.isStart).toBe(false);
    expect(mc.anchor.origin.entityId).toBe("HhKEnMRHLJLB.top");
    // A midpoint needs no secondary reference.
    expect(anchorT(mc)).toBe(0.5);
  });

  it("reads Sketch 4's connector: the CENTRE of a circular cap edge", async () => {
    const mc = await connectorOf(doc, SKETCH_4);
    expect(mc.inference).toBe("CENTER");
    expect(mc.anchor.kind).toBe("capEntity");
    if (mc.anchor.kind !== "capEntity") throw new Error("unreachable");
    expect(mc.anchor.opFeatureId).toBe("FEQ7ajZembzIAXS_1");
    // A single-part sketch entity id — the shape a two-part decoder loses.
    expect(mc.anchor.origin.entityId).toBe("EDMomUfcnXmW");
    expect(mc.anchor.origin.sketchFeatureId).toBe("FiVOzOHdKGa6422_1");
    // A centre is not a position ALONG an edge.
    expect(anchorT(mc)).toBeNull();
  });

  it("reads #419's CENTROID connector as the sketch REGION it names, not one of its edges", async () => {
    const doc419 = JSON.parse(
      readFileSync(join(__dirname, "fixtures", "onshape-419-features.json"), "utf8"),
    );
    const mc = await connectorOf(doc419, "FTYfIm6bhe5zwOg_7"); // Sketch 9
    expect(mc.inference).toBe("CENTROID");
    // Ticket #447. This anchor's query says `entityType: FACE`,
    // `queryType: IMPRINT` at its top level, and hangs a `disambiguationData`
    // block in FRONT of both that names the four sides of the rectangle
    // `DThSZU2d8ySx` — each with the ±1 flank the face lies on. Reading the
    // first type token in the stream therefore answered „EDGE, the rectangle's
    // bottom", and the connector's +Z became that line's TANGENT instead of the
    // face's NORMAL: Onshape's own 3D answer put `Sketch 9` at 90.0° to where
    // we placed it. The type has to come from the DOCUMENT, not the stream.
    expect(mc.anchor.kind).toBe("region");
    if (mc.anchor.kind !== "region") throw new Error("unreachable");
    expect(mc.anchor.sketchFeatureId).toBe("F5TEqDQMJSgYkmZ_0");
    expect(mc.anchor.edges.map((e) => e.entityId).sort()).toEqual([
      "DThSZU2d8ySx.bottom",
      "DThSZU2d8ySx.left",
      "DThSZU2d8ySx.right",
      "DThSZU2d8ySx.top",
    ]);
    // The flank is what tells the region from the hole around it (#442).
    expect(mc.anchor.edges.map((e) => e.side)).toEqual([-1, 1, 1, -1]);
    // A FACE has no position along an edge, whatever the inference says.
    expect(anchorT(mc)).toBeNull();
  });

  it("reads the realignment: this document adds none", async () => {
    for (const id of [SKETCH_2, SKETCH_4, SKETCH_6]) {
      const mc = await connectorOf(doc, id);
      expect(mc.offsetMm, id).toEqual([0, 0, 0]);
      expect(mc.rotationDeg, id).toBeCloseTo(0, 9);
      expect(mc.flipPrimary, id).toBe(false);
    }
  });

  it("reads a realignment when there IS one, in millimetres and degrees", async () => {
    const sketch = structuredClone(feature(doc, SKETCH_2));
    const sub = sketch.subFeatures[0];
    const param = (id: string) => sub.parameters.find((p: any) => p.parameterId === id);
    param("translationZ").value = 0.0125; // Onshape reports SI: 12.5 mm
    param("rotation").value = Math.PI / 2;
    param("flipPrimary").value = true;
    const mc = await readMateConnector(sub);
    expect(mc.offsetMm).toEqual([0, 0, 12.5]);
    expect(mc.rotationDeg).toBeCloseTo(90, 9);
    expect(mc.flipPrimary).toBe(true);
  });

  it("reports an unreadable anchor rather than inventing one", async () => {
    const anchor = await decodeMateAnchorQuery('query=qNothingWeKnow("x");');
    expect(anchor.kind).toBe("unsupported");
    expect(describeAnchor(anchor)).toContain("nicht lesbar");
  });
});

/**
 * The frames themselves — MEASURED, not derived from our own import.
 *
 * A sketch's frame is the rigid map from its 2D coordinates to world space:
 * `P = origin + u·x + v·y`. Onshape reports both sides (2D in the feature
 * payload, 3D in the sketches endpoint), so the frame is over-determined and
 * can be CHECKED rather than guessed. These are the numbers a FieldDraft sketch
 * on a mate connector has to reproduce; they are the acceptance data for the
 * kernel gap described in docs/ONSHAPE-IMPORT-GAP-MATRIX.md.
 */
const GROUND_TRUTH_FRAMES: Record<string, { origin: number[]; x: number[]; y: number[] }> = {
  // Sketch 1 — on the document's own Right plane. Included as the CONTROL: it
  // reproduces FieldDraft's `origin_plane_yz` exactly, which is what makes the
  // three connector frames below trustworthy.
  FeniPjG8I3vr6X2_0: { origin: [0, 0, 0], x: [0, 1, 0], y: [0, 0, 1] },
  FfRxz4hmBrm96Lm_0: { origin: [0, 6.5, -65], x: [1, 0, 0], y: [0, -1, 0] },
  F2BP3BGeuInpe6G_1: { origin: [3, -10.1, -65], x: [1, 0, 0], y: [0, 0, -1] },
  FUYLAXOHGV4RBJ9_2: { origin: [-50, -19.2, -55.83], x: [1, 0, 0], y: [0, 1, 0] },
};

/** Every 2D point of a sketch feature we can name, in METRES, keyed by the
 *  3D endpoint id the sketches endpoint uses for it. */
function points2d(feat: any): Map<string, [number, number]> {
  const out = new Map<string, [number, number]>();
  for (const raw of feat.entities ?? []) {
    const e = raw.message ?? raw;
    const g = e.geometry?.message ?? e.geometry;
    if (!g) continue;
    if (String(g.btType).startsWith("BTCurveGeometryLine")) {
      for (const [suffix, p] of [
        ["start", e.startParam],
        ["end", e.endParam],
      ] as const) {
        if (typeof p !== "number") continue;
        out.set(`${e.entityId}.${suffix}`, [g.pntX + p * g.dirX, g.pntY + p * g.dirY]);
      }
    } else if (String(g.btType).startsWith("BTCurveGeometryCircle")) {
      out.set(`${e.entityId}.center`, [g.xCenter, g.yCenter]);
    }
  }
  return out;
}

describe("mate-connector sketch frames — against Onshape's own 3D geometry", () => {
  const doc = ticket416();
  const three = sketches3d();

  for (const [featureId, frame] of Object.entries(GROUND_TRUTH_FRAMES)) {
    const sketch = three.sketches.find((s: any) => s.featureId === featureId);

    it(`places every entity of ${sketch.sketch} where Onshape places it`, () => {
      const flat = points2d(feature(doc, featureId));
      const byId = new Map<string, any>(sketch.geomEntities.map((e: any) => [e.id, e]));
      let checked = 0;
      for (const [id, [u, v]] of flat) {
        const ent = byId.get(id);
        const p = ent?.center ?? ent?.point;
        if (!p) continue;
        checked += 1;
        for (let c = 0; c < 3; c++) {
          // 2D is metres in the payload, the frame origin is millimetres.
          const world = frame.origin[c] / 1000 + u * frame.x[c] + v * frame.y[c];
          expect(world, `${id} axis ${c}`).toBeCloseTo(p[c], 9);
        }
      }
      // A frame that matched nothing would pass an empty loop.
      expect(checked, `${sketch.sketch}: no 2D↔3D correspondence found`).toBeGreaterThan(1);
    });
  }
});
