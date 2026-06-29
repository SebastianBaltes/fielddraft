/**
 * Screen-Space-CSG — Stage 1 (OpenCSG-Stil, EIN Boolean-Schritt)
 * docs/ADAPTIVE-RENDERING-PLAN.md → "Achse 3: WebGL-Pfad + Screenspace-CSG",
 * Phasenplan Phase 3 ("Screenspace-CSG Stufe 1 (einstufige A−B) als Experiment").
 *
 * EXPERIMENTELL & OPT-IN. Aktiv NUR wenn das Setting `fielddraft.screenSpaceCsg`
 * an ist (default AUS). Mit Flag AUS wird dieses Modul gar nicht gemountet → der
 * bestehende WebGL- und WebGPU-Pfad bleibt bit-für-bit unverändert.
 *
 * ─────────────────────────────────────────────────────────────────────────────
 * EHRLICHKEIT (Pflicht): Der GPU-Compositor (Tiefen-Capture per WebGL2-MRT +
 * Fullscreen-Fragmentshader) ist hier verdrahtet und kompiliert, aber die
 * VISUELLE Korrektheit ist HEADLESS NICHT verifizierbar (kein Browser/GPU in der
 * Build-Umgebung). Headless verifiziert ist ausschließlich die REINE Logik
 * unten (Operanden-Sammlung + Intervall-Klassifikation), siehe
 * screenSpaceCsg.test.ts. Ob die gerenderten Schnittflächen stimmen, muss der
 * User im Browser prüfen.
 *
 * Abgedeckt (Stage 1):
 *   - A − B (Subtraktion), einstufig, ohne Depth-Peeling. Operanden kommen aus
 *     `bodyHybridInfo`: Minuend = `role:"mesh"`-Leaves, Subtrahend =
 *     `role:"skip"`-Leaves (genau die, die der WebGL-Pfad heute weglässt — der
 *     Klassifikator markiert die rechte Seite eines `Difference` als "skip").
 *
 * Bewusst NICHT abgedeckt (Stage 2 / später):
 *   - Allgemeiner CSG-Baum, Depth-Peeling (k Layer, Goldfeather/SCS).
 *   - Union-Compositing für mdo0 (überlappende `role:"mesh"`-Hüllen): braucht
 *     min-Tiefe statt Intervall-Subtraktion; siehe `classifyUnionPixel` (reine
 *     Logik schon da & getestet), aber der GPU-Pfad fasst die Union-Leaves
 *     vorerst nur opak zusammen (kein Z-Fight-Resolve). Ehrlich offen.
 *   - Mehrere Subtrahenden pro Body in EINEM Pass (aktuell: ein A−B-Pass je
 *     Subtrahend, sequenziell — korrekt, aber nicht der billigste Pfad).
 */

import * as THREE from "three";
import {
  createPrimitiveGeometry,
  buildWarpedLeafGeometry,
  applyLeafTransform,
} from "./sdfDeferredRenderer";
import type { HybridLeafInfo } from "./kernel";
import { LIGHTS, AMBIENT } from "./pbrShading";

/* ════════════════════════════════════════════════════════════════════════
 * TEIL 1 — REINE LOGIK (headless testbar, KEINE GPU/THREE-Abhängigkeit für die
 * Klassifikation selbst). Das ist der einzige Teil mit echter Test-Deckung.
 * ════════════════════════════════════════════════════════════════════════ */

/**
 * Ein Tiefenintervall [front, back] eines geschlossenen, konvexen-genug
 * Operanden entlang des Sicht-Strahls durch einen Pixel. `front <= back`.
 * Beide in linearer Sichttiefe (Kamera-Z, positiv vor der Kamera), NICHT in
 * nicht-linearem NDC — die Klassifikation vergleicht nur Größen, daher genügt
 * eine konsistente monotone Tiefe.
 */
export interface DepthInterval {
  front: number;
  back: number;
}

/**
 * Stage-1-Kernfrage für A − B an EINEM Pixel:
 * Liegt die sichtbare Minuend-Oberflächentiefe `aDepth` INNERHALB des
 * Subtrahend-Intervalls [bFront, bBack]? Wenn ja, ist dieser Punkt von A
 * weggeschnitten — der Pixel muss durch die Subtrahend-Rückfläche (die
 * Schnittfläche) ersetzt werden, sofern diese ihrerseits noch in A liegt.
 *
 * Reine Funktion, exakt das, was der Fragmentshader pro Pixel auswertet —
 * hier headless verifizierbar.
 *
 * @returns
 *   - "keep"    : Minuend-Pixel unverändert sichtbar lassen.
 *   - "cut"     : Minuend-Pixel ersetzen durch die Subtrahend-Backface bei
 *                 `replacementDepth` (= bBack).
 *   - "discard" : Pixel komplett entfernen (Subtrahend-Backface liegt hinter
 *                 dem Minuend-Austritt → kein Material mehr → Loch nach hinten
 *                 offen; für einen geschlossenen Minuend sollte das selten sein,
 *                 wird aber sauber behandelt statt falsch gefüllt).
 */
export interface SubtractionResult {
  action: "keep" | "cut" | "discard";
  replacementDepth: number;
}

const EPS = 1e-5;

export function classifySubtractionPixel(
  aDepth: number,
  bInterval: DepthInterval | null,
  aBackDepth?: number,
): SubtractionResult {
  // Kein Subtrahend an diesem Pixel → Minuend unverändert.
  if (!bInterval) return { action: "keep", replacementDepth: aDepth };

  const { front: bFront, back: bBack } = bInterval;

  // Minuend-Oberfläche VOR dem Subtrahend-Eintritt → nicht betroffen.
  if (aDepth <= bFront + EPS) return { action: "keep", replacementDepth: aDepth };

  // Minuend-Oberfläche HINTER dem Subtrahend-Austritt → der subtrahierte
  // Bereich liegt vor der sichtbaren Fläche, schneidet sie aber nicht weg.
  if (aDepth >= bBack - EPS) return { action: "keep", replacementDepth: aDepth };

  // aDepth liegt echt in (bFront, bBack): der vordere Teil von A ist hier
  // weggeschnitten. Die neue sichtbare Fläche ist die Subtrahend-Rückfläche
  // bei bBack — ABER nur, solange sie noch INNERHALB des Minuend-Körpers liegt
  // (d.h. vor dem Minuend-Austritt aBackDepth). Liegt bBack dahinter, ist an
  // dieser Stelle nach dem Schnitt gar kein Material mehr → discard.
  if (aBackDepth !== undefined && bBack >= aBackDepth - EPS) {
    return { action: "discard", replacementDepth: bBack };
  }
  return { action: "cut", replacementDepth: bBack };
}

