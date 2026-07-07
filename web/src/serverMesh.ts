/**
 * Server-side CGAL meshing client (plan §P5 / M4).
 *
 * Drives the backend job queue: POST the mesh order for a SAVED project
 * part, poll the job to done/error (cheap DB reads), download the binary
 * STL artifact and parse it into the export-preview buffers. Cancel maps
 * to DELETE (best effort — only queued jobs abort server-side).
 */

/** Mesher report as forwarded by the backend (fd-cgal-mesh stderr JSON). */
export interface ServerMeshReport {
  bodies?: number;
  edges?: number;
  vertices?: number;
  triangles?: number;
}

export interface ServerMeshResult {
  /** The binary STL exactly as the server built it (save = these bytes). */
  bytes: Uint8Array;
  report: ServerMeshReport;
}

const POLL_INTERVAL_MS = 1500;

async function jsonOrThrow(res: Response, what: string): Promise<any> {
  if (!res.ok) {
    let detail = "";
    try {
      detail = ((await res.json()) as { error?: string }).error ?? "";
    } catch {
      // non-JSON error body
    }
    throw new Error(detail !== "" ? detail : `${what}: HTTP ${res.status}`);
  }
  return res.json();
}

/** Submit one mesh job and run it to completion. `onStatus` gets coarse
 *  user-facing phases ("Queued…", "Meshing on server…"). Aborting the signal
 *  cancels the job (DELETE) and rejects with an AbortError. */
export async function runServerMesh(
  projectId: string,
  partId: string,
  params: { cellSizeMm: number; facetDistMult?: number },
  onStatus: (status: string) => void,
  signal: AbortSignal,
): Promise<ServerMeshResult> {
  const submit = await fetch(`/api/projects/${projectId}/mesh`, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ partId, params }),
    signal,
  });
  const { jobId } = (await jsonOrThrow(submit, "submit")) as { jobId: string };
  onStatus("Queued…");

  try {
    for (;;) {
      await sleep(POLL_INTERVAL_MS, signal);
      const res = await fetch(`/api/mesh-jobs/${jobId}`, {
        credentials: "include",
        signal,
      });
      const job = (await jsonOrThrow(res, "poll")) as {
        status: string;
        url?: string;
        report?: ServerMeshReport;
        error?: string;
      };
      if (job.status === "error") {
        throw new Error(job.error ?? "server meshing failed");
      }
      if (job.status === "done" && job.url) {
        onStatus("Downloading…");
        const stl = await fetch(job.url, { credentials: "include", signal });
        if (!stl.ok) throw new Error(`artifact: HTTP ${stl.status}`);
        return {
          bytes: new Uint8Array(await stl.arrayBuffer()),
          report: job.report ?? {},
        };
      }
      onStatus(job.status === "running" ? "Meshing on server…" : "Queued…");
    }
  } catch (err) {
    if (signal.aborted) {
      // Best effort: a still-queued job is freed server-side; a running one
      // finishes unobserved (its result is simply never fetched).
      void fetch(`/api/mesh-jobs/${jobId}`, {
        method: "DELETE",
        credentials: "include",
      }).catch(() => {});
    }
    throw err;
  }
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(new DOMException("aborted", "AbortError"));
      return;
    }
    const t = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(t);
      reject(new DOMException("aborted", "AbortError"));
    };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

/** Parse a binary STL into the export-preview buffers. Vertices stay
 *  unwelded (trivial 0..n indices) — plenty for preview rendering, and the
 *  saved file is the untouched server STL anyway. */
export function parseBinaryStl(bytes: Uint8Array): {
  positions: Float32Array;
  indices: Uint32Array;
} {
  if (bytes.length < 84) throw new Error("STL too short");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const ntri = view.getUint32(80, true);
  if (bytes.length < 84 + ntri * 50) throw new Error("STL truncated");
  const positions = new Float32Array(ntri * 9);
  const indices = new Uint32Array(ntri * 3);
  for (let t = 0; t < ntri; t++) {
    const off = 84 + t * 50 + 12; // skip the normal
    for (let k = 0; k < 9; k++) {
      positions[t * 9 + k] = view.getFloat32(off + k * 4, true);
    }
    indices[t * 3] = t * 3;
    indices[t * 3 + 1] = t * 3 + 1;
    indices[t * 3 + 2] = t * 3 + 2;
  }
  return { positions, indices };
}
