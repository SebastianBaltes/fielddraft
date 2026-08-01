/**
 * Zweistufiges Picking auf synthetischen Offset-Flächen
 * (#384, docs/OFFSET-TREE-PLAN.md §9).
 *
 * Offset/Shell expandieren zur Regen-Zeit in Korrekturkörper, die kein
 * Dokument-Feature gezeichnet hat: **Kanäle** an den dualen Boolean-Nähten,
 * **Vertex-Kugeln** an Triple-Junctions und **Kontakt-Prismen** an koplanaren
 * Kontakten. Sie sind pickbar (der Pick-Baum fährt dieselbe Expansion wie der
 * Render-Pfad), tauchen aber in KEINER gewobenen B-Rep auf — es gibt also
 * weder `qFace`-Hash noch Fläche im Outliner.
 *
 * Die Regel dafür ist zweistufig:
 *
 *  1. **Identität** bleibt die getroffene Fläche selbst (ihre Branch-Id) bzw.
 *     das Feature, das den Körper synthetisiert hat (`owner`).
 *  2. **Referenz** sind die Eltern: ein Kanal *erbt* die beiden Flächen,
 *     deren Naht er rundet — für Bemaßung und Snapping, nicht für Identität.
 *
 * Dieses Modul ist die Web-Seite von (2): es findet die Eltern-Flächen in den
 * analytischen B-Reps und liefert sie als reguläre `PickedElement`s zurück,
 * mit denen der Rest der App (Measure, Element-Queries) schon umgehen kann.
 */

import type { BRepFace, PickResult, SyntheticFace, SyntheticKind } from "./kernel";
import type { PickedElement } from "./elementQuery";

/** Deutsche Bezeichnung der Flächenklasse (UI-Texte, Fehlermeldungen). */
export const SYNTHETIC_KIND_LABEL: Record<SyntheticKind, string> = {
  channel: "Offset-Kanal",
  vertex: "Offset-Eckkugel",
  prism: "Kontakt-Prisma",
};

/** Die synthetischen Flächen eines Pick-Treffers (leer bei gewöhnlicher
 *  Geometrie oder auf einem älteren Kernel ohne das Feld). */
export function syntheticFaces(hit: PickResult | null): SyntheticFace[] {
  if (!hit || !hit.hit) return [];
  return hit.synthetic ?? [];
}

/** Die „vorderste" synthetische Fläche eines Treffers: die erste, die
 *  überhaupt Eltern mitbringt, sonst die erste überhaupt. So gewinnt am
 *  Kanal-Rand der Kanal (mit Eltern) gegen eine bloß mitgetroffene
 *  Prismenfläche. `null`, wenn der Treffer rein reguläre Geometrie ist. */
export function primarySynthetic(hit: PickResult | null): SyntheticFace | null {
  const all = syntheticFaces(hit);
  return all.find((s) => s.parents.length > 0) ?? all[0] ?? null;
}

/** Die Kernel-Scheibe, die dieses Modul liest. */
export interface SyntheticProvenanceKernel {
  bodyCount(): number;
  bodyBrepFaces(i: number): BRepFace[];
}

/**
 * Die Eltern-Flächen eines Pick-Treffers auf einer synthetischen Fläche, als
 * reguläre `PickedElement`s (Stufe 2: Mess-/Snap-Referenz).
 *
 * Aufgelöst wird über die Branch-Id — genau der Schlüssel, den die B-Rep-
 * Flächen tragen. Eltern ohne Treffer (mid-Regen, oder eine Fläche, die die
 * Expansion vollständig verschluckt hat) fallen still weg; die Reihenfolge
 * der gefundenen bleibt die des Kernels (geordnetes Paar). Leer, wenn der
 * Treffer keine synthetische Fläche ist.
 */
export function syntheticParentElements(
  kernel: SyntheticProvenanceKernel,
  hit: PickResult | null,
): PickedElement[] {
  const syn = primarySynthetic(hit);
  if (!syn || syn.parents.length === 0) return [];

  const byBranch = new Map<number, BRepFace>();
  let count = 0;
  try {
    count = kernel.bodyCount();
  } catch {
    return [];
  }
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    try {
      faces = kernel.bodyBrepFaces(i);
    } catch {
      continue;
    }
    for (const f of faces) {
      if (!byBranch.has(f.branchId)) byBranch.set(f.branchId, f);
    }
  }

  const out: PickedElement[] = [];
  for (const branch of syn.parents) {
    const face = byBranch.get(branch);
    if (!face) continue;
    const el: PickedElement = { kind: "face", idHash: face.idHash };
    if (face.query) el.query = face.query;
    if (!out.some((e) => e.idHash === el.idHash)) out.push(el);
  }
  return out;
}
