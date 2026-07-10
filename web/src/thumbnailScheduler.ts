/**
 * Throttled project-thumbnail capture (ticket 14cff61, variant A).
 *
 * Wired to fire when a SERVER-BOUND project finishes syncing to the server
 * (App.tsx watches the `syncing → clean` transition). Best-effort and heavily
 * throttled: a thumbnail is nice-to-have, never worth stealing a frame on every
 * autosave, and any failure is swallowed (a missing thumbnail is harmless).
 */
import { platformApi } from "./api/platform";
import { renderThumbnailBlob, type ThumbnailBody } from "./thumbnailCapture";

/** At most one capture per project this often (leading-edge). */
export const THUMBNAIL_MIN_INTERVAL_MS = 30_000;

let lastCaptureMs = -Infinity;

/** Injectable dependencies (for tests). */
export interface ThumbnailDeps {
  render?: (bodies: readonly ThumbnailBody[]) => Promise<Blob | null>;
  put?: (projectId: string, png: Blob) => Promise<void>;
  nowMs?: number;
}

/**
 * Capture + upload a thumbnail for `projectId`, throttled to at most one every
 * {@link THUMBNAIL_MIN_INTERVAL_MS}. Resolves to `true` when an upload happened.
 * No-op (returns `false`) when throttled, when there is no geometry, or on any
 * capture/upload failure.
 */
export async function maybeCaptureThumbnail(
  projectId: string,
  bodies: readonly ThumbnailBody[],
  deps: ThumbnailDeps = {},
): Promise<boolean> {
  const render = deps.render ?? renderThumbnailBlob;
  const put = deps.put ?? ((id, png) => platformApi.putThumbnail(id, png));
  const nowMs = deps.nowMs ?? performance.now();

  if (nowMs - lastCaptureMs < THUMBNAIL_MIN_INTERVAL_MS) return false;
  if (!projectId || !bodies || bodies.length === 0) return false;
  // Claim the throttle window up-front so overlapping saves don't double-capture.
  lastCaptureMs = nowMs;

  try {
    const blob = await render(bodies);
    if (!blob) return false;
    await put(projectId, blob);
    return true;
  } catch {
    return false;
  }
}

/** Test hook: clear the throttle so a fresh capture is allowed immediately. */
export function resetThumbnailThrottle(): void {
  lastCaptureMs = -Infinity;
}
