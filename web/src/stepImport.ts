/**
 * M14i: STEP import bridge.
 *
 * Wraps the shared OCCT worker channel with a typed API for importing
 * STEP files. Shares the worker instance (and its requestId space)
 * with stepExport.ts via occtWorkerSingleton — responses are matched
 * by requestId, so a parallel export can't capture this import's
 * answer (or vice versa).
 */

import type { StepImportRequest } from "./occt.worker";
import { getOcctChannel } from "./occtWorkerSingleton";

export type StepProgress = (phase: string) => void;

export async function importStep(
  fileBytes: Uint8Array,
  onProgress?: StepProgress,
): Promise<{ positions: Float32Array; normals: Float32Array; indices: Uint32Array }> {
  const result = await getOcctChannel().call((requestId) => {
    const request: StepImportRequest = {
      type: "import-step",
      requestId,
      data: fileBytes,
    };
    return { message: request, transfer: [fileBytes.buffer] };
  }, onProgress);
  if (result.type !== "import-result") {
    throw new Error(
      `OCCT worker answered a STEP import with "${result.type}"`,
    );
  }
  return {
    positions: result.positions,
    normals: result.normals,
    indices: result.indices,
  };
}
