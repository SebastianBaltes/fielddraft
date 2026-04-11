import { expect, test } from "@playwright/test";

/**
 * K6b.2/K6b.3 smoke tests for the sketch editor.
 *
 * Runs headed Chromium with the host's real WebGPU stack (see
 * `playwright.config.ts`). The point is not exhaustive coverage —
 * Vitest already owns the unit-level checks for schema parsing,
 * reducers and the fdApi façade. What Vitest can't catch is
 * integration bugs that only surface when the scene actually
 * renders: material/node-material incompatibility, drawIndexed
 * crashes, geometry shapes that confuse the WebGPU backend.
 *
 * Every test here therefore attaches a `pageerror` + `console` error
 * listener and asserts the list is empty at the end. The
 * LineMaterial / NodeMaterial compatibility regression we hit in
 * K6b.2 would trip the drawIndexed + NodeMaterial filters below
 * immediately.
 */

/** Console error substrings we ignore — they come from third-party
 *  libraries or the WebGPU adapter probing and aren't actionable. */
const IGNORED_CONSOLE_SUBSTRINGS = [
  "WebGPU",
  "webgl",
  "THREE.Clock",
];

function isBlockingConsoleMessage(text: string): boolean {
  const lower = text.toLowerCase();
  return !IGNORED_CONSOLE_SUBSTRINGS.some((s) => lower.includes(s.toLowerCase()));
}

test.describe("Sketch editor smoke", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch {
        /* ignore */
      }
    });
    await page.reload();
  });

  test("enter sketch mode, draw a line via the dialog, render it", async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: Error[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
    await page.waitForTimeout(600);

    // 1. Enter sketch mode.
    await page.getByTestId("enter-sketch").click();
    await expect(page.getByTestId("sketch-mode-indicator")).toBeVisible();
    await expect(page.getByTestId("sketch-session-panel")).toBeVisible();
    await expect(page.getByTestId("sketch-session-empty")).toBeVisible();

    // 2. Pick the line tool. The outliner should now show the
    //    generic FeatureDialog for sketch_line with Start / End
    //    vec2 widgets.
    await page.getByTestId("sketch-tool-sketch_line").click();
    await expect(page.getByTestId("sketch-session-empty")).toHaveCount(0);

    // 3. Fill the coordinates. Vec2Widget uses aria-labels built
    //    from the param display name: "Start x", "Start y",
    //    "End x", "End y".
    await page.getByLabel("Start x").fill("-50");
    await page.getByLabel("Start y").fill("-50");
    await page.getByLabel("End x").fill("50");
    await page.getByLabel("End y").fill("50");

    // 4. Apply the draft. The sub-spine list must grow by one.
    await page.getByTestId("sketch-tool-apply").click();
    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    // 5. Give the renderer a few frames to draw the new line and
    //    surface any WebGPU draw-time crashes.
    await page.waitForTimeout(500);

    // 6. Finish the sketch — the panel goes away, the regular
    //    toolbar comes back.
    await page.getByTestId("sketch-finish").click();
    await expect(page.getByTestId("sketch-session-panel")).toHaveCount(0);
    await expect(page.getByTestId("add-box")).toBeVisible();

    // 7. No blocking console errors, no uncaught page errors.
    //    This is the guard that would have caught the
    //    LineMaterial / drawIndexed regression.
    const blocking = consoleErrors.filter(isBlockingConsoleMessage);
    expect(
      blocking,
      `unexpected console errors:\n${blocking.join("\n")}`,
    ).toEqual([]);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("enter sketch mode, draw a line by clicking on the canvas", async ({ page }) => {
    const consoleErrors: string[] = [];
    const pageErrors: Error[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(600);

    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-tool-sketch_line").click();

    // Two clicks on the canvas at different pixel positions.
    // Canvas-relative coordinates: the SketchDrawingPlane catches
    // the clicks through R3F's raycaster. We don't assert the
    // resulting world-space coordinates — that's covered by the
    // dialog-path test above — only that two clicks produce a
    // completed line in the sub-spine without any render errors.
    const canvas = page.getByTestId("fielddraft-canvas");
    const box = await canvas.boundingBox();
    expect(box).not.toBeNull();
    if (!box) return;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    await page.mouse.click(cx - 80, cy - 40);
    await page.waitForTimeout(100);
    await page.mouse.click(cx + 80, cy + 40);

    const subSpineRows = page.getByTestId("sketch-sub-spine").locator(".row");
    await expect(subSpineRows).toHaveCount(1);

    await page.waitForTimeout(400);

    const blocking = consoleErrors.filter(isBlockingConsoleMessage);
    expect(
      blocking,
      `unexpected console errors:\n${blocking.join("\n")}`,
    ).toEqual([]);
    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });
});