/**
 * Mehr-Intervall-Subtraktion für NICHT-KONVEXE Subtrahenden (Ticket yt4o).
 *
 * Ein konvexer Subtrahend wird vom Sicht-Strahl genau zweimal gekreuzt (ein
 * Intervall). Ein nicht-konvexer Subtrahend (z.B. Torus als Ringnut) wird VIER
 * Mal gekreuzt → zwei disjunkte Intervalle (nahe Röhre, ferne Röhre). Der
 * Single-Interval-Walk klassifiziert die ferne Nutwand falsch als "keep", weil
 * die Minuend-Front dort im ZWEITEN Intervall liegt, aber nur gegen das erste
 * geprüft wird → die Box scheint durch.
 *
 * Diese Funktion betrachtet die Vereinigung R = I1 ∪ I2 (I2 optional) und ist
 * exakt das, was der Composite-Shader im Subtraktions-Modus auswertet.
 *
 * KONSISTENZ: Mit `i2 === null` ergibt der Walk bit-genau dasselbe Resultat wie
 * `classifySubtractionPixel` (siehe Tests). Der bestehende 1-Intervall-Fall
 * bleibt damit erhalten.
 *
 * @param aFront  Minuend-Front-Tiefe (≤ 0 ⇒ kein Treffer, hier nicht behandelt:
 *                der Aufrufer/Shader discardet vorher).
 * @param aBack   Minuend-Back-Tiefe (Minuend-Austritt).
 * @param i1      erstes Subtrahend-Intervall (oder null = kein Subtrahend).
 * @param i2      zweites Subtrahend-Intervall jenseits von i1 (oder null).
 */
export function classifySubtractionPixelMulti(
  aFront: number,
  aBack: number,
  i1: DepthInterval | null,
  i2: DepthInterval | null,
): SubtractionResult {
  // Kein Subtrahend → Minuend unverändert.
  if (!i1) return { action: "keep", replacementDepth: aFront };

  const bF1 = i1.front;
  const bB1 = i1.back;
  const hasI2 = i2 !== null;
  const bF2 = i2 ? i2.front : 0;
  const bB2 = i2 ? i2.back : 0;

  // Liegt aFront in der entfernten Vereinigung R = I1 ∪ I2?
  const inI1 = aFront > bF1 + EPS && aFront < bB1 - EPS;
  const inI2 = hasI2 && aFront > bF2 + EPS && aFront < bB2 - EPS;

  // Weder in I1 noch I2 → keep (deckt auch "vor/hinter I1" ab).
  if (!inI1 && !inI2) {
    return { action: "keep", replacementDepth: aFront };
  }

  // aFront in R → Ende des zusammenhängenden entfernten Segments suchen.
  let exitZ: number;
  if (inI1) {
    exitZ = bB1;
    // I1 und I2 überlappen/berühren (verschmolzen) → Segment reicht bis bB2.
    if (hasI2 && bF2 <= bB1 + EPS) {
      exitZ = bB2;
    }
  } else {
    // inI2 ohne inI1 (disjunkt, aFront in der fernen Röhre).
    exitZ = bB2;
  }

  // Segment-Austritt hinter dem Minuend-Austritt → kein Material → discard.
  if (exitZ >= aBack - EPS) {
    return { action: "discard", replacementDepth: exitZ };
  }
  return { action: "cut", replacementDepth: exitZ };
}

/**
 * Stage-1 Union-Klassifikation (reine Logik, schon getestet; GPU-Pfad nutzt sie
 * noch NICHT — siehe Modul-Kopf). Für Union ist die sichtbare Tiefe schlicht das
 * Minimum der Operanden-Frontflächen (näher = sichtbar). Stellt sicher, dass
 * zwei überlappende Hüllen (mdo0) ohne Z-Fighting komponiert würden.
 */
export function classifyUnionPixel(depths: (number | null)[]): number | null {
  let best: number | null = null;
  for (const d of depths) {
    if (d === null) continue;
    if (best === null || d < best) best = d;
  }
  return best;
}

/**
 * Operanden eines Bodys für die einstufige A − B Komposition. Aus
 * `bodyHybridInfo` rein nach `role` getrennt: der WebGL-Pfad zeichnet heute nur
 * die `mesh`-Leaves und lässt die `skip`-Leaves (Subtrahenden) komplett weg —
 * Stage 1 holt genau diese als Schnitt-Operanden zurück.
 */
export interface CsgOperands {
  minuend: HybridLeafInfo[];
  subtrahend: HybridLeafInfo[];
}

/** Reine Aufteilung — keine GPU-Abhängigkeit, daher headless testbar. */
export function collectCsgOperands(leaves: HybridLeafInfo[]): CsgOperands {
  const minuend: HybridLeafInfo[] = [];
  const subtrahend: HybridLeafInfo[] = [];
  for (const leaf of leaves) {
    if (leaf.role === "skip") subtrahend.push(leaf);
    else minuend.push(leaf);
  }
  return { minuend, subtrahend };
}

/**
 * Lohnt sich ein Screen-Space-CSG-Pass für diesen Body? Nur wenn es überhaupt
 * einen Subtrahenden gibt UND einen Minuenden — sonst ist nichts zu schneiden
 * und der bestehende Pfad ist exakt richtig.
 */
export function bodyNeedsSubtractionPass(leaves: HybridLeafInfo[]): boolean {
  const { minuend, subtrahend } = collectCsgOperands(leaves);
  return minuend.length > 0 && subtrahend.length > 0;
}

/* ──────────────────────────────────────────────────────────────────────────
 * Intersection A ∩ B (Stage 1, NUR binär).
 *
 * Am echten Körper verifiziert (siehe Ticket): ein `Intersection(A,B)` hat in
 * `bodyHybridInfo` GENAU zwei Leaves, BEIDE mit `role:"skip"` — KEIN mesh-Leaf,
 * `is_union_only=false`, `needs_full_sdf=false`. Das ist der eindeutige
 * Fingerabdruck: Difference hat ein mesh-Leaf (Minuend), Union ist
 * `is_union_only` und kommt hier gar nicht her.
 * ────────────────────────────────────────────────────────────────────────── */

