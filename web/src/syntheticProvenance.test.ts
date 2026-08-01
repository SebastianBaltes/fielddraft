/**
 * #384 §9 — zweistufiges Picking auf synthetischen Offset-Flächen.
 *
 * Der Kernel liefert im Pick-Payload jetzt `synthetic`: Klasse + Eltern-
 * Referenzen jeder Fläche, die die Offset/Shell-Expansion synthetisiert hat.
 * Hier wird die Web-Seite geprüft: Eltern-Branch-Ids → reguläre
 * `PickedElement`s der analytischen B-Rep.
 */

import { describe, expect, it } from "vitest";
import {
  SYNTHETIC_KIND_LABEL,
  primarySynthetic,
  syntheticFaces,
  syntheticParentElements,
  type SyntheticProvenanceKernel,
} from "./syntheticProvenance";
import { FEATURE_SHIFT, SUB_BITS } from "./selectionLinks";
import type { BRepFace, PickResult, SyntheticFace } from "./kernel";

const pack = (feature: number, sub = 0, instance = 0) =>
  feature * 2 ** FEATURE_SHIFT + instance * 2 ** SUB_BITS + sub;

/** Box-Deckel (Feature 1, sub 4) und Zylindermantel (Feature 2, sub 0) — die
 *  beiden Kanten-Eltern des Kanals in der Kernel-Regression. */
const PARENT_A = pack(1, 4);
const PARENT_B = pack(2, 0);
/** Der Kanal selbst: Feature 1, sub im reservierten 0x700+-Band. */
const CHANNEL = pack(1, 0x700);

function face(idHash: string, branchId: number, query?: string): BRepFace {
  return {
    faceIndex: 0,
    idHash,
    ...(query ? { query } : {}),
    kind: "face",
    branchId,
    featureId: Math.floor(branchId / 2 ** FEATURE_SHIFT),
    surfaceKind: "plane",
    centroidLocal: [0, 0, 0],
    normalLocal: [0, 0, 1],
  };
}

function stubKernel(faces: BRepFace[][]): SyntheticProvenanceKernel {
  return {
    bodyCount: () => faces.length,
    bodyBrepFaces: (i: number) => faces[i],
  };
}

function hit(synthetic?: SyntheticFace[]): PickResult {
  return {
    hit: true,
    point: [0, 0, 0],
    normal: [0, 0, 1],
    t: 1,
    element: "face",
    contributors: [1],
    branches: [CHANNEL],
    ...(synthetic ? { synthetic } : {}),
  } as PickResult;
}

const channelHit = hit([
  { branch: CHANNEL, kind: "channel", owner: 1, parents: [PARENT_A, PARENT_B] },
]);

describe("syntheticProvenance", () => {
  it("reports nothing for ordinary geometry or a miss", () => {
    expect(syntheticFaces(null)).toEqual([]);
    expect(syntheticFaces({ hit: false })).toEqual([]);
    expect(syntheticFaces(hit([]))).toEqual([]);
    // Älterer Kernel ohne das Feld: kein Crash, keine Erfindung.
    expect(syntheticFaces(hit())).toEqual([]);
    expect(primarySynthetic(hit())).toBeNull();
  });

  it("prefers the synthetic face that actually carries parents", () => {
    const prism: SyntheticFace = {
      branch: pack(0x80040, 0x1000),
      kind: "prism",
      owner: 1,
      parents: [],
    };
    const channel: SyntheticFace = {
      branch: CHANNEL,
      kind: "channel",
      owner: 1,
      parents: [PARENT_A, PARENT_B],
    };
    expect(primarySynthetic(hit([prism, channel]))?.kind).toBe("channel");
    // Ohne Eltern-Träger gewinnt schlicht der erste Eintrag.
    expect(primarySynthetic(hit([prism]))?.kind).toBe("prism");
  });

  it("resolves both edge parents to B-Rep face elements", () => {
    const kernel = stubKernel([
      [face("h-top", PARENT_A, "qFace(h-top @ fp)"), face("h-side", pack(1, 5))],
      [face("h-shell", PARENT_B)],
    ]);
    const parents = syntheticParentElements(kernel, channelHit);
    expect(parents).toEqual([
      { kind: "face", idHash: "h-top", query: "qFace(h-top @ fp)" },
      { kind: "face", idHash: "h-shell" },
    ]);
  });

  it("keeps the kernel's parent order and drops unresolvable parents", () => {
    // Nur der zweite Elternteil ist gewoben (der erste steckt z. B. komplett
    // im Körper) — die Referenz fällt still weg statt zu raten.
    const kernel = stubKernel([[face("h-shell", PARENT_B)]]);
    expect(syntheticParentElements(kernel, channelHit)).toEqual([
      { kind: "face", idHash: "h-shell" },
    ]);
  });

  it("returns nothing when the hit carries no parents", () => {
    const kernel = stubKernel([[face("h-top", PARENT_A)]]);
    const prismHit = hit([
      { branch: pack(0x80040, 0x1000), kind: "prism", owner: 1, parents: [] },
    ]);
    expect(syntheticParentElements(kernel, prismHit)).toEqual([]);
    expect(syntheticParentElements(kernel, null)).toEqual([]);
  });

  it("survives a kernel that throws mid-regen", () => {
    const kernel: SyntheticProvenanceKernel = {
      bodyCount: () => 2,
      bodyBrepFaces: (i: number) => {
        if (i === 0) throw new Error("mid-regen");
        return [face("h-shell", PARENT_B)];
      },
    };
    expect(syntheticParentElements(kernel, channelHit)).toEqual([
      { kind: "face", idHash: "h-shell" },
    ]);
  });

  it("labels every synthetic class in German", () => {
    expect(SYNTHETIC_KIND_LABEL.channel).toBe("Offset-Kanal");
    expect(SYNTHETIC_KIND_LABEL.vertex).toBe("Offset-Eckkugel");
    expect(SYNTHETIC_KIND_LABEL.prism).toBe("Kontakt-Prisma");
  });
});
