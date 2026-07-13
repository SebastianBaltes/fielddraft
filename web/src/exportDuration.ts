/**
 * Export runtime formatting (ticket 6a0354e8).
 *
 * The export dialog measures how long a meshing pass took — in the WEB, with
 * `performance.now()` around the call into the kernel/worker. NOT in Rust:
 * `std::time` panics on wasm32 ("time not implemented") and once took the
 * whole viewport down with it.
 */

/** Human runtime for the export report: "412 ms", "12.4 s", "1 min 25 s". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds - minutes * 60);
  // 119.8 s must read "2 min 0 s", not "1 min 60 s".
  if (rest === 60) return `${minutes + 1} min 0 s`;
  return `${minutes} min ${rest} s`;
}
