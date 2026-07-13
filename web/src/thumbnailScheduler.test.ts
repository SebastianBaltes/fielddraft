import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  armProjectThumbnailCapture,
  maybeCaptureThumbnail,
  rearmProjectThumbnailCapture,
  resetThumbnailThrottle,
  takeArmedProjectCapture,
  THUMBNAIL_MIN_INTERVAL_MS,
} from "./thumbnailScheduler";
import type { ThumbnailBody } from "./thumbnailCapture";

const body = (): ThumbnailBody => ({
  positions: new Float32Array([0, 0, 0]),
  normals: new Float32Array([0, 1, 0]),
  indices: new Uint32Array([0]),
});

const pngBlob = () => new Blob(["png"], { type: "image/png" });

describe("maybeCaptureThumbnail", () => {
  beforeEach(() => resetThumbnailThrottle());

  it("renders + hands the blob to the sink when there is geometry", async () => {
    const render = vi.fn().mockResolvedValue(pngBlob());
    const sink = vi.fn().mockResolvedValue(undefined);
    const ok = await maybeCaptureThumbnail([body()], sink, {
      render,
      nowMs: 1000,
    });
    expect(ok).toBe(true);
    expect(render).toHaveBeenCalledOnce();
    expect(sink).toHaveBeenCalledWith(expect.any(Blob));
  });

  it("skips with no geometry and does NOT consume the throttle window", async () => {
    const render = vi.fn();
    const sink = vi.fn();
    expect(await maybeCaptureThumbnail([], sink, { render, nowMs: 1000 })).toBe(
      false,
    );
    expect(render).not.toHaveBeenCalled();

    // A real capture shortly after still fires — the empty attempt didn't claim
    // the throttle.
    const render2 = vi.fn().mockResolvedValue(pngBlob());
    const sink2 = vi.fn().mockResolvedValue(undefined);
    expect(
      await maybeCaptureThumbnail([body()], sink2, {
        render: render2,
        nowMs: 1100,
      }),
    ).toBe(true);
  });

  it("throttles within the interval, then allows again past it", async () => {
    expect(
      await maybeCaptureThumbnail(
        [body()],
        vi.fn().mockResolvedValue(undefined),
        { render: vi.fn().mockResolvedValue(pngBlob()), nowMs: 0 },
      ),
    ).toBe(true);

    const withinRender = vi.fn();
    expect(
      await maybeCaptureThumbnail([body()], vi.fn(), {
        render: withinRender,
        nowMs: THUMBNAIL_MIN_INTERVAL_MS - 1,
      }),
    ).toBe(false);
    expect(withinRender).not.toHaveBeenCalled();

    expect(
      await maybeCaptureThumbnail(
        [body()],
        vi.fn().mockResolvedValue(undefined),
        {
          render: vi.fn().mockResolvedValue(pngBlob()),
          nowMs: THUMBNAIL_MIN_INTERVAL_MS + 1,
        },
      ),
    ).toBe(true);
  });

  it("swallows a failed sink (best-effort)", async () => {
    const render = vi.fn().mockResolvedValue(pngBlob());
    const sink = vi.fn().mockRejectedValue(new Error("boom"));
    expect(
      await maybeCaptureThumbnail([body()], sink, { render, nowMs: 9000 }),
    ).toBe(false);
  });

  it("skips a null render (no geometry drawn)", async () => {
    const render = vi.fn().mockResolvedValue(null);
    const sink = vi.fn();
    expect(
      await maybeCaptureThumbnail([body()], sink, { render, nowMs: 50000 }),
    ).toBe(false);
    expect(sink).not.toHaveBeenCalled();
  });
});

describe("armProjectThumbnailCapture / takeArmedProjectCapture", () => {
  beforeEach(() => resetThumbnailThrottle());

  it("is one-shot: fires once after arming, then not again", () => {
    expect(takeArmedProjectCapture("p1")).toBe(false);
    armProjectThumbnailCapture("p1");
    expect(takeArmedProjectCapture("p1")).toBe(true);
    expect(takeArmedProjectCapture("p1")).toBe(false);
  });

  // Ticket #343: the arm belongs to ONE project. The geometry of the project we
  // just left must not consume it — that is how a fresh project ended up wearing
  // the previous project's picture.
  it("only fires for the project it was armed for", () => {
    armProjectThumbnailCapture("new-project");
    expect(takeArmedProjectCapture("old-project")).toBe(false);
    expect(takeArmedProjectCapture(null)).toBe(false);
    expect(takeArmedProjectCapture("new-project")).toBe(true);
  });

  it("re-arming restores a failed one-shot", () => {
    armProjectThumbnailCapture("p1");
    expect(takeArmedProjectCapture("p1")).toBe(true);
    rearmProjectThumbnailCapture("p1"); // capture failed → try again next mesh
    expect(takeArmedProjectCapture("p1")).toBe(true);
    expect(takeArmedProjectCapture("p1")).toBe(false);
  });

  it("resetThumbnailThrottle disarms", () => {
    armProjectThumbnailCapture("p1");
    resetThumbnailThrottle();
    expect(takeArmedProjectCapture("p1")).toBe(false);
  });
});

// Ticket #343: the armed one-shot must not be eaten by the throttle window a
// preceding editor-exit capture claimed — the project would then never get a
// preview at all (the arm is spent, and nothing re-triggers it).
describe("forced capture", () => {
  beforeEach(() => resetThumbnailThrottle());

  it("captures inside the throttle window when forced", async () => {
    const render = vi.fn().mockResolvedValue(new Blob(["x"]));
    const sink = vi.fn().mockResolvedValue(undefined);
    expect(
      await maybeCaptureThumbnail([body()], sink, { render, nowMs: 0 }),
    ).toBe(true);
    // Throttled…
    expect(
      await maybeCaptureThumbnail([body()], sink, { render, nowMs: 1000 }),
    ).toBe(false);
    // …but the one-shot goes through.
    expect(
      await maybeCaptureThumbnail([body()], sink, {
        render,
        nowMs: 1000,
        force: true,
      }),
    ).toBe(true);
    expect(sink).toHaveBeenCalledTimes(2);
  });

  it("still reports failure when there is no geometry to draw", async () => {
    const render = vi.fn().mockResolvedValue(new Blob(["x"]));
    const sink = vi.fn().mockResolvedValue(undefined);
    expect(await maybeCaptureThumbnail([], sink, { render, force: true })).toBe(
      false,
    );
    expect(render).not.toHaveBeenCalled();
  });
});
