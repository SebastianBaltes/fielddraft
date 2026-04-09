import { expect, test } from "@playwright/test";

test.describe("FieldDraft smoke", () => {
  test.beforeEach(async ({ page }) => {
    // Start each test from an empty document. We navigate first so the
    // origin exists, then clear localStorage exactly once. We deliberately
    // avoid `addInitScript` here because it would re-run on every
    // `page.reload()` and break the persistence test.
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

  test("loads the app shell, toolbar and outliner", async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });

    await page.goto("/");
    await expect(page.getByText("FieldDraft", { exact: true })).toBeVisible();
    await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
    await expect(page.getByTestId("fielddraft-toolbar")).toBeVisible();
    await expect(page.getByTestId("fielddraft-outliner")).toBeVisible();
    await page.waitForTimeout(800);

    // Default document is empty.
    await expect(page.getByTestId("feature-count")).toHaveText("0 features");

    const blocking = consoleErrors.filter(
      (msg) =>
        !msg.includes("WebGPU") &&
        !msg.toLowerCase().includes("webgl") &&
        !msg.includes("THREE.Clock"),
    );
    expect(blocking, `unexpected console errors:\n${blocking.join("\n")}`).toEqual([]);
  });

  test("M2: add sphere, switch op, add box, verify outliner & undo/redo", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(600);

    // Add a sphere with the default op (Union).
    await page.getByTestId("add-sphere").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    await expect(page.getByTestId("feature-row-1")).toBeVisible();
    await expect(page.getByTestId("feature-row-1")).toHaveAttribute(
      "data-feature-op",
      "union",
    );

    // Switch the next-add op to Difference and add a box.
    await page.getByTestId("op-1").click();
    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-op",
      "difference",
    );

    // Undo removes the box.
    await page.getByTestId("undo").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    await expect(page.getByTestId("feature-row-2")).toHaveCount(0);

    // Redo brings it back.
    await page.getByTestId("redo").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");
    await expect(page.getByTestId("feature-row-2")).toBeVisible();

    // Selecting the box and pressing Delete removes it again.
    await page.getByTestId("feature-row-2").click();
    await page.keyboard.press("Delete");
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
  });

  test("M2: per-row op cycle button advances union → difference → intersect", async ({
    page,
  }) => {
    await page.goto("/");
    await page.waitForTimeout(400);
    await page.getByTestId("add-sphere").click();

    const row = page.getByTestId("feature-row-1");
    await expect(row).toHaveAttribute("data-feature-op", "union");

    // Click the op button (the small "+" / "−" / "∩" inside the row).
    const opButton = row.locator(".fielddraft-op");
    await opButton.click();
    await expect(row).toHaveAttribute("data-feature-op", "difference");
    await opButton.click();
    await expect(row).toHaveAttribute("data-feature-op", "intersect");
    await opButton.click();
    await expect(row).toHaveAttribute("data-feature-op", "union");
  });

  test("M2: raymarcher renders the SDF result of two-primitive subtraction", async ({
    page,
  }) => {
    await page.goto("/");
    // Wait for the WASM kernel + WebGPU pipeline to be live.
    await page.waitForTimeout(1000);

    // Build a known scene: a base sphere, then a box subtracted from it.
    await page.getByTestId("add-sphere").click();
    await page.getByTestId("op-1").click(); // Difference
    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    // Give the raymarcher several frames to settle. Headless WebGPU on
    // swiftshader is slow but deterministic.
    await page.waitForTimeout(2500);

    // Screenshot the canvas via Playwright. We can't use a 2D-context
    // drawImage on a WebGPU-backed canvas — its drawing buffer is
    // invalidated immediately after present, so a CPU read returns an
    // empty image. Playwright's `screenshot` captures from the OS
    // compositor and works regardless of the canvas backend.
    const buf = await page.getByTestId("fielddraft-canvas").screenshot();

    // Decode the PNG inside the page (using a fresh blob → ImageBitmap
    // → 2D canvas) to read pixels. This is run in the page context
    // because Node has no built-in PNG decoder.
    const stats = await page.evaluate(async (b64: string) => {
      const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const blob = new Blob([bin], { type: "image/png" });
      const bitmap = await createImageBitmap(blob);
      const cv = document.createElement("canvas");
      cv.width = Math.min(bitmap.width, 256);
      cv.height = Math.min(bitmap.height, 256);
      const ctx = cv.getContext("2d")!;
      ctx.drawImage(bitmap, 0, 0, cv.width, cv.height);
      const data = ctx.getImageData(0, 0, cv.width, cv.height).data;

      let lightPixels = 0;
      let bluePixels = 0;
      let darkPixels = 0;
      let rSum = 0,
        gSum = 0,
        bSum = 0;
      const total = cv.width * cv.height;
      for (let i = 0; i < data.length; i += 4) {
        const r = data[i];
        const g = data[i + 1];
        const b = data[i + 2];
        rSum += r;
        gSum += g;
        bSum += b;
        if (r < 30 && g < 30 && b < 30) darkPixels++;
        // "Light" — anywhere in the upper-right of the value triangle.
        // The background, anti-aliased grid, and bright sky shading
        // all qualify. Anything below this is from the raymarched
        // primitive's lit geometry.
        if (r > 200 && g > 200 && b > 200) {
          lightPixels++;
          continue;
        }
        // Lit primitive material is blueish (base ~ rgb(140,166,217)
        // modulated by lambert + ambient).
        if (b > r && b > g && r < 200 && g < 200) {
          bluePixels++;
        }
      }
      return {
        total,
        lightPixels,
        bluePixels,
        darkPixels,
        avg: [
          Math.round(rSum / total),
          Math.round(gSum / total),
          Math.round(bSum / total),
        ],
      };
    }, buf.toString("base64"));


    // Loose thresholds — exact counts depend on viewport size and
    // camera framing. The point is "raymarcher actually drew SDF
    // pixels into the canvas, into a light-mode background".
    expect(stats.total).toBeGreaterThan(1000);
    // Avg luminance should be clearly in the upper half (light-mode UI)
    // and not just black emptiness.
    const avgLum = (stats.avg[0] + stats.avg[1] + stats.avg[2]) / 3;
    expect(avgLum, `viewport not light-mode: ${JSON.stringify(stats)}`).toBeGreaterThan(150);
    // Most of the canvas should be light (background + sky shading).
    expect(
      stats.lightPixels,
      `not enough light pixels: ${JSON.stringify(stats)}`,
    ).toBeGreaterThan(stats.total * 0.4);
    // And the raymarcher must have drawn at least *some* lit primitive
    // pixels — proves the SDF eval ran on the GPU and produced a hit.
    expect(
      stats.bluePixels,
      `no lit primitive pixels: ${JSON.stringify(stats)}`,
    ).toBeGreaterThan(0);
  });

  test("persists feature list across reloads via localStorage", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(400);

    await page.getByTestId("add-box").click();
    await page.getByTestId("op-1").click();
    await page.getByTestId("add-sphere").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    // Wait for the autosave debounce (500 ms) to flush.
    await page.waitForTimeout(800);

    await page.reload();
    await page.waitForTimeout(800);

    await expect(page.getByTestId("feature-count")).toHaveText("2 features");
  });
});
