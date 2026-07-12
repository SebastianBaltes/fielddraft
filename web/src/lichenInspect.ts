/**
 * Lichen-Punkt-Inspektor (Betreiber-Wunsch 2026-07-12).
 *
 * Baut aus dem geparsten Kernel-Snapshot einen VOLLSTÄNDIGEN, kopierbaren
 * Diagnose-Block zu einem angeklickten Punkt — genug, damit der Agent ohne
 * Rückfragen versteht, worüber gesprochen wird: Position, Klassifikation,
 * SDF-Wert, U(p), alle Flächen-Normalen, Nachbarn im Netz, und welche
 * Defekt-Klassen (offene Kante / 2-Stimmen / überlang) den Punkt berühren.
 */

import type { LichenSnapshot } from "./lichenDebugStore";

const FLAG_LABEL: Record<number, string> = {
  1: "Oberflächen-Partikel",
  2: "Kantenzug (B-Rep-Kante)",
  3: "Ecke (B-Rep-Vertex, gepinnt)",
};

const f = (x: number, n = 3): string => x.toFixed(n);

export interface LichenPointInfo {
  index: number;
  pos: [number, number, number];
  sdf: number;
  u: number;
  flag: number;
  normals: [number, number, number][];
  neighbours: number[];
  triangles: number[][];
  openEdges: [number, number][];
  longEdges: [number, number][];
  weakTris: number[][];
}

export function inspectPoint(
  snap: LichenSnapshot,
  i: number,
): LichenPointInfo | null {
  if (i < 0 || i >= snap.total) return null;
  const d = snap.data;
  const b = snap.P + i * 9;
  const pos: [number, number, number] = [d[b], d[b + 1], d[b + 2]];

  // Flächen-Normalen aus dem Trailing-Record-Block (eine pro angrenzender
  // Fläche — Kantenpunkte tragen zwei, Ecken ≥3).
  const normals: [number, number, number][] = [];
  for (let r = 0; r < snap.nFaceRec; r++) {
    const o = snap.faceBase + 1 + r * 4;
    if (d[o] === i) normals.push([d[o + 1], d[o + 2], d[o + 3]]);
  }

  // Inzidente Dreiecke + Nachbarn.
  const triangles: number[][] = [];
  const nb = new Set<number>();
  for (let t = 0; t < snap.triCount; t++) {
    const a = d[snap.I + t * 3];
    const bb = d[snap.I + t * 3 + 1];
    const c = d[snap.I + t * 3 + 2];
    if (a === i || bb === i || c === i) {
      triangles.push([a, bb, c]);
      for (const x of [a, bb, c]) if (x !== i) nb.add(x);
    }
  }

  const openEdges: [number, number][] = [];
  for (let e = 0; e < snap.nOpenEdge; e++) {
    const a = d[snap.diagBase + 1 + e * 2];
    const bb = d[snap.diagBase + 1 + e * 2 + 1];
    if (a === i || bb === i) openEdges.push([a, bb]);
  }
  const longEdges: [number, number][] = [];
  for (let e = 0; e < snap.nLongEdge; e++) {
    const a = d[snap.longBase + 1 + e * 2];
    const bb = d[snap.longBase + 1 + e * 2 + 1];
    if (a === i || bb === i) longEdges.push([a, bb]);
  }
  const weakTris: number[][] = [];
  for (let t = 0; t < snap.nWeakTri; t++) {
    const a = d[snap.weakBase + 1 + t * 3];
    const bb = d[snap.weakBase + 1 + t * 3 + 1];
    const c = d[snap.weakBase + 1 + t * 3 + 2];
    if (a === i || bb === i || c === i) weakTris.push([a, bb, c]);
  }

  return {
    index: i,
    pos,
    sdf: d[b + 3],
    u: d[b + 7],
    flag: d[b + 8],
    normals,
    neighbours: [...nb].sort((x, y) => x - y),
    triangles,
    openEdges,
    longEdges,
    weakTris,
  };
}

/** Kompakter Text-Block für die Zwischenablage (an den Agenten). */
export function formatPointReport(
  info: LichenPointInfo,
  snap: LichenSnapshot,
  settings: Record<string, unknown>,
): string {
  const d = snap.data;
  const at = (i: number): string => {
    const b = snap.P + i * 9;
    return `[${f(d[b])}, ${f(d[b + 1])}, ${f(d[b + 2])}]`;
  };
  const flagOf = (i: number): string => {
    const fl = d[snap.P + i * 9 + 8];
    return FLAG_LABEL[fl] ?? `flag ${fl}`;
  };
  const L: string[] = [];
  L.push(`LICHEN-PUNKT #${info.index}`);
  L.push(`  Position   ${at(info.index)}`);
  L.push(`  Klasse     ${FLAG_LABEL[info.flag] ?? info.flag}`);
  L.push(`  SDF f(p)   ${info.sdf.toExponential(2)}`);
  L.push(`  U(p)       ${f(info.u)}`);
  L.push(
    `  Normalen   ${
      info.normals.length === 0
        ? "—"
        : info.normals
            .map((n) => `[${f(n[0], 2)}, ${f(n[1], 2)}, ${f(n[2], 2)}]`)
            .join("  ")
    }  (eine je angrenzender Fläche)`,
  );
  L.push(`  Grad       ${info.neighbours.length} Nachbarn, ${info.triangles.length} Dreiecke`);
  L.push("  Nachbarn:");
  for (const j of info.neighbours) {
    const b = snap.P + info.index * 9;
    const c = snap.P + j * 9;
    const dist = Math.hypot(d[b] - d[c], d[b + 1] - d[c + 1], d[b + 2] - d[c + 2]);
    L.push(`    #${j} ${at(j)} d=${f(dist)} — ${flagOf(j)}`);
  }
  if (info.openEdges.length > 0) {
    L.push(`  OFFENE KANTEN (rot): ${info.openEdges.map(([a, b2]) => `#${a}–#${b2}`).join(", ")}`);
  }
  if (info.longEdges.length > 0) {
    L.push(`  ÜBERLANGE KANTEN (violett): ${info.longEdges.map(([a, b2]) => `#${a}–#${b2}`).join(", ")}`);
  }
  if (info.weakTris.length > 0) {
    L.push(
      `  2-STIMMEN-KANDIDATEN (orange): ${info.weakTris
        .map((t) => `(${t.map((x) => `#${x}`).join(",")})`)
        .join(", ")}`,
    );
  }
  L.push(`  Netz: ${snap.total} Punkte, ${snap.triCount} Dreiecke, ${snap.nOpenEdge} offene Kanten`);
  L.push(`  Settings: ${JSON.stringify(settings)}`);
  return L.join("\n");
}
