import { expect, test } from "@playwright/test";

/**
 * B-Rep UI roadmap (A→C) — live-browser proof for tool-free EDGE and CORNER
 * selection. The face path already has a headful WebGPU proof
 * (`shell-inner-wall.spec.ts`, DL-75 4c); edges and vertices previously had
 * only unit-level coverage (`ElementInfo.test.tsx`, `elementQuery.test.ts`).
 * This closes that asymmetry: a real box is woven in the live kernel, an edge
 * and a corner are selected the way a tool-free viewport click does
 * (`store.selectElement({kind, idHash})`), and the ElementInfo panel the user
 * actually reads is asserted for each.
 */

type EdgeLite = { a: number; b: number; points: number[]; idHash: string };
type VertexLite = {
  vertexIndex: number;
  idHash: string;
  kind: "vertex";
  positionLocal: [number, number, number];
  branches: number[];
};

test("B-Rep edge and corner are tool-free selectable in the live browser", async ({ page }) => {
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

  // 1) A box — 12 edges, 8 corners.
  await page.evaluate(() => {
    (window as any).__fdStore?.addPrimitive("box");
  });
  await page.waitForTimeout(500);

  // 2) The box's woven B-Rep edges and vertices, straight from the kernel.
  const { edges, vertices } = (await page.evaluate(() => {
    const k = (window as any).__fdKernel;
    return k
      ? { edges: k.bodyBrepEdges(0), vertices: k.bodyBrepVertices(0) }
      : { edges: [], vertices: [] };
  })) as { edges: EdgeLite[]; vertices: VertexLite[] };
  logs.push(`edges=${edges.length} vertices=${vertices.length}`);
  expect(edges.length, "box should weave 12 edges").toBeGreaterThanOrEqual(12);
  expect(vertices.length, "box should weave 8 corners").toBeGreaterThanOrEqual(8);

  // Each edge carries a stable id_hash (the addressable handle) and a
  // world-space polyline; each vertex a stable id_hash and a local position.
  const edge = edges[0];
  expect(edge.idHash, "edge has an id_hash handle").toBeTruthy();
  expect(edge.points.length, "edge has a sampled polyline").toBeGreaterThanOrEqual(6);
  const vertex = vertices[0];
  expect(vertex.idHash, "vertex has an id_hash handle").toBeTruthy();
  expect(vertex.kind).toBe("vertex");

  // 3) Select the EDGE exactly as a tool-free viewport click does.
  await page.evaluate(
    ({ idHash }) => {
      (window as any).__fdStore?.selectElement({ kind: "edge", idHash });
    },
    { idHash: edge.idHash },
  );
  await page.waitForTimeout(300);

  const edgeSel = await page.evaluate(() =>
    (window as any).__fdStore?.getState?.().selectedElement ??
    (window as any).__fdStore?.state?.selectedElement,
  );
  logs.push(`edge selectedElement: ${JSON.stringify(edgeSel)}`);
  expect(edgeSel?.kind).toBe("edge");
  expect(edgeSel?.idHash).toBe(edge.idHash);

  // The ElementInfo panel shows the edge: kind label "Kante", a length row,
  // and the canonical qEdge(<idHash>) query.
  await expect(page.getByTestId("element-info-kind")).toHaveText("Kante");
  await expect(page.getByText("Länge").first()).toBeVisible();
  await expect(page.getByTestId("element-info-query")).toHaveText(
    `qEdge(${edge.idHash})`,
  );

  // 4) Select the CORNER the same way.
  await page.evaluate(
    ({ idHash }) => {
      (window as any).__fdStore?.selectElement({ kind: "vertex", idHash });
    },
    { idHash: vertex.idHash },
  );
  await page.waitForTimeout(300);

  const vertSel = await page.evaluate(() =>
    (window as any).__fdStore?.getState?.().selectedElement ??
    (window as any).__fdStore?.state?.selectedElement,
  );
  logs.push(`vertex selectedElement: ${JSON.stringify(vertSel)}`);
  expect(vertSel?.kind).toBe("vertex");
  expect(vertSel?.idHash).toBe(vertex.idHash);

  // The panel switches to the corner: kind label "Ecke", a position row, and
  // the canonical qVertex(<idHash>) query. (Corners have no actions by design.)
  await expect(page.getByTestId("element-info-kind")).toHaveText("Ecke");
  await expect(page.getByText("Position").first()).toBeVisible();
  await expect(page.getByTestId("element-info-query")).toHaveText(
    `qVertex(${vertex.idHash})`,
  );

  console.log("=== BREP EDGE/VERTEX PROBE ===\n" + logs.join("\n"));
});
