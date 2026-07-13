/**
 * On-demand tessellation for the project thumbnail (ticket #343).
 *
 * The gallery preview is rendered from triangle meshes — but the app only HAS
 * meshes in the "mesh" and "webgl" render modes: everywhere else App.tsx keeps
 * Dual Contouring switched off (`setDcMeshingEnabled`), because the SDF renderers
 * raymarch the field and never need triangles. `state.meshedBodies` is therefore
 * empty in the DEFAULT render mode, and the thumbnail capture silently rendered
 * nothing: a project imported or opened in SDF mode never got a preview at all.
 *
 * So the capture meshes its own document, once, in a SHORT-LIVED mesher worker:
 *   · off the main thread — a Dual-Contouring pass on a heavy model would
 *     otherwise freeze the tab for the length of the mesh;
 *   · in its own kernel — the live document, the live mesher and
 *     `state.meshedBodies` are untouched, so no consumer suddenly sees meshes
 *     that the render mode says should not exist;
 *   · at COARSE resolution — it is a 512-px picture, not an export.
 *
 * The worker is terminated as soon as the bodies are in (or the guard timeout
 * fires). Everything is best-effort: a missing thumbnail is harmless, so every
 * failure resolves to an empty body list rather than throwing.
 */

import type { MesherBodyPayload, MesherResponse } from "./mesher.worker";
import type { ThumbnailBody } from "./thumbnailCapture";

/** Mesh resolution of a thumbnail pass. Coarse on purpose (see module doc). */
const THUMBNAIL_MESH_RESOLUTION = 32;

/** Give up if the worker has not answered by then (ms). */
const THUMBNAIL_MESH_TIMEOUT_MS = 20_000;

/** Injectable seam for tests — the real thing spawns the mesher worker. */
export interface ThumbnailMeshDeps {
  spawn?: () => Worker;
  timeoutMs?: number;
}

function defaultSpawn(): Worker {
  return new Worker(new URL("./mesher.worker.ts", import.meta.url), {
    type: "module",
  });
}

/**
 * Tessellate `documentJson` (a kernel document) into thumbnail bodies. Resolves
 * `[]` when the document has no geometry, the worker fails, or it takes longer
 * than the guard timeout — the caller then simply skips the capture.
 */
export function meshDocumentForThumbnail(
  documentJson: string,
  deps: ThumbnailMeshDeps = {},
): Promise<ThumbnailBody[]> {
  const spawn = deps.spawn ?? defaultSpawn;
  const timeoutMs = deps.timeoutMs ?? THUMBNAIL_MESH_TIMEOUT_MS;

  return new Promise<ThumbnailBody[]>((resolve) => {
    let worker: Worker;
    try {
      worker = spawn();
    } catch {
      resolve([]);
      return;
    }

    let done = false;
    const finish = (bodies: ThumbnailBody[]) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      worker.terminate();
      resolve(bodies);
    };

    const timer = setTimeout(() => finish([]), timeoutMs);

    worker.addEventListener("message", (event: MessageEvent<unknown>) => {
      const msg = event.data as MesherResponse | undefined;
      if (!msg || msg.type !== "bodies") return;
      if (!msg.ok || !msg.bodies) {
        finish([]);
        return;
      }
      finish(msg.bodies.map(toThumbnailBody));
    });
    worker.addEventListener("error", () => finish([]));

    worker.postMessage({
      type: "rebuild",
      requestId: 1,
      documentJson,
      activeMeshResolution: THUMBNAIL_MESH_RESOLUTION,
      contextMeshResolution: THUMBNAIL_MESH_RESOLUTION,
      activeBodyId: 0,
      meshAlgorithm: "mdc",
    });
  });
}

function toThumbnailBody(b: MesherBodyPayload): ThumbnailBody {
  return { positions: b.positions, normals: b.normals, indices: b.indices };
}
