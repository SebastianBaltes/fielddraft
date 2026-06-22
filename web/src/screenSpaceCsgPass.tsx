/**
 * Screen-Space-CSG R3F-Pass — die OPT-IN-Verdrahtung von {@link
 * ScreenSpaceCsgCompositor} in den WebGL-Renderpfad.
 * docs/ADAPTIVE-RENDERING-PLAN.md → Achse 3, Phase 3 (Stufe 1, einstufige A−B).
 *
 * Mounten NUR wenn das Setting `fielddraft.screenSpaceCsg` an ist (default AUS).
 * Mit Flag AUS wird diese Komponente vom WebGL-Viewport gar nicht gerendert, der
 * bestehende Pfad bleibt unverändert.
 *
 * EHRLICHKEIT: Dieser Pass führt den Tiefen-Capture + Composite je Body mit
 * Subtrahend aus. Er ist VISUELL UNVERIFIZIERT (kein Browser/GPU im Build); ob
 * die Schnittflächen korrekt erscheinen, muss der User prüfen. Headless belegt
 * ist nur die reine Klassifikation (screenSpaceCsg.test.ts). Das volle
 * Re-Shading der Schnittfläche fehlt noch (Stage-1-Feinschliff, im COMPOSITE_FS
 * als TODO). Der Pass ist absichtlich robust: jeder Fehler wird geschluckt, der
 * normale WebGL-Pfad zeichnet ohnehin parallel weiter.
 */

import { useEffect, useMemo, useRef } from "react";
import * as THREE from "three";
import { useFrame, useThree } from "@react-three/fiber";
import { useKernelStore } from "./store";
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

/**
 * Sammelt die A−B-Operanden aller Bodys, die einen Subtrahenden haben. Bodys
 * ohne Subtrahend werden ausgelassen — der bestehende WebGL-Pfad rendert die
 * korrekt (nichts wegzuschneiden).
 */
function collectSubtractionBodies(
  kernel: {
    bodyCount(): number;
    bodyHybridInfo(i: number): { leaves: CsgOperands["minuend"] };
  },
): { bodyIndex: number; operands: CsgOperands }[] {
  const out: { bodyIndex: number; operands: CsgOperands }[] = [];
  let count = 0;
  try {
    count = kernel.bodyCount();
  } catch {
    return out;
  }
  for (let i = 0; i < count; i++) {
    let leaves;
    try {
      leaves = kernel.bodyHybridInfo(i).leaves;
    } catch {
      continue;
    }
    if (!bodyNeedsSubtractionPass(leaves)) continue;
    out.push({ bodyIndex: i, operands: collectCsgOperands(leaves) });
  }
  return out;
}

/**
 * Experimenteller A−B-Compositor-Pass. Läuft NACH dem normalen WebGL-Render
 * (kleinere `renderOrder`-/Frame-Reihenfolge via useFrame mit Priorität), führt
 * je Subtraktions-Body einen Tiefen-Capture + Composite aus.
 *
 * VISUELL UNVERIFIZIERT. Mit Flag AUS nie gemountet.
 */
export function ScreenSpaceCsgPass() {
  const { store } = useKernelStore();
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
  }, []);

  useEffect(() => {
    compositorRef.current?.setSize(size.width, size.height);
  }, [size.width, size.height]);

  const bodies = useMemo(() => {
    if (!store) return [];
    return collectSubtractionBodies(store.kernel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, fingerprint]);

  // Priorität > 0 → R3F übernimmt NICHT das automatische Rendern dieses
  // Subscribers; wir rendern den Composite-Pass selbst, NACH dem Haupt-Render.
  useFrame(({ camera }) => {
    const comp = compositorRef.current;
    if (!comp || bodies.length === 0) return;
    try {
      for (const { operands } of bodies) {
        comp.captureDepths(gl, camera, operands);
        // Composite ins aktuelle (Default-)Framebuffer-Ziel mischen.
        comp.composite(gl);
      }
    } catch {
      // Defensiv: der normale WebGL-Pfad zeichnet ohnehin; ein Fehler hier darf
      // den Viewport nicht einfrieren.
    }
  }, 1);

  return null;
}
