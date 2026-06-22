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

/* ════════════════════════════════════════════════════════════════════════
 * TEIL 2 — GPU-Compositor (WebGL2-MRT-Tiefen-Capture + Fullscreen-A−B-Shader).
 * VISUELL UNVERIFIZIERT (kein Browser im Build). Kapselt sämtliche GL-Ressourcen
 * und räumt sie über dispose() wieder auf.
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
 * Linearisiert gepackte Tiefe → Sicht-Z. Wir rendern die Operanden-Tiefe als
 * lineare Sichttiefe (view.z) direkt in eine Float-Textur, NICHT den
 * nicht-linearen gl_FragCoord.z, damit die Intervall-Vergleiche im Shader
 * dieselbe Metrik wie `classifySubtractionPixel` nutzen.
 */
const LINEAR_VIEW_DEPTH_GLSL = /* glsl */ `
  // view-space distance vor der Kamera (positiv). vViewZ wird im VS gesetzt.
  varying float vViewZ;
`;

/** Vertex-Shader, der die lineare Sichttiefe ins Fragment durchreicht. */
const DEPTH_VS = /* glsl */ `
  varying float vViewZ;
  void main() {
    vec4 viewPos = modelViewMatrix * vec4(position, 1.0);
    vViewZ = -viewPos.z; // Kamera blickt -Z → vor der Kamera = positiv
    gl_Position = projectionMatrix * viewPos;
  }
`;

/** Front- bzw. Back-Tiefe als R-Float. Cull-Modus entscheidet front/back. */
const DEPTH_FS = /* glsl */ `
  precision highp float;
  varying float vViewZ;
  void main() {
    gl_FragColor = vec4(vViewZ, 0.0, 0.0, 1.0);
  }
`;

/**
 * Fullscreen-A−B-Compositor-Shader (Skelett, Stage 1). Liest:
 *   - tAFront / tABack : Minuend Front-/Back-Tiefe
 *   - tBFront / tBBack : Subtrahend Front-/Back-Tiefe
 * und wendet pro Pixel exakt die `classifySubtractionPixel`-Logik an. Der
 * Ausgabe-Pass schreibt eine Tiefenmaske (cut/keep/discard) und die ersetzte
 * Tiefe; das Shading der Schnittfläche selbst (Normalen/Beleuchtung der
 * Subtrahend-Backface) ist als TODO markiert — Stage 1 belegt erst die
 * Klassifikation, das volle Re-Shading der Schnittfläche ist Stage-1-Feinschliff.
 */
const COMPOSITE_FS = /* glsl */ `
  precision highp float;
  varying vec2 vUv;
  uniform sampler2D tAFront;
  uniform sampler2D tABack;
  uniform sampler2D tBFront;
  uniform sampler2D tBBack;
  const float CSG_EPS = 1e-5;
  const float NO_HIT = 1e30;

  void main() {
    float aFront = texture2D(tAFront, vUv).r;
    float aBack  = texture2D(tABack,  vUv).r;
    float bFront = texture2D(tBFront, vUv).r;
    float bBack  = texture2D(tBBack,  vUv).r;

    // Kein Minuend-Treffer → nichts zu komponieren.
    if (aFront >= NO_HIT) { discard; }

    // Kein Subtrahend an diesem Pixel → Minuend behalten.
    if (bFront >= NO_HIT) {
      gl_FragColor = vec4(aFront, 0.0, 0.0, 1.0); // action=keep
      return;
    }

    // Spiegelt classifySubtractionPixel():
    if (aFront <= bFront + CSG_EPS || aFront >= bBack - CSG_EPS) {
      gl_FragColor = vec4(aFront, 0.0, 0.0, 1.0); // keep
      return;
    }
    if (bBack >= aBack - CSG_EPS) {
      discard; // action=discard: nach dem Schnitt kein Material
    }
    // action=cut: sichtbare Fläche ist die Subtrahend-Backface bei bBack.
    // TODO(Stage-1-Feinschliff): Subtrahend-Backface-Normale rekonstruieren und
    // beleuchten statt nur die Tiefe zu schreiben.
    gl_FragColor = vec4(bBack, 1.0, 0.0, 1.0); // .g=1 markiert eine Schnittfläche
  }
`;

const FULLSCREEN_VS = /* glsl */ `
  varying vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = vec4(position.xy, 0.0, 1.0);
  }
`;

