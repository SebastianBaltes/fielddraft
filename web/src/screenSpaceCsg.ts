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
 * TEIL 2 — GPU-Compositor (WebGL2-G-Buffer-Capture + Fullscreen-A−B-Shader).
 *
 * Stage 1 rendert eine ECHTE, geshadete Subtraktion A − B: vier RGBA-Float-
 * Render-Targets fangen pro Pixel die lineare Sichttiefe `viewZ` UND die
 * View-Space-Normale der relevanten Flächen, ein Fullscreen-Shader klassifiziert
 * jeden Pixel exakt wie `classifySubtractionPixel`, beleuchtet die sichtbare
 * Fläche (Lambert, View-Space-Licht) und schreibt Farbe + `gl_FragDepth`, sodass
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
 * Fixed-Function-Projektion: aus der Projektionsmatrix gilt für einen Punkt mit
 * Sicht-z = -viewZ:
 *   clip.z = m10 * (-viewZ) + m14 * 1
 *   clip.w = -(-viewZ)? …  → für die übliche perspektivische Matrix ist
 *   clip.w = viewZ (m23 = -1 projiziert -z_view auf +w).
 * Damit ndc.z = clip.z / clip.w = (m10 * (-viewZ) + m14) / viewZ.
 * Die Window-Depth (gl_FragDepth) ist 0.5*ndc.z + 0.5 (default depth-range 0..1).
 *
 * Reine Funktion → headless testbar (screenSpaceCsg.test.ts deckt sie ab); der
 * Composite-Shader rechnet dieselbe Formel inline.
 *
 * @param viewZ  lineare Sichttiefe (> 0)
 * @param m10    projectionMatrix.elements[10]
 * @param m14    projectionMatrix.elements[14]
 */
export function viewZToNdcDepth(viewZ: number, m10: number, m14: number): number {
  const zndc = (m10 * -viewZ + m14) / viewZ;
  return 0.5 * zndc + 0.5;
}

/**
 * G-Buffer-Vertex-Shader (GLSL3): reicht lineare Sichttiefe + View-Space-Normale
 * ins Fragment. `viewZ = -(modelViewMatrix*pos).z` (positiv vor der Kamera),
 * `vn = normalize(normalMatrix * normal)`.
 */
const GBUFFER_VS = /* glsl */ `
  out float vViewZ;
  out vec3 vViewNormal;
  void main() {
    vec4 viewPos = modelViewMatrix * vec4(position, 1.0);
    vViewZ = -viewPos.z; // Kamera blickt -Z → vor der Kamera = positiv
    vViewNormal = normalize(normalMatrix * normal);
    gl_Position = projectionMatrix * viewPos;
  }
`;

/**
 * G-Buffer-Fragment-Shader (GLSL3). `uWriteNormal` entscheidet, ob die Normale in
 * gba geschrieben wird (Minuend-Front, Subtrahend-Back) oder nur die Tiefe in r
 * (Minuend-Back, Subtrahend-Front). r = viewZ (> 0 ⇒ Treffer).
 */
const GBUFFER_FS = /* glsl */ `
  precision highp float;
  in float vViewZ;
  in vec3 vViewNormal;
  uniform float uWriteNormal; // 1.0 → Normale schreiben, 0.0 → nur Tiefe
  out vec4 fragColor;
  void main() {
    vec3 n = uWriteNormal > 0.5 ? vViewNormal : vec3(0.0);
    fragColor = vec4(vViewZ, n);
  }
`;

/**
 * Fullscreen-A−B-Compositor (GLSL3). Liest die vier G-Buffer und klassifiziert
 * jeden Pixel exakt wie `classifySubtractionPixel`, beleuchtet die sichtbare
 * Fläche (Lambert, View-Space-Licht) und schreibt Farbe + gl_FragDepth.
 */
