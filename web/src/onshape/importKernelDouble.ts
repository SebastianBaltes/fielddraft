/**
 * The kernel the Onshape acceptance suites drive — ONE definition, the app's
 * surface (ticket #448).
 *
 * The importer takes an {@link ImportKernel}, and almost every member of that
 * interface is OPTIONAL: a small unit-test double should not have to implement
 * `listSketchRegions` to check an expression. The end-to-end suites are the
 * opposite case — they exist to answer „what does the USER get", and the user
 * gets the full {@link Kernel} class, handed over by `onshapeProjectImport.ts`.
 *
 * Six copies of that double were maintained by hand, and every one of them had
 * silently drifted: none passed `resolvedMateConnectors`, so `resolvePlaneFrame`
 * could not place a single sketch on a mate connector, and the suites measured a
 * WORSE import than the app performs (#419: 14 bodies instead of 18, 99 failing
 * features instead of 89, eight skew sketch planes instead of one). Worse, they
 * could not see regressions in anything downstream of such a sketch — #446's
 * mirrored-circle flank bug was invisible here for exactly that reason.
 *
 * Two type-level locks keep that from coming back:
 *
 *  1. The double is typed {@link FullImportKernel} = `Required<ImportKernel>`.
 *     An optional member added to `ImportKernel` is NOT optional here, so the
 *     compiler drags the suites along the moment the importer learns to ask
 *     something new.
 *  2. {@link AppKernelCoversImporter} proves the app's own `Kernel` class
 *     satisfies that same full surface. If the importer ever grows a hook the
 *     app cannot answer, this file stops compiling rather than the suites
 *     quietly measuring a fiction.
 *
 * The bodies below mirror `Kernel`'s wrappers exactly (the JSON parsing, the
 * synthetic sketch-sub-call ids, the tolerant `resolvedMateConnectors`); the
 * suites cannot use `Kernel` itself because they hold the raw wasm object for
 * assertions the app has no reason to expose (`toJson`, `bodySdfEval`,
 * `bodyBrepCounts`, `setFeatureSuppressed`) and because `Kernel.create()`
 * imposes the app's display mesh resolution on a run that never displays.
 */

// eslint-disable-next-line import/no-unresolved
import type { Kernel as WasmKernel } from "../wasm/fielddraft_kernel";
import type { Kernel, VarKind } from "../kernel";
import type { ImportKernel } from "./onshapeImport";
import type { SketchSubCall } from "./onshapeSketch";

/** {@link ImportKernel} with nothing left optional — the surface the app has. */
export type FullImportKernel = Required<ImportKernel>;

/** Fails to compile unless `T` is exactly `true`. */
type Assert<T extends true> = T;

/**
 * Compile-time SSOT check, no runtime cost: the class
 * `onshapeProjectImport.ts` hands the importer answers every hook the
 * importer can ask. When this line stops compiling, the app and the importer
 * have drifted apart — fix `Kernel`, do not weaken the check.
 */
export type AppKernelCoversImporter = Assert<
  Kernel extends FullImportKernel ? true : false
>;

/** The double plus the raw wasm handle the suites assert against. */
export type SuiteKernel = FullImportKernel & { raw: WasmKernel };

/**
 * Wrap a raw wasm `Kernel` in the surface the app gives the importer.
 * `raw` stays reachable for the assertions that read the document back.
 */
export function wrapImportKernel(k: WasmKernel): SuiteKernel {
  return {
    raw: k,
    addSketch: (planeQuery: string, subCalls: SketchSubCall[]) =>
      k.addSketch(
        planeQuery,
        JSON.stringify(
          subCalls.map((c, i) => ({ type: c.type, id: String(i + 1), params: c.params })),
        ),
      ),
    addFeatureCall: (type: string, paramsJson: string) => k.addFeatureCall(type, paramsJson),
    resolveQuery: (q: string) => {
      const parsed = JSON.parse(k.resolveQuery(q)) as unknown;
      return Array.isArray(parsed) ? (parsed as any[]) : [];
    },
    // The geometry readers the end-condition translation measures against.
    sceneAabb: () => JSON.parse(k.sceneAabb()),
    bodyCount: () => k.bodyCount(),
    bodyBrepFaces: (i: number) => JSON.parse(k.bodyBrepFaces(i)),
    bodyPlacement: (i: number) => JSON.parse(k.bodyPlacement(i)),
    // The imprint-face list (#412) — ranks candidate faces by area when an
    // Onshape region selection is resolved to an atomic face.
    regionQueryAtPoint: (sketchId: number, u: number, v: number) =>
      k.regionQueryAtPoint(sketchId, u, v),
    listSketchRegions: (sketchId: number, resolution: number) =>
      k.listSketchRegions(sketchId, resolution),
    // The #410 batch-meshing deferral, exercised end-to-end.
    setSyncMeshing: (enabled: boolean) => k.setSyncMeshing(enabled),
    setVariable: (name: string, value: number, kind: VarKind, formula?: string) =>
      k.setVariable(name, value, kind, formula),
    // #409: a placed mate connector's CSYS is a plane frame in its own right.
    // Without this a sketch ON a connector cannot resolve its plane AT ALL —
    // the omission this whole file exists to make impossible.
    resolvedMateConnectors: () => {
      try {
        return JSON.parse(k.resolvedMateConnectors());
      } catch {
        return [];
      }
    },
  };
}
