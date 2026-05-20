/**
 * M14c: STEP export bridge.
 *
 * Wraps the OCCT Web Worker with a promise-based API. The worker
 * (and the ~25 MB opencascade.js WASM) is loaded lazily on the
 * first call to `exportStep()`.
 */

import type {
  StepCommand,
  StepExportRequest,
  StepWorkerMessage,
} from "./occt.worker";
import { getOcctWorker } from "./occtWorkerSingleton";

export type StepProgress = (phase: string) => void;

let inflight = false;

export function exportStep(
  commands: StepCommand[],
  onProgress?: StepProgress,
): Promise<Uint8Array> {
  if (inflight) {
    return Promise.reject(new Error("A STEP export is already in progress"));
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
        case "step-result":
          worker.removeEventListener("message", handler);
          inflight = false;
          resolve(e.data.data);
          break;
        case "step-error":
          worker.removeEventListener("message", handler);
          inflight = false;
          reject(new Error(e.data.message));
          break;
      }
    };

    worker.addEventListener("message", handler);

    const request: StepExportRequest = {
      type: "export-step",
      commands,
    };
    worker.postMessage(request);
  });
}