/** Operanden eines binären Schnitts A ∩ B (beide skip-Leaves, je ein Leaf). */
export interface IntersectionOperands {
  a: HybridLeafInfo[];
  b: HybridLeafInfo[];
}

/** CSG-Modus für den GPU-Compositor. */
export type CsgMode = "subtract" | "intersect";

/**
 * Operanden-Union für `ScreenSpaceCsgCompositor.render`: entweder ein A−B-Satz
 * ({minuend, subtrahend}) oder ein A∩B-Satz ({a, b}). A-Slot = minuend bzw. a,
 * B-Slot = subtrahend bzw. b.
 */
export type CsgRenderOperands = CsgOperands | IntersectionOperands;

/**
 * Erkennt einen binären Intersection-Body: genau zwei Leaves, BEIDE `skip`
 * (kein mesh-Leaf). Difference hat ein mesh-Leaf → false; Union ist
 * `is_union_only` und landet nicht hier.
 */
export function bodyNeedsIntersectionPass(leaves: HybridLeafInfo[]): boolean {
  return (
    leaves.length === 2 && leaves.every((leaf) => leaf.role === "skip")
  );
}

/** Splittet die zwei skip-Leaves in a=[leaf0], b=[leaf1]. */
export function collectIntersectionOperands(
  leaves: HybridLeafInfo[],
): IntersectionOperands {
  return { a: [leaves[0]], b: [leaves[1]] };
}

/**
 * Stage-1-Klassifikation für A ∩ B an EINEM Pixel. Der sichtbare Schnitt ist
 * das Intervall [entry, exit] mit `entry = max(aFront, bFront)` und
 * `exit = min(aBack, bBack)`. Die sichtbare Fläche ist die Frontfläche des
 * Operanden, dessen Front WEITER hinten liegt (= die Schnittgrenze, die den
 * Eintritt bestimmt).
 *
 * Tiefenkonvention identisch zur Subtraktion (lineare Sichttiefe, größer =
 * weiter weg; Front <= 0 = kein Treffer). Reine Funktion → headless testbar;
 * der Composite-Shader wertet dieselbe Logik inline aus.
 *
 * @returns
 *   - `action:"discard"` : kein Treffer in A oder B, oder leerer Schnitt.
 *   - `action:"show"`    : sichtbar bei `depth=entry`; `useA` wählt, welche
 *                          Front-Normale die Schnittgrenze liefert.
 */
export interface IntersectionResult {
  action: "show" | "discard";
  depth: number;
  useA: boolean;
}

export function classifyIntersectionPixel(
  aFront: number,
  aBack: number,
  bFront: number,
  bBack: number,
): IntersectionResult {
  // Kein Treffer in einem der beiden Operanden → nichts zu zeigen.
  if (aFront <= 0 || bFront <= 0) {
    return { action: "discard", depth: 0, useA: false };
  }
  const entry = Math.max(aFront, bFront);
  const exit = Math.min(aBack, bBack);
  // Leeres Schnittintervall (Eintritt nicht echt vor dem Austritt).
  if (entry >= exit - EPS) {
    return { action: "discard", depth: 0, useA: false };
  }
  // Sichtbare Grenze = der Operand mit der weiter hinten liegenden Frontfläche.
  return { action: "show", depth: entry, useA: aFront >= bFront };
}

/* ════════════════════════════════════════════════════════════════════════
 * TEIL 2 — GPU-Compositor (WebGL2-G-Buffer-Capture + Fullscreen-A−B-Shader).
 *
 * Stage 1 rendert eine ECHTE, geshadete Subtraktion A − B: vier RGBA-Float-
 * Render-Targets fangen pro Pixel die lineare Sichttiefe `viewZ` UND die
 * View-Space-Normale der relevanten Flächen, ein Fullscreen-Shader klassifiziert
 * jeden Pixel exakt wie `classifySubtractionPixel`, beleuchtet die sichtbare
 * Fläche (Lambert mit demselben WELT-fixen Szenen-Lichtrig wie der übrige
 * WebGL-Pfad, für Material-Parität) und schreibt Farbe + `gl_FragDepth`, sodass
 * das Ergebnis tiefenkorrekt mit dem übrigen Scene-Render verschmilzt.
 *
 * VISUELL UNVERIFIZIERT (kein Browser/GPU im Build). Headless geprüft ist nur die
 * reine Klassifikation (Teil 1, siehe screenSpaceCsg.test.ts) und die reine
 * viewZ→gl_FragDepth-Formel (`viewZToNdcDepth` unten). Ob die gerenderten
 * Schnittflächen + Normalen-Vorzeichen optisch stimmen, muss der User prüfen.
 *
 * Konventionen (bewusst gewählt, ggf. final visuell zu justieren):
 *   - viewZ == 0 (≤ 0) ⇒ KEIN Treffer. Echte Treffer haben viewZ ≥ near > 0.
 *     KEIN 1e30-Sentinel/setClearColor-Hack (Float-Clear clampt nicht zuverlässig
 *     überall); Clear ist schlicht (0,0,0,0).
 *   - Schnittflächen-Normale = `-bBack.vn` (invertierte Subtrahend-Außennormale):
 *     die Schnittfläche zeigt INS entfernte Volumen. Vorzeichen ist die größte
 *     Unsicherheit — falls die Schnittfläche zu dunkel/hell erscheint, hier flippen.
 *   - gl_FragDepth via WebGL2/GLSL3 (`gl_FragDepth` nativ, kein Extension-Boilerplate).
 * ════════════════════════════════════════════════════════════════════════ */

/** Baut die Welt-Raum-Meshes eines Operanden-Satzes (mit Leaf-Transform). */
function buildOperandMeshes(
  leaves: HybridLeafInfo[],
  material: THREE.Material,
): THREE.Mesh[] {
  const out: THREE.Mesh[] = [];
  for (const leaf of leaves) {
    if (leaf.warp) {
      const geo = buildWarpedLeafGeometry(leaf);
      if (!geo) continue;
      out.push(new THREE.Mesh(geo, material));
      continue;
    }
    const geo = createPrimitiveGeometry(leaf);
    if (!geo) continue;
    const mesh = new THREE.Mesh(geo, material);
    applyLeafTransform(mesh, leaf);
    out.push(mesh);
  }
  return out;
}

