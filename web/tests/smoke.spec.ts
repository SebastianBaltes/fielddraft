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
