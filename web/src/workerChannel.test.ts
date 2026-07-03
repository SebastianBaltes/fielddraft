import { describe, expect, it } from "vitest";
import {
  CoalescingWorkerChannel,
  CorrelatedWorkerChannel,
  type ChannelEvent,
  type WorkerLike,
} from "./workerChannel";

/**
 * A scriptable fake worker: records posted messages; the test decides when
 * (and in which order) responses fire — that's exactly what's needed to
 * exercise requestId correlation and latest-wins coalescing.
 */
class FakeWorker implements WorkerLike {
  readonly posted: unknown[] = [];
  private readonly listeners = new Set<(e: MessageEvent<unknown>) => void>();

  postMessage(message: unknown): void {
    this.posted.push(message);
  }

  addEventListener(
    _type: "message",
    listener: (e: MessageEvent<unknown>) => void,
  ): void {
    this.listeners.add(listener);
  }

  removeEventListener(
    _type: "message",
    listener: (e: MessageEvent<unknown>) => void,
  ): void {
    this.listeners.delete(listener);
  }

  emit(data: unknown): void {
    for (const l of this.listeners) l({ data } as MessageEvent<unknown>);
  }
}

/* ── CorrelatedWorkerChannel ──────────────────────────────────── */

// A miniature of the OCCT wire: two result flavours + error + progress,
// all matched by requestId.
type MiniResult =
  | { type: "export-done"; requestId: number; data: string }
  | { type: "import-done"; requestId: number; mesh: string };

function parseMini(data: unknown): ChannelEvent<MiniResult, string> | null {
  const msg = data as { type?: string; requestId?: number } | null;
  if (!msg || typeof msg !== "object" || typeof msg.requestId !== "number") {
    return null;
  }
  switch (msg.type) {
    case "export-done":
    case "import-done":
      return {
        kind: "result",
        requestId: msg.requestId,
        value: msg as MiniResult,
      };
    case "mini-error":
      return {
        kind: "error",
        requestId: msg.requestId,
        error: (msg as { message?: string }).message ?? "?",
      };
    case "mini-progress":
      return {
        kind: "progress",
        requestId: msg.requestId,
        progress: (msg as { phase?: string }).phase ?? "",
      };
    default:
      return null;
  }
}

