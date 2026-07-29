/**
 * Sketch-Frame-Abgleich Onshape ↔ FieldDraft (#415).
 *
 * Ein Sketch ist 2D-Geometrie plus die starre Abbildung, die sie in den Raum
 * hebt: `P = Ursprung + u·x + v·y`. Beide Systeme leiten diesen Frame für eine
 * FLÄCHENGEBUNDENE Ebene deterministisch her — und verschieden:
 *
 * - FieldDraft nimmt allein die Flächennormale (`planeBasisFromNormal`,
 *   Konventions-Lock mit dem Kernel: Weltachse mit der geringsten Ausrichtung
 *   zur Normalen als Saat, zweimal kreuzen).
 * - Onshape nimmt die Parametrisierung der Fläche.
 *
 * Auf der Bodenwand des #415-Dokuments (n = −Z) hat FieldDraft x = +Y / y = +X,
 * Onshape x = +X / y = −Y — also 90° Unterschied. Wer Onshapes 2D-Koordinaten
 * wörtlich einbäckt, legt den Sketch quer in die Ebene.
 *
 * Dieses Modul löst das, ohne an der Konvention einer der beiden Seiten zu
 * rühren: der IMPORTER rechnet um. `B_os` wird an Onshapes eigener Antwort
 * GEMESSEN (`GET /partstudios/…/sketches?includeGeometry=true&output3D=true`
 * meldet jede Entity in Weltkoordinaten — zusammen mit der 2D-Geometrie aus dem
 * Feature-Payload ist der Frame überbestimmt und damit prüfbar statt geraten),
 * `B_fd` kommt aus dem gemeinsamen Resolver `../sketchPlaneFrame`. Die
 * In-Plane-Abbildung `B_fd⁻¹ ∘ B_os` ist eine exakte 2D-Drehung/Spiegelung samt
 * Ursprungsversatz; für gleiche Frames (Ursprungsebenen) ist sie die Identität.
 *
 * Umgerechnet wird der ONSHAPE-PAYLOAD, nicht das Übersetzungsergebnis: so
 * sehen Sketch-Übersetzung, Region-Innenpunkte (`sketchEntityVertices`) und
 * jeder andere 2D-Leser dieselben Zahlen, ohne dass die Umrechnung an drei
 * Stellen gespiegelt werden müsste.
 */

import type { PlaneFrame } from "../sketchPlaneFrame";

type Vec3 = [number, number, number];
type Vec2 = [number, number];
type AnyMsg = Record<string, any>;

const msg = (x: any): AnyMsg => (x?.message ?? x) as AnyMsg;

const M_TO_MM = 1000;

/** Toleranz für „gleicher Frame" — 1 nm in mm bzw. dimensionslos. */
const EPS = 1e-9;
/** Toleranz für die Messung selbst: Onshape liefert Doubles in Metern. */
const FIT_TOL_MM = 1e-6;

// ── Onshape `sketches?output3D=true` (die gelesene Teilmenge) ───────

export interface OnshapeSketch3dEntity {
  id?: string;
  entityType?: string;
  point?: number[];
  center?: number[];
}

export interface OnshapeSketch3d {
  /** Der Feature-Id des Sketches — der Schlüssel zum Feature-Payload. */
  featureId?: string;
  /** Anzeigename („Sketch 3") — nur für Meldungen. */
  sketch?: string;
  geomEntities?: OnshapeSketch3dEntity[];
}

export interface OnshapeSketches3d {
  sketches?: OnshapeSketch3d[];
}

/** Die 3D-Antwort nach Feature-Id aufgeschlüsselt. Fehlt der Payload, ist die
 *  Map leer und der Import verhält sich exakt wie ohne diese Datenquelle. */
export function sketches3dByFeature(
  payload: OnshapeSketches3d | null | undefined,
): Map<string, OnshapeSketch3d> {
  const out = new Map<string, OnshapeSketch3d>();
  for (const raw of payload?.sketches ?? []) {
    const s = msg(raw) as OnshapeSketch3d;
    if (typeof s.featureId === "string") out.set(s.featureId, s);
  }
  return out;
}

// ── Vektor-Kleinkram ───────────────────────────────────────────────

