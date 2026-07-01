import { useState, useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { useKernelStore } from "./store";
import {
  buildTelemetryRecord,
  recordTelemetry,
  getGpuInfo,
  getLastRecord,
} from "./telemetry";

export interface PerfSnapshot {
  frameTimeMs: number;
  gpuTimeMs: number | null;
  fps: number;
  bodyCount: number;
  renderMode: string;
  resolution: string;
  timestamp: number;
}

export class PerfMonitor {
  private ring: (PerfSnapshot | undefined)[];
  private head = 0;
  private count = 0;
  private readonly size = 600;

  constructor() {
    this.ring = new Array(this.size);
  }

  addSample(s: PerfSnapshot): void {
    this.ring[this.head] = s;
    this.head = (this.head + 1) % this.size;
    if (this.count < this.size) this.count++;
  }

  private samples(): PerfSnapshot[] {
    if (this.count === 0) return [];
    const out: PerfSnapshot[] = [];
    const start = (this.head - this.count + this.size) % this.size;
    for (let i = 0; i < this.count; i++) {
      const s = this.ring[(start + i) % this.size];
      if (s) out.push(s);
    }
    return out;
  }

  private percentile(p: number): number {
    const sorted = this.samples()
      .map((s) => s.frameTimeMs)
      .sort((a, b) => a - b);
    if (sorted.length === 0) return 0;
    const idx = Math.ceil((sorted.length * p) / 100) - 1;
    return sorted[Math.max(0, idx)];
  }

  p50(): number {
    return this.percentile(50);
  }

  p95(): number {
    return this.percentile(95);
  }

  p99(): number {
    return this.percentile(99);
  }

  avgFps(): number {
    const all = this.samples();
    if (all.length === 0) return 0;
    const avg = all.reduce((a, s) => a + s.frameTimeMs, 0) / all.length;
    return avg > 0 ? 1000 / avg : 0;
  }

  latest(): PerfSnapshot | null {
    if (this.count === 0) return null;
    return this.ring[(this.head - 1 + this.size) % this.size] ?? null;
  }

  toCSV(): string {
    const lines = [
      "timestamp,frameTimeMs,gpuTimeMs,fps,bodyCount,renderMode,resolution",
    ];
    for (const s of this.samples()) {
      lines.push(
        `${s.timestamp},${s.frameTimeMs.toFixed(2)},${s.gpuTimeMs?.toFixed(2) ?? ""},${s.fps.toFixed(1)},${s.bodyCount},${s.renderMode},${s.resolution}`,
      );
    }
    return lines.join("\n");
  }

  sampleCount(): number {
    return this.count;
  }

  avgFrameMs(): number {
    const all = this.samples();
    if (all.length === 0) return 0;
    return all.reduce((a, s) => a + s.frameTimeMs, 0) / all.length;
  }

  reset(): void {
    this.head = 0;
    this.count = 0;
  }
}

export const perfMonitor = new PerfMonitor();

if (typeof window !== "undefined" && import.meta.env.DEV) {
  (window as unknown as { __fdPerfMonitor?: PerfMonitor }).__fdPerfMonitor =
    perfMonitor;
}

/** A frame within this gap belongs to a CONTINUOUS render run (camera motion /
 *  always-mode); only true demand-mode idle gaps (seconds apart) exceed it.
 *  Deliberately generous — 250 ms ≈ 4 fps — so a slow-but-rendering low-end GPU
 *  is still measured (and tiered "low"), not mistaken for idle and skipped. */
const ACTIVE_FRAME_MS = 250;
/** Window of recent frames inspected for an active run. */
const ACTIVE_WINDOW = 90;
/** Min active frames in the window before a telemetry record is trustworthy. */
const ACTIVE_MIN = 30;
/** Throttle between persisted telemetry records (ms). */
const TELEMETRY_INTERVAL_MS = 2000;

export function PerfProbe({ renderMode }: { renderMode: string }) {
  const { size, gl } = useThree();
  const { store } = useKernelStore();
  const activeRef = useRef<number[]>([]);
  const lastTelemetryRef = useRef(0);

  useFrame((_, delta) => {
    const frameTimeMs = delta * 1000;
    const fps = delta > 0 ? 1 / delta : 0;
    const bodyCount = store?.kernel.bodyCount() ?? 0;
    const dpr =
      (gl as unknown as { getPixelRatio?: () => number }).getPixelRatio?.() ??
      window.devicePixelRatio ??
      1;
    const resolution = `${Math.round(size.width * dpr)}x${Math.round(size.height * dpr)}`;

    perfMonitor.addSample({
      frameTimeMs,
      gpuTimeMs: null,
      fps,
      bodyCount,
      renderMode,
      resolution,
      timestamp: performance.now(),
    });

    // Telemetry: only sample FPS during a sustained run of continuous frames
    // over a non-empty scene — that is the one window where frame time
    // reflects real raymarch throughput. perfMonitor.avgFps() can't be used
    // here: its ring mixes in demand-mode idle gaps and would read near-zero.
    const win = activeRef.current;
    win.push(frameTimeMs);
    if (win.length > ACTIVE_WINDOW) win.shift();
    const active = win.filter((ms) => ms < ACTIVE_FRAME_MS);
    const now = performance.now();
    if (
      active.length >= ACTIVE_MIN &&
      bodyCount > 0 &&
      now - lastTelemetryRef.current > TELEMETRY_INTERVAL_MS
    ) {
      lastTelemetryRef.current = now;
      const sorted = [...active].sort((a, b) => a - b);
      const mean = active.reduce((a, b) => a + b, 0) / active.length;
      const pct = (p: number) =>
        sorted[
          Math.min(
            sorted.length - 1,
            Math.max(0, Math.ceil((sorted.length * p) / 100) - 1),
          )
        ] ?? 0;
      recordTelemetry(
        buildTelemetryRecord({
          renderMode,
          resolution,
          devicePixelRatio: dpr,
          bodyCount,
          avgFps: mean > 0 ? 1000 / mean : 0,
          p50Ms: pct(50),
          p95Ms: pct(95),
          timestamp: now,
        }),
      );
    }
  });

  return null;
}

// ── Benchmark infrastructure ──────────────────────────────────────

export interface BenchmarkResult {
  scene: string;
  renderMode: string;
  resolution: string;
  samples: number;
  avgFrameMs: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
  avgFps: number;
}

export function collectBenchmarkResult(
  sceneName: string,
): BenchmarkResult | null {
  const latest = perfMonitor.latest();
  if (!latest) return null;

  return {
    scene: sceneName,
    renderMode: latest.renderMode,
    resolution: latest.resolution,
    samples: perfMonitor.sampleCount(),
    avgFrameMs: perfMonitor.avgFrameMs(),
    p50Ms: perfMonitor.p50(),
    p95Ms: perfMonitor.p95(),
    p99Ms: perfMonitor.p99(),
    avgFps: perfMonitor.avgFps(),
  };
}

export function benchmarkResultsToCSV(results: BenchmarkResult[]): string {
  const lines = [
    "scene,renderMode,resolution,samples,avgFrameMs,p50Ms,p95Ms,p99Ms,avgFps",
  ];
  for (const r of results) {
    lines.push(
      `${r.scene},${r.renderMode},${r.resolution},${r.samples},` +
        `${r.avgFrameMs.toFixed(2)},${r.p50Ms.toFixed(2)},${r.p95Ms.toFixed(2)},` +
        `${r.p99Ms.toFixed(2)},${r.avgFps.toFixed(1)}`,
    );
  }
  return lines.join("\n");
}

export function downloadCSV(csv: string, filename: string): void {
  const blob = new Blob([csv], { type: "text/csv" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

if (typeof window !== "undefined" && import.meta.env.DEV) {
  (
    window as unknown as {
      __fdBenchmark?: {
        collect: typeof collectBenchmarkResult;
        toCSV: typeof benchmarkResultsToCSV;
        download: typeof downloadCSV;
        monitor: PerfMonitor;
      };
    }
  ).__fdBenchmark = {
    collect: collectBenchmarkResult,
    toCSV: benchmarkResultsToCSV,
    download: downloadCSV,
    monitor: perfMonitor,
  };
}

export function PerfHud({ renderMode }: { renderMode: string }) {
  const [visible, setVisible] = useState(false);
  const [, tick] = useState(0);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Ticket B: the perf HUD toggles on F9 (moved off F2, which now belongs
      // solely to the Report-Issue dialog). Close-button dismissal below.
      if (e.key === "F9") {
        setVisible((v) => !v);
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  useEffect(() => {
    if (!visible) return;
    const id = setInterval(() => tick((v) => v + 1), 100);
    return () => clearInterval(id);
  }, [visible]);

  if (!visible) return null;

  const latest = perfMonitor.latest();
  const fps = perfMonitor.avgFps();
  const p95 = perfMonitor.p95();
  const gpu = getGpuInfo();
  const telemetry = getLastRecord();
  const gpuLabel = gpu
    ? [gpu.vendor, gpu.architecture].filter(Boolean).join(" ") || "unknown"
    : "—";

  return (
    <div className="fd-perf-hud" data-testid="perf-hud">
      {/* Ticket B: mouse-dismissable close (×) — the HUD no longer depends on a
          keypress to go away. */}
      <button
        type="button"
        className="close"
        data-testid="perf-hud-close"
        aria-label="Close performance monitor"
        title="Close (F9 to toggle)"
        onClick={() => setVisible(false)}
      >
        &times;
      </button>
      <div>
        FPS: {fps.toFixed(0)} | Frame: {latest?.frameTimeMs.toFixed(1) ?? "—"}ms
        | p95: {p95.toFixed(1)}ms | Mode: {renderMode} | Bodies:{" "}
        {latest?.bodyCount ?? 0}
      </div>
      <div>
        GPU: {gpuLabel}
        {gpu?.isFallback ? " (software)" : ""} | Tier:{" "}
        {telemetry
          ? `${telemetry.tier} @ ${telemetry.avgFps.toFixed(0)}fps`
          : "—"}
      </div>
    </div>
  );
}
