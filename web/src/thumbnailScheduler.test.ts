import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  armProjectThumbnailCapture,
  maybeCaptureThumbnail,
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
    expect(takeArmedProjectCapture()).toBe(false);
    armProjectThumbnailCapture();
    expect(takeArmedProjectCapture()).toBe(true);
    expect(takeArmedProjectCapture()).toBe(false);
  });

  it("resetThumbnailThrottle disarms", () => {
    armProjectThumbnailCapture();
    resetThumbnailThrottle();
    expect(takeArmedProjectCapture()).toBe(false);
  });
});
