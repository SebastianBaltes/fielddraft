import { expect, test } from "@playwright/test";

/**
 * Custom-SDF GPU↔CPU parity.
 *
 * The `custom_sdf` feature carries a user expression compiled to an
 * `SdfExprOp` tape (`OP_PRIM_CUSTOM`), interpreted by a NEW inner loop in
 * the GPU VM (own es*-register stack). This test loads a body whose
 * expression exercises every hot op class (sin/cos products, sqrt of a
 * sum of squares, min/max, division, unary minus) and proves the LIVE
 * TSL interpreter (window.__fdSdfVmProbe) matches the CPU mirror
 * (kernel.bodySdfEval) over shape-critical points + a coarse AABB grid.
 * Together with the native parity tests (logical CSG == CPU mirror ==
 * dual gradients) the chain logical == CPU == GPU is closed.
 */

const DOC = JSON.stringify({
  schema_version: 2,
  name: "CustomSdf",
  feature_calls: [
    {
      type: "custom_sdf",
      id: 1,
      params: {
        expression:
          "max(sin(x/4)*cos(y/4) + sin(y/4)*cos(z/4) + sin(z/4)*cos(x/4) - 0.4, sqrt(x*x + y*y + z*z)/3 - 10)",
        lipschitz: 2.0,
        half_extents: [25.0, 25.0, 25.0],
        op: "new",
        blend_radius: { value: 0.0, unit: "mm" },
        chamfer_size: { value: 0.0, unit: "mm" },
        shell_thickness: { value: 0.0, unit: "mm" },
      },
    },
  ],
  next_feature_id: 2,
});

test("custom sdf: GPU expression interpreter matches the CPU mirror", async ({ page }) => {
  test.setTimeout(300_000);
  const logs: string[] = [];
  page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));

  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem("fielddraft.renderMode", "sdf-vm");
    localStorage.removeItem("fielddraft.project.v2");
  });
  await page.reload();
  await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
  await page.waitForTimeout(1500);

  const loaded = await page.evaluate((doc) => {
    const project = JSON.stringify({
      schemaVersion: 1,
      name: "CustomSdf",
      activeTabId: "tab_1",
      tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson: doc }],
    });
    return (window as any).__fdStore.loadProjectFromJson(project);
  }, DOC);
  expect(loaded, "project loads").toBe(true);
  await page.waitForTimeout(1000);

  await page.waitForFunction(() => typeof (window as any).__fdSdfVmProbe === "function", null, {
    timeout: 180_000,
  });

  // The opcode upload is frame-driven (demand frameloop): retry a sentinel
  // point until GPU and CPU agree there, so the grid probe below never
  // races the async buffer update.
  await page.waitForFunction(
    async () => {
      const kernel = (window as any).__fdKernel;
      const probe = (window as any).__fdSdfVmProbe;
      if (!kernel || typeof probe !== "function") return false;
      try {
        const gpu = (await probe([[6.3, 0, 0]], 0)) as number[];
        const cpu = kernel.bodySdfEval(0, 6.3, 0, 0) as number;
        return Math.abs(gpu[0] - cpu) < 1e-3;
      } catch {
        return false;
      }
    },
    null,
    { timeout: 120_000, polling: 500 },
  );

  const result = await page.evaluate(async () => {
    const kernel = (window as any).__fdKernel;
    const probe = (window as any).__fdSdfVmProbe;
    const pts: [number, number, number][] = [
      [0, 0, 0], // gyroid saddle at the origin
      [6.3, 0, 0], // near a lattice wall
      [0, 6.3, 0],
      [12, 5, -7],
      [24.5, 24.5, 24.5], // just inside the clip box corner
      [30, 0, 0], // outside the box — box term dominates
      [-30, -30, 0],
    ];
    const aabb = kernel.bodySdfAabb(0) as number[];
    const n = 5;
    for (let iz = 0; iz <= n; iz++)
      for (let iy = 0; iy <= n; iy++)
        for (let ix = 0; ix <= n; ix++)
          pts.push([
            aabb[0] + ((aabb[3] - aabb[0]) * ix) / n,
            aabb[1] + ((aabb[4] - aabb[1]) * iy) / n,
            aabb[2] + ((aabb[5] - aabb[2]) * iz) / n,
          ]);
    const cpu = pts.map((p) => kernel.bodySdfEval(0, p[0], p[1], p[2]) as number);
    // The opcode buffer is uploaded on a demand-driven frame, so the FIRST
    // probe of a freshly-loaded body can read a not-yet-populated tape and come
    // back as the 1e30 "empty" sentinel — the single-point gate above only
    // proves ONE point has landed, not the whole grid. Re-probe the full set
    // until it agrees with the CPU (or give up after a bounded number of tries;
    // the assertion below still reports the real worst-case if it never does).
    let gpu = (await probe(pts, 0)) as number[];
    for (let attempt = 0; attempt < 20; attempt++) {
      const worst = Math.max(...gpu.map((g, i) => Math.abs(g - cpu[i])));
      if (worst < 1e-2) break;
      await new Promise((r) => requestAnimationFrame(() => r(null)));
      gpu = (await probe(pts, 0)) as number[];
    }
    return { pts, gpu, cpu, aabb };
  });

  // The clip box gives the body a real AABB (±25 mm local).
  expect(result.aabb[0], "AABB min x").toBeLessThan(-20);
  expect(result.aabb[3], "AABB max x").toBeGreaterThan(20);
  // Outside the clip box the field must be positive.
  expect(result.cpu[5], "outside the box is outside").toBeGreaterThan(0);
  // The lattice has inside points somewhere on the grid.
  expect(
    result.cpu.some((d) => d < 0),
    "the lattice has interior",
  ).toBe(true);

  let worst = 0;
  let worstIdx = -1;
  for (let i = 0; i < result.pts.length; i++) {
    const diff = Math.abs(result.gpu[i] - result.cpu[i]);
    if (diff > worst) {
      worst = diff;
      worstIdx = i;
    }
  }
  logs.push(
    `custom-sdf parity over ${result.pts.length} points: worst |gpu-cpu| = ${worst} at ${JSON.stringify(
      result.pts[worstIdx],
    )} (gpu ${result.gpu[worstIdx]}, cpu ${result.cpu[worstIdx]})`,
  );
  console.log("=== CUSTOM-SDF PARITY ===\n" + logs.join("\n"));
  expect(worst, "GPU expression interpreter matches CPU mirror").toBeLessThan(1e-2);
});