function makeDepthTarget(w: number, h: number): THREE.WebGLRenderTarget {
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
 * Kapselt den gesamten GPU-Zustand für eine einstufige A − B Komposition eines
 * Bodys. Bewusst klein gehalten: vier Tiefen-Targets (A-Front/Back, B-Front/Back),
 * ein Fullscreen-Quad mit dem Composite-Shader.
 *
 * VISUELL UNVERIFIZIERT. Headless geprüft ist nur, dass es konstruiert/disposed
 * werden kann und die reine Klassifikation (Teil 1) stimmt.
 */
export class ScreenSpaceCsgCompositor {
  private readonly aFrontRT: THREE.WebGLRenderTarget;
  private readonly aBackRT: THREE.WebGLRenderTarget;
  private readonly bFrontRT: THREE.WebGLRenderTarget;
  private readonly bBackRT: THREE.WebGLRenderTarget;

  private readonly depthFrontMat: THREE.ShaderMaterial;
  private readonly depthBackMat: THREE.ShaderMaterial;
  private readonly compositeMat: THREE.ShaderMaterial;

  private readonly fsScene: THREE.Scene;
  private readonly fsCamera: THREE.OrthographicCamera;
  private readonly fsQuad: THREE.Mesh;

  private width: number;
  private height: number;

  constructor(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);

    this.aFrontRT = makeDepthTarget(this.width, this.height);
    this.aBackRT = makeDepthTarget(this.width, this.height);
    this.bFrontRT = makeDepthTarget(this.width, this.height);
    this.bBackRT = makeDepthTarget(this.width, this.height);

    // Front-Tiefe = vordere Flächen (FrontSide-Cull), Back-Tiefe = hintere.
    this.depthFrontMat = new THREE.ShaderMaterial({
      vertexShader: DEPTH_VS,
      fragmentShader: LINEAR_VIEW_DEPTH_GLSL + DEPTH_FS,
      side: THREE.FrontSide,
      depthTest: true,
      depthWrite: true,
    });
    this.depthBackMat = new THREE.ShaderMaterial({
      vertexShader: DEPTH_VS,
      fragmentShader: LINEAR_VIEW_DEPTH_GLSL + DEPTH_FS,
      side: THREE.BackSide,
      // Back-Tiefe = am WEITESTEN hinten → Greater-Depth-Vergleich.
      depthFunc: THREE.GreaterDepth,
      depthTest: true,
      depthWrite: true,
    });

    this.compositeMat = new THREE.ShaderMaterial({
      vertexShader: FULLSCREEN_VS,
      fragmentShader: COMPOSITE_FS,
      transparent: true,
      uniforms: {
        tAFront: { value: this.aFrontRT.texture },
        tABack: { value: this.aBackRT.texture },
        tBFront: { value: this.bFrontRT.texture },
        tBBack: { value: this.bBackRT.texture },
      },
    });

    this.fsScene = new THREE.Scene();
    this.fsCamera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1);
    this.fsQuad = new THREE.Mesh(
      new THREE.PlaneGeometry(2, 2),
      this.compositeMat,
    );
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
  }

  /**
   * Rendert die vier Tiefen-Targets eines Bodys aus seinen Operanden-Meshes.
   * Reiner Tiefen-Capture-Schritt (kein Composite, kein Output-Blit) — bewusst
   * separat, damit ein Test bzw. ein späterer Output-Pass darauf aufsetzen kann.
   *
   * VISUELL UNVERIFIZIERT.
   */
  captureDepths(
    renderer: THREE.WebGLRenderer,
    camera: THREE.Camera,
    operands: CsgOperands,
  ): void {
    const aMeshesFront = buildOperandMeshes(operands.minuend, this.depthFrontMat);
    const aMeshesBack = buildOperandMeshes(operands.minuend, this.depthBackMat);
    const bMeshesFront = buildOperandMeshes(
      operands.subtrahend,
      this.depthFrontMat,
    );
    const bMeshesBack = buildOperandMeshes(
      operands.subtrahend,
      this.depthBackMat,
    );

    const prevTarget = renderer.getRenderTarget();
    const prevClear = renderer.getClearColor(new THREE.Color());
    const prevClearAlpha = renderer.getClearAlpha();
    // NO_HIT-Sentinel als Clear-Wert: Float-RT clear ist (r,g,b,a); r = große
    // Tiefe markiert "kein Treffer" (vgl. NO_HIT im Shader).
    renderer.setClearColor(new THREE.Color(1e30, 0, 0), 1);

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

    renderer.setRenderTarget(prevTarget);
    renderer.setClearColor(prevClear, prevClearAlpha);

    // Operanden-Geometrien wieder freigeben (das Material bleibt, gehört uns).
    for (const m of [
      ...aMeshesFront,
      ...aMeshesBack,
      ...bMeshesFront,
      ...bMeshesBack,
    ]) {
      m.geometry.dispose();
    }
  }

  /**
   * Komposit-Pass: wertet die vier Tiefen-Targets per `classifySubtractionPixel`-
   * Shader aus und schreibt das Ergebnis in das aktuell gebundene Ziel.
   *
   * VISUELL UNVERIFIZIERT — Skelett. Aktuell schreibt der Shader Tiefe + eine
   * Cut-Maske; das volle Re-Shading der Schnittfläche fehlt (Stage-1-Feinschliff,
   * im COMPOSITE_FS als TODO markiert).
   */
  composite(renderer: THREE.WebGLRenderer): void {
    renderer.render(this.fsScene, this.fsCamera);
  }

  dispose(): void {
    this.aFrontRT.dispose();
    this.aBackRT.dispose();
    this.bFrontRT.dispose();
    this.bBackRT.dispose();
    this.depthFrontMat.dispose();
    this.depthBackMat.dispose();
    this.compositeMat.dispose();
    this.fsQuad.geometry.dispose();
  }
}
