/**
 * Der Sketch-Frame-Abgleich (#415) gegen die echten Fixtures.
 *
 * Onshapes Frame wird an Onshapes EIGENER Antwort gemessen (die 3D-Geometrie
 * aus `sketches?output3D=true`), nie an unserem Import — die Methodik-Lehre aus
 * #412. Die Kontrollzeile ist Sketch 1: er liegt auf der Ursprungsebene und muss
 * FieldDrafts eigenen Frame auf die letzte Stelle reproduzieren. Erst das macht
 * die beiden gemessenen Wandflächen-Frames glaubwürdig.
 */
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import {
  describeXform,
  inPlaneXform,
  measureOnshapeSketchFrame,
  sketches3dByFeature,
  sketchPoints2d,
  transformSketchFeature,
  type MeasuredSketchFrame,
} from "./onshapeSketchFrame";
import { planeBasisFromNormal, type PlaneFrame } from "../sketchPlaneFrame";

const fx = (name: string): any =>
  JSON.parse(readFileSync(join(__dirname, "fixtures", name), "utf8"));

const features415 = () => fx("onshape-415-features.json");
const three415 = () => sketches3dByFeature(fx("onshape-415-sketches3d.json"));

const sketchNamed = (doc: any, name: string): any =>
  doc.features.map((f: any) => f.message ?? f).find((f: any) => f.name === name);

/** Der FieldDraft-Frame einer Ebene mit dieser Normalen und diesem Ursprung —
 *  über die SSOT `planeBasisFromNormal`, nicht über eine Kopie der Regel. */
function fdFrame(origin: [number, number, number], normal: [number, number, number]): PlaneFrame {
  const { xAxis, yAxis } = planeBasisFromNormal(normal);
  return { origin, xAxis, yAxis, normal };
}

describe("Onshapes Sketch-Frame messen", () => {
  const doc = features415();
  const three = three415();

  it("reproduziert auf der Ursprungsebene FieldDrafts eigenen Frame (Kontrolle)", () => {
    const s1 = sketchNamed(doc, "Sketch 1");
    const f = measureOnshapeSketchFrame(s1, three.get(s1.featureId))!;
    expect(f).toBeTruthy();
    expect(f.origin.map((v) => Math.round(v * 1e6) / 1e6)).toEqual([0, 0, 0]);
    for (const [axis, want] of [
      [f.xAxis, [1, 0, 0]],
      [f.yAxis, [0, 1, 0]],
      [f.normal, [0, 0, 1]],
    ] as const) {
      for (let c = 0; c < 3; c++) expect(axis[c]).toBeCloseTo(want[c], 12);
    }
    // Über-, nicht unterbestimmt: das ist es, was die Messung zur Messung macht.
    expect(f.samples).toBeGreaterThan(3);
    expect(f.residualMm).toBeLessThan(1e-6);
  });

  it("misst die Bodenwand, auf der Onshape und FieldDraft auseinandergehen", () => {
    const s3 = sketchNamed(doc, "Sketch 3");
    const f = measureOnshapeSketchFrame(s3, three.get(s3.featureId))!;
    expect(f.origin[2]).toBeCloseTo(-3, 9);
    // Onshape: x = +X, y = −Y ⇒ Normale −Z. FieldDraft hätte dort x = +Y.
    expect(f.xAxis[0]).toBeCloseTo(1, 12);
    expect(f.yAxis[1]).toBeCloseTo(-1, 12);
    expect(f.normal[2]).toBeCloseTo(-1, 12);
  });

  it("misst die Deckelfläche", () => {
    const s4 = sketchNamed(doc, "Sketch 4");
    const f = measureOnshapeSketchFrame(s4, three.get(s4.featureId))!;
    expect(f.origin[2]).toBeCloseTo(32.5, 9);
    expect(f.xAxis[0]).toBeCloseTo(1, 12);
    expect(f.yAxis[1]).toBeCloseTo(1, 12);
  });

  it("rät nicht, wenn die Stützpunkte kollinear liegen", () => {
    // Sketch 5 ist eine Reihe von vier Ellipsen auf EINER Geraden — aus den
    // Punkten allein folgt der Frame nicht, und geraten wird er nicht.
    const s5 = sketchNamed(doc, "Sketch 5");
    expect(measureOnshapeSketchFrame(s5, three.get(s5.featureId))).toBeNull();
  });

  it("liefert null ohne 3D-Antwort", () => {
    expect(measureOnshapeSketchFrame(sketchNamed(doc, "Sketch 3"), undefined)).toBeNull();
  });

  it("liest Linien-Endpunkte und Kreis-/Ellipsen-Mittelpunkte als Stützpunkte", () => {
    const pts = sketchPoints2d(sketchNamed(doc, "Sketch 3"));
    expect([...pts.keys()].some((k) => k.endsWith(".start"))).toBe(true);
    expect([...pts.keys()].some((k) => k.endsWith(".center"))).toBe(true);
  });
});

