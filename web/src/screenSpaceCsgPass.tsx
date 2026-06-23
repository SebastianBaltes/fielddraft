/**
 * Screen-Space-CSG R3F-Pass — die OPT-IN-Verdrahtung von {@link
 * ScreenSpaceCsgCompositor} in den WebGL-Renderpfad.
 * docs/ADAPTIVE-RENDERING-PLAN.md → Achse 3, Phase 3 (Stufe 1, einstufige A−B).
 *
 * Mounten NUR wenn das Setting `fielddraft.screenSpaceCsg` an ist (default AUS).
 * Mit Flag AUS wird diese Komponente vom WebGL-Viewport gar nicht gerendert, der
 * bestehende Pfad bleibt unverändert.
 *
 * EHRLICHKEIT: Dieser Pass führt je Subtraktions-Body einen G-Buffer-Capture +
 * geshadeten Composite (A − B) aus und komponiert das Ergebnis tiefenkorrekt über
 * den übrigen Scene-Render. Er ist VISUELL UNVERIFIZIERT (kein Browser/GPU im
 * Build); ob die Schnittflächen + Normalen korrekt erscheinen, muss der User
 * prüfen. Headless belegt ist nur die reine Klassifikation + Tiefenformel
 * (screenSpaceCsg.test.ts). Der Pass ist absichtlich robust: jeder Fehler wird
 * geschluckt, der Viewport friert dadurch nicht ein.
 *
 * WICHTIG (kein Doppel-Draw): Difference-Bodys, die hier komponiert werden,
 * dürfen NICHT zusätzlich ihr DC-Mesh zeichnen. `MeshPreview` überspringt bei
 * aktivem SS-CSG-Flag genau die `bodyNeedsSubtractionPass`-Bodys (siehe
 * viewportOverlays.tsx) — bei Flag AUS bleibt alles unverändert.
 */

import { useEffect, useMemo, useRef } from "react";
import * as THREE from "three";
import { useFrame, useThree } from "@react-three/fiber";
import { useKernelStore } from "./store";
import { resolveMaterials } from "./materialBridge";
import { BODY_COLORS } from "./sdfDeferredRenderer";
import type { FeatureSummary } from "./kernel";
import {
  ScreenSpaceCsgCompositor,
  collectCsgOperands,
  bodyNeedsSubtractionPass,
  type CsgOperands,
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

interface SubtractionBody {
  bodyIndex: number;
  operands: CsgOperands;
  color: THREE.Color;
}

/**
 * Sammelt die A−B-Operanden + Body-Farbe aller Bodys mit Subtrahend. Bodys ohne
 * Subtrahend werden ausgelassen — der bestehende WebGL-Pfad rendert die korrekt.
 *
 * Die Farbe wird IDENTISCH zu `WebGLBodies` bestimmt: User-Material (falls
 * gesetzt) über `resolveMaterials`, sonst die Part-Palette `BODY_COLORS`. So
 * trifft der CSG-Compositor optisch denselben Ton wie der übrige Scene-Render.
 */
function collectSubtractionBodies(
  kernel: {
    bodyCount(): number;
    bodyHybridInfo(i: number): { leaves: CsgOperands["minuend"] };
  },
  features: FeatureSummary[] | undefined,
): SubtractionBody[] {
  const out: SubtractionBody[] = [];
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
    let leaves;
    try {
      leaves = kernel.bodyHybridInfo(i).leaves;
    } catch {
      continue;
    }
    if (!bodyNeedsSubtractionPass(leaves)) continue;
    const um = userMats.get(i);
    const [r, g, b] = um ? um.color : BODY_COLORS[i % BODY_COLORS.length];
    out.push({
      bodyIndex: i,
      operands: collectCsgOperands(leaves),
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
    return collectSubtractionBodies(store.kernel, state?.features);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint, state?.features]);

  // Priorität > 0 → R3F übernimmt NICHT das automatische Rendern dieses
  // Subscribers; wir rendern den Composite-Pass selbst, NACH dem Haupt-Render.
  useFrame(({ camera }) => {
    const comp = compositorRef.current;
    if (!comp || bodies.length === 0) return;
    try {
      for (const { operands, color } of bodies) {
        comp.render(gl, camera, operands, color);
      }
    } catch {
      // Defensiv: ein Fehler hier darf den Viewport nicht einfrieren.
    }
  }, 1);

  return null;
}
