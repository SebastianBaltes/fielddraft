import { chromium } from '@playwright/test';

const MODES = ["sdf-vm", "sdf-deferred"];
const WARMUP_MS = 4000;
const MEASURE_MS = 5000;

const browser = await chromium.launch({
  headless: false,
  args: [
    '--no-sandbox',
    '--enable-features=Vulkan,WebGPU',
    '--enable-unsafe-webgpu',
    '--use-angle=vulkan',
  ],
});

const results = [];

for (const mode of MODES) {
  console.log(`\n--- Testing mode: ${mode} ---`);
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await context.newPage();

  const errors = [];
  page.on('console', msg => {
    if (msg.type() === 'error') {
      const t = msg.text();
      if (!t.includes('THREE.TSL') && !t.includes('storageTexture') &&
          !t.includes('WebGPU') && !t.includes('webgl') && !t.includes('THREE.Clock'))
        errors.push(t.slice(0, 120));
    }
  });

  await page.addInitScript((m) => {
    localStorage.setItem('fielddraft.renderMode', m);
    localStorage.removeItem('fielddraft.project.v1');
  }, mode);

  await page.goto('http://localhost:5173/');
  await page.waitForTimeout(2000);

  // Add bodies: sphere + box with difference
  await page.evaluate(() => {
    const store = window.__fdStore;
    if (!store) throw new Error('no store');
    store.addPrimitive('sphere');
    store.setNextOp(1);
    store.addPrimitive('box');
  });

  await page.waitForTimeout(1000);
  await page.screenshot({ path: `/tmp/fd-${mode}-initial.png` });

  // Start continuous invalidation
  await page.evaluate(() => {
    const inv = window.__fdInvalidate;
    if (!inv) return;
    let running = true;
    window.__fdStopBench = () => { running = false; };
    const loop = () => { if (!running) return; inv(); requestAnimationFrame(loop); };
    requestAnimationFrame(loop);
  });

  await page.waitForTimeout(WARMUP_MS);
  await page.evaluate(() => { window.__fdPerfMonitor?.reset(); });
  await page.waitForTimeout(MEASURE_MS);

  // Toggle F2 HUD on, screenshot
  await page.keyboard.press('F2');
  await page.waitForTimeout(300);
  await page.screenshot({ path: `/tmp/fd-${mode}-hud.png` });

  await page.evaluate(() => { window.__fdStopBench?.(); });

  const result = await page.evaluate((m) => {
    const bm = window.__fdBenchmark;
    if (!bm) return null;
    const mon = bm.monitor;
    const latest = mon.latest();
    return {
      mode: m, samples: mon.sampleCount(), avgFps: mon.avgFps(),
      avgFrameMs: mon.avgFrameMs(), p50Ms: mon.p50(), p95Ms: mon.p95(),
      p99Ms: mon.p99(), resolution: latest?.resolution ?? 'unknown',
      bodyCount: latest?.bodyCount ?? 0,
    };
  }, mode);

  if (result) {
    results.push({ ...result, errors });
    console.log(`  Samples: ${result.samples}, FPS: ${result.avgFps.toFixed(1)}, Frame: ${result.avgFrameMs.toFixed(2)}ms, p95: ${result.p95Ms.toFixed(2)}ms`);
  } else {
    results.push({ mode, samples: 0, avgFps: 0, avgFrameMs: 0, p50Ms: 0, p95Ms: 0, p99Ms: 0, resolution: 'unknown', bodyCount: 0, errors: [...errors, 'PerfMonitor N/A'] });
    console.log(`  No perf data available`);
  }
  await context.close();
}

// Summary
console.log('\n' + '='.repeat(95));
console.log('  SDF RENDERING PERFORMANCE BENCHMARK');
console.log('='.repeat(95));
console.log(`${'Mode'.padEnd(14)} ${'Samples'.padStart(8)} ${'Avg FPS'.padStart(10)} ${'Avg ms'.padStart(10)} ${'p50 ms'.padStart(10)} ${'p95 ms'.padStart(10)} ${'p99 ms'.padStart(10)} ${'Bodies'.padStart(8)} ${'Resolution'.padStart(12)}`);
console.log('-'.repeat(95));
for (const r of results) {
  const err = r.errors.length > 0 ? ' *' : '';
  console.log(`${r.mode.padEnd(14)} ${String(r.samples).padStart(8)} ${r.avgFps.toFixed(1).padStart(10)} ${r.avgFrameMs.toFixed(2).padStart(10)} ${r.p50Ms.toFixed(2).padStart(10)} ${r.p95Ms.toFixed(2).padStart(10)} ${r.p99Ms.toFixed(2).padStart(10)} ${String(r.bodyCount).padStart(8)} ${r.resolution.padStart(12)}${err}`);
}
console.log('='.repeat(95));
if (results.some(r => r.errors.length > 0)) {
  console.log('\nErrors:');
  for (const r of results) {
    if (r.errors.length > 0) console.log(`  ${r.mode}: ${r.errors.slice(0, 3).join('; ')}`);
  }
}
console.log(`\n${results.filter(r => r.samples > 0).length}/${results.length} modes produced data.`);
await browser.close();
