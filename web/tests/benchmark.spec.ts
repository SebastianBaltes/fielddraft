import { test } from "@playwright/test";

const RENDER_MODES = ["sdf-vm", "sdf-deferred"] as const;
const WARMUP_MS = 3000;
const MEASURE_MS = 5000;

async function waitForShaderCompile(page: import("@playwright/test").Page) {
  await page.waitForFunction(
    () => {
      const el = document.getElementById("fd-shader-compiling");
      return !el || el.style.display === "none";
    },
    { timeout: 25_000 },
  );
}

interface ModeResult {
  mode: string;
  samples: number;
  avgFps: number;
  avgFrameMs: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
  resolution: string;
  bodyCount: number;
  errors: string[];
}

test.describe("SDF Rendering Performance Benchmark", () => {
  test.setTimeout(300_000);

  test("benchmark all render modes with 2-body CSG scene", async ({ page }) => {
    const results: ModeResult[] = [];

    for (const mode of RENDER_MODES) {
      const errors: string[] = [];
      const consoleHandler = (msg: import("@playwright/test").ConsoleMessage) => {
        const text = msg.text();
        if (msg.type() !== "error") return;
        if (text.includes("THREE.TSL:")) return;
        if (text.includes("storageTexture")) return;
        if (text.includes("WebGPU")) return;
        if (text.includes("webgl")) return;
        if (text.includes("THREE.Clock")) return;
        errors.push(text);
      };
      const errorHandler = (err: Error) => errors.push(err.message);
      page.on("console", consoleHandler);
      page.on("pageerror", errorHandler);

      await page.addInitScript((m: string) => {
        localStorage.setItem("fielddraft.renderMode", m);
        localStorage.removeItem("fielddraft.project.v1");
      }, mode);

      await page.goto("/");
      await page.waitForTimeout(1500);

      // Add bodies
      await page.evaluate(() => {
        const store = (window as unknown as {
          __fdStore?: {
            addPrimitive: (k: string) => void;
            setNextOp: (op: number) => void;
          };
        }).__fdStore;
        if (!store) throw new Error("no store");
        store.addPrimitive("sphere");
        store.setNextOp(1);
        store.addPrimitive("box");
      });

      await waitForShaderCompile(page);

      // Start continuous invalidation to force frame rendering
      await page.evaluate(() => {
        const inv = (window as unknown as {
          __fdInvalidate?: () => void;
        }).__fdInvalidate;
        if (!inv) return;
        let running = true;
        (window as unknown as { __fdStopBench?: () => void }).__fdStopBench = () => { running = false; };
        const loop = () => {
          if (!running) return;
          inv();
          requestAnimationFrame(loop);
        };
        requestAnimationFrame(loop);
      });

      // Warmup
      await page.waitForTimeout(WARMUP_MS);

      // Reset monitor, then measure
      await page.evaluate(() => {
        const mon = (window as unknown as {
          __fdPerfMonitor?: { reset: () => void };
        }).__fdPerfMonitor;
        mon?.reset();
      });

      await page.waitForTimeout(MEASURE_MS);

      // Stop continuous invalidation
      await page.evaluate(() => {
        (window as unknown as { __fdStopBench?: () => void }).__fdStopBench?.();
      });

      const result = await page.evaluate((m: string) => {
        const bm = (window as unknown as {
          __fdBenchmark?: {
            monitor: {
              sampleCount: () => number;
              avgFps: () => number;
              avgFrameMs: () => number;
              p50: () => number;
              p95: () => number;
              p99: () => number;
              latest: () => { resolution: string; bodyCount: number } | null;
            };
          };
        }).__fdBenchmark;
        if (!bm) return null;
        const mon = bm.monitor;
        const latest = mon.latest();
        return {
          mode: m,
          samples: mon.sampleCount(),
          avgFps: mon.avgFps(),
          avgFrameMs: mon.avgFrameMs(),
          p50Ms: mon.p50(),
          p95Ms: mon.p95(),
          p99Ms: mon.p99(),
          resolution: latest?.resolution ?? "unknown",
          bodyCount: latest?.bodyCount ?? 0,
        };
      }, mode);

      if (result) {
        results.push({ ...result, errors });
      } else {
        results.push({
          mode,
          samples: 0,
          avgFps: 0,
          avgFrameMs: 0,
          p50Ms: 0,
          p95Ms: 0,
          p99Ms: 0,
          resolution: "unknown",
          bodyCount: 0,
          errors: [...errors, "PerfMonitor not available"],
        });
      }

      page.removeListener("console", consoleHandler);
      page.removeListener("pageerror", errorHandler);
    }

    // Print results table
    console.log("\n" + "=".repeat(95));
    console.log("  SDF RENDERING PERFORMANCE BENCHMARK");
    console.log("  Scene: Sphere + Box (Difference) — Software Rendering (SwiftShader)");
    console.log("=".repeat(95));
    console.log(
      `${"Mode".padEnd(14)} ${"Samples".padStart(8)} ${"Avg FPS".padStart(10)} ${"Avg ms".padStart(10)} ${"p50 ms".padStart(10)} ${"p95 ms".padStart(10)} ${"p99 ms".padStart(10)} ${"Bodies".padStart(8)} ${"Resolution".padStart(12)}`,
    );
    console.log("-".repeat(95));

    for (const r of results) {
      const errTag = r.errors.length > 0 ? ` *` : "";
      console.log(
        `${r.mode.padEnd(14)} ${String(r.samples).padStart(8)} ${r.avgFps.toFixed(1).padStart(10)} ${r.avgFrameMs.toFixed(2).padStart(10)} ${r.p50Ms.toFixed(2).padStart(10)} ${r.p95Ms.toFixed(2).padStart(10)} ${r.p99Ms.toFixed(2).padStart(10)} ${String(r.bodyCount).padStart(8)} ${r.resolution.padStart(12)}${errTag}`,
      );
    }

    console.log("=".repeat(95));
    console.log("  * = had non-fatal errors during run");
    console.log("  Note: SwiftShader software rendering — real GPU will be significantly faster");

    if (results.some((r) => r.errors.length > 0)) {
      console.log("\nErrors per mode:");
      for (const r of results) {
        if (r.errors.length > 0) {
          console.log(`  ${r.mode}:`);
          for (const e of r.errors.slice(0, 3)) {
            console.log(`    - ${e.slice(0, 120)}`);
          }
          if (r.errors.length > 3) console.log(`    ... and ${r.errors.length - 3} more`);
        }
      }
    }

    // Don't assert hard — this is a benchmark, not a pass/fail test
    const working = results.filter((r) => r.samples > 0);
    console.log(`\n${working.length}/${results.length} modes produced frame data.`);
  });
});
