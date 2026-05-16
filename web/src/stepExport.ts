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

export type StepProgress = (phase: string) => void;

let workerInstance: Worker | null = null;

function getWorker(): Worker {
  if (!workerInstance) {
    workerInstance = new Worker(
      new URL("./occt.worker.ts", import.meta.url),
      { type: "module" },
    );
  }
  return workerInstance;
}

export function exportStep(
  commands: StepCommand[],
  onProgress?: StepProgress,
): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const worker = getWorker();

    const handler = (e: MessageEvent<StepWorkerMessage>) => {
      switch (e.data.type) {
        case "step-progress":
          onProgress?.(e.data.phase);
          break;
        case "step-result":
          worker.removeEventListener("message", handler);
          resolve(e.data.data);
          break;
        case "step-error":
          worker.removeEventListener("message", handler);
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

