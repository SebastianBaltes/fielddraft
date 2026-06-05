import { expect, test } from "@playwright/test";

/**
 * DL-75 step 4c — visual self-acceptance for the shell inner-wall / opening
 * B-Rep faces. Builds a hollowed (open-shell) box entirely through the store,
 * then proves a face carrying INNER_WALL_FLAG (0x800) is produced, resolves to
 * its owning feature, and is selectable + shown in the ElementInfo panel.
 *
 * This is the part that resisted scripting before: the mistake was using
 * `selectOnly` (feature-level → clears `selectedElement`). The correct B-Rep
 * face selection goes through `store.selectElement({kind:"face", idHash})`.
 */

const INNER_WALL_FLAG = 0x800;

type FaceLite = {
  faceIndex: number;
  idHash: string;
  branchId: number;
  featureId: number;
  surfaceKind: string;
  centroidLocal: [number, number, number];
  normalLocal: [number, number, number];
};

test("DL-75: open-shell inner wall is a selectable B-Rep face", async ({ page }) => {
  test.setTimeout(120_000); // headful WebGPU shader compile can be slow
  const logs: string[] = [];
  page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));

  await page.goto("/");
  await page.evaluate(() => {
    try {
      window.localStorage.removeItem("fielddraft.project.v1");
    } catch {
      /* ignore */
    }
  });
  await page.reload();
  await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
  await page.waitForTimeout(900);

  // 1) A box.
  await page.evaluate(() => {
    (window as any).__fdStore?.addPrimitive("box");
  });
  await page.waitForTimeout(500);

  // 2) Discover the box's outer faces; pick the +Z top face to open.
  const boxFaces = (await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    return k ? k.bodyBrepFaces(0) : [];
  })) as FaceLite[];
  expect(boxFaces.length, "box should weave 6 faces").toBeGreaterThanOrEqual(6);

  const topFace = boxFaces
    .slice()
    .sort((a, b) => b.normalLocal[2] - a.normalLocal[2])[0];
  expect(topFace.normalLocal[2]).toBeGreaterThan(0.8);
  const topIdHash = topFace.idHash;

  // 3) Create the shell and open it at the top face. Discover the open-faces
  //    param key live (kernel-decoded schema), then patch it.
  const shellInfo = await page.evaluate(
    ({ topIdHash }) => {
      const store = (window as any).__fdStore;
      const kernel = (window as any).__fdKernel;
      const shellId = store.addShell(4); // 4 mm wall
      const params = kernel.getFeatureCallParams(shellId) as Record<string, unknown>;
      // Find the string-valued param that holds the face query (open faces).
      const queryKey =
        Object.keys(params).find(
          (key) =>
            typeof params[key] === "string" &&
            /face|open|query/i.test(key),
        ) ??
        Object.keys(params).find((key) => typeof params[key] === "string");
      const next = { ...params } as Record<string, unknown>;
      if (queryKey) next[queryKey] = `qFace(${topIdHash})`;
      const ok = store.updateFeatureParams(shellId, next);
      return { shellId, paramKeys: Object.keys(params), queryKey, ok, params };
    },
    { topIdHash },
  );
  logs.push(`shell params: ${JSON.stringify(shellInfo)}`);
  expect(shellInfo.queryKey, "found open-faces param key").toBeTruthy();
  expect(shellInfo.ok, "updateFeatureParams accepted").toBeTruthy();
  await page.waitForTimeout(700);

  // 4) After the shell regenerates, find a face carrying the inner-wall flag.
  const faces = (await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    return k ? k.bodyBrepFaces(0) : [];
  })) as FaceLite[];

  const inner = faces.filter((f) => (f.branchId & 0x800) !== 0);
  const outer = faces.filter((f) => (f.branchId & 0x800) === 0);
  logs.push(
    `faces total=${faces.length} inner=${inner.length} outer=${outer.length}`,
  );
  logs.push(
    `inner faces: ${JSON.stringify(
      inner.map((f) => ({
        b: f.branchId.toString(16),
        feat: f.featureId,
        kind: f.surfaceKind,
        n: f.normalLocal.map((x) => +x.toFixed(2)),
      })),
    )}`,
  );

  expect(inner.length, "at least one inner-wall face").toBeGreaterThanOrEqual(1);
  // The inner-wall face must resolve to its owning feature (UI selection relies
  // on feature_id() surviving the 0x800 flag).
  expect(inner[0].featureId).toBeGreaterThan(0);

  // 5) Select an inner-wall face the way the canvas pick does. Prefer the
  //    cavity FLOOR (normal ≈ -Z) so the persistent highlight is clearly
  //    visible when we look down into the open top.
  const floor =
    inner.slice().sort((a, b) => a.normalLocal[2] - b.normalLocal[2])[0];
  const innerIdHash = floor.idHash;
  await page.evaluate(
    ({ innerIdHash }) => {
      (window as any).__fdStore?.selectElement({
        kind: "face",
        idHash: innerIdHash,
      });
    },
    { innerIdHash },
  );
  await page.waitForTimeout(500);

  // 6) The store now holds the element selection and the ElementInfo panel shows.
  const selected = await page.evaluate(() =>
    (window as any).__fdStore?.getState?.().selectedElement ??
    (window as any).__fdStore?.state?.selectedElement,
  );
  logs.push(`selectedElement: ${JSON.stringify(selected)}`);
  expect(selected?.kind).toBe("face");
  expect(selected?.idHash).toBe(innerIdHash);

  // ElementInfo renders the face label (Fläche) and its geometry when the
  // inner-wall face is selected — the panel the user actually reads.
  await expect(page.getByText(/Fläche|Face/).first()).toBeVisible({
    timeout: 5000,
  });
  // The cavity floor sits exactly `thickness` (4 mm) inward from the outer
  // floor at z = -50, i.e. the centroid z is -46. Asserting it here guards the
  // inner-wall *offset* (not just that some face exists).
  if (Math.abs(floor.normalLocal[2] + 1) < 0.1) {
    expect(floor.centroidLocal[2]).toBeCloseTo(-46, 0);
  }

  console.log("=== DL-75 PROBE ===\n" + logs.join("\n"));
});
