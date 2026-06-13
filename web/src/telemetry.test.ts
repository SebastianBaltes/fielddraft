import { describe, it, expect, beforeEach } from "vitest";
import {
  tierForFps,
  buildTelemetryRecord,
  recordTelemetry,
  getLastRecord,
  getTelemetryHistory,
  setGpuInfo,
  getGpuInfo,
  type GpuInfo,
} from "./telemetry";

const GPU: GpuInfo = {
  vendor: "nvidia",
  architecture: "lovelace",
  device: "",
  description: "",
  isFallback: false,
  maxStorageBufferBindingSize: 2147483644,
  maxComputeInvocationsPerWorkgroup: 1024,
};

const baseArgs = {
  renderMode: "sdf-deferred",
  resolution: "1920x1080",
  devicePixelRatio: 1,
  bodyCount: 8,
  avgFps: 60,
  p50Ms: 16,
  p95Ms: 20,
  timestamp: 1000,
};

describe("tierForFps", () => {
  it("reports idle for an empty scene regardless of fps", () => {
    expect(tierForFps(240, 0)).toBe("idle");
    expect(tierForFps(0, 0)).toBe("idle");
  });

  it("buckets sustained fps against the 30-fps target", () => {
    expect(tierForFps(60, 5)).toBe("high");
    expect(tierForFps(55, 5)).toBe("high");
    expect(tierForFps(54.9, 5)).toBe("mid");
    expect(tierForFps(30, 5)).toBe("mid");
    expect(tierForFps(29.9, 5)).toBe("low");
    expect(tierForFps(15, 5)).toBe("low");
    expect(tierForFps(14.9, 5)).toBe("unplayable");
    expect(tierForFps(3, 5)).toBe("unplayable");
  });
});

describe("buildTelemetryRecord", () => {
  it("attaches the captured GPU info and derives the tier", () => {
    setGpuInfo(GPU);
    const rec = buildTelemetryRecord(baseArgs);
    expect(rec.gpu).toEqual(GPU);
    expect(rec.tier).toBe("high");
    expect(rec.avgFps).toBe(60);
    expect(rec.renderMode).toBe("sdf-deferred");
  });

  it("derives a low tier from a slow sample", () => {
    const rec = buildTelemetryRecord({ ...baseArgs, avgFps: 22 });
    expect(rec.tier).toBe("low");
  });
});

describe("recordTelemetry", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("updates latest, appends history, and persists to localStorage", () => {
    setGpuInfo(GPU);
    const rec = buildTelemetryRecord({ ...baseArgs, timestamp: 2000 });
    recordTelemetry(rec);

    expect(getLastRecord()).toEqual(rec);
    expect(getTelemetryHistory().at(-1)).toEqual(rec);

    const persisted = JSON.parse(localStorage.getItem("fd.telemetry")!);
    expect(persisted.tier).toBe("high");
    expect(persisted.gpu.vendor).toBe("nvidia");
  });
});

describe("getGpuInfo", () => {
  it("returns the most recently set adapter info", () => {
    setGpuInfo(GPU);
    expect(getGpuInfo()).toEqual(GPU);
  });
});
