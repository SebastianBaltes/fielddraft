/**
 * M14i: STEP import bridge.
 *
 * Wraps the OCCT Web Worker with a promise-based API for importing
 * STEP files. Shares the same worker instance as stepExport.ts via
 * the occtWorkerSingleton module.
 */

import type {
  StepImportRequest,
  StepWorkerMessage,
} from "./occt.worker";
import { getOcctWorker } from "./occtWorkerSingleton";

export type StepProgress = (phase: string) => void;

let inflight = false;

export function importStep(
  fileBytes: Uint8Array,
  onProgress?: StepProgress,
): Promise<{ positions: Float32Array; normals: Float32Array; indices: Uint32Array }> {
  if (inflight) {
    return Promise.reject(new Error("A STEP import is already in progress"));
  }
  inflight = true;
  let worker: Worker;
  try {
    worker = getOcctWorker();
  } catch (e) {
    inflight = false;
    return Promise.reject(e);
  }
  return new Promise((resolve, reject) => {
    const handler = (e: MessageEvent<StepWorkerMessage>) => {
      switch (e.data.type) {
        case "step-progress":
          onProgress?.(e.data.phase);
          break;
        case "import-result":
          worker.removeEventListener("message", handler);
          inflight = false;
          resolve({
            positions: e.data.positions,
            normals: e.data.normals,
            indices: e.data.indices,
          });
          break;
        case "step-error":
          worker.removeEventListener("message", handler);
          inflight = false;
          reject(new Error(e.data.message));
          break;
      }
    };

    worker.addEventListener("message", handler);

    const request: StepImportRequest = {
      type: "import-step",
      data: fileBytes,
    };
    worker.postMessage(request, [fileBytes.buffer]);
  });
}
