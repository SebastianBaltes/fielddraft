/**
 * Screen-Space-CSG R3F-Pass — die OPT-IN-Verdrahtung von {@link
 * ScreenSpaceCsgCompositor} in den WebGL-Renderpfad.
 * docs/ADAPTIVE-RENDERING-PLAN.md → Achse 3, Phase 3 (Stufe 1, binäre A−B / A∩B).
 *
 * Mounten NUR wenn das Setting `fielddraft.screenSpaceCsg` an ist (default AUS).
 * Mit Flag AUS wird diese Komponente vom WebGL-Viewport gar nicht gerendert, der
 * bestehende Pfad bleibt unverändert.
 *
 * EHRLICHKEIT: Dieser Pass führt je CSG-Body (Subtraktion A−B ODER binäre
 * Intersection A∩B) einen G-Buffer-Capture + geshadeten Composite aus und
 * komponiert das Ergebnis tiefenkorrekt über den übrigen Scene-Render. Er ist
 * VISUELL UNVERIFIZIERT (kein Browser/GPU im Build); ob die Schnittflächen +
 * Normalen korrekt erscheinen, muss der User prüfen. Headless belegt ist nur die
 * reine Klassifikation + Tiefenformel (screenSpaceCsg.test.ts). Der Pass ist
 * absichtlich robust: jeder Fehler wird geschluckt, der Viewport friert nicht ein.
 *
 * WICHTIG (kein Doppel-Draw): CSG-Bodys, die hier komponiert werden, dürfen NICHT
 * zusätzlich ihr DC-Mesh zeichnen. `MeshPreview`/`WebGLBodies` überspringen bei
 * aktivem SS-CSG-Flag genau die `bodyNeedsSubtractionPass`/
 * `bodyNeedsIntersectionPass`-Bodys (siehe webglBodies.tsx) — bei Flag AUS
 * bleibt alles unverändert.
 */

import { useEffect, useMemo, useRef } from "react";
import * as THREE from "three";
import { useFrame, useThree } from "@react-three/fiber";
import { useKernelStore } from "./store";
import { resolveMaterials } from "./materialBridge";
import { BODY_COLORS } from "./sdfDeferredRenderer";
import type { FeatureSummary, HybridLeafInfo } from "./kernel";
import {
  ScreenSpaceCsgCompositor,
  collectCsgOperands,
  collectIntersectionOperands,
  bodyNeedsSubtractionPass,
  bodyNeedsIntersectionPass,
  type CsgMode,
  type CsgRenderOperands,
} from "./screenSpaceCsg";