/**
 * Lineare Sichttiefe `viewZ` (positiv vor der Kamera) → NDC-z, identisch zur
 * Fixed-Function-Projektion und gültig für BEIDE Projektionsarten (perspektiv
 * UND orthographisch). Aus der Projektionsmatrix gilt für einen Punkt mit
 * Sicht-z = -viewZ (Spalten-major elements):
 *   clip.z = m10 * (-viewZ) + m14
 *   clip.w = m11 * (-viewZ) + m15      (elements[11], elements[15])
 *     perspektivisch: m11 = -1, m15 =  0  → clip.w =  viewZ   (alte Formel)
 *     orthographisch: m11 =  0, m15 =  1  → clip.w =  1       (kein Divide-by-viewZ)
 * Damit ndc.z = clip.z / clip.w; die Window-Depth (gl_FragDepth) ist
 * 0.5*ndc.z + 0.5 (default depth-range 0..1).
 *
 * WICHTIG: Vor dem Ortho-Default (Ticket m62b / 0cdb6c1) teilte diese Formel fix
 * durch viewZ und brach so die Tiefen-Komposition (gl_FragDepth) der SS-CSG-Körper
 * im Ortho-Modus → reguläre Mesh-Körper wurden nicht mehr verdeckt (Ticket sa0k).
 *
 * Reine Funktion → headless testbar (screenSpaceCsg.test.ts deckt sie ab); der
 * Composite-Shader rechnet dieselbe Formel inline.
 *
 * @param viewZ  lineare Sichttiefe (> 0)
 * @param m10    projectionMatrix.elements[10]
 * @param m14    projectionMatrix.elements[14]
 * @param m11    projectionMatrix.elements[11]  (clip.w-Skala für -viewZ)
 * @param m15    projectionMatrix.elements[15]  (clip.w-Offset)
 */
export function viewZToNdcDepth(
  viewZ: number,
  m10: number,
  m14: number,
  m11: number,
  m15: number,
): number {
  const zndc = (m10 * -viewZ + m14) / (m11 * -viewZ + m15);
  return 0.5 * zndc + 0.5;
}

/**
 * G-Buffer-Vertex-Shader (GLSL3): reicht lineare Sichttiefe + View-Space-Normale
 * ins Fragment. `viewZ = -(modelViewMatrix*pos).z` (positiv vor der Kamera),
 * `vn = normalize(normalMatrix * normal)`.
 */
const GBUFFER_VS = /* glsl */ `
  out float vViewZ;
  out vec3 vWorldNormal;
  void main() {
    vec4 viewPos = modelViewMatrix * vec4(position, 1.0);
    vViewZ = -viewPos.z; // Kamera blickt -Z → vor der Kamera = positiv
    // WELT-Raum-Normale (für Parität mit dem Szenen-Lichtrig, das welt-fixe
    // Lichter nutzt — so driftet die Schnittflächen-Beleuchtung beim Orbiten
    // NICHT gegenüber den übrigen Körperflächen). mat3(modelMatrix) genügt für
    // starre/uniform-skalierte Leaf-Transforms (CAD-Primitive).
    vWorldNormal = normalize(mat3(modelMatrix) * normal);
    gl_Position = projectionMatrix * viewPos;
  }
`;

/**
 * G-Buffer-Fragment-Shader (GLSL3). Schreibt IMMER r = viewZ (> 0 ⇒ Treffer)
 * UND die View-Space-Normale in gba. Die Subtraktion nutzt aFront+bBack-Normale,
 * die Intersection aFront+bFront-Normale — daher wird die Normale auf ALLEN VIER
 * RTs geschrieben (kein Rollen-Gate mehr; ungenutzte Normalen kosten nur etwas
 * Bandbreite, vermeiden aber, dass ein zweiter Modus eine fehlende Normale findet).
 */
const GBUFFER_FS = /* glsl */ `
  precision highp float;
  in float vViewZ;
  in vec3 vWorldNormal;
  out vec4 fragColor;
  void main() {
    fragColor = vec4(vViewZ, vWorldNormal);
  }
`;

/**
 * Peel-Fragment-Shader (GLSL3) für Layer 2 des Subtrahenden (Ticket yt4o).
 *
 * Rendert die Subtrahend-Meshes ERNEUT und verwirft per Fragment alle Flächen,
 * die NICHT echt jenseits der Layer-1-Backface (`bBack1`) liegen — d.h. Layer 1
 * wird „weggepeelt". Unter den Überlebenden gewinnt wie üblich die Depth-Test-
 * Funktion (LessEqual) die nächste Fläche: mit FrontSide-Culling die zweite
 * Subtrahend-Front (bFront2), mit BackSide-Culling die zweite Backface (bBack2).
 *
 * `bBack1` wird an `gl_FragCoord.xy / uResolution` gesampelt (Screen-Space). Ein
 * Fragment überlebt nur, wenn `vViewZ > bBack1 + EPS` (echt dahinter). Fragmente
 * mit bBack1 == 0 (Layer 1 traf hier nicht) überleben ebenfalls — dann ist
 * Layer 2 ohnehin leer/irrelevant, weil ein nicht-getroffener Subtrahend kein
 * zweites Intervall hat. viewZ==0-Konvention (kein Treffer) wie GBUFFER_FS.
 */
const PEEL_FS = /* glsl */ `
  precision highp float;
  in float vViewZ;
  in vec3 vWorldNormal;
  uniform sampler2D uBBack1; // Layer-1-Backface (r=viewZ) zum Wegpeelen
  uniform vec2 uResolution;  // RT-Größe für Screen-Space-Sample
  out vec4 fragColor;
  const float PEEL_EPS = 1e-5;
  void main() {
    float bBack1 = texture(uBBack1, gl_FragCoord.xy / uResolution).r;
    // Fragmente bis einschließlich Layer-1-Austritt verwerfen (nur echt dahinter
    // gehört zu Layer 2). bBack1 <= 0 ⇒ Layer 1 traf nicht ⇒ nichts wegzupeelen.
    if (vViewZ <= bBack1 + PEEL_EPS) { discard; }
    fragColor = vec4(vViewZ, vWorldNormal);
  }
`;