describe("die In-Plane-Abbildung B_fd⁻¹ ∘ B_os", () => {
  const doc = features415();
  const three = three415();

  it("ist auf der Ursprungsebene die Identität", () => {
    const s1 = sketchNamed(doc, "Sketch 1");
    const os = measureOnshapeSketchFrame(s1, three.get(s1.featureId))!;
    // Für eine URSPRUNGSEBENE ist FieldDrafts Frame NICHT `planeBasisFromNormal`,
    // sondern die fest verdrahtete Datum-Tabelle (x = +X, y = +Y) — genau die
    // Unterscheidung, die `resolvePlaneFrame` trifft.
    const x = inPlaneXform(os, {
      origin: [0, 0, 0],
      xAxis: [1, 0, 0],
      yAxis: [0, 1, 0],
      normal: [0, 0, 1],
    });
    expect(x.identity).toBe(true);
  });

  it("ist auf der Bodenwand die 90°-Drehung (u, v) → (−v, u)", () => {
    const s3 = sketchNamed(doc, "Sketch 3");
    const os = measureOnshapeSketchFrame(s3, three.get(s3.featureId))!;
    // FieldDraft auf n = −Z: x = +Y, y = +X (die Saat-Regel des Kernels).
    const fd = fdFrame([0, 0, -3], [0, 0, -1]);
    expect(fd.xAxis.map(Math.abs)).toEqual([0, 1, 0]);
    expect(fd.yAxis.map(Math.abs)).toEqual([1, 0, 0]);
    expect(fd.xAxis[1]).toBe(1);
    expect(fd.yAxis[0]).toBe(1);

    const x = inPlaneXform(os, fd);
    expect(x.identity).toBe(false);
    expect([x.a, x.b, x.c, x.d]).toEqual([0, -1, 1, 0]);
    expect([x.tx, x.ty]).toEqual([0, 0]);
    expect(x.det).toBe(1);
    expect(x.axisAligned).toBe(true);
    expect(x.axesSwapped).toBe(true);
    // Gleiche Ebene, nur andere Basis — genau das trennt einen Frame-Konflikt
    // von einem Fehlgriff auf die falsche Fläche.
    expect(Math.abs(x.normalDot)).toBeCloseTo(1, 12);
    expect(x.outOfPlaneMm).toBeCloseTo(0, 9);
  });

  it("meldet eine ANDERE Ebene als solche statt sie wegzurechnen", () => {
    const s3 = sketchNamed(doc, "Sketch 3");
    const os = measureOnshapeSketchFrame(s3, three.get(s3.featureId))!;
    const x = inPlaneXform(os, fdFrame([0, 0, 20], [0, 0, -1]));
    expect(x.outOfPlaneMm).toBeCloseTo(23, 6);
  });

  it("erkennt die Spiegelung, wenn die Ebenennormalen gegeneinander zeigen", () => {
    const os: MeasuredSketchFrame = {
      origin: [0, 0, 0],
      xAxis: [1, 0, 0],
      yAxis: [0, 1, 0],
      normal: [0, 0, 1],
      residualMm: 0,
      samples: 4,
    };
    // FieldDraft auf n = −Z: x = +Y, y = +X.
    const x = inPlaneXform(os, fdFrame([0, 0, 0], [0, 0, -1]));
    expect(x.det).toBe(-1);
    expect([x.a, x.b, x.c, x.d]).toEqual([0, 1, 1, 0]);
  });

  it("beschreibt sich in Worten", () => {
    const s3 = sketchNamed(doc, "Sketch 3");
    const os = measureOnshapeSketchFrame(s3, three.get(s3.featureId))!;
    expect(describeXform(inPlaneXform(os, fdFrame([0, 0, -3], [0, 0, -1])))).toBe("90° gedreht");
  });
});

