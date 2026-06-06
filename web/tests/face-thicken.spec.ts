import { expect, test } from "@playwright/test";

/**
 * Face thicken (curved P1), live-browser proof.
 *
 * Picks the spherical face of a sphere in the real WebGPU kernel and thickens
 * it through the store — the same path the ElementInfo "Fläche verdicken"
 * action drives. Proves end to end that a curved analytic face becomes an
 * offset-solid wall: a new feature is created and the solid grows outward by
 * the wall thickness (the sphere's radius gains ~thickness on every axis).
 */

type FaceLite = {
  idHash: string;
  surfaceKind: string;
};

test("thickening a sphere's face grows the solid outward by the wall", async ({
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
    (window as any).__fdStore?.addPrimitive("sphere");
  });
  await page.waitForTimeout(500);

  // Learn the spherical face handle and the pre-thicken scene extent.
  const before = await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    const faces = (k?.bodyBrepFaces(0) ?? []) as FaceLite[];
    const sphere = faces.find((f) => f.surfaceKind === "sphere");
    const aabb = k?.sceneAabb();
    const features = (window as any).__fdStore?.state.features.length ?? 0;
    return { idHash: sphere?.idHash ?? null, maxX: aabb?.max[0] ?? 0, features };
  });
  expect(before.idHash, "sphere has a spherical face").not.toBeNull();

  // Thicken it (default outward 2 mm) through the store.
  await page.evaluate((hash: string) => {
    (window as any).__fdStore?.addThicken([`qFace(${hash})`]);
  }, before.idHash!);
  await page.waitForTimeout(900);

  const after = await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    const aabb = k?.sceneAabb();
    const features = (window as any).__fdStore?.state.features.length ?? 0;
    return { maxX: aabb?.max[0] ?? 0, features };
  });

  // A new feature was committed, and the solid grew outward by ~the thickness.
  expect(after.features, "thicken added one feature").toBe(before.features + 1);
  expect(
    after.maxX,
    `solid grew outward (before ${before.maxX}, after ${after.maxX})`,
  ).toBeGreaterThan(before.maxX + 1);

  console.log("=== FACE THICKEN PROBE ===\n" + logs.join("\n"));
});