/**
 * Fullscreen-CSG-Compositor (GLSL3). Liest die vier G-Buffer und klassifiziert
 * jeden Pixel je nach `uMode`:
 *   - uMode 0 (subtract): exakt wie `classifySubtractionPixel` (UNVERÄNDERT).
 *   - uMode 1 (intersect): exakt wie `classifyIntersectionPixel`.
 * Beleuchtet die sichtbare Fläche (Lambert, View-Space-Licht) und schreibt Farbe
 * + gl_FragDepth (identische uM10/uM14-Formel in beiden Modi).
 *
 * Slot-Belegung: A-Slot = Minuend bzw. Schnitt-Operand A (tAFront/tABack),
 * B-Slot = Subtrahend bzw. Schnitt-Operand B (tBFront/tBBack).
 */
const COMPOSITE_FS = /* glsl */ `
  precision highp float;
  in vec2 vUv;
  uniform sampler2D tAFront;  // r=viewZ, gba=A-Front-Normale
  uniform sampler2D tABack;   // r=viewZ, gba=A-Back-Normale
  uniform sampler2D tBFront;  // r=viewZ, gba=B-Front-Normale (Layer 1)
  uniform sampler2D tBBack;   // r=viewZ, gba=B-Back-Normale (Layer-1-Schnittfläche)
  uniform sampler2D tBFront2; // r=viewZ (Layer-2-Subtrahend-Front jenseits bBack1)
  uniform sampler2D tBBack2;  // r=viewZ, gba=B-Back-Normale (Layer-2-Schnittfläche)
  uniform vec3 uBodyColor;    // lineare Body-Farbe
  uniform float uM10;         // projectionMatrix.elements[10]  (clip.z-Skala)
  uniform float uM14;         // projectionMatrix.elements[14]  (clip.z-Offset)
  uniform float uM11;         // projectionMatrix.elements[11]  (clip.w-Skala für -viewZ)
  uniform float uM15;         // projectionMatrix.elements[15]  (clip.w-Offset)
  uniform float uMode;        // 0.0 = subtract (A−B), 1.0 = intersect (A∩B)
  // Live-Lichtrig (Parität mit WebGLSceneLights/pbrShading.ts), pro Frame aus dem
  // Compositor gesetzt: WELT-Raum-Lichtrichtungen (view-space LIGHTS dirs, mit der
  // Kamera-Quaternion in Weltraum rotiert) + zugehörige Radianzen, plus Ambient.
  uniform vec3 uLightDirs[3]; // normalisierte Welt-Richtungen ZUM Licht hin
  uniform vec3 uLightRads[3]; // Radianzen je Licht (LIGHTS[i].radiance)
  uniform float uAmbient;     // ambientLight-Intensität (AMBIENT)
  out vec4 fragColor;

  const float CSG_EPS = 1e-5;

  // Material-/Shading-Parität zum übrigen WebGL-Pfad: die Szenenkörper sind matte
  // MeshStandardMaterial (DEFAULT_ROUGHNESS, metalness 0), durch three's
  // ACES-Tonemapping + sRGB-Output gerendert. Wir spiegeln das hier: Lambert-
  // Diffus mit demselben Lichtrig (HDR, key=3.0 → würde roh überstrahlen), dann
  // dieselbe ACES-Tonemap + sRGB-Encodierung wie three sie auf die Körper anwendet.
  // SO matcht die Schnittfläche Helligkeit UND Farbton der Plain-WebGL-Körper.

  // three.js ACESFilmicToneMapping (RRTAndODTFit-Approximation, identisch zu
  // tonemapping_pars_fragment.glsl.js → ACESFilmicToneMapping).
  vec3 acesFilmic(vec3 color) {
    const mat3 ACESInputMat = mat3(
      0.59719, 0.07600, 0.02840,
      0.35458, 0.90834, 0.13383,
      0.04823, 0.01566, 0.83777
    );
    const mat3 ACESOutputMat = mat3(
       1.60475, -0.10208, -0.00327,
      -0.53108,  1.10813, -0.07276,
      -0.07367, -0.00605,  1.07602
    );
    color = ACESInputMat * color;
    vec3 a = color * (color + 0.0245786) - 0.000090537;
    vec3 b = color * (0.983729 * color + 0.4329510) + 0.238081;
    color = a / b;
    color = ACESOutputMat * color;
    return clamp(color, 0.0, 1.0);
  }

  // Linear → sRGB (three's outputColorSpace = SRGBColorSpace), identisch zur
  // sRGBTransferOETF in three's color-space-Shader.
  vec3 linearToSRGB(vec3 c) {
    return mix(
      c * 12.92,
      1.055 * pow(c, vec3(0.41666)) - 0.055,
      step(0.0031308, c)
    );
  }

  const float RECIPROCAL_PI = 0.3183098861837907;

  vec3 shade(vec3 color, vec3 n) {
    vec3 N = normalize(n);
    // Lambert-Diffus mit dem Live-Lichtrig (gleiche Richtungen/Radianzen wie die
    // Szenenkörper). Spiegelt three's RE_Direct_Physical/IndirectDiffuse für ein
    // mattes MeshStandardMaterial (metalness 0): diffuse = BRDF_Lambert(color) ·
    // (ambient + Σ radiance·NdotL), BRDF_Lambert = color/PI. three multipliziert
    // Licht-Intensitäten NICHT mit PI (WebGLLights), daher MUSS hier das 1/PI aus
    // dem Lambert-BRDF stehen — sonst wäre die Schnittfläche ~π× zu hell.
    // Spekular vernachlässigt (roughness 0.35, diffus-dominiert → Helligkeits-/
    // Tonparität zählt, nicht der Glanzpunkt).
    float weight = uAmbient;
    for (int i = 0; i < 3; i++) {
      weight += uLightRads[i].x * max(0.0, dot(N, uLightDirs[i]));
    }
    vec3 lit = color * RECIPROCAL_PI * weight;
    // HDR → LDR exakt wie three es auf die Körper anwendet (ACES + sRGB).
    return linearToSRGB(acesFilmic(lit));
  }

  void main() {
    vec4 aF = texture(tAFront, vUv);
    float aFrontZ = aF.r;

    float aBackZ = texture(tABack, vUv).r;
    vec4 bF = texture(tBFront, vUv);
    vec4 bB = texture(tBBack, vUv);
    float bFrontZ = bF.r;
    float bBackZ = bB.r;

    float viewZ;
    vec3 N;

    if (uMode > 0.5) {
      // ── Intersection A ∩ B (spiegelt classifyIntersectionPixel) ───────────
      // Intersection UNVERÄNDERT: nutzt nur Layer 1 (tBFront/tBBack), keine
      // Layer-2-Texturen.
      if (aFrontZ <= 0.0 || bFrontZ <= 0.0) { discard; }
      float entry = max(aFrontZ, bFrontZ);
      float exit = min(aBackZ, bBackZ);
      if (entry >= exit - CSG_EPS) { discard; }
      viewZ = entry;
      // Sichtbare Grenze = Front-Normale des Operanden mit der weiter hinten
      // liegenden Frontfläche.
      N = (aFrontZ >= bFrontZ) ? aF.gba : bF.gba;
    } else {
      // ── Subtraction A − B (Mehr-Intervall-Walk, spiegelt
      //    classifySubtractionPixelMulti — Ticket yt4o) ───────────────────────
      // Kein Minuend an diesem Pixel → Hintergrund/andere Bodys durchlassen.
      if (aFrontZ <= 0.0) { discard; }

      // Bis zu zwei Subtrahend-Intervalle: I1=[bF1,bB1] (Layer 1), I2=[bF2,bB2]
      // (Layer 2, gepeelt jenseits bB1). Ii gültig gdw. bFi > 0 (UND bBi > 0).
      vec4 bB2v = texture(tBBack2, vUv);
      float bF1 = bFrontZ;
      float bB1 = bBackZ;
      float bF2 = texture(tBFront2, vUv).r;
      float bB2 = bB2v.r;

      bool hasI1 = (bF1 > 0.0 && bB1 > 0.0);
      bool hasI2 = (bF2 > 0.0 && bB2 > 0.0);

      if (!hasI1) {
        // Kein Subtrahend → Minuend unverändert (keep).
        viewZ = aFrontZ;
        N = aF.gba;
      } else {
        // Liegt aFront in der entfernten Vereinigung R = I1 ∪ I2?
        bool inI1 = (aFrontZ > bF1 + CSG_EPS && aFrontZ < bB1 - CSG_EPS);
        bool inI2 = (hasI2 && aFrontZ > bF2 + CSG_EPS && aFrontZ < bB2 - CSG_EPS);

        if (!inI1 && !inI2) {
          // keep: sichtbare Fläche ist die Minuend-Front.
          viewZ = aFrontZ;
          N = aF.gba;
        } else {
          // aFront in R → Ende des zusammenhängenden entfernten Segments suchen.
          float exitZ;
          vec3 exitN;
          if (inI1) {
            exitZ = bB1;
            exitN = -bB.gba;
            // I1,I2 verschmolzen (überlappen/berühren) → Segment reicht bis bB2.
            if (hasI2 && bF2 <= bB1 + CSG_EPS) {
              exitZ = bB2;
              exitN = -bB2v.gba;
            }
          } else {
            // inI2 ohne inI1 (disjunkt, aFront in der fernen Röhre).
            exitZ = bB2;
            exitN = -bB2v.gba;
          }

          if (exitZ >= aBackZ - CSG_EPS) {
            // discard: Segment-Austritt hinter Minuend-Austritt → kein Material.
            discard;
          }
          // cut: sichtbare Fläche ist die Subtrahend-Backface (Schnittfläche).
          // Normale = invertierte Subtrahend-Außennormale (zeigt ins entfernte Volumen).
          viewZ = exitZ;
          N = exitN;
        }
      }
    }

    vec3 color = shade(uBodyColor, N);

    // gl_FragDepth aus linearer Sichttiefe (siehe viewZToNdcDepth in TS).
    // Allgemeiner clip.w-Nenner → korrekt für perspektiv UND ortho (Ticket sa0k).
    float zndc = (uM10 * (-viewZ) + uM14) / (uM11 * (-viewZ) + uM15);
    gl_FragDepth = 0.5 * zndc + 0.5;
    fragColor = vec4(color, 1.0);
  }
`;

