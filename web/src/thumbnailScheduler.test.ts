import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  maybeCaptureThumbnail,
  resetThumbnailThrottle,
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

  it("renders + uploads when there is geometry", async () => {
    const render = vi.fn().mockResolvedValue(pngBlob());
    const put = vi.fn().mockResolvedValue(undefined);
    const ok = await maybeCaptureThumbnail("p1", [body()], {
      render,
      put,
      nowMs: 1000,
    });
    expect(ok).toBe(true);
    expect(render).toHaveBeenCalledOnce();
    expect(put).toHaveBeenCalledWith("p1", expect.any(Blob));
  });

  it("skips with no geometry and does NOT consume the throttle window", async () => {
    const render = vi.fn();
    const put = vi.fn();
    expect(
      await maybeCaptureThumbnail("p1", [], { render, put, nowMs: 1000 }),
    ).toBe(false);
    expect(render).not.toHaveBeenCalled();

    // A real capture shortly after still fires — the empty attempt didn't claim
    // the throttle.
    const render2 = vi.fn().mockResolvedValue(pngBlob());
    const put2 = vi.fn().mockResolvedValue(undefined);
    expect(
      await maybeCaptureThumbnail("p1", [body()], {
        render: render2,
        put: put2,
        nowMs: 1100,
      }),
    ).toBe(true);
  });

  it("throttles within the interval, then allows again past it", async () => {
    const first = {
      render: vi.fn().mockResolvedValue(pngBlob()),
      put: vi.fn().mockResolvedValue(undefined),
    };
    expect(
      await maybeCaptureThumbnail("p1", [body()], { ...first, nowMs: 0 }),
    ).toBe(true);

    const within = { render: vi.fn(), put: vi.fn() };
    expect(
      await maybeCaptureThumbnail("p1", [body()], {
        ...within,
        nowMs: THUMBNAIL_MIN_INTERVAL_MS - 1,
      }),
    ).toBe(false);
    expect(within.render).not.toHaveBeenCalled();

    const after = {
      render: vi.fn().mockResolvedValue(pngBlob()),
      put: vi.fn().mockResolvedValue(undefined),
    };
    expect(
      await maybeCaptureThumbnail("p1", [body()], {
        ...after,
        nowMs: THUMBNAIL_MIN_INTERVAL_MS + 1,
      }),
    ).toBe(true);
  });

  it("swallows a failed upload (best-effort)", async () => {
    const render = vi.fn().mockResolvedValue(pngBlob());
    const put = vi.fn().mockRejectedValue(new Error("boom"));
    expect(
      await maybeCaptureThumbnail("p1", [body()], { render, put, nowMs: 9000 }),
    ).toBe(false);
  });

  it("skips a null render (no geometry drawn)", async () => {
    const render = vi.fn().mockResolvedValue(null);
    const put = vi.fn();
    expect(
      await maybeCaptureThumbnail("p1", [body()], { render, put, nowMs: 50000 }),
    ).toBe(false);
    expect(put).not.toHaveBeenCalled();
  });
});