const ROT90 = { a: 0, b: -1, c: 1, d: 0, tx: 0, ty: 0, det: 1, angle: Math.PI / 2 };
const rot90 = {
  ...ROT90,
  identity: false,
  axisAligned: true,
  axesSwapped: true,
  normalDot: 1,
  outOfPlaneMm: 0,
};

describe("den Onshape-Payload umschreiben", () => {
  it("dreht Punkte, Linien-Stützpunkte und -Richtungen", () => {
    const feat = {
      entities: [
        { btType: "BTMSketchPoint-158", entityId: "P", x: 0.002, y: 0.003 },
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "L",
          startParam: 0,
          endParam: 0.01,
          geometry: {
            btType: "BTCurveGeometryLine-117",
            pntX: 0.002,
            pntY: 0,
            dirX: 1,
            dirY: 0,
          },
        },
      ],
    };
    const { feature } = transformSketchFeature(feat, rot90);
    const [p, l] = feature.entities as any[];
    expect([p.x, p.y]).toEqual([-0.003, 0.002]);
    expect([l.geometry.pntX, l.geometry.pntY]).toEqual([0, 0.002]);
    expect([l.geometry.dirX, l.geometry.dirY]).toEqual([0, 1]);
    // Die Bogenlänge entlang `dir` ist von einer starren Abbildung unberührt.
    expect(l.endParam).toBe(0.01);
    // Und das Original bleibt unangetastet.
    expect(feat.entities[0].x).toBe(0.002);
  });

  it("dreht einen Bogen mit, Winkel wie Mittelpunkt", () => {
    const feat = {
      entities: [
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "A",
          startPointId: "A.start",
          endPointId: "A.end",
          startParam: 0,
          endParam: Math.PI / 2,
          geometry: {
            btType: "BTCurveGeometryCircle-115",
            xCenter: 0.01,
            yCenter: 0,
            radius: 0.005,
          },
        },
      ],
    };
    const a = transformSketchFeature(feat, rot90).feature.entities[0] as any;
    expect([a.geometry.xCenter, a.geometry.yCenter]).toEqual([0, 0.01]);
    expect(a.startParam).toBeCloseTo(Math.PI / 2, 12);
    expect(a.endParam).toBeCloseTo(Math.PI, 12);
    // Eine Drehung erhält den Umlaufsinn: Start bleibt Start.
    expect(a.startPointId).toBe("A.start");
  });

  it("dreht bei einer SPIEGELUNG den Umlaufsinn des Bogens um", () => {
    const mirror = {
      a: 1,
      b: 0,
      c: 0,
      d: -1,
      tx: 0,
      ty: 0,
      det: -1,
      angle: 0,
      identity: false,
      axisAligned: true,
      axesSwapped: false,
      normalDot: -1,
      outOfPlaneMm: 0,
    };
    const feat = {
      entities: [
        {
          btType: "BTMSketchCurveSegment-155",
          entityId: "A",
          startPointId: "A.start",
          endPointId: "A.end",
          startParam: 0,
          endParam: Math.PI / 2,
          geometry: {
            btType: "BTCurveGeometryCircle-115",
            xCenter: 0,
            yCenter: 0.01,
            radius: 0.005,
          },
        },
      ],
    };
    const a = transformSketchFeature(feat, mirror).feature.entities[0] as any;
    expect([a.geometry.xCenter, a.geometry.yCenter]).toEqual([0, -0.01]);
    // Derselbe Bogen läuft gegen den Uhrzeigersinn jetzt vom früheren ENDpunkt.
    expect(a.startParam).toBeCloseTo(-Math.PI / 2, 12);
    expect(a.endParam).toBeCloseTo(0, 12);
    expect(a.startPointId).toBe("A.end");
    expect(a.endPointId).toBe("A.start");
  });

  it("tauscht HORIZONTAL und VERTICAL, wenn die Achsen tauschen", () => {
    const feat = {
      entities: [],
      constraints: [
        { btType: "BTMSketchConstraint-2", constraintType: "HORIZONTAL" },
        { btType: "BTMSketchConstraint-2", constraintType: "VERTICAL" },
        { btType: "BTMSketchConstraint-2", constraintType: "COINCIDENT" },
      ],
    };
    const { feature, droppedAxisConstraints } = transformSketchFeature(feat, rot90);
    expect(feature.constraints.map((c: any) => c.constraintType)).toEqual([
      "VERTICAL",
      "HORIZONTAL",
      "COINCIDENT",
    ]);
    expect(droppedAxisConstraints).toBe(0);
  });

  it("verwirft HORIZONTAL/VERTICAL bei einer schiefen Drehung, statt den Solver dagegen arbeiten zu lassen", () => {
    const a = Math.cos(Math.PI / 6);
    const s = Math.sin(Math.PI / 6);
    const skew = {
      a,
      b: -s,
      c: s,
      d: a,
      tx: 0,
      ty: 0,
      det: 1,
      angle: Math.PI / 6,
      identity: false,
      axisAligned: false,
      axesSwapped: false,
      normalDot: 1,
      outOfPlaneMm: 0,
    };
    const feat = {
      entities: [],
      constraints: [
        { btType: "BTMSketchConstraint-2", constraintType: "HORIZONTAL" },
        { btType: "BTMSketchConstraint-2", constraintType: "PARALLEL" },
      ],
    };
    const out = transformSketchFeature(feat, skew);
    expect(out.droppedAxisConstraints).toBe(1);
    expect(out.feature.constraints.map((c: any) => c.constraintType)).toEqual(["PARALLEL"]);
  });

  it("verwirft Bedingungen auf Onshapes Ursprung, wenn der versetzt liegt", () => {
    const shifted = { ...rot90, tx: 5, ty: -2 };
    const feat = {
      entities: [],
      constraints: [
        {
          btType: "BTMSketchConstraint-2",
          constraintType: "COINCIDENT",
          parameters: [
            { btType: "BTMParameterString-149", parameterId: "localFirst", value: "origin" },
            { btType: "BTMParameterString-149", parameterId: "localSecond", value: "L.start" },
          ],
        },
        { btType: "BTMSketchConstraint-2", constraintType: "COINCIDENT" },
      ],
    };
    const out = transformSketchFeature(feat, shifted);
    expect(out.droppedOriginConstraints).toBe(1);
    expect(out.feature.constraints).toHaveLength(1);
    // Ohne Versatz bleibt dieselbe Bedingung stehen.
    expect(transformSketchFeature(feat, rot90).droppedOriginConstraints).toBe(0);
  });
});

