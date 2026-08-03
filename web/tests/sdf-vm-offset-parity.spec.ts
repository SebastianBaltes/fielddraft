import { expect, test } from "@playwright/test";

/**
 * SDF-VM GPU↔CPU parity on an ERODED BOOLEAN — the offset-channel case.
 *
 * #384 P5b-1A widened the OP_ARC_CHANNEL header from 41 to 44 words
 * (mode + A_seam + A_h) and changed the stop-limit chain to
 * `δ − min(de + A_seam, (s4+δ) + A_h)`. The TSL interpreter mirrors that
 * change 1:1, but no vitest executes it — this spec is the runtime proof
 * that the GPU reader consumes the new header in lockstep with the CPU
 * mirror. A desynced reader would misparse every arc after the header, so
 * parity over points inside the channel influence zone is a sharp probe.
 *
 * Fixture: two overlapping boxes, explicit Union, then a −2 mm offset
 * (erosion). The concave seam ring where the tower meets the slab is
 * exactly the geometry that synthesizes straight-line offset channels —
 * the class that P5b certifies (mode Proved, nonzero addends).
 */

const ERODED_UNION_DOC = JSON.stringify({
  schema_version: 2,
  name: "ErodedUnion",
  feature_calls: [
    {
      type: "box",
      id: 1,
      params: { half_extents: [20.0, 20.0, 10.0], translate: [0.0, 0.0, 0.0], op: "new" },
    },
    {
      type: "box",
      id: 2,
      params: { half_extents: [10.0, 10.0, 20.0], translate: [0.0, 0.0, 15.0], op: "new" },
    },
    {
      type: "boolean",
      id: 3,
      // Union convention (see lib_tests.rs): ALL participants are tools,
      // targets stays empty — the first tool seeds the fused body.
      params: {
        operation: "Union",
        tools: "qCreatedBy(1),qCreatedBy(2)",
        targets: "",
        keep_tools: false,
      },
    },
    {
      type: "offset",
      id: 4,
      params: { targets: "qCreatedBy(1)", delta: { value: -2.0, unit: "mm" } },
    },
  ],
  next_feature_id: 5,
});

test("eroded union: GPU SDF-VM interpreter matches the CPU mirror (44-word channel header)", async ({
  page,
}) => {
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
      name: "ErodedUnion",
      activeTabId: "tab_1",
      tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson: doc }],
    });
    return (window as any).__fdStore.loadProjectFromJson(project);
  }, ERODED_UNION_DOC);
  expect(loaded, "project loads").toBe(true);
  await page.waitForTimeout(1500);

  // The fixture must actually synthesize offset channels — otherwise this
  // spec silently stops testing the 44-word header.
  // `offsetChannelDiagnostics` is not delegated by the JS wrapper — reach
  // through to the raw WasmKernel (TS-private `inner` is a plain runtime
  // property).
  const diag = await page.evaluate(() =>
    JSON.parse((window as any).__fdKernel.inner.offsetChannelDiagnostics()),
  );
  logs.push(`channel diagnostics: ${JSON.stringify(diag)}`);
  const nChannels = (diag as { channels: number }[]).reduce((a, d) => a + d.channels, 0);
  expect(nChannels, "eroded union synthesizes offset channels").toBeGreaterThan(0);

  await page.waitForFunction(() => typeof (window as any).__fdSdfVmProbe === "function", null, {
    timeout: 180_000,
  });

  const result = await page.evaluate(async () => {
    const kernel = (window as any).__fdKernel;
    const probe = (window as any).__fdSdfVmProbe;
    // Seam-focused points: the concave ring sits where the tower
    // (|x|,|y| ≤ 10) meets the slab top (z = 10); erosion pulls both
    // walls in by 2. Sample the seam corridor densely on all four sides,
    // inside, outside, and across the limit's trust radius.
    const pts: [number, number, number][] = [
      [0, 0, 0], // slab core — stays solid after −2 mm
      [9, 0, 9], // inside the pre-erosion tower wall, outside afterwards
    ];
    for (const s of [-1, 1]) {
      for (let t = -12; t <= 12; t += 3) {
        pts.push([s * 9, t, 9]); // just inside the eroded tower wall, at seam height
        pts.push([s * 11, t, 11]); // outside both eroded surfaces, in the channel notch
        pts.push([s * 7, t, 7]); // deeper inside — limit register still active
        pts.push([t, s * 9, 9]);
        pts.push([t, s * 11, 11]);
      }
    }
    const aabb = kernel.bodySdfAabb(0) as number[];
    const n = 6;
    for (let iz = 0; iz <= n; iz++)
      for (let iy = 0; iy <= n; iy++)
        for (let ix = 0; ix <= n; ix++)
          pts.push([
            aabb[0] + ((aabb[3] - aabb[0]) * ix) / n,
            aabb[1] + ((aabb[4] - aabb[1]) * iy) / n,
            aabb[2] + ((aabb[5] - aabb[2]) * iz) / n,
          ]);
    const gpu = (await probe(pts, 0)) as number[];
    const cpu = pts.map((p) => kernel.bodySdfEval(0, p[0], p[1], p[2]) as number);
    return { pts, gpu, cpu };
  });

  // Shape sanity on the CPU mirror: erosion actually happened.
  const cpuOf = (p: [number, number, number]) =>
    result.cpu[result.pts.findIndex((q) => q[0] === p[0] && q[1] === p[1] && q[2] === p[2])];
  expect(cpuOf([0, 0, 0]), "slab core stays solid").toBeLessThan(0);
  expect(cpuOf([9, 0, 9]), "point inside the eroded tower wall is no longer solid").toBeGreaterThan(
    0,
  );

  // Parity: GPU == CPU everywhere (f32 noise budget only). A header
  // desync shows up here as gross disagreement, not noise.
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
    `parity over ${result.pts.length} points: worst |gpu-cpu| = ${worst} at ${JSON.stringify(
      result.pts[worstIdx],
    )} (gpu ${result.gpu[worstIdx]}, cpu ${result.cpu[worstIdx]})`,
  );
  console.log("=== SDF-VM OFFSET PARITY ===\n" + logs.join("\n"));
  expect(worst, "GPU interpreter matches CPU mirror").toBeLessThan(1e-2);
});
