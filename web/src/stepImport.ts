/**
 * M14i: STEP import bridge.
 *
 * Wraps the OCCT Web Worker with a promise-based API for importing
 * STEP files. Reuses the same worker instance as stepExport.ts.
 * The worker reads the STEP file via OpenCASCADE, tessellates the
 * B-Rep shapes, and returns positions + indices for MeshSdf.
 */

import type {
  StepImportRequest,
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

export function importStep(
  fileBytes: Uint8Array,
  onProgress?: StepProgress,
): Promise<{ positions: Float32Array; normals: Float32Array; indices: Uint32Array }> {
  return new Promise((resolve, reject) => {
    const worker = getWorker();

    const handler = (e: MessageEvent<StepWorkerMessage>) => {
      switch (e.data.type) {
        case "step-progress":
          onProgress?.(e.data.phase);
          break;
        case "import-result":
          worker.removeEventListener("message", handler);
          resolve({
            positions: e.data.positions,
            normals: e.data.normals,
            indices: e.data.indices,
          });
          break;
        case "step-error":
          worker.removeEventListener("message", handler);
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
