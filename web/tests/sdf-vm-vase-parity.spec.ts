import { expect, test } from "@playwright/test";

/**
 * SDF-VM GPU↔CPU parity on the canonical vase test case.
 *
 * The vase (free lines + interpolated spline → loop inference → revolve →
 * 2 mm shell opened on the top rim) is the body that exposed two GPU
 * interpreter bugs that no other test could see:
 *   - the 32-iteration loop cap on OP_PRIM2D_POLY (v0.6.84), and
 *   - the flattened profile's synthetic on-axis closing edge, which Shell
 *     inflated into a phantom 2 mm rod (fixed by OP_PRIM2D_PATH, v0.6.85).
 *
 * This test builds the vase through the store, then evaluates the LIVE TSL
 * interpreter (window.__fdSdfVmProbe, a compute pass over the same opcode
 * buffer the raymarcher reads) against the CPU mirror interpreter
 * (kernel.bodySdfEval). Both interpret identical bytes — any disagreement
 * is a GPU-side interpreter bug, no matter how it shades.
 *
 * Mirrors the native test `vase_sdf_vm_opcode_stream_keeps_shell_open`
 * (features.rs), which proves CPU-mirror == logical CSG SDF; together the
 * chain logical == CPU mirror == GPU is closed.
 */

const VASE_DOC = JSON.stringify({
  schema_version: 2,
  name: "Vase",
  feature_calls: [
    {
      type: "sketch",
      id: 1,
      params: {
        plane: "qCreatedBy(origin_plane_xy)",
        sub_calls: [
          { type: "sketch_point", id: "p1", params: { at: [0.0, -70.0] } },
          { type: "sketch_point", id: "p2", params: { at: [0.0, 68.0] } },
          { type: "sketch_point", id: "p3", params: { at: [45.0, 68.0] } },
          { type: "sketch_point", id: "p4", params: { at: [30.0, -62.0] } },
          { type: "sketch_point", id: "p5", params: { at: [26.0, -70.0] } },
          { type: "sketch_line", id: "l1", params: { start: [0.0, -70.0], end: [0.0, 68.0] } },
          { type: "sketch_line", id: "l2", params: { start: [0.0, 68.0], end: [45.0, 68.0] } },
          { type: "sketch_line", id: "l3", params: { start: [30.0, -62.0], end: [26.0, -70.0] } },
          { type: "sketch_line", id: "l4", params: { start: [26.0, -70.0], end: [0.0, -70.0] } },
          {
            type: "sketch_spline",
            id: "s1",
            params: {
              points: [45.0, 68.0, 33.044, 50.471, 26.707, 42.324, 38.476, 20.596, 50.245, -9.732, 44.36, -45.039, 30.0, -62.0],
              closed: false,
              mode: "interpolate",
            },
          },
        ],
      },
    },
    { type: "revolve", id: 2, params: { profiles: "qCreatedBy(1)" } },
  ],
  next_feature_id: 3,
});

type FaceLite = {
  idHash: string;
  surfaceKind: string;
  centroidLocal: [number, number, number];
  normalLocal: [number, number, number];
};

test("vase: GPU SDF-VM interpreter matches the CPU mirror (shell stays open)", async ({ page }) => {
  test.setTimeout(300_000); // weave (~15 s WASM) + WebGPU shader compile can be slow
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

  // 1) Load sketch + revolve.
  const loaded = await page.evaluate((doc) => {
    const project = JSON.stringify({
      schemaVersion: 1,
      name: "Vase",
      activeTabId: "tab_1",
      tabs: [{ id: "tab_1", kind: "part", name: "Part 1", documentJson: doc }],
    });
    return (window as any).__fdStore.loadProjectFromJson(project);
  }, VASE_DOC);
  expect(loaded, "project loads").toBe(true);
  await page.waitForTimeout(1000);

  // 2) Find the planar top rim face (normal +Y at y = 68) — pays one weave.
  const topFace = (await page.evaluate(() => {
    const faces = (window as any).__fdKernel.bodyBrepFaces(0) as FaceLite[];
    return faces.find(
      (f) => f.normalLocal[1] > 0.99 && Math.abs(f.centroidLocal[1] - 68) < 1e-2,
    );
  })) as FaceLite | undefined;
  expect(topFace, "weave exposes the planar top rim face").toBeTruthy();

  // 3) Shell (2 mm), opened on the top rim — same param-patch dance as
  //    shell-inner-wall.spec.ts.
  const shellOk = await page.evaluate((idHash) => {
    const store = (window as any).__fdStore;
    const kernel = (window as any).__fdKernel;
    const shellId = store.addShell(2);
    const params = kernel.getFeatureCallParams(shellId) as Record<string, unknown>;
    const queryKey =
      Object.keys(params).find(
        (key) => typeof params[key] === "string" && /face|open|query/i.test(key),
      ) ?? Object.keys(params).find((key) => typeof params[key] === "string");
    if (!queryKey) return false;
    return store.updateFeatureParams(shellId, { ...params, [queryKey]: `qFace(${idHash})` });
  }, topFace!.idHash);
  expect(shellOk, "shell created and opened on the rim").toBe(true);
  await page.waitForTimeout(1000);

  // 4) Wait for the SDF-VM pipeline (shader compile) to expose the GPU probe.
  await page.waitForFunction(() => typeof (window as any).__fdSdfVmProbe === "function", null, {
    timeout: 180_000,
  });

  // 5) Probe points: shape-critical spots + a coarse grid over the AABB.
  const result = await page.evaluate(async () => {
    const kernel = (window as any).__fdKernel;
    const probe = (window as any).__fdSdfVmProbe;
    const pts: [number, number, number][] = [
      [0, 0, 0],      // cavity centre — carved out
      [1, 0, 0],      // next to the axis — the phantom-rod regression spot
      [0, 67, 0],     // inside the top opening
      [20, 67, 0],
      [49, -10, 0],   // inside the belly wall
      [0, -69, 0],    // bottom stays closed
      [60, 0, 0],     // outside
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
    const gpu = (await probe(pts, 0)) as number[];
    const cpu = pts.map((p) => kernel.bodySdfEval(0, p[0], p[1], p[2]) as number);
    return { pts, gpu, cpu };
  });

  // Shape: the shell opening and walls survive into the stream (CPU mirror).
  const cpuAt = (i: number) => result.cpu[i];
  expect(cpuAt(0), "cavity carved out").toBeGreaterThan(0);
  expect(cpuAt(1), "no phantom axis rod").toBeGreaterThan(0);
  expect(cpuAt(2), "top rim open").toBeGreaterThan(0);
  expect(cpuAt(4), "belly wall solid").toBeLessThan(0);
  expect(cpuAt(5), "bottom closed").toBeLessThan(0);
  expect(cpuAt(6), "outside stays outside").toBeGreaterThan(0);

  // Parity: GPU == CPU everywhere (f32 noise budget only).
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
  console.log("=== SDF-VM PARITY ===\n" + logs.join("\n"));
  expect(worst, "GPU interpreter matches CPU mirror").toBeLessThan(1e-2);
});
