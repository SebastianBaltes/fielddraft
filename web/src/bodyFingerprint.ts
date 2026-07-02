/**
 * Body fingerprints — the ONE place that decides "did the bodies change?".
 *
 * Two families:
 *   - {@link computeBodyFingerprint} / {@link useBodyFingerprint}: count +
 *     per-body `id.version`, bumps on any geometry edit or placement move.
 *     Drives the B-Rep overlay/pick caches and the screen-space CSG operands.
 *   - {@link computeSdfFingerprints}: the SDF pipelines' three-way split
 *     (volume topology / opcode structure / data versions) that decides
 *     shader recompile vs. buffer re-upload.
 */

import { useMemo } from "react";
import { useKernelStore } from "./store";

/** Minimal kernel surface the id.version fingerprint reads. */
export interface FingerprintKernel {
  bodyCount(): number;
  bodyId(i: number): number;
  bodyVersion(i: number): number;
}

/** Fingerprint that bumps on any body geometry edit or placement move —
 *  count plus per-body `id.version`. */
export function computeBodyFingerprint(k: FingerprintKernel): string {
  const n = k.bodyCount();
  let fp = String(n);
  for (let i = 0; i < n; i++) fp += `:${k.bodyId(i)}.${k.bodyVersion(i)}`;
  return fp;
}

/** Hook form of {@link computeBodyFingerprint}: consumers memo their kernel
 *  reads on this string so they re-run exactly when — and only when — the
 *  bodies actually change (any store commit re-evaluates it). */
export function useBodyFingerprint(): string {
  const { store, state } = useKernelStore();
  return useMemo(
    () => (store ? computeBodyFingerprint(store.kernel) : ""),
    // state in deps: every store commit produces a fresh state snapshot.
    [store, state],
  );
}

/** Kernel surface for the SDF pipeline fingerprints. */
export interface SdfFingerprintKernel extends FingerprintKernel {
  bodySdfOpcodesView(i: number): Float32Array;
  bodyVolumeCount(i: number): number;
}

/**
 * SDF-pipeline fingerprints (shared by SdfVmOverlay/SdfDeferredOverlay):
 *   - `volumeFp` — total volume-texture count; a change forces a shader
 *     recompile (texture bindings are baked into the pipeline).
 *   - `structFp` — per-body id + opcode length + volume count; a change means
 *     the packed opcode buffer layout moved → rebuild.
 *   - `dataFp` — per-body versions; a change with stable structure is a pure
 *     data update (no recompile).
 */
export function computeSdfFingerprints(
  k: SdfFingerprintKernel,
  count: number,
): { volumeFp: string; structFp: string; dataFp: string } {
  let totalVolumes = 0;
  const structParts: (string | number)[] = [count];
  const dataParts: number[] = [];
  for (let i = 0; i < count; i++) {
    const volCount = k.bodyVolumeCount(i);
    totalVolumes += volCount;
    structParts.push(k.bodyId(i), k.bodySdfOpcodesView(i).length, volCount);
    dataParts.push(k.bodyVersion(i));
  }
  return {
    volumeFp: String(totalVolumes),
    structFp: structParts.join(","),
    dataFp: dataParts.join(","),
  };
}
