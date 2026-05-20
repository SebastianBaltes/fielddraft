let workerInstance: Worker | null = null;

export function getOcctWorker(): Worker {
  if (!workerInstance) {
    workerInstance = new Worker(
      new URL("./occt.worker.ts", import.meta.url),
      { type: "module" },
    );
  }
  return workerInstance;
}
