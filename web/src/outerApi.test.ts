import { describe, expect, it } from "vitest";
import { installInnerApi, type FdInnerApi } from "./workerSandbox";
import { handleFdCallRequest, isFdCallRequest } from "./workerRpc";
import { OuterApiClient } from "./outerApi";
import type { WorkerLike } from "./workerChannel";

/**
 * A synchronous fake worker that loopbacks `fd_call` requests
 * through the same dispatcher the real worker uses, so tests
 * exercise the exact protocol the production shell runs on —
 * not a mock of the protocol. The "worker" has its own Inner API
 * instance, installed via `installInnerApi` on a fresh scope
 * object; calls resolve in `queueMicrotask` order so the test
 * still sees promise semantics (i.e. `await` yields).
 */
class LoopbackWorker implements WorkerLike {
  private readonly listeners = new Set<
    (event: MessageEvent<unknown>) => void
  >();
  private readonly api: FdInnerApi;
  /** Intercept hook for tests that want to inspect incoming requests. */
  public onRequest: ((data: unknown) => void) | null = null;
  /** When set, every incoming `fd_call` resolves with the frozen response. */
  public forceResponse: unknown = null;

  constructor() {
    const scope = { Date, Math } as unknown as Parameters<
      typeof installInnerApi
    >[0];
    this.api = installInnerApi(scope);
  }

  postMessage(message: unknown): void {
    this.onRequest?.(message);
    if (!isFdCallRequest(message)) return;
    // Dispatch in a microtask so `await` yields between
    // `OuterApiClient.call` returning and the resolve firing —
    // this more closely mirrors the real async postMessage
    // roundtrip and catches any "same tick resolution" bugs.
    queueMicrotask(() => {
      const response =
        this.forceResponse ?? handleFdCallRequest(this.api, message);
      for (const l of this.listeners) {
        l({ data: response } as MessageEvent<unknown>);
      }
    });
  }

  addEventListener(
    _type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void {
    this.listeners.add(listener);
  }

  removeEventListener(
    _type: "message",
    listener: (event: MessageEvent<unknown>) => void,
  ): void {
    this.listeners.delete(listener);
  }
}

describe("OuterApiClient", () => {
  it("round-trips ping through the worker", async () => {
    const worker = new LoopbackWorker();
    const client = new OuterApiClient(worker);
    const result = await client.ping("hello");
    expect(result).toBe("pong: hello");
  });

  it("round-trips a structured-cloneable echo payload unchanged", async () => {
    const worker = new LoopbackWorker();
    const client = new OuterApiClient(worker);
    const payload = { a: 1, b: [2, 3], nested: { c: true } };
    const result = await client.echo(payload);
    expect(result).toEqual(payload);
  });

  it("reads the Inner API version marker via the protocol", async () => {
    const worker = new LoopbackWorker();
    const client = new OuterApiClient(worker);
    const version = await client.version();
    expect(typeof version).toBe("string");
    expect(version).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it("correlates multiple in-flight requests by requestId", async () => {
    const worker = new LoopbackWorker();
    const client = new OuterApiClient(worker);
    const results = await Promise.all([
      client.ping("one"),
      client.ping("two"),
      client.ping("three"),
    ]);
    expect(results).toEqual(["pong: one", "pong: two", "pong: three"]);
  });

  it("rejects the promise on a worker-side error", async () => {
    const worker = new LoopbackWorker();
    const client = new OuterApiClient(worker);
    await expect(client.call("nonexistent", [])).rejects.toThrow(
      /unknown fd method: nonexistent/,
    );
  });

  it("assigns monotonically increasing request ids", async () => {
    const worker = new LoopbackWorker();
    const seen: number[] = [];
    worker.onRequest = (data) => {
      if (
        data &&
        typeof data === "object" &&
        "requestId" in data &&
        typeof (data as { requestId: unknown }).requestId === "number"
      ) {
        seen.push((data as { requestId: number }).requestId);
      }
    };
    const client = new OuterApiClient(worker);
    await client.ping("a");
    await client.ping("b");
    await client.ping("c");
    expect(seen).toEqual([1, 2, 3]);
  });

  it("dispose rejects every outstanding call", async () => {
    const worker = new LoopbackWorker();
    // Freeze the worker so no response ever fires.
    worker.forceResponse = null;
    const client = new OuterApiClient(worker);
    const neverResolves: Promise<unknown>[] = [];
    // Patch postMessage so requests go in but nothing comes out.
    const original = worker.postMessage.bind(worker);
    worker.postMessage = () => {};
    neverResolves.push(client.call("ping", ["a"]));
    neverResolves.push(client.call("ping", ["b"]));
    client.dispose();
    await expect(neverResolves[0]).rejects.toThrow(
      /OuterApiClient disposed with in-flight calls/,
    );
    await expect(neverResolves[1]).rejects.toThrow(
      /OuterApiClient disposed with in-flight calls/,
    );
    worker.postMessage = original;
  });

  it("ignores stale responses that don't match any pending request", async () => {
    const worker = new LoopbackWorker();
    const client = new OuterApiClient(worker);
    // Fire an unrelated response directly at the client — it
    // should be dropped silently, no throw, no crash.
    const stray = {
      type: "fd_result",
      requestId: 99999,
      ok: true,
      value: "ignored",
    };
    for (const l of (worker as unknown as {
      listeners: Set<(event: MessageEvent<unknown>) => void>;
    }).listeners) {
      l({ data: stray } as MessageEvent<unknown>);
    }
    // A live ping still resolves normally.
    await expect(client.ping("live")).resolves.toBe("pong: live");
  });
});
