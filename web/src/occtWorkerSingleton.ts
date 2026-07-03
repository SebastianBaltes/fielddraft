/**
 * Shared OCCT worker channel.
 *
 * STEP export (stepExport.ts) and STEP import (stepImport.ts) run on ONE
 * worker instance (the ~25 MB OCCT WASM should load once). They therefore
 * must also share ONE {@link CorrelatedWorkerChannel} — a single requestId
 * space over the worker — so a parallel export and import each get exactly
 * their own result/error/progress messages back. (The previous bridges
 * matched responses by message *type* only, so a concurrent run could
 * resolve/reject the other caller's promise.)
 */

import type { StepWorkerMessage } from "./occt.worker";
import {
  CorrelatedWorkerChannel,
  type ChannelEvent,
} from "./workerChannel";

/** The terminal payloads a STEP request can resolve to; each bridge narrows
 *  to the variant it asked for. */
export type StepWorkerResult = Extract<
  StepWorkerMessage,
  { type: "step-result" | "import-result" }
>;

function parseStepEvent(
  data: unknown,
): ChannelEvent<StepWorkerResult, string> | null {
  if (!data || typeof data !== "object") return null;
  const msg = data as StepWorkerMessage;
  if (typeof (msg as { requestId?: unknown }).requestId !== "number") {
    return null;
  }
  switch (msg.type) {
    case "step-result":
    case "import-result":
      return { kind: "result", requestId: msg.requestId, value: msg };
    case "step-error":
      return { kind: "error", requestId: msg.requestId, error: msg.message };
    case "step-progress":
      return {
        kind: "progress",
        requestId: msg.requestId,
        progress: msg.phase,
      };
    default:
      return null;
  }
}

let channel: CorrelatedWorkerChannel<StepWorkerResult, string> | null = null;

export function getOcctChannel(): CorrelatedWorkerChannel<
  StepWorkerResult,
  string
> {
  if (!channel) {
    const worker = new Worker(new URL("./occt.worker.ts", import.meta.url), {
      type: "module",
    });
    channel = new CorrelatedWorkerChannel(
      worker,
      parseStepEvent,
      "OCCT worker channel",
    );
  }
  return channel;
}