describe("CorrelatedWorkerChannel", () => {
  it("routes parallel responses by requestId, not arrival order or type", async () => {
    // The concrete OCCT bug: export and import share the worker; matching
    // by message type let a parallel run capture the sibling's answer.
    const worker = new FakeWorker();
    const channel = new CorrelatedWorkerChannel<MiniResult, string>(
      worker,
      parseMini,
    );

    const exportCall = channel.call((requestId) => ({
      message: { type: "export", requestId },
    }));
    const importCall = channel.call((requestId) => ({
      message: { type: "import", requestId },
    }));

    const [exportId, importId] = worker.posted.map(
      (m) => (m as { requestId: number }).requestId,
    );
    expect(exportId).not.toBe(importId);

    // Import finishes FIRST (out of order) — the export promise must not
    // grab it.
    worker.emit({ type: "import-done", requestId: importId, mesh: "tris" });
    worker.emit({ type: "export-done", requestId: exportId, data: "step" });

    await expect(importCall).resolves.toEqual({
      type: "import-done",
      requestId: importId,
      mesh: "tris",
    });
    await expect(exportCall).resolves.toEqual({
      type: "export-done",
      requestId: exportId,
      data: "step",
    });
  });

  it("rejects only the failing request, the sibling still resolves", async () => {
    const worker = new FakeWorker();
    const channel = new CorrelatedWorkerChannel<MiniResult, string>(
      worker,
      parseMini,
    );

    const a = channel.call((requestId) => ({
      message: { type: "export", requestId },
    }));
    const b = channel.call((requestId) => ({
      message: { type: "import", requestId },
    }));
    const [aId, bId] = worker.posted.map(
      (m) => (m as { requestId: number }).requestId,
    );

    worker.emit({ type: "mini-error", requestId: aId, message: "boom" });
    worker.emit({ type: "import-done", requestId: bId, mesh: "ok" });

    await expect(a).rejects.toThrow("boom");
    await expect(b).resolves.toMatchObject({ mesh: "ok" });
  });

  it("streams progress to the call that owns the requestId", async () => {
    const worker = new FakeWorker();
    const channel = new CorrelatedWorkerChannel<MiniResult, string>(
      worker,
      parseMini,
    );

    const phasesA: string[] = [];
    const phasesB: string[] = [];
    const a = channel.call(
      (requestId) => ({ message: { type: "export", requestId } }),
      (p) => phasesA.push(p),
    );
    const b = channel.call(
      (requestId) => ({ message: { type: "import", requestId } }),
      (p) => phasesB.push(p),
    );
    const [aId, bId] = worker.posted.map(
      (m) => (m as { requestId: number }).requestId,
    );

    worker.emit({ type: "mini-progress", requestId: bId, phase: "loading" });
    worker.emit({ type: "mini-progress", requestId: aId, phase: "writing" });
    worker.emit({ type: "export-done", requestId: aId, data: "x" });
    worker.emit({ type: "import-done", requestId: bId, mesh: "y" });

    await Promise.all([a, b]);
    expect(phasesA).toEqual(["writing"]);
    expect(phasesB).toEqual(["loading"]);
  });

  it("drops responses for unknown requestIds silently", async () => {
    const worker = new FakeWorker();
    const channel = new CorrelatedWorkerChannel<MiniResult, string>(
      worker,
      parseMini,
    );
    worker.emit({ type: "export-done", requestId: 999, data: "stray" });
    const live = channel.call((requestId) => ({
      message: { type: "export", requestId },
    }));
    const id = (worker.posted[0] as { requestId: number }).requestId;
    worker.emit({ type: "export-done", requestId: id, data: "real" });
    await expect(live).resolves.toMatchObject({ data: "real" });
  });

  it("dispose rejects every outstanding call", async () => {
    const worker = new FakeWorker();
    const channel = new CorrelatedWorkerChannel<MiniResult, string>(
      worker,
      parseMini,
      "test channel",
    );
    const a = channel.call((requestId) => ({
      message: { type: "export", requestId },
    }));
    channel.dispose();
    await expect(a).rejects.toThrow(
      /test channel disposed with in-flight calls/,
    );
  });
});

/* ── CoalescingWorkerChannel ──────────────────────────────────── */

interface JobResponse {
  type: "job";
  requestId: number;
  ok: boolean;
  value?: string;
  error?: string;
}

function parseJob(data: unknown): ChannelEvent<string> | null {
  const msg = data as JobResponse | null;
  if (!msg || typeof msg !== "object" || msg.type !== "job") return null;
  return msg.ok
    ? { kind: "result", requestId: msg.requestId, value: msg.value ?? "" }
    : { kind: "error", requestId: msg.requestId, error: msg.error ?? "?" };
}

function makeCoalescing(worker: FakeWorker, opts?: {
  followUpAfterInitial?: boolean;
  buildNull?: () => boolean;
}) {
  const results: string[] = [];
  const errors: string[] = [];
  const passes: ("initial" | "followup")[] = [];
  const channel = new CoalescingWorkerChannel<string>({
    worker,
    parse: parseJob,
    buildRequest: (requestId, pass) => {
      if (opts?.buildNull?.()) return null;
      passes.push(pass);
      return { message: { type: "job-request", requestId, pass } };
    },
    onResult: (value) => results.push(value),
    onError: (error) => errors.push(error),
    followUpAfterInitial: opts?.followUpAfterInitial,
  });
  return { channel, results, errors, passes };
}

function lastPosted(worker: FakeWorker): { requestId: number; pass: string } {
  return worker.posted[worker.posted.length - 1] as {
    requestId: number;
    pass: string;
  };
}

