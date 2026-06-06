import { expect, test } from "@playwright/test";

/**
 * Face extrude (P1), live-browser proof.
 *
 * Picks the top (+Z) face of a box in the real WebGPU kernel and extrudes it
 * through the store — the same path the ElementInfo "Fläche extrudieren" action
 * drives. Proves end to end that a planar B-Rep face becomes an extrude profile:
 * a new feature is created and the solid grows by the blind depth along the
 * face normal.
 */

type FaceLite = {
  idHash: string;
  surfaceKind: string;
  normalLocal: [number, number, number];
};

test("extruding a box's top face grows the solid along its normal", async ({
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
    (window as any).__fdStore?.addPrimitive("box");
  });
  await page.waitForTimeout(500);

  // Learn the +Z face handle and the pre-extrude scene height.
  const before = await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    const faces = (k?.bodyBrepFaces(0) ?? []) as FaceLite[];
    const top = faces.find((f) => f.normalLocal[2] > 0.9);
    const aabb = k?.sceneAabb();
    const features = (window as any).__fdStore?.state.features.length ?? 0;
    return { idHash: top?.idHash ?? null, maxZ: aabb?.max[2] ?? 0, features };
  });
  expect(before.idHash, "box has a +Z planar face").not.toBeNull();

  // Extrude it (default blind 10 mm outward) through the store.
  await page.evaluate((hash: string) => {
    (window as any).__fdStore?.addExtrudeFaces([`qFace(${hash})`]);
  }, before.idHash!);
  await page.waitForTimeout(900);

  const after = await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    const aabb = k?.sceneAabb();
    const features = (window as any).__fdStore?.state.features.length ?? 0;
    return { maxZ: aabb?.max[2] ?? 0, features };
  });

  // A new feature was committed, and the solid grew along +Z by ~the depth.
  expect(after.features, "extrude added one feature").toBe(before.features + 1);
  expect(
    after.maxZ,
    `solid grew along +Z (before ${before.maxZ}, after ${after.maxZ})`,
  ).toBeGreaterThan(before.maxZ + 5);

  console.log("=== FACE EXTRUDE PROBE ===\n" + logs.join("\n"));
});