const FULLSCREEN_VS = /* glsl */ `
  out vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = vec4(position.xy, 0.0, 1.0);
  }
`;

/** RGBA-Float-Target für ein G-Buffer (r=viewZ, gba=View-Normale). */
function makeGBufferTarget(w: number, h: number): THREE.WebGLRenderTarget {
  return new THREE.WebGLRenderTarget(w, h, {
    type: THREE.FloatType,
    format: THREE.RGBAFormat,
    minFilter: THREE.NearestFilter,
    magFilter: THREE.NearestFilter,
    depthBuffer: true,
    stencilBuffer: false,
  });
}

/**
 * Kapselt den gesamten GPU-Zustand für eine einstufige, geshadete A − B
 * Komposition eines Bodys: vier G-Buffer (A-Front/Back, B-Front/Back), je ein
 * Tiefen-/Normalen-Material pro Capture-Rolle, ein Fullscreen-Quad mit dem
 * Composite-Shader, das Farbe + gl_FragDepth ins aktuell gebundene Ziel schreibt.
 *
 * VISUELL UNVERIFIZIERT. Headless geprüft ist nur, dass es konstruiert/disposed
 * werden kann und die reine Klassifikation/Tiefenformel (Teil 1 + viewZToNdcDepth).
 */
export class ScreenSpaceCsgCompositor {
  private readonly aFrontRT: THREE.WebGLRenderTarget;
  private readonly aBackRT: THREE.WebGLRenderTarget;
  private readonly bFrontRT: THREE.WebGLRenderTarget;
  private readonly bBackRT: THREE.WebGLRenderTarget;
  // Layer-2-Peel des Subtrahenden (Ticket yt4o, nur Subtraktions-Modus).
  private readonly bFront2RT: THREE.WebGLRenderTarget;
  private readonly bBack2RT: THREE.WebGLRenderTarget;

