/**
 * Client telemetry — captures the GPU adapter identity plus a steady-state
 * performance summary, so we can understand what hardware real users run on and
 * whether they reach an interactive framerate on the SDF raymarcher.
 *
 * Collection is fully LOCAL: the latest record is kept in memory, mirrored to
 * `localStorage`, and exposed on `window.__fdTelemetry`. Nothing is sent
 * off-device — shipping hardware/perf data to a server is a privacy decision,
 * so the network hop is left as a single explicit, consent-gated call
 * ({@link flushTelemetry}).
 *
 * Why "steady-state": the default frameloop is demand-driven, so most frames
 * are seconds apart (idle) and their frame time says nothing about GPU
 * throughput. The recorder (in `perfMonitor`'s PerfProbe) therefore samples
 * FPS only from a run of CONTINUOUS frames over a non-empty scene — i.e. while
 * the user is actually orbiting real geometry, which is exactly when raymarch
 * cost bites.
 */

export interface GpuInfo {
  /** From `GPUAdapter.info`. Chrome masks `device`/`description` for privacy,
   *  but `vendor` + `architecture` (e.g. "nvidia" / "lovelace") are enough to
   *  bucket a client into a hardware tier. */
  vendor: string;
  architecture: string;
  device: string;
  description: string;
  /** A software/fallback adapter — a strong "this will be slow" signal. */
  isFallback: boolean;
  maxStorageBufferBindingSize: number;
  maxComputeInvocationsPerWorkgroup: number;
}

/** Coarse performance bucket derived from sustained FPS at the sampled load. */
export type PerfTier = "high" | "mid" | "low" | "unplayable" | "idle";

export interface TelemetryRecord {
  gpu: GpuInfo | null;
  renderMode: string;
  /** Physical render resolution, e.g. "1920x1080". */
  resolution: string;
  devicePixelRatio: number;
  bodyCount: number;
  avgFps: number;
  p50Ms: number;
  p95Ms: number;
  tier: PerfTier;
  timestamp: number;
}

const LS_KEY = "fd.telemetry";
const HISTORY_CAP = 50;

let gpuInfo: GpuInfo | null = null;
let lastRecord: TelemetryRecord | null = null;
const history: TelemetryRecord[] = [];

/** Record the GPU adapter identity once, at renderer init. */
export function setGpuInfo(info: GpuInfo): void {
  gpuInfo = info;
}

export function getGpuInfo(): GpuInfo | null {
  return gpuInfo;
}

/**
 * Classify sustained FPS into a coarse hardware tier. An empty scene
 * (`bodyCount === 0`) is almost free to raymarch and tells us nothing about
 * the GPU, so it reports `idle` rather than a misleadingly high tier. The cuts
 * track the 30-fps interactivity target: ≥55 comfortable, ≥30 acceptable,
 * ≥15 sluggish, below that unusable.
 */
export function tierForFps(fps: number, bodyCount: number): PerfTier {
  if (bodyCount === 0) return "idle";
  if (fps >= 55) return "high";
  if (fps >= 30) return "mid";
  if (fps >= 15) return "low";
  return "unplayable";
}

export function buildTelemetryRecord(args: {
  renderMode: string;
  resolution: string;
  devicePixelRatio: number;
  bodyCount: number;
  avgFps: number;
  p50Ms: number;
  p95Ms: number;
  timestamp: number;
}): TelemetryRecord {
  return { gpu: gpuInfo, ...args, tier: tierForFps(args.avgFps, args.bodyCount) };
}

/** Store a record: in-memory history (capped), latest, and localStorage. */
export function recordTelemetry(rec: TelemetryRecord): void {
  lastRecord = rec;
  history.push(rec);
  if (history.length > HISTORY_CAP) history.shift();
  try {
    localStorage.setItem(LS_KEY, JSON.stringify(rec));
  } catch {
    // localStorage can be unavailable (private mode / quota). Telemetry is
    // strictly best-effort — never let it interfere with rendering.
  }
}

export function getLastRecord(): TelemetryRecord | null {
  return lastRecord;
}

export function getTelemetryHistory(): readonly TelemetryRecord[] {
  return history;
}

/**
 * Hook point for server-side aggregation. A deliberate no-op by default:
 * sending hardware/perf data off-device must be an explicit, consent-gated
 * choice. Returns the record that WOULD be sent so a caller can wire it up,
 * e.g. `navigator.sendBeacon("/api/telemetry", JSON.stringify(flushTelemetry()))`.
 */
export function flushTelemetry(): TelemetryRecord | null {
  return lastRecord;
}

// Exposed in every build (not DEV-gated) so production sessions are
// inspectable too — `window.__fdTelemetry.last()` in the console reports the
// client's GPU + measured tier. All accessors are read-only.
if (typeof window !== "undefined") {
  (window as unknown as { __fdTelemetry?: unknown }).__fdTelemetry = {
    gpu: getGpuInfo,
    last: getLastRecord,
    history: getTelemetryHistory,
    flush: flushTelemetry,
  };
}