describe("CoalescingWorkerChannel", () => {
  it("coalesces schedules during flight into ONE fresh follow-up", () => {
    const worker = new FakeWorker();
    const { channel, results } = makeCoalescing(worker);

    channel.schedule();
    expect(worker.posted).toHaveLength(1);

    // Three rapid-fire schedules while the first job runs …
    channel.schedule();
    channel.schedule();
    channel.schedule();
    expect(worker.posted).toHaveLength(1);

    // … one response → exactly one follow-up, built fresh.
    const first = lastPosted(worker);
    worker.emit({ type: "job", requestId: first.requestId, ok: true, value: "v1" });
    expect(results).toEqual(["v1"]);
    expect(worker.posted).toHaveLength(2);

    const second = lastPosted(worker);
    worker.emit({ type: "job", requestId: second.requestId, ok: true, value: "v2" });
    expect(results).toEqual(["v1", "v2"]);
    expect(worker.posted).toHaveLength(2); // pipeline idle again
  });

  it("drops stale responses (older requestId than the latest post)", () => {
    const worker = new FakeWorker();
    const { channel, results } = makeCoalescing(worker);

    channel.schedule();
    channel.schedule(); // queued
    const first = lastPosted(worker);
    worker.emit({ type: "job", requestId: first.requestId, ok: true, value: "old-a" });
    const second = lastPosted(worker);
    expect(second.requestId).toBeGreaterThan(first.requestId);

    // A duplicate/late response for the FIRST request must not be delivered.
    worker.emit({ type: "job", requestId: first.requestId, ok: true, value: "old-b" });
    expect(results).toEqual(["old-a"]);

    worker.emit({ type: "job", requestId: second.requestId, ok: true, value: "new" });
    expect(results).toEqual(["old-a", "new"]);
  });

  it("propagates worker errors to onError and keeps the pipeline moving", () => {
    const worker = new FakeWorker();
    const { channel, results, errors } = makeCoalescing(worker);

    channel.schedule();
    channel.schedule(); // queued
    const first = lastPosted(worker);
    worker.emit({ type: "job", requestId: first.requestId, ok: false, error: "kaputt" });
    expect(errors).toEqual(["kaputt"]);

    // The queued schedule still posted its follow-up after the failure.
    const second = lastPosted(worker);
    expect(second.requestId).toBeGreaterThan(first.requestId);
    worker.emit({ type: "job", requestId: second.requestId, ok: true, value: "recovered" });
    expect(results).toEqual(["recovered"]);
  });

  it("followUpAfterInitial posts the refine pass after the initial response", () => {
    const worker = new FakeWorker();
    const { channel, passes } = makeCoalescing(worker, {
      followUpAfterInitial: true,
    });

    channel.schedule();
    expect(passes).toEqual(["initial"]);
    const first = lastPosted(worker);
    worker.emit({ type: "job", requestId: first.requestId, ok: true, value: "coarse" });
    // No queued schedule — the refine follow-up posts anyway.
    expect(passes).toEqual(["initial", "followup"]);
    const second = lastPosted(worker);
    worker.emit({ type: "job", requestId: second.requestId, ok: true, value: "fine" });
    expect(passes).toEqual(["initial", "followup"]); // and then idle
  });

  it("stays idle when buildRequest returns null (nothing to do)", () => {
    const worker = new FakeWorker();
    let empty = true;
    const { channel, results } = makeCoalescing(worker, {
      buildNull: () => empty,
    });

    channel.schedule();
    expect(worker.posted).toHaveLength(0);

    // Work appears → the next schedule posts normally.
    empty = false;
    channel.schedule();
    expect(worker.posted).toHaveLength(1);
    worker.emit({ type: "job", requestId: lastPosted(worker).requestId, ok: true, value: "v" });
    expect(results).toEqual(["v"]);
  });
});