  // Vier Capture-Materialien: A/B × Front/Back. Alle schreiben Tiefe UND Normale
  // (beide CSG-Modi brauchen unterschiedliche Normalen-Slots). Geteilt über alle
  // Bodys (zustandslos je Pixel), nur die Geometrien werden pro Body neu gebaut.
  private readonly aFrontMat: THREE.ShaderMaterial; // FrontSide
  private readonly aBackMat: THREE.ShaderMaterial; // BackSide
  private readonly bFrontMat: THREE.ShaderMaterial; // FrontSide
  private readonly bBackMat: THREE.ShaderMaterial; // BackSide
  // Layer-2-Peel-Materialien: gleicher Peel-Shader, FrontSide/BackSide. Beide
  // sampeln bBack1, um Layer 1 wegzupeelen (Ticket yt4o).
  private readonly bFront2Mat: THREE.ShaderMaterial; // FrontSide
  private readonly bBack2Mat: THREE.ShaderMaterial; // BackSide
  private readonly compositeMat: THREE.ShaderMaterial;

  private readonly fsScene: THREE.Scene;
  private readonly fsCamera: THREE.OrthographicCamera;
  private readonly fsQuad: THREE.Mesh;

  private width: number;
  private height: number;

  constructor(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);

    this.aFrontRT = makeGBufferTarget(this.width, this.height);
    this.aBackRT = makeGBufferTarget(this.width, this.height);
    this.bFrontRT = makeGBufferTarget(this.width, this.height);
    this.bBackRT = makeGBufferTarget(this.width, this.height);
    this.bFront2RT = makeGBufferTarget(this.width, this.height);
    this.bBack2RT = makeGBufferTarget(this.width, this.height);

