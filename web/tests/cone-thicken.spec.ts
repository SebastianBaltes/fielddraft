import { expect, test } from "@playwright/test";

/**
 * DL-76 live-browser proof: an apex-correct cone weaves into a watertight B-Rep
 * with pickable rim edges, and its conical wall thickens.
 *
 * Before DL-76 a frustum's analytic cone wall was off-apex, so the octree
 * weaver rejected both rim seams — the cone came out with faces but 0 edges,
 * not watertight, and the wall could not be selected or thickened. This drives
 * the real WebGPU kernel: add a cone, assert it is watertight with rim edges
 * and an analytic "cone" face, then thicken that wall through the store and
 * assert a new feature is committed and the solid grows.
 */

type FaceLite = {
  idHash: string;
  surfaceKind: string;
};

test("a cone weaves watertight with rims and its wall thickens (DL-76)", async ({
  page,
}) => {
  test.setTimeout(120_000); // headful WebGPU shader compile can be slow
  const logs: string[] = [];
  page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));

  await page.goto("/");
  await page.evaluate(() => {
    try {
      window.localStorage.removeItem("fielddraft.project.v1");
      window.localStorage.removeItem("fielddraft.parametricWeave");
    } catch {
      /* ignore */
    }
  });
  await page.reload();
  await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
  await page.waitForTimeout(900);

  await page.evaluate(() => {
    (window as any).__fdStore?.addPrimitive("cone");
  });
  await page.waitForTimeout(500);

  // DL-76: the cone must weave with rim edges and an analytic cone wall — the
  // whole point of the apex-convention fix (the octree weaver used to reject
  // both rim seams, leaving 0 edges). Watertightness itself is asserted by the
  // kernel unit test; here the live proof is "rims exist + wall is pickable".
  const before = await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    const edges = (k?.bodyBrepEdges(0) ?? []) as unknown[];
    const faces = (k?.bodyBrepFaces(0) ?? []) as FaceLite[];
    const wall = faces.find((f) => f.surfaceKind === "cone");
    const aabb = k?.sceneAabb();
    const features = (window as any).__fdStore?.state.features.length ?? 0;
    return {
      edgeCount: edges.length,
      wallHash: wall?.idHash ?? null,
      maxX: aabb?.max[0] ?? 0,
      features,
    };
  });
  expect(before.edgeCount, "cone has rim edges (DL-76)").toBeGreaterThan(0);
  expect(before.wallHash, "cone has an analytic conical wall face").not.toBeNull();

  // Thicken the conical wall (default outward 2 mm) through the store.
  await page.evaluate((hash: string) => {
    (window as any).__fdStore?.addThicken([`qFace(${hash})`]);
  }, before.wallHash!);
  await page.waitForTimeout(900);

  const after = await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    const aabb = k?.sceneAabb();
    const features = (window as any).__fdStore?.state.features.length ?? 0;
    return { maxX: aabb?.max[0] ?? 0, features };
  });

  expect(after.features, "thicken added one feature").toBe(before.features + 1);
  expect(
    after.maxX,
    `solid grew outward (before ${before.maxX}, after ${after.maxX})`,
  ).toBeGreaterThan(before.maxX + 0.5);

  console.log("=== CONE THICKEN PROBE ===\n" + logs.join("\n"));
});
