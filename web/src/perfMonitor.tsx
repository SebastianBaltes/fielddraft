import { useState, useEffect } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { useKernelStore } from "./store";

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
    const avg =
      all.reduce((a, s) => a + s.frameTimeMs, 0) / all.length;
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

export function PerfProbe({ renderMode }: { renderMode: string }) {
  const { size, gl } = useThree();
  const { store } = useKernelStore();

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
  (window as unknown as {
    __fdBenchmark?: {
      collect: typeof collectBenchmarkResult;
      toCSV: typeof benchmarkResultsToCSV;
      download: typeof downloadCSV;
      monitor: PerfMonitor;
    };
  }).__fdBenchmark = {
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
      if (e.key === "F2") {
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

  return (
    <div className="fd-perf-hud" data-testid="perf-hud">
      FPS: {fps.toFixed(0)} | Frame:{" "}
      {latest?.frameTimeMs.toFixed(1) ?? "—"}ms | p95:{" "}
      {p95.toFixed(1)}ms | Mode: {renderMode} | Bodies:{" "}
      {latest?.bodyCount ?? 0}
    </div>
  );
}