const dot3 = (a: Vec3, b: Vec3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross3 = (a: Vec3, b: Vec3): Vec3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norm3 = (a: Vec3): number => Math.hypot(a[0], a[1], a[2]);

/** 3×3 mit Teilpivotisierung. `null`, wenn das System entartet ist — genau der
 *  Fall „alle Stützpunkte des Sketches liegen auf einer Geraden", in dem der
 *  Frame aus den Punkten allein nicht folgt. */
function solve3(a: number[][], b: number[]): number[] | null {
  const m = [
    [a[0][0], a[0][1], a[0][2], b[0]],
    [a[1][0], a[1][1], a[1][2], b[1]],
    [a[2][0], a[2][1], a[2][2], b[2]],
  ];
  const scale = Math.max(...m.flat().map(Math.abs), 1e-300);
  for (let col = 0; col < 3; col++) {
    let piv = col;
    for (let r = col + 1; r < 3; r++) {
      if (Math.abs(m[r][col]) > Math.abs(m[piv][col])) piv = r;
    }
    if (Math.abs(m[piv][col]) < 1e-12 * scale) return null;
    [m[col], m[piv]] = [m[piv], m[col]];
    for (let r = 0; r < 3; r++) {
      if (r === col) continue;
      const f = m[r][col] / m[col][col];
      for (let c = col; c < 4; c++) m[r][c] -= f * m[col][c];
    }
  }
  return [m[0][3] / m[0][0], m[1][3] / m[1][1], m[2][3] / m[2][2]];
}

// ── B_os messen ────────────────────────────────────────────────────

export interface MeasuredSketchFrame {
  /** Weltursprung des Sketches in MILLIMETERN (der Payload rechnet in Metern). */
  origin: Vec3;
  xAxis: Vec3;
  yAxis: Vec3;
  /** x × y — die Orientierung, die Onshape der Ebene gibt. */
  normal: Vec3;
  /** Größter Restfehler der Anpassung, in mm. */
  residualMm: number;
  /** Wie viele 2D↔3D-Paare die Messung trugen. */
  samples: number;
}

/** Jeder 2D-Punkt eines Sketch-Features, den wir benennen können — in METERN,
 *  unter dem Id, den die `sketches`-Antwort für ihn verwendet. */
export function sketchPoints2d(feat: AnyMsg): Map<string, Vec2> {
  const out = new Map<string, Vec2>();
  for (const raw of (feat.geometry ?? feat.entities ?? []) as AnyMsg[]) {
    const e = msg(raw);
    const id = e.entityId as string | undefined;
    if (!id) continue;
    const g = msg(e.geometry);
    const bt = typeof g?.btType === "string" ? g.btType : "";
    if (typeof e.x === "number" && typeof e.y === "number") {
      out.set(id, [e.x, e.y]);
      continue;
    }
    if (bt.startsWith("BTCurveGeometryLine")) {
      for (const [suffix, p] of [
        ["start", e.startParam],
        ["end", e.endParam],
      ] as const) {
        if (typeof p !== "number") continue;
        out.set(`${id}.${suffix}`, [g.pntX + p * g.dirX, g.pntY + p * g.dirY]);
      }
    } else if (
      bt.startsWith("BTCurveGeometryCircle") ||
      bt.startsWith("BTCurveGeometryEllipse")
    ) {
      if (typeof g.xCenter === "number" && typeof g.yCenter === "number") {
        out.set(`${id}.center`, [g.xCenter, g.yCenter]);
      }
    }
  }
  return out;
}

/**
 * Onshapes eigenen Sketch-Frame aus 2D↔3D-Korrespondenz bestimmen.
 *
 * Ausgleichsrechnung über alle Paare (drei Unbekannte je Weltachse: x, y,
 * Ursprung), danach die Gegenprobe: die Achsen müssen orthonormal sein und
 * jeder Punkt muss auf 1e-6 mm getroffen werden. Ist das Gleichungssystem
 * entartet (alle Punkte kollinear — #415 „Sketch 5", vier Ellipsen auf einer
 * Linie) oder passt die Rekonstruktion nicht, liefert die Funktion `null`
 * statt eines geratenen Frames.
 */
export function measureOnshapeSketchFrame(
  feat: AnyMsg,
  three: OnshapeSketch3d | undefined,
): MeasuredSketchFrame | null {
  if (!three) return null;
  const flat = sketchPoints2d(feat);
  const byId = new Map<string, OnshapeSketch3dEntity>();
  for (const raw of three.geomEntities ?? []) {
    const e = msg(raw) as OnshapeSketch3dEntity;
    if (typeof e.id === "string") byId.set(e.id, e);
  }

  const pairs: { u: number; v: number; p: Vec3 }[] = [];
  for (const [id, [u, v]] of flat) {
    const e = byId.get(id);
    const p = e?.point ?? e?.center;
    if (!p || p.length !== 3) continue;
    pairs.push({ u, v, p: [p[0], p[1], p[2]] });
  }
  if (pairs.length < 3) return null;

  // Normalgleichungen über die Designmatrix [u, v, 1].
  let n00 = 0,
    n01 = 0,
    n02 = 0,
    n11 = 0,
    n12 = 0,
    n22 = 0;
  const bu: Vec3 = [0, 0, 0];
  const bv: Vec3 = [0, 0, 0];
  const b1: Vec3 = [0, 0, 0];
  for (const { u, v, p } of pairs) {
    n00 += u * u;
    n01 += u * v;
    n02 += u;
    n11 += v * v;
    n12 += v;
    n22 += 1;
    for (let c = 0; c < 3; c++) {
      bu[c] += u * p[c];
      bv[c] += v * p[c];
      b1[c] += p[c];
    }
  }
  const normal3 = [
    [n00, n01, n02],
    [n01, n11, n12],
    [n02, n12, n22],
  ];
  const xAxis: Vec3 = [0, 0, 0];
  const yAxis: Vec3 = [0, 0, 0];
  const origin: Vec3 = [0, 0, 0];
  for (let c = 0; c < 3; c++) {
    const s = solve3(normal3, [bu[c], bv[c], b1[c]]);
    if (!s) return null;
    xAxis[c] = s[0];
    yAxis[c] = s[1];
    origin[c] = s[2];
  }

  // Gegenprobe 1: orthonormale Achsen (sonst war die Anpassung unterbestimmt).
  if (
    Math.abs(norm3(xAxis) - 1) > 1e-6 ||
    Math.abs(norm3(yAxis) - 1) > 1e-6 ||
    Math.abs(dot3(xAxis, yAxis)) > 1e-6
  ) {
    return null;
  }
  // Gegenprobe 2: der Frame trifft jeden Punkt, den er erklären soll.
  let residualMm = 0;
  for (const { u, v, p } of pairs) {
    for (let c = 0; c < 3; c++) {
      const world = origin[c] + u * xAxis[c] + v * yAxis[c];
      residualMm = Math.max(residualMm, Math.abs(world - p[c]) * M_TO_MM);
    }
  }
  if (residualMm > FIT_TOL_MM) return null;

  return {
    origin: [origin[0] * M_TO_MM, origin[1] * M_TO_MM, origin[2] * M_TO_MM],
    xAxis,
    yAxis,
    normal: cross3(xAxis, yAxis),
    residualMm,
    samples: pairs.length,
  };
}

// ── B_fd⁻¹ ∘ B_os ──────────────────────────────────────────────────

/** Negative Null einebnen — sie vergleicht sich nicht wie 0 und hat in
 *  Koordinaten nichts zu suchen. */
const nz = (v: number): number => (v === 0 ? 0 : v);

/** Auf 0/±1 einrasten, wo der Wert das offensichtlich meint — Onshapes Doubles
 *  liefern `0.9999999999999998` für eine achsparallele Drehung. */
const snap = (v: number): number => {
  const r = Math.round(v);
  return nz(Math.abs(v - r) < EPS ? r : v);
};

export interface SketchXform {
  /** `u' = a·u + b·v + tx`, `v' = c·u + d·v + ty` (mm bzw. dimensionslos). */
  a: number;
  b: number;
  c: number;
  d: number;
  /** Ursprungsversatz IN FieldDrafts Ebene, mm. */
  tx: number;
  ty: number;
  /** +1 Drehung, −1 Spiegelung (die Ebenennormalen zeigen dann gegeneinander). */
  det: number;
  /** Drehwinkel (det = +1) bzw. doppelter Winkel der Spiegelachse (det = −1). */
  angle: number;
  /** Nichts zu tun — die beiden Frames sind derselbe. */
  identity: boolean;
  /** Bildet Achsen auf Achsen ab; nur dann überleben HORIZONTAL/VERTICAL. */
  axisAligned: boolean;
  /** x_os → ±y_fd: horizontal und vertikal tauschen die Rollen. */
  axesSwapped: boolean;
  /** |n_os · n_fd| — 1 sagt „gleiche Ebene". Alles andere heißt: die
   *  FieldDraft-Ebenen-Query hat eine ANDERE Fläche erwischt. */
  normalDot: number;
  /** Abstand der beiden Ebenen entlang der FieldDraft-Normalen, mm. */
  outOfPlaneMm: number;
}

/** Die In-Plane-Abbildung, die Onshapes 2D-Koordinaten in FieldDrafts Basis
 *  trägt. Exakt: beide Frames sind orthonormal, das Ergebnis ist eine
 *  Drehung/Spiegelung plus Verschiebung — keine Näherung, keine Heuristik. */
export function inPlaneXform(os: MeasuredSketchFrame, fd: PlaneFrame): SketchXform {
  const fdX = fd.xAxis as Vec3;
  const fdY = fd.yAxis as Vec3;
  const fdN = fd.normal as Vec3;
  const delta: Vec3 = [
    os.origin[0] - fd.origin[0],
    os.origin[1] - fd.origin[1],
    os.origin[2] - fd.origin[2],
  ];
  const a = snap(dot3(os.xAxis, fdX));
  const b = snap(dot3(os.yAxis, fdX));
  const c = snap(dot3(os.xAxis, fdY));
  const d = snap(dot3(os.yAxis, fdY));
  const tx = snap(dot3(delta, fdX));
  const ty = snap(dot3(delta, fdY));
  const det = a * d - b * c;
  const isInt = (v: number) => Math.abs(v - Math.round(v)) < EPS;
  const axisAligned = isInt(a) && isInt(b) && isInt(c) && isInt(d);
  return {
    a,
    b,
    c,
    d,
    tx,
    ty,
    det,
    angle: Math.atan2(c, a),
    identity:
      Math.abs(a - 1) < EPS &&
      Math.abs(d - 1) < EPS &&
      Math.abs(b) < EPS &&
      Math.abs(c) < EPS &&
      Math.abs(tx) < EPS &&
      Math.abs(ty) < EPS,
    axisAligned,
    axesSwapped: axisAligned && Math.abs(a) < EPS,
    normalDot: snap(dot3(os.normal, fdN)),
    outOfPlaneMm: snap(dot3(delta, fdN)),
  };
}

/** Einen 2D-Punkt (Meter im Onshape-Payload) in FieldDrafts Basis abbilden. */
function mapPointM(x: SketchXform, u: number, v: number): Vec2 {
  return [
    nz(x.a * u + x.b * v + x.tx / M_TO_MM),
    nz(x.c * u + x.d * v + x.ty / M_TO_MM),
  ];
}

/** Eine RICHTUNG abbilden — ohne den Ursprungsversatz. */
function mapDir(x: SketchXform, u: number, v: number): Vec2 {
  return [nz(x.a * u + x.b * v), nz(x.c * u + x.d * v)];
}

// ── Den Payload umschreiben ────────────────────────────────────────

/** Onshape-Ids, die der Sketch-Übersetzer auf FieldDrafts Ursprung (Entity 0)
 *  legt. Sitzt Onshapes Ursprung nicht auf FieldDrafts, ist jede Bedingung
 *  darauf gegenstandslos — sie zöge Geometrie an einen Punkt, den dieser
 *  Sketch nicht hat. */
const ORIGIN_IDS = new Set(["origin", "Origin", "sketchOrigin"]);

export interface SketchTransformResult {
  /** Der umgeschriebene Feature-Payload — eine Kopie, das Original bleibt. */
  feature: AnyMsg;
  /** Verworfene HORIZONTAL/VERTICAL-Bedingungen (nur bei schiefer Drehung). */
  droppedAxisConstraints: number;
  /** Verworfene Bedingungen auf Onshapes Sketch-Ursprung (nur bei Versatz). */
  droppedOriginConstraints: number;
}

/**
 * Onshapes 2D-Geometrie in FieldDrafts Sketch-Basis umschreiben.
 *
 * Umgeschrieben wird der Payload, nicht das Übersetzungsergebnis — dadurch
 * sehen ALLE 2D-Leser (Sketch-Übersetzung, Region-Innenpunkte, spätere
 * Auswertungen) dieselben Zahlen. Bewegt werden Punkte, Linien-Stützpunkte und
 * -Richtungen, Kreis-/Ellipsen-Mittelpunkte und -Achsen sowie die Winkel eines
 * Bogens; Längen, Radien und Entity-Ids bleiben, weil die Abbildung starr ist.
 *
 * Zwei Bedingungsarten überleben eine Umrechnung nicht wörtlich:
 * - HORIZONTAL/VERTICAL sind an die Achsen gebunden. Vertauscht die Abbildung
 *   die Achsen, tauschen auch sie; bei einer SCHIEFEN Drehung gibt es keine
 *   treue Entsprechung — dann werden sie verworfen (die Geometrie steht ohnehin
 *   explizit da) statt den Solver gegen sie arbeiten zu lassen.
 * - Bedingungen auf Onshapes Sketch-Ursprung, wenn dieser nicht auf
 *   FieldDrafts Ursprung fällt.
 */
export function transformSketchFeature(
  feat: AnyMsg,
  x: SketchXform,
): SketchTransformResult {
  const out: AnyMsg = structuredClone(feat);
  let droppedAxisConstraints = 0;
  let droppedOriginConstraints = 0;

  for (const raw of (out.geometry ?? out.entities ?? []) as AnyMsg[]) {
    const e = msg(raw);
    if (typeof e.x === "number" && typeof e.y === "number") {
      [e.x, e.y] = mapPointM(x, e.x, e.y);
    }
    const g = msg(e.geometry);
    const bt = typeof g?.btType === "string" ? g.btType : "";
    if (bt.startsWith("BTCurveGeometryLine")) {
      // `startParam`/`endParam` sind Bogenlängen entlang `dir` — starr, bleiben.
      [g.pntX, g.pntY] = mapPointM(x, g.pntX, g.pntY);
      [g.dirX, g.dirY] = mapDir(x, g.dirX, g.dirY);
    } else if (
      bt.startsWith("BTCurveGeometryCircle") ||
      bt.startsWith("BTCurveGeometryEllipse")
    ) {
      [g.xCenter, g.yCenter] = mapPointM(x, g.xCenter, g.yCenter);
      // `startParam`/`endParam` sind Winkel im EIGENEN Frame des Kreises, nicht
      // in der Skizze (#444) — starr abgebildet wird deshalb der FRAME, nicht
      // der Winkel. Bei einer Spiegelung kippt zusaetzlich sein Umlaufsinn: die
      // Senkrechte zur x-Achse zeigt danach zur anderen Seite. Die Parameter
      // selbst bleiben, und mit ihnen bleibt Start Start.
      [g.xDir, g.yDir] = mapDir(
        x,
        typeof g.xDir === "number" ? g.xDir : 1,
        typeof g.yDir === "number" ? g.yDir : 0,
      );
      if (x.det < 0) g.clockwise = !g.clockwise;
    }
  }

  const keep: AnyMsg[] = [];
  for (const raw of (out.constraints ?? []) as AnyMsg[]) {
    const con = msg(raw);
    const type = con.constraintType;
    if (type === "HORIZONTAL" || type === "VERTICAL") {
      if (!x.axisAligned) {
        droppedAxisConstraints += 1;
        continue;
      }
      if (x.axesSwapped) {
        con.constraintType = type === "HORIZONTAL" ? "VERTICAL" : "HORIZONTAL";
      }
    }
    if (Math.hypot(x.tx, x.ty) > EPS && referencesOrigin(con)) {
      droppedOriginConstraints += 1;
      continue;
    }
    keep.push(raw);
  }
  if (out.constraints) out.constraints = keep;

  return { feature: out, droppedAxisConstraints, droppedOriginConstraints };
}

function referencesOrigin(con: AnyMsg): boolean {
  for (const raw of (con.parameters ?? []) as AnyMsg[]) {
    const p = msg(raw);
    if (typeof p.value === "string" && ORIGIN_IDS.has(p.value)) return true;
  }
  return false;
}

/** Menschenlesbare Kurzform einer Abbildung — für Import-Meldungen. */
export function describeXform(x: SketchXform): string {
  const deg = (Math.round(((x.angle * 180) / Math.PI) * 1000) / 1000 + 360) % 360;
  const turn = x.det >= 0 ? `${deg}° gedreht` : `an ${deg / 2}° gespiegelt`;
  const shift =
    Math.hypot(x.tx, x.ty) > EPS
      ? `, Ursprung um (${x.tx.toFixed(3)}; ${x.ty.toFixed(3)}) mm versetzt`
      : "";
  return `${turn}${shift}`;
}
