/**
 * M14c: STEP export bridge.
 *
 * Wraps the shared OCCT worker channel with a typed API. The worker
 * (and the ~25 MB opencascade.js WASM) is loaded lazily on the
 * first call to `exportStep()`. Responses are correlated by
 * requestId, so an export running in parallel with a STEP import
 * (same worker) can't capture the import's answer.
 */

import type { StepCommand, StepExportRequest } from "./occt.worker";
import { getOcctChannel } from "./occtWorkerSingleton";

export type StepProgress = (phase: string) => void;

export async function exportStep(
  commands: StepCommand[],
  onProgress?: StepProgress,
): Promise<Uint8Array> {
  const result = await getOcctChannel().call((requestId) => {
    const request: StepExportRequest = {
      type: "export-step",
      requestId,
      commands,
    };
    return { message: request };
  }, onProgress);
  if (result.type !== "step-result") {
    throw new Error(
      `OCCT worker answered a STEP export with "${result.type}"`,
    );
  }
  return result.data;
}
