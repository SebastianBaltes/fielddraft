import { expect, test } from "@playwright/test";
import { pickSketchTool } from "./sketchHarness";

/**
 * Trimming a composite (rectangle / polygon). Trim isn't a kernel tool — it
 * rewrites the sub-call list (see `web/src/sketchTrim.ts`). A rectangle is a
 * single `sketch_rectangle` sub-call; cutting one of its edges decomposes
 * the whole thing into individual `sketch_line` sub-calls (the clicked edge
 * trimmed, the siblings carried over). The geometry is exhaustively
 * unit-tested; this E2E proves the interactive path and, crucially, that the
 * composite → lines rewrite survives `replaceSketchSubCallAt` (which rewrites
 * the downstream constraint refs around the rectangle's 8 entities + 4
 * constraints) and a real re-solve. Cold WebGPU first-paint is slow, so the
 * timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch trim — composites", () => {
  test("trimming one rectangle edge flattens it into three lines", async ({
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

    // Draw a rectangle centred on the origin via a drag gesture.
    await pickSketchTool(page, "sketch_rectangle");
    await page.mouse.move(cx - 120, cy - 80);
    await page.mouse.down();
    await page.mouse.move(cx, cy, { steps: 5 });
    await page.mouse.move(cx + 120, cy + 80, { steps: 10 });
    await page.mouse.up();
    await page.waitForTimeout(200);

    const counts = async () =>
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
        return {
          rect: calls.filter((c) => c.type === "sketch_rectangle").length,
          line: calls.filter((c) => c.type === "sketch_line").length,
        };
      });

    expect(await counts()).toEqual({ rect: 1, line: 0 });

    // Trim: pick the mode, click the middle of the top edge (uncrossed) — it
    // is removed and the rectangle decomposes into the three other edges.
    await page.getByTestId("sketch-tool-sketch_trim").click();
    await page.mouse.click(cx, cy - 80);
    await page.waitForTimeout(400);

    expect(await counts()).toEqual({ rect: 0, line: 3 });

    await page.getByTestId("sketch-detail-accept").click();
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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
    expect(committed!.filter((c) => c.type === "sketch_line").length).toBe(3);
    expect(committed!.filter((c) => c.type === "sketch_rectangle").length).toBe(
      0,
    );

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