/** Body-Versionierung — Rebuild der Operanden, wenn sich Geometrie ändert. */
function useScsBodyFingerprint(): string {
  const { store, state } = useKernelStore();
  return useMemo(() => {
    if (!store) return "";
    const n = store.kernel.bodyCount();
    let fp = String(n);
    for (let i = 0; i < n; i++)
      fp += `:${store.kernel.bodyId(i)}.${store.kernel.bodyVersion(i)}`;
    return fp;
    // store-Identität ist pro Session stabil; das Fingerprint treibt den Rebuild.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, state]);
}

interface CsgBody {
  bodyIndex: number;
  operands: CsgRenderOperands;
  mode: CsgMode;
  color: THREE.Color;
}

/**
 * Sammelt die CSG-Operanden + Modus + Body-Farbe aller Bodys, die der
 * Screen-Space-Pass komponiert: Subtraktions-Bodys (mode 'subtract', operands
 * {minuend, subtrahend}) UND binäre Intersection-Bodys (mode 'intersect',
 * operands {a, b}). Alle anderen Bodys (reine Union, einfache Primitive) werden
 * ausgelassen — der bestehende WebGL-Pfad rendert die korrekt.
 *
 * Die Farbe wird IDENTISCH zu `WebGLBodies` bestimmt: User-Material (falls
 * gesetzt) über `resolveMaterials`, sonst die Part-Palette `BODY_COLORS`. So
 * trifft der CSG-Compositor optisch denselben Ton wie der übrige Scene-Render.
 */
function collectCsgBodies(
  kernel: {
    bodyCount(): number;
    bodyHybridInfo(i: number): { leaves: HybridLeafInfo[] };
  },
  features: FeatureSummary[] | undefined,
): CsgBody[] {
  const out: CsgBody[] = [];
  let count = 0;
  try {
    count = kernel.bodyCount();
  } catch {
    return out;
  }
  // resolveMaterials braucht den echten Kernel-Typ; der lokale Struktur-Typ oben
  // ist ein Subset davon — cast nur für diesen Aufruf.
  const userMats = features
    ? resolveMaterials(kernel as never, features)
    : new Map();
  for (let i = 0; i < count; i++) {
    let leaves: HybridLeafInfo[];
    try {
      leaves = kernel.bodyHybridInfo(i).leaves;
    } catch {
      continue;
    }
    let operands: CsgRenderOperands;
    let mode: CsgMode;
    if (bodyNeedsSubtractionPass(leaves)) {
      operands = collectCsgOperands(leaves);
      mode = "subtract";
    } else if (bodyNeedsIntersectionPass(leaves)) {
      operands = collectIntersectionOperands(leaves);
      mode = "intersect";
    } else {
      continue;
    }
    const um = userMats.get(i);
    const [r, g, b] = um ? um.color : BODY_COLORS[i % BODY_COLORS.length];
    out.push({
      bodyIndex: i,
      operands,
      mode,
      color: new THREE.Color(r, g, b),
    });
  }
  return out;
}

/**
 * Experimenteller, geshadeter A−B-Compositor-Pass. Läuft NACH dem normalen
 * WebGL-Render (useFrame-Priorität 1) und blendet je Subtraktions-Body Farbe +
 * gl_FragDepth tiefenkorrekt über die Scene.
 *
 * VISUELL UNVERIFIZIERT. Mit Flag AUS nie gemountet.
 */
export function ScreenSpaceCsgPass() {
  const { store, state } = useKernelStore();
  const fingerprint = useScsBodyFingerprint();
  const gl = useThree((s) => s.gl) as unknown as THREE.WebGLRenderer;
  const size = useThree((s) => s.size);
  const compositorRef = useRef<ScreenSpaceCsgCompositor | null>(null);

  // Compositor lazy bauen + auf Canvas-Größe halten.
  useEffect(() => {
    const comp = new ScreenSpaceCsgCompositor(size.width, size.height);
    compositorRef.current = comp;
    return () => {
      comp.dispose();
      compositorRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    compositorRef.current?.setSize(size.width, size.height);
  }, [size.width, size.height]);

  const bodies = useMemo(() => {
    if (!store) return [];
    return collectCsgBodies(store.kernel, state?.features);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint, state?.features]);

  // Priorität > 0 → R3F unterdrückt sein automatisches Szenen-Rendering (es
  // erwartet, dass der höchstpriorisierte Subscriber selbst rendert). Wir sind
  // dieser Subscriber und treiben deshalb den GANZEN Frame: ZUERST die normale
  // Szene (Datum-Planes, Sketch, Nicht-CSG-Körper), DANN den CSG-Composite
  // tiefenkorrekt darüber.
  //
  // Tickets 257b/xq87: Vorher rief dieser Callback NUR den Composite und kehrte
  // bei 0 Körpern früh zurück — dann blieb die Szene (Datum-Planes/Sketch) im
  // WebGL-Modus mit aktivem Flag komplett ungerendert. Daher jetzt IMMER zuerst
  // `gl.render(scene, camera)` (auch ohne Subtraktions-Körper).
  useFrame(({ scene, camera }) => {
    gl.render(scene, camera);
    const comp = compositorRef.current;
    if (!comp || bodies.length === 0) return;
    try {
      for (const { operands, color, mode } of bodies) {
        comp.render(gl, camera, operands, color, mode);
      }
    } catch {
      // Defensiv: ein Fehler hier darf den Viewport nicht einfrieren.
    }
  }, 1);

  return null;
}
