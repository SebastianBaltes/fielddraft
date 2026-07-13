import { expect, test } from "@playwright/test";

/**
 * Interactive trim (UI gap #2 follow-up). Trim isn't a kernel tool — it's
 * an editing mode that cuts the curve under the cursor at its nearest
 * intersections and drops the picked piece (see `web/src/sketchTrim.ts`).
 * The trim geometry itself is exhaustively unit-tested; this E2E proves the
 * interactive path: a click on the middle of a line that crosses two other
 * lines splits it into two, so the committed sketch holds one more line than
 * was drawn. Cold WebGPU first-paint is slow, so the timeout is generous.
 */
// Heavier than the sibling specs — it draws three lines before the cut, on
// top of the one-off cold WebGPU first-paint, so it needs more headroom.
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch trim", () => {
  test("clicking a crossed segment splits the line in two", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();

    const overlay = page.getByTestId("sketch-canvas-2d");
    const box = await overlay.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    const drawLine = async (
      x1: number,
      y1: number,
      x2: number,
      y2: number,
    ) => {
      await page.getByTestId("sketch-tool-sketch_line").click();
      await page.mouse.move(x1, y1);
      await page.mouse.down();
      await page.mouse.move((x1 + x2) / 2, (y1 + y2) / 2, { steps: 4 });
      await page.mouse.move(x2, y2, { steps: 8 });
      await page.mouse.up();
      await page.waitForTimeout(150);
    };

    // A long horizontal line crossed by two verticals at ±60 px.
    await drawLine(cx - 120, cy, cx + 120, cy);
    await drawLine(cx - 60, cy - 80, cx - 60, cy + 80);
    await drawLine(cx + 60, cy - 80, cx + 60, cy + 80);

    const lineCount = async () =>
      page.evaluate(() => {
        const store = (
          window as unknown as {
            __fdStore?: {
              state: {
                sketchSession: { subCalls: Array<{ type: string }> };
              };
            };
          }
        ).__fdStore;
        const calls = store?.state.sketchSession.subCalls ?? [];
        return calls.filter((c) => c.type === "sketch_line").length;
      });

    expect(await lineCount()).toBe(3);

    // Trim: pick the mode, click the middle of the horizontal line (between
    // the two crossings) — that segment is removed, splitting it into two.
    await page.getByTestId("sketch-tool-sketch_trim").click();
    await page.mouse.click(cx, cy);
    await page.waitForTimeout(400);

    expect(await lineCount()).toBe(4);

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeVisible();

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{ type: string }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    const lines = committed!.filter((c) => c.type === "sketch_line").length;
    expect(lines).toBe(4);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
