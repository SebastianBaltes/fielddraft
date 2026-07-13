/**
 * Ticket #343 — the thumbnail meshes its own document, because in the default
 * (SDF) render mode the app has no meshes at all. A fake worker stands in for
 * the mesher: what matters here is the request, the mapping and that every
 * failure mode ends as "no bodies" rather than an exception.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import { meshDocumentForThumbnail } from "./thumbnailMesh";

type Listener = (event: { data?: unknown }) => void;

class FakeWorker {
  posted: unknown[] = [];
  terminated = false;
  private listeners: Record<string, Listener[]> = {};

  addEventListener(type: string, fn: Listener) {
    (this.listeners[type] ??= []).push(fn);
  }
  postMessage(msg: unknown) {
    this.posted.push(msg);
  }
  terminate() {
    this.terminated = true;
  }
  emit(type: string, event: { data?: unknown }) {
    for (const fn of this.listeners[type] ?? []) fn(event);
  }
}

function body(id: number) {
  return {
    id,
    seedId: id,
    version: 1,
    positions: new Float32Array([0, 0, 0]),
    normals: new Float32Array([0, 1, 0]),
    indices: new Uint32Array([0]),
    classifications: new Uint8Array([0]),
    contributors: new Uint32Array([0]),
    transformDelta: null,
  };
}

let worker: FakeWorker;
const spawn = () => worker as unknown as Worker;

beforeEach(() => {
  worker = new FakeWorker();
  vi.useRealTimers();
});

describe("meshDocumentForThumbnail", () => {
  it("asks the worker to mesh the document and maps the bodies", async () => {
    const pending = meshDocumentForThumbnail('{"doc":1}', { spawn });
    worker.emit("message", {
      data: { type: "bodies", requestId: 1, ok: true, bodies: [body(1), body(2)] },
    });

    const bodies = await pending;
    expect(bodies).toHaveLength(2);
    expect(bodies[0].positions).toEqual(new Float32Array([0, 0, 0]));
    expect(worker.posted).toEqual([
      expect.objectContaining({ type: "rebuild", documentJson: '{"doc":1}' }),
    ]);
    // One-shot: the worker is gone again as soon as the bodies are in.
    expect(worker.terminated).toBe(true);
  });

  it("resolves empty (never throws) when the worker reports a failure", async () => {
    const pending = meshDocumentForThumbnail("{}", { spawn });
    worker.emit("message", { data: { type: "bodies", requestId: 1, ok: false } });
    expect(await pending).toEqual([]);
    expect(worker.terminated).toBe(true);
  });

  it("resolves empty when the worker errors out", async () => {
    const pending = meshDocumentForThumbnail("{}", { spawn });
    worker.emit("error", {});
    expect(await pending).toEqual([]);
    expect(worker.terminated).toBe(true);
  });

  it("gives up (and cleans up) after the guard timeout", async () => {
    const bodies = await meshDocumentForThumbnail("{}", { spawn, timeoutMs: 1 });
    expect(bodies).toEqual([]);
    expect(worker.terminated).toBe(true);
  });

  it("resolves empty when the worker cannot even be spawned", async () => {
    const bodies = await meshDocumentForThumbnail("{}", {
      spawn: () => {
        throw new Error("no workers here");
      },
    });
    expect(bodies).toEqual([]);
  });
});
