import { expect, test } from "@playwright/test";

/**
 * Profile-operation tools (UI gap #2): mirror and offset. Both act on
 * the whole accumulated profile rather than drawing a new entity, so the
 * flow is: draw a region, pick the op tool, (optionally tweak its param),
 * then click the canvas to apply. We assert the committed sketch holds the
 * region plus the op sub-calls with their parameters. The geometric truth
 * of mirror/offset is covered by the kernel unit tests
 * (`build_sketch2d_*` in `sketch_features.rs`); here we prove the UI path
 * wires through to the committed feature. Cold WebGPU first-paint is slow,
 * so the per-test timeout is generous.
 */
test.describe.configure({ timeout: 60_000 });

test.describe("Sketch profile-operation tools", () => {
  test("mirror + offset commit profile-operation sub-calls", async ({
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

    // 1) Draw a rectangle in the +x quadrant via a drag gesture.
    await page.getByTestId("sketch-tool-sketch_rectangle").click();
    const rStartX = cx + 60;
    const rStartY = cy + 20;
    const rEndX = cx + 140;
    const rEndY = cy - 60;
    await page.mouse.move(rStartX, rStartY);
    await page.mouse.down();
    await page.mouse.move((rStartX + rEndX) / 2, (rStartY + rEndY) / 2, {
      steps: 5,
    });
    await page.mouse.move(rEndX, rEndY, { steps: 10 });
    await page.mouse.up();

    // 2) Mirror: pick the tool, apply with a canvas click (default axis).
    await page.getByTestId("sketch-tool-sketch_mirror").click();
    await page.mouse.click(cx, cy);

    // 3) Offset: pick the tool, set a non-zero distance through the store
    //    test hook, then apply with a canvas click.
    await page.getByTestId("sketch-tool-sketch_offset").click();
    await page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            setSketchToolDraft: (d: Record<string, unknown>) => void;
          };
        }
      ).__fdStore;
      store?.setSketchToolDraft({ distance: 4 });
    });
    await page.mouse.click(cx, cy);

    // Three committed sub-calls: rectangle, mirror, offset.
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(3);

    await page.waitForTimeout(500);
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    const committed = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            getSketchSubCalls: (id: number) => Array<{
              type: string;
              params: Record<string, unknown>;
            }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      const sketch = kernel.listFeatures().find((f) => f.kind === "sketch");
      if (!sketch) return null;
      return kernel.getSketchSubCalls(sketch.id);
    });

    expect(committed).not.toBeNull();
    const types = committed!.map((c) => c.type);
    expect(types).toEqual(["sketch_rectangle", "sketch_mirror", "sketch_offset"]);

    const offset = committed!.find((c) => c.type === "sketch_offset")!;
    expect(Number(offset.params.distance)).toBeCloseTo(4, 3);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
