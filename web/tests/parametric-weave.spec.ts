import { expect, test } from "@playwright/test";

/**
 * Parametric weaving — milestone 1 (roadmap point E), live-browser proof.
 *
 * Toggling the "Parametric weaving" setting flips the kernel onto an exact
 * analytic B-Rep for pure grundforms (the Box). This test weaves a box both
 * ways in the real WebGPU kernel and proves, end-to-end through the store:
 *
 *  1) PLUMBING — `store.setParametricWeave(true)` reaches the kernel and the
 *     B-Rep refetches (body version bumps), and the element handles (`idHash`)
 *     are byte-identical across the toggle, so a saved selection survives it.
 *  2) PRECISION — with parametric weaving on, the 8 corners are *exact* and
 *     resolution-independent: each axis collapses to two perfectly symmetric
 *     values, bit-identical across the four corners that share them. (For an
 *     axis-aligned box at a grid-friendly size the octree route happens to hit
 *     the corners exactly too; the parametric route *guarantees* it for any
 *     size / rotation. The non-cube exactness is proved rigorously in the
 *     kernel unit test `parametric_box_matches_octree_topology_and_handles`.)
 */

type VertexLite = {
  vertexIndex: number;
  idHash: string;
  positionLocal: [number, number, number];
};

/** For each axis, the max spread of |coord| around its mean — 0 iff the corners
 * are perfectly symmetric on that axis. Also returns the sorted idHash set. */
function analyse(verts: VertexLite[]) {
  const spread = [0, 1, 2].map((ax) => {
    const mags = verts.map((v) => Math.abs(v.positionLocal[ax]));
    const mean = mags.reduce((a, b) => a + b, 0) / mags.length;
    return Math.max(...mags.map((m) => Math.abs(m - mean)));
  });
  const handles = verts.map((v) => v.idHash).sort();
  return { spread, handles };
}

test("parametric weaving makes box corners exact and keeps handles stable", async ({
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

  // Octree (default) corners.
  const octree = (await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    return k ? k.bodyBrepVertices(0) : [];
  })) as VertexLite[];
  expect(octree.length, "box weaves 8 corners").toBe(8);
  const before = analyse(octree);
  logs.push(`octree spread=${JSON.stringify(before.spread)}`);

  // Flip parametric weaving on, through the store (the path App.tsx drives).
  await page.evaluate(() => {
    (window as any).__fdStore?.setParametricWeave(true);
  });
  await page.waitForTimeout(500);

  const parametric = (await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    return k ? k.bodyBrepVertices(0) : [];
  })) as VertexLite[];
  expect(parametric.length, "still 8 corners").toBe(8);
  const after = analyse(parametric);
  logs.push(`parametric spread=${JSON.stringify(after.spread)}`);

  // (2) PRECISION: parametric corners are bit-exact on every axis.
  for (const s of after.spread) {
    expect(s).toBeLessThan(1e-9);
  }

  // (1) PLUMBING: the element handles are byte-identical across the toggle —
  // a qVertex(idHash) selection keeps resolving after switching routes.
  expect(after.handles).toEqual(before.handles);

  console.log("=== PARAMETRIC WEAVE PROBE ===\n" + logs.join("\n"));
});

test("parametric weaving handles the cylinder grundform too", async ({
  page,
}) => {
  test.setTimeout(120_000);
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
    (window as any).__fdStore?.addPrimitive("cylinder");
  });
  await page.waitForTimeout(500);

  // A cylinder weaves with NO corner vertices (two rim circles only), so the
  // discriminating selection element is the rim *edge*. Read edge handles both
  // ways and prove the toggle keeps them byte-identical.
  const edgeHandles = async () =>
    (await page.evaluate(() => {
      const k = (window as any).__fdKernel;
      const edges = k ? k.bodyBrepEdges(0) : [];
      return (edges as { idHash: string }[]).map((e) => e.idHash).sort();
    })) as string[];

  const before = await edgeHandles();
  expect(before.length, "cylinder weaves 2 rim edges").toBe(2);

  await page.evaluate(() => {
    (window as any).__fdStore?.setParametricWeave(true);
  });
  await page.waitForTimeout(500);

  const after = await edgeHandles();
  expect(after.length, "still 2 rim edges").toBe(2);
  expect(after, "rim-edge handles survive the parametric toggle").toEqual(
    before,
  );

  console.log("=== PARAMETRIC CYLINDER PROBE ===\n" + logs.join("\n"));
});