    const captureMat = (side: THREE.Side): THREE.ShaderMaterial =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader: GBUFFER_VS,
        fragmentShader: GBUFFER_FS,
        side,
        depthTest: true,
        depthWrite: true,
      });

    // Konvex: genau eine Front-/eine Rückfläche je Operand. Default-DepthFunc
    // (LessEqual) liefert für FrontSide die NÄCHSTE Fläche (= viewZ-Minimum) und
    // — da BackSide-Culling die Frontflächen entfernt — für BackSide die HINTERE.
    // Alle vier schreiben Tiefe + Normale (beide Modi brauchen je andere Normalen).
    this.aFrontMat = captureMat(THREE.FrontSide); // A-Front
    this.aBackMat = captureMat(THREE.BackSide); // A-Back
    this.bFrontMat = captureMat(THREE.FrontSide); // B-Front
    this.bBackMat = captureMat(THREE.BackSide); // B-Back

    // Peel-Materialien (Layer 2): gleicher Peel-Shader, je Seite eine Instanz.
    // uBBack1 zeigt auf die Layer-1-Backface-Textur (zum Wegpeelen), uResolution
    // wird pro render() auf die RT-Größe gesetzt.
    const peelMat = (side: THREE.Side): THREE.ShaderMaterial =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader: GBUFFER_VS,
        fragmentShader: PEEL_FS,
        side,
        depthTest: true,
        depthWrite: true,
        uniforms: {
          uBBack1: { value: this.bBackRT.texture },
          uResolution: {
            value: new THREE.Vector2(this.width, this.height),
          },
        },
      });
    this.bFront2Mat = peelMat(THREE.FrontSide); // B-Front Layer 2
    this.bBack2Mat = peelMat(THREE.BackSide); // B-Back Layer 2

    this.compositeMat = new THREE.ShaderMaterial({
      glslVersion: THREE.GLSL3,
      vertexShader: FULLSCREEN_VS,
      fragmentShader: COMPOSITE_FS,
      // Opak, tiefen-komponiert mit dem übrigen Scene-Render.
      transparent: false,
      depthTest: true,
      depthWrite: true,
      uniforms: {
        tAFront: { value: this.aFrontRT.texture },
        tABack: { value: this.aBackRT.texture },
        tBFront: { value: this.bFrontRT.texture },
        tBBack: { value: this.bBackRT.texture },
        tBFront2: { value: this.bFront2RT.texture },
        tBBack2: { value: this.bBack2RT.texture },
        uBodyColor: { value: new THREE.Color(0.7, 0.7, 0.72) },
        uM10: { value: 0 },
        uM14: { value: 0 },
        uM11: { value: 0 },
        uM15: { value: 0 },
        uMode: { value: 0 },
        // Live-Lichtrig (pro Frame in render() aus der Kamera gesetzt). Werte hier
        // sind nur Platzhalter; uAmbient ist die einzige fixe Konstante (AMBIENT).
        uLightDirs: {
          value: LIGHTS.map(() => new THREE.Vector3(0, 0, 1)),
        },
        uLightRads: {
          value: LIGHTS.map(
            (l) => new THREE.Vector3(l.radiance, l.radiance, l.radiance),
          ),
        },
        uAmbient: { value: AMBIENT },
      },
    });

    this.fsScene = new THREE.Scene();
    this.fsCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
    this.fsQuad = new THREE.Mesh(
      new THREE.PlaneGeometry(2, 2),
      this.compositeMat,
    );
    // Das Fullscreen-Quad sitzt bei z=0 in Clip-Space; gl_FragDepth übernimmt die
    // echte Tiefe, daher Frustum-Culling abschalten (die Plane-BSphere ist klein).
    this.fsQuad.frustumCulled = false;
    this.fsScene.add(this.fsQuad);
  }

  setSize(width: number, height: number): void {
    const w = Math.max(1, width);
    const h = Math.max(1, height);
    if (w === this.width && h === this.height) return;
    this.width = w;
    this.height = h;
    this.aFrontRT.setSize(w, h);
    this.aBackRT.setSize(w, h);
    this.bFrontRT.setSize(w, h);
    this.bBackRT.setSize(w, h);
    this.bFront2RT.setSize(w, h);
    this.bBack2RT.setSize(w, h);
    (this.bFront2Mat.uniforms.uResolution.value as THREE.Vector2).set(w, h);
    (this.bBack2Mat.uniforms.uResolution.value as THREE.Vector2).set(w, h);
  }

  /**
   * Rendert die geshadete CSG-Komposition eines Bodys (A − B oder A ∩ B): captured
   * die vier G-Buffer aus den Operanden-Meshes und blendet anschließend das
   * Fullscreen-Quad (Farbe + gl_FragDepth) ins AKTUELL gebundene Ziel
   * (Default-Framebuffer = Haupt-Scene).
   *
   * Operanden-Slots: A-Slot = `operands.minuend` (subtract) bzw. `operands.a`
   * (intersect) → aFront/aBack-RTs; B-Slot = `operands.subtrahend` bzw.
   * `operands.b` → bFront/bBack-RTs. `mode` setzt `uMode` (subtract: bestehende
   * Logik UNVERÄNDERT; intersect: classifyIntersectionPixel).
   *
   * VISUELL UNVERIFIZIERT.
   */
  render(
    renderer: THREE.WebGLRenderer,
    camera: THREE.Camera,
    operands: CsgRenderOperands,
    bodyColor: THREE.Color,
    mode: CsgMode,
  ): void {
    // Operanden auf A/B-Slots mappen — egal welcher Modus.
    const aLeaves =
      "minuend" in operands ? operands.minuend : operands.a;
    const bLeaves =
      "minuend" in operands ? operands.subtrahend : operands.b;

    // Composite-Uniforms aus Kamera + Body-Farbe + Modus.
    const proj = (camera as THREE.PerspectiveCamera).projectionMatrix;
    this.compositeMat.uniforms.uM10.value = proj.elements[10];
    this.compositeMat.uniforms.uM14.value = proj.elements[14];
    // clip.w-Koeffizienten → ortho-tauglicher Tiefen-Nenner (Ticket sa0k).
    this.compositeMat.uniforms.uM11.value = proj.elements[11];
    this.compositeMat.uniforms.uM15.value = proj.elements[15];
    this.compositeMat.uniforms.uMode.value = mode === "intersect" ? 1 : 0;
    (this.compositeMat.uniforms.uBodyColor.value as THREE.Color).copy(bodyColor);

    // Live-Lichtrig in Weltraum: die view-space LIGHTS-Richtungen mit der Kamera-
    // Quaternion rotieren — EXAKT wie WebGLSceneLights die DirectionalLights je
    // Frame platziert. So beleuchtet die Schnittfläche identisch zu den übrigen
    // (camera-following) Körperflächen; `vWorldNormal` ist welt-fix, daher hier
    // die Welt-Lichtrichtung (kein View-Space-Mix).
    const dirUniform = this.compositeMat.uniforms.uLightDirs
      .value as THREE.Vector3[];
    for (let i = 0; i < LIGHTS.length; i++) {
      const d = LIGHTS[i].dir;
      dirUniform[i]
        .set(d[0], d[1], d[2])
        .applyQuaternion(camera.quaternion)
        .normalize();
    }

    const isSubtract = mode !== "intersect";

    // ── Layer-1-G-Buffer-Capture ─────────────────────────────────────────────
    const aMeshesFront = buildOperandMeshes(aLeaves, this.aFrontMat);
    const aMeshesBack = buildOperandMeshes(aLeaves, this.aBackMat);
    const bMeshesFront = buildOperandMeshes(bLeaves, this.bFrontMat);
    const bMeshesBack = buildOperandMeshes(bLeaves, this.bBackMat);
    // Layer-2-Peel-Meshes NUR im Subtraktions-Modus (Intersection nutzt sie nicht).
    const bMeshesFront2 = isSubtract
      ? buildOperandMeshes(bLeaves, this.bFront2Mat)
      : [];
    const bMeshesBack2 = isSubtract
      ? buildOperandMeshes(bLeaves, this.bBack2Mat)
      : [];

    const prevTarget = renderer.getRenderTarget();
    const prevClear = renderer.getClearColor(new THREE.Color());
    const prevClearAlpha = renderer.getClearAlpha();
    const prevAutoClear = renderer.autoClear;
    // Clear auf (0,0,0,0): r == 0 markiert "kein Treffer" (Konvention oben).
    renderer.setClearColor(0x000000, 0);
    renderer.autoClear = true;

    const renderInto = (rt: THREE.WebGLRenderTarget, meshes: THREE.Mesh[]) => {
      renderer.setRenderTarget(rt);
      renderer.clear(true, true, false);
      const scene = new THREE.Scene();
      for (const m of meshes) scene.add(m);
      renderer.render(scene, camera);
    };

    renderInto(this.aFrontRT, aMeshesFront);
    renderInto(this.aBackRT, aMeshesBack);
    renderInto(this.bFrontRT, bMeshesFront);
    renderInto(this.bBackRT, bMeshesBack);
    // Layer-2-Peel: nach Layer 1 (sampelt bBackRT.texture als uBBack1). Im
    // Intersection-Modus auf (0,0,0,0) geleert, damit kein Stale-Layer-2 im
    // (vom Composite ohnehin ignorierten) Slot steht.
    renderInto(this.bFront2RT, bMeshesFront2);
    renderInto(this.bBack2RT, bMeshesBack2);

    // ── Composite ins zuvor gebundene Ziel ──────────────────────────────────
    renderer.setRenderTarget(prevTarget);
    renderer.setClearColor(prevClear, prevClearAlpha);
    // autoClear AUS: das Composite-Quad muss ÜBER den bestehenden Scene-Render
    // (Unions/Primitive/Overlays) komponieren, nicht ihn löschen.
    renderer.autoClear = false;
    renderer.render(this.fsScene, this.fsCamera);
    renderer.autoClear = prevAutoClear;

    // Operanden-Geometrien wieder freigeben (Materialien bleiben, gehören uns).
    for (const m of [
      ...aMeshesFront,
      ...aMeshesBack,
      ...bMeshesFront,
      ...bMeshesBack,
      ...bMeshesFront2,
      ...bMeshesBack2,
    ]) {
      m.geometry.dispose();
    }
  }

  dispose(): void {
    this.aFrontRT.dispose();
    this.aBackRT.dispose();
    this.bFrontRT.dispose();
    this.bBackRT.dispose();
    this.bFront2RT.dispose();
    this.bBack2RT.dispose();
    this.aFrontMat.dispose();
    this.aBackMat.dispose();
    this.bFrontMat.dispose();
    this.bBackMat.dispose();
    this.bFront2Mat.dispose();
    this.bBack2Mat.dispose();
    this.compositeMat.dispose();
    this.fsQuad.geometry.dispose();
  }
}
