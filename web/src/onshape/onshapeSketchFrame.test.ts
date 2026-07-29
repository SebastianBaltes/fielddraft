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
  measureCollinearSketchFrame,
  measureOnshapeSketchFrame,
  type RobustFitReport,
  sketches3dByFeature,
  sketchPoints2d,
  transformSketchFeature,
  type MeasuredSketchFrame,
} from "./onshapeSketchFrame";
import { arcWorldSpan } from "./onshapeSketch";
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

/** Float-Rauschen der Drehung wegschneiden (und −0 auf 0 legen). */
const round = (v: number) => Math.round(v * 1e12) / 1e12 + 0;

/** Die beiden Endpunkte eines Bogen-Payloads in Sketch-Koordinaten (Meter),
 *  gelesen wie der Importer sie liest — über den EIGENEN Frame des Kreises
 *  ({@link arcWorldSpan}). Das ist die Größe, die eine Abbildung erhalten muss;
 *  `startParam` allein sagt ohne den Frame nichts. */
function arcEnds(e: any): [number, number][] {
  const g = e.geometry;
  const { startAngle, sweep } = arcWorldSpan(g, e.startParam, e.endParam);
  return [startAngle, startAngle + sweep].map(
    (a) =>
      [round(g.xCenter + g.radius * Math.cos(a)), round(g.yCenter + g.radius * Math.sin(a))] as [
        number,
        number,
      ],
  );
}

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
    // `startParam`/`endParam` sind Winkel im EIGENEN Frame des Kreises (#444) —
    // abgebildet wird deshalb der Frame, nicht der Winkel. Gemessen wird, was
    // man sehen kann: der Bogen von (0,015 | 0) nach (0,01 | 0,005) muss
    // dorthin wandern, wohin die Drehung diese beiden PUNKTE trägt.
    expect(a.startParam).toBe(0);
    expect(a.endParam).toBeCloseTo(Math.PI / 2, 12);
    expect(arcEnds(a)).toEqual([
      [0, 0.015],
      [-0.005, 0.01],
    ]);
    // Eine Drehung erhält den Umlaufsinn: Start bleibt Start.
    expect(a.geometry.clockwise).toBeFalsy();
    expect(a.startPointId).toBe("A.start");
  });

  it("kippt bei einer SPIEGELUNG den Umlaufsinn des Bogen-Frames", () => {
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
    // Eine Spiegelung kippt den Umlaufsinn des Frames — und NUR den. Die
    // Parameter bleiben, damit bleibt Start Start; der Bogen von (0,005 | 0,01)
    // nach (0 | 0,015) landet gespiegelt bei (0,005 | −0,01) → (0 | −0,015).
    expect(a.geometry.clockwise).toBe(true);
    expect(a.startParam).toBe(0);
    expect(a.endParam).toBeCloseTo(Math.PI / 2, 12);
    expect(arcEnds(a)).toEqual([
      [0.005, -0.01],
      [0, -0.015],
    ]);
    expect(a.startPointId).toBe("A.start");
    expect(a.endPointId).toBe("A.end");
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

/**
 * Ticket #427: die Ebene liegt PARALLEL DANEBEN.
 *
 * Der Importer hat diesen Fall früher wie „schiefe Ebene" behandelt und die
 * Umrechnung ganz verworfen — Onshapes 2D-Zahlen landeten dann roh auf
 * FieldDrafts anders orientierten Achsen. Dieser Test hält fest, warum das
 * falsch war: die In-Plane-Abbildung ist auch bei beliebigem Versatz EXAKT, und
 * die umgerechnete Geometrie trifft Onshapes eigene Punkte auf die letzte
 * Stelle — verschoben um genau den gemeldeten Versatz, sonst nichts.
 */
describe("parallel versetzte Ebene (#427)", () => {
  const doc = features415();
  const three3d = fx("onshape-415-sketches3d.json");
  const three = sketches3dByFeature(three3d);
  const offsetMm = 50;

  it("die Abbildung bleibt exakt, der Versatz bleibt der einzige Fehler", () => {
    const feat = sketchNamed(doc, "Sketch 3");
    const os = measureOnshapeSketchFrame(feat, three.get(feat.featureId))!;
    // Dieselbe Ebene wie im Test darüber, nur um `offsetMm` entlang ihrer
    // Normalen verschoben — die Fläche, die eine Query „daneben" greift.
    const fd = fdFrame([0, 0, -3 + offsetMm * -1], [0, 0, -1]);
    const x = inPlaneXform(os, fd);

    // Die Basis ist unverändert bestimmt; NUR der Abstand meldet den Fehlgriff.
    expect(Math.abs(x.normalDot)).toBeCloseTo(1, 12);
    expect(Math.abs(x.outOfPlaneMm)).toBeCloseTo(offsetMm, 9);
    expect(x.axisAligned).toBe(true);

    const moved = transformSketchFeature(feat, x).feature;
    const byId = new Map<string, any>(
      (three3d.sketches.find((s: any) => s.featureId === feat.featureId).geomEntities as any[]).map(
        (e: any) => [e.id, e],
      ),
    );
    let checked = 0;
    for (const [id, [u, v]] of sketchPoints2d(moved)) {
      const p = byId.get(id)?.point ?? byId.get(id)?.center;
      if (!p) continue;
      checked += 1;
      for (let c = 0; c < 3; c++) {
        const world = fd.origin[c] / 1000 + u * fd.xAxis[c] + v * fd.yAxis[c];
        // Onshapes Punkt plus der Versatz entlang FieldDrafts Normalen — der
        // Rest stimmt auf Nanometer.
        expect(world, `${id} Achse ${c}`).toBeCloseTo(
          p[c] - (x.outOfPlaneMm / 1000) * fd.normal[c],
          9,
        );
      }
    }
    expect(checked).toBeGreaterThan(3);
  });
});

describe("Frame aus KOLLINEAREN Stützpunkten (#452)", () => {
  /** Ein Sketch aus EINER Linie — zwei Stützpunkte, also immer kollinear. */
  const lineFeature = (
    pnt: [number, number],
    dir: [number, number],
    half: number,
  ): any => ({
    entities: [
      {
        btType: "BTMSketchCurveSegment-155",
        entityId: "L",
        geometry: {
          btType: "BTCurveGeometryLine-117",
          pntX: pnt[0],
          pntY: pnt[1],
          dirX: dir[0],
          dirY: dir[1],
        },
        startParam: -half,
        endParam: half,
      },
    ],
  });

  /** Onshapes 3D-Antwort, EXAKT aus dem gegebenen Frame gerechnet — so ist die
   *  Korrespondenz starr und die Messung muss den Frame zurückgeben. */
  const threeFrom = (
    feat: any,
    origin: [number, number, number],
    xAxis: [number, number, number],
    yAxis: [number, number, number],
  ): any => {
    const g = feat.entities[0].geometry;
    const at = (t: number) => {
      const u = g.pntX + t * g.dirX;
      const v = g.pntY + t * g.dirY;
      return [0, 1, 2].map((c) => origin[c] / 1000 + u * xAxis[c] + v * yAxis[c]);
    };
    return {
      geomEntities: [
        { id: "L.start", entityType: "point", point: at(feat.entities[0].startParam) },
        { id: "L.end", entityType: "point", point: at(feat.entities[0].endParam) },
      ],
    };
  };

  it("legt Ursprung UND Drehung aus zwei Punkten, wenn die Ebene bekannt ist", () => {
    // Trägerfläche: die Ebene z = 7, FieldDrafts eigene Achsen darauf.
    const fd = fdFrame([0, 0, 7], [0, 0, 1]);
    // Onshapes Basis darauf, um 30° gedreht und 12/−5 mm versetzt — genau die
    // Art Differenz, die eine wörtliche Übernahme in die Ebene hineinlegt.
    const a = (30 * Math.PI) / 180;
    const osX: [number, number, number] = [Math.cos(a), Math.sin(a), 0];
    const osY: [number, number, number] = [-Math.sin(a), Math.cos(a), 0];
    const osOrigin: [number, number, number] = [12, -5, 7];

    const feat = lineFeature([0.004, -0.011], [0.6, 0.8], 0.025);
    const three = threeFrom(feat, osOrigin, osX, osY);
    const f = measureCollinearSketchFrame(feat, three, fd)!;
    expect(f, "zwei kollineare Punkte reichen mit bekannter Ebene").toBeTruthy();
    expect(f.samples).toBe(2);
    expect(f.residualMm).toBeLessThan(1e-9);
    for (let c = 0; c < 3; c++) {
      expect(f.origin[c]).toBeCloseTo(osOrigin[c], 9);
      expect(f.xAxis[c]).toBeCloseTo(osX[c], 9);
      expect(f.yAxis[c]).toBeCloseTo(osY[c], 9);
    }
    // Und die In-Plane-Abbildung ist dann eine reine Drehung — keine Spiegelung.
    expect(inPlaneXform(f, fd).det).toBeCloseTo(1, 12);
  });

  it("misst dieselbe Drehung auf einer schrägen Trägerfläche", () => {
    const n: [number, number, number] = [0, 1 / Math.SQRT2, 1 / Math.SQRT2];
    const fd = fdFrame([3, 4, 5], n);
    const a = -0.7;
    const osX: [number, number, number] = [0, 1, 2].map(
      (c) => Math.cos(a) * fd.xAxis[c] + Math.sin(a) * fd.yAxis[c],
    ) as [number, number, number];
    const osY: [number, number, number] = [0, 1, 2].map(
      (c) => -Math.sin(a) * fd.xAxis[c] + Math.cos(a) * fd.yAxis[c],
    ) as [number, number, number];
    const feat = lineFeature([-0.02, 0.008], [-0.28, 0.96], 0.031);
    const three = threeFrom(feat, [3, 4, 5], osX, osY);
    const f = measureCollinearSketchFrame(feat, three, fd)!;
    expect(f).toBeTruthy();
    expect(inPlaneXform(f, fd).det).toBeCloseTo(1, 12);
    expect(inPlaneXform(f, fd).angle).toBeCloseTo(a, 9);
  });

  it("lehnt ab, wenn Onshapes zwei Antworten nicht STARR zueinander passen", () => {
    // Genau der #419-Fall `Sketch 12`: die 3D-Antwort setzt den Anfangspunkt
    // 0,5 mm neben den, den der 2D-Payload aus `startParam` ergibt. Zwischen
    // zwei Punktepaaren verschiedenen Abstands gibt es keine starre Abbildung —
    // und eine Anpassung, die das überginge, verschöbe die Geometrie.
    const fd = fdFrame([0, 0, 0], [0, 0, 1]);
    const feat = lineFeature([0, 0], [1, 0], 0.025);
    const three = threeFrom(feat, [0, 0, 0], [1, 0, 0], [0, 1, 0]);
    // LÄNGS verschoben, wie im Dokument: die beiden Antworten melden dann
    // verschiedene ABSTÄNDE, und daran scheitert jede starre Abbildung.
    three.geomEntities[0].point[0] -= 0.0005;
    const report = { residualMm: NaN, samples: 0 };
    expect(measureCollinearSketchFrame(feat, three, fd, report)).toBeNull();
    // …und der Grund ist beziffert, nicht bloss „nicht bestimmbar".
    expect(report.samples).toBe(2);
    expect(report.residualMm).toBeCloseTo(0.5, 3);

    // Auch eine QUER verschobene Antwort wird abgelehnt — dort ist der Rest
    // nur kleiner (0,5²/2·50 mm), aber er ist da, und die Schwelle ist scharf.
    const three2 = threeFrom(feat, [0, 0, 0], [1, 0, 0], [0, 1, 0]);
    three2.geomEntities[0].point[1] += 0.0005;
    expect(measureCollinearSketchFrame(feat, three2, fd)).toBeNull();
  });

  it("lehnt ab, wenn nur EIN Stützpunkt gemeldet ist", () => {
    const fd = fdFrame([0, 0, 0], [0, 0, 1]);
    const feat = lineFeature([0, 0], [1, 0], 0.025);
    const three = threeFrom(feat, [0, 0, 0], [1, 0, 0], [0, 1, 0]);
    three.geomEntities.pop();
    expect(measureCollinearSketchFrame(feat, three, fd)).toBeNull();
  });

  it("lehnt ab, wenn die Linie gar nicht in der aufgelösten Ebene liegt", () => {
    // Die Weltrichtung läuft aus der Ebene heraus: dann ist nicht der
    // Drehwinkel falsch, sondern die Fläche.
    const fd = fdFrame([0, 0, 0], [0, 0, 1]);
    const feat = lineFeature([0, 0], [1, 0], 0.025);
    const three = threeFrom(feat, [0, 0, 0], [0, 0, 1], [0, 1, 0]);
    expect(measureCollinearSketchFrame(feat, three, fd)).toBeNull();
  });
});

describe("ausreisser-robuster Frame-Fit (#454)", () => {
  /** Ein Sketch aus N benannten PUNKTEN — `sketchPoints2d` liest `x`/`y`. */
  const pointFeature = (pts: [number, number][]): any => ({
    entities: pts.map((p, i) => ({ entityId: `P${i}`, x: p[0], y: p[1] })),
  });

  /** Onshapes 3D-Antwort, exakt aus dem Frame gerechnet (Meter). */
  const threeFrom = (
    pts: [number, number][],
    origin: [number, number, number],
    xAxis: [number, number, number],
    yAxis: [number, number, number],
  ): any => ({
    geomEntities: pts.map(([u, v], i) => ({
      id: `P${i}`,
      entityType: "point",
      point: [0, 1, 2].map((c) => origin[c] / 1000 + u * xAxis[c] + v * yAxis[c]),
    })),
  });

  // Zehn Punkte in allgemeiner Lage — dreifach überbestimmt, und keine drei
  // davon kollinear, damit das Verwerfen nie an der Entartung scheitert.
  const PTS: [number, number][] = [
    [0.01, 0.0], [0.03, 0.011], [-0.02, 0.017], [0.008, -0.024], [0.031, -0.009],
    [-0.014, -0.006], [0.022, 0.028], [-0.027, 0.004], [0.005, 0.019], [0.017, -0.031],
  ];
  const OS_ORIGIN: [number, number, number] = [7, -3, 11];
  const A = 0.4;
  const OS_X: [number, number, number] = [Math.cos(A), Math.sin(A), 0];
  const OS_Y: [number, number, number] = [-Math.sin(A), Math.cos(A), 0];

  it("nimmt den exakten Pfad, wo er trägt — und verwirft dabei NICHTS", () => {
    const report: RobustFitReport = { dropped: [], total: 0 };
    const f = measureOnshapeSketchFrame(
      pointFeature(PTS),
      threeFrom(PTS, OS_ORIGIN, OS_X, OS_Y),
      report,
    )!;
    expect(f).toBeTruthy();
    expect(report.dropped, "der robuste Pfad wird gar nicht betreten").toEqual([]);
    expect(f.samples).toBe(PTS.length);
    for (let c = 0; c < 3; c++) {
      expect(f.origin[c]).toBeCloseTo(OS_ORIGIN[c], 9);
      expect(f.xAxis[c]).toBeCloseTo(OS_X[c], 9);
    }
  });

  it("rettet den Frame, wenn EIN Stützpunkt daneben liegt — und nennt ihn", () => {
    // Der #419-`Sketch 11`-Fall in klein: 9 von 10 Punkten exakt, einer 1 mm weg.
    const three = threeFrom(PTS, OS_ORIGIN, OS_X, OS_Y);
    three.geomEntities[4].point[2] += 0.001;
    const report: RobustFitReport = { dropped: [], total: 0 };
    const f = measureOnshapeSketchFrame(pointFeature(PTS), three, report)!;
    expect(f, "ein Ausreisser darf die Skizze nicht kosten").toBeTruthy();
    expect(report.total).toBe(10);
    expect(report.dropped.map((d) => d.id)).toEqual(["P4"]);
    expect(report.dropped[0].errMm).toBeGreaterThan(0.5);
    expect(f.samples).toBe(9);
    // Und es ist DERSELBE Frame wie ohne den Ausreisser — nicht ein
    // ausgeglichener Kompromiss zwischen ihm und dem Rest.
    expect(f.residualMm).toBeLessThan(1e-9);
    for (let c = 0; c < 3; c++) {
      expect(f.origin[c]).toBeCloseTo(OS_ORIGIN[c], 9);
      expect(f.xAxis[c]).toBeCloseTo(OS_X[c], 9);
      expect(f.yAxis[c]).toBeCloseTo(OS_Y[c], 9);
    }
  });

  it("lehnt eine SKALIERUNG ab, so viele Punkte man auch verwürfe", () => {
    // Onshapes Antwort mit gedehnter y-Achse: jeder einzelne Punkt liesse sich
    // durch Verwerfen „erklären", aber was überlebt, ist keine Drehung. Genau
    // der #409-`Sketch 2`-Fall (‖y‖ = 1,1000).
    const three = threeFrom(PTS, OS_ORIGIN, OS_X, [
      OS_Y[0] * 1.1,
      OS_Y[1] * 1.1,
      OS_Y[2] * 1.1,
    ]);
    const report: RobustFitReport = { dropped: [], total: 0 };
    expect(measureOnshapeSketchFrame(pointFeature(PTS), three, report)).toBeNull();
  });

  it("verwirft nie so viel, dass die Mehrheit oder die Überbestimmung fällt", () => {
    // Die HÄLFTE der Punkte daneben: dann ist nicht mehr entscheidbar, welche
    // Teilmenge die Ausreisser sind — es sind zwei Datensätze, kein verrauschter.
    const three = threeFrom(PTS, OS_ORIGIN, OS_X, OS_Y);
    for (const i of [0, 1, 2, 3, 4]) three.geomEntities[i].point[2] += 0.002 * (i + 1);
    expect(measureOnshapeSketchFrame(pointFeature(PTS), three)).toBeNull();

    // Und bei sechs Paaren wird gar nicht erst verworfen: der Rest fiele unter
    // die zweifache Überbestimmung, und „exakt getroffen" wäre dort
    // Interpolation statt einer Aussage.
    const few = PTS.slice(0, 6);
    const three6 = threeFrom(few, OS_ORIGIN, OS_X, OS_Y);
    three6.geomEntities[2].point[2] += 0.001;
    expect(measureOnshapeSketchFrame(pointFeature(few), three6)).toBeNull();
    // Bei sieben trägt der Rest die Grenze gerade noch — die Schranke ist
    // scharf, nicht ungefähr.
    const seven = PTS.slice(0, 7);
    const three7 = threeFrom(seven, OS_ORIGIN, OS_X, OS_Y);
    three7.geomEntities[2].point[2] += 0.001;
    expect(measureOnshapeSketchFrame(pointFeature(seven), three7)).toBeTruthy();
  });
});