describe("die Umrechnung landet, wo Onshape landet", () => {
  const doc = features415();
  const three3d = fx("onshape-415-sketches3d.json");
  const three = sketches3dByFeature(three3d);

  /** Die eigentliche Abnahme: jede umgerechnete 2D-Entity, durch FieldDrafts
   *  Frame in den Raum gehoben, muss auf Onshapes eigenem 3D-Punkt landen. */
  it.each([
    ["Sketch 3", [0, 0, -3], [0, 0, -1]],
    ["Sketch 4", [0, 0, 32.5], [0, 0, 1]],
  ] as const)("%s", (name, origin, normal) => {
    const feat = sketchNamed(doc, name);
    const os = measureOnshapeSketchFrame(feat, three.get(feat.featureId))!;
    const fd = fdFrame(origin as any, normal as any);
    const x = inPlaneXform(os, fd);
    expect(x.identity, `${name}: Frames stimmen bereits überein`).toBe(false);

    const moved = transformSketchFeature(feat, x).feature;
    const pts = sketchPoints2d(moved);
    const byId = new Map<string, any>(
      (three3d.sketches.find((s: any) => s.featureId === feat.featureId).geomEntities as any[]).map(
        (e: any) => [e.id, e],
      ),
    );
    let checked = 0;
    for (const [id, [u, v]] of pts) {
      const p = byId.get(id)?.point ?? byId.get(id)?.center;
      if (!p) continue;
      checked += 1;
      for (let c = 0; c < 3; c++) {
        // 2D ist Meter, der Frame-Ursprung Millimeter.
        const world = fd.origin[c] / 1000 + u * fd.xAxis[c] + v * fd.yAxis[c];
        expect(world, `${id} Achse ${c}`).toBeCloseTo(p[c], 9);
      }
    }
    expect(checked, `${name}: keine 2D↔3D-Zuordnung gefunden`).toBeGreaterThan(3);
  });
});