const COMPOSITE_FS = /* glsl */ `
  precision highp float;
  in vec2 vUv;
  uniform sampler2D tAFront; // r=viewZ, gba=Minuend-Front-Normale
  uniform sampler2D tABack;  // r=viewZ
  uniform sampler2D tBFront; // r=viewZ
  uniform sampler2D tBBack;  // r=viewZ, gba=Subtrahend-Back-Normale (Schnittfläche)
  uniform vec3 uBodyColor;   // lineare Body-Farbe
  uniform float uM10;        // projectionMatrix.elements[10]
  uniform float uM14;        // projectionMatrix.elements[14]
  out vec4 fragColor;

  const float CSG_EPS = 1e-5;
  const float AMBIENT = 0.35;
  // Feste View-Space-Lichtrichtung. Kamera blickt -Z, also Licht aus +Z (zur
  // Kamera) leicht von oben/rechts — entspricht grob dem WebGL-Headlight-Look.
  const vec3 L = normalize(vec3(0.3, 0.5, 1.0));

  vec3 shade(vec3 color, vec3 n) {
    float diff = max(0.0, dot(normalize(n), L));
    return color * (AMBIENT + (1.0 - AMBIENT) * diff);
  }

  void main() {
    vec4 aF = texture(tAFront, vUv);
    float aFrontZ = aF.r;
    // Kein Minuend an diesem Pixel → Hintergrund/andere Bodys durchlassen.
    if (aFrontZ <= 0.0) { discard; }

    float aBackZ = texture(tABack, vUv).r;
    vec4 bB = texture(tBBack, vUv);
    float bFrontZ = texture(tBFront, vUv).r;
    float bBackZ = bB.r;

    bool hasB = (bFrontZ > 0.0 && bBackZ > 0.0);

    float viewZ;
    vec3 N;
    // classifySubtractionPixel(): keep, wenn kein Subtrahend ODER A-Front vor dem
    // Subtrahend-Eintritt ODER hinter dem Subtrahend-Austritt liegt.
    if (!hasB || aFrontZ <= bFrontZ + CSG_EPS || aFrontZ >= bBackZ - CSG_EPS) {
      // keep: sichtbare Fläche ist die Minuend-Front.
      viewZ = aFrontZ;
      N = aF.gba;
    } else {
      // aFrontZ liegt echt in (bFrontZ, bBackZ): vorderer Teil von A weggeschnitten.
      if (bBackZ >= aBackZ - CSG_EPS) {
        // discard: Subtrahend frisst durch den Minuend-Austritt → kein Material.
        discard;
      }
      // cut: sichtbare Fläche ist die Subtrahend-Backface (Schnittfläche) bei bBack.
      // Normale = invertierte Subtrahend-Außennormale (zeigt ins entfernte Volumen).
      viewZ = bBackZ;
      N = -bB.gba;
    }

    vec3 color = shade(uBodyColor, N);

    // gl_FragDepth aus linearer Sichttiefe (siehe viewZToNdcDepth in TS).
    float zndc = (uM10 * (-viewZ) + uM14) / viewZ;
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

  // Vier Capture-Materialien: Front/Back × mit/ohne Normale. Geteilt über alle
  // Bodys (zustandslos je Pixel), nur die Geometrien werden pro Body neu gebaut.
  private readonly aFrontMat: THREE.ShaderMaterial; // FrontSide, Normale
  private readonly aBackMat: THREE.ShaderMaterial; // BackSide, nur Tiefe
  private readonly bFrontMat: THREE.ShaderMaterial; // FrontSide, nur Tiefe
  private readonly bBackMat: THREE.ShaderMaterial; // BackSide, Normale (Schnitt)
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

    const captureMat = (
      side: THREE.Side,
      writeNormal: boolean,
    ): THREE.ShaderMaterial =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        vertexShader: GBUFFER_VS,
        fragmentShader: GBUFFER_FS,
        side,
        depthTest: true,
        depthWrite: true,
        uniforms: { uWriteNormal: { value: writeNormal ? 1 : 0 } },
      });

    // Konvex: genau eine Front-/eine Rückfläche je Operand. Default-DepthFunc
    // (LessEqual) liefert für FrontSide die NÄCHSTE Fläche (= viewZ-Minimum) und
    // — da BackSide-Culling die Frontflächen entfernt — für BackSide die HINTERE.
    this.aFrontMat = captureMat(THREE.FrontSide, true); // Minuend-Front + Normale
    this.aBackMat = captureMat(THREE.BackSide, false); // Minuend-Back, nur Tiefe
    this.bFrontMat = captureMat(THREE.FrontSide, false); // Subtrahend-Front, Tiefe
    this.bBackMat = captureMat(THREE.BackSide, true); // Subtrahend-Back + Normale

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
        uBodyColor: { value: new THREE.Color(0.7, 0.7, 0.72) },
        uM10: { value: 0 },
        uM14: { value: 0 },
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
  }

  /**
   * Rendert die geshadete A − B Komposition eines Bodys: captured die vier
   * G-Buffer aus den Operanden-Meshes und blendet anschließend das Fullscreen-
   * Quad (Farbe + gl_FragDepth) ins AKTUELL gebundene Ziel (Default-Framebuffer
   * = Haupt-Scene).
   *
   * VISUELL UNVERIFIZIERT.
   */
  render(
    renderer: THREE.WebGLRenderer,
    camera: THREE.Camera,
    operands: CsgOperands,
    bodyColor: THREE.Color,
  ): void {
    // Composite-Uniforms aus Kamera + Body-Farbe.
    const proj = (camera as THREE.PerspectiveCamera).projectionMatrix;
    this.compositeMat.uniforms.uM10.value = proj.elements[10];
    this.compositeMat.uniforms.uM14.value = proj.elements[14];
    (this.compositeMat.uniforms.uBodyColor.value as THREE.Color).copy(bodyColor);

    // ── G-Buffer-Capture ────────────────────────────────────────────────────
    const aMeshesFront = buildOperandMeshes(operands.minuend, this.aFrontMat);
    const aMeshesBack = buildOperandMeshes(operands.minuend, this.aBackMat);
    const bMeshesFront = buildOperandMeshes(operands.subtrahend, this.bFrontMat);
    const bMeshesBack = buildOperandMeshes(operands.subtrahend, this.bBackMat);

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
    ]) {
      m.geometry.dispose();
    }
  }

  dispose(): void {
    this.aFrontRT.dispose();
    this.aBackRT.dispose();
    this.bFrontRT.dispose();
    this.bBackRT.dispose();
    this.aFrontMat.dispose();
    this.aBackMat.dispose();
    this.bFrontMat.dispose();
    this.bBackMat.dispose();
    this.compositeMat.dispose();
    this.fsQuad.geometry.dispose();
  }
}
