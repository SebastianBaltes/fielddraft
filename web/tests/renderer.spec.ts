import { expect, test } from "@playwright/test";

const RENDER_MODES = ["sdf-vm", "sdf-deferred"] as const;

/**
 * Wait for the deferred renderer to finish compiling its shader.
 * The "Compiling shader…" overlay disappears when the pipeline is ready.
 */
async function waitForShaderCompile(page: import("@playwright/test").Page) {
  await page.waitForFunction(
    () => {
      const el = document.getElementById("fd-shader-compiling");
      return !el || el.style.display === "none";
    },
    { timeout: 25_000 },
  );
}

/**
 * Open the render-mode dropdown. The menu button only opens a MENU; the render
 * mode lives one level deeper, in the Settings DIALOG behind its "Settings…"
 * item. Reaching straight for the select after clicking the trigger finds
 * nothing and waits for it until the test times out.
 */
async function openRenderModeSelect(page: import("@playwright/test").Page) {
  await page.getByTestId("settings-menu-trigger").click();
  await expect(page.getByTestId("settings-menu-popover")).toBeVisible();
  await page.getByTestId("menu-settings").click();
  const select = page.getByTestId("setting-render-mode-select");
  await expect(select).toBeVisible();
  return select;
}

test.describe("SDF Renderer: mode switching & rendering", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => {
      try {
        window.localStorage.removeItem("fielddraft.project.v1");
      } catch { /* ignore */ }
    });
    await page.reload();
  });

  for (const mode of RENDER_MODES) {
    test(`${mode}: renders bodies without GPU errors`, async ({ page }) => {
      const gpuErrors: string[] = [];
      page.on("pageerror", (err) => gpuErrors.push(err.message));
      page.on("console", (msg) => {
        if (msg.type() === "error") {
          const t = msg.text();
          if (t.includes("GPU error")) gpuErrors.push(t);
        }
      });

      await page.addInitScript((m: string) => {
        localStorage.setItem("fielddraft.renderMode", m);
      }, mode);
      await page.goto("/");
      await page.waitForTimeout(500);

      // Add a sphere + box with difference
      await page.evaluate(() => {
        const store = (window as unknown as {
          __fdStore?: {
            addPrimitive: (k: string) => void;
            setNextOp: (op: number) => void;
          };
        }).__fdStore;
        if (!store) throw new Error("no store");
        store.addPrimitive("sphere");
        store.setNextOp(1);
        store.addPrimitive("box");
      });

      await waitForShaderCompile(page);
      await page.waitForTimeout(1000);

      // Verify the HUD reports the correct mode. The toggle is F9 — F2 opens
      // the Report Issue dialog, so pressing it here left the HUD closed and
      // the read below waited for it until the test timed out (the `.catch()`
      // never fires: a locator read has no timeout of its own).
      await page.evaluate(() => {
        window.dispatchEvent(
          new KeyboardEvent("keydown", { key: "F9", code: "F9", keyCode: 120, bubbles: true }),
        );
      });
      const hud = page.locator("[data-testid='perf-hud']");
      await expect(hud).toBeVisible();
      expect(await hud.textContent()).toContain(mode);

      // Verify bodies are present
      const bodyCount = await page.evaluate(() => {
        const store = (window as unknown as {
          __fdStore?: { kernel: { bodyCount: () => number } };
        }).__fdStore;
        return store?.kernel.bodyCount() ?? 0;
      });
      expect(bodyCount).toBeGreaterThanOrEqual(1);

      // No GPU errors during rendering
      const blocking = gpuErrors.filter(
        (e) => !e.includes("THREE.TSL") && !e.includes("storageTexture") &&
          !e.includes("WebGPU") && !e.includes("webgl") && !e.includes("THREE.Clock"),
      );
      expect(blocking).toEqual([]);
    });
  }

  test("settings offers every render mode", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(500);

    const select = await openRenderModeSelect(page);
    const options = await select.locator("option").allTextContents();
    expect(options).toEqual([
      "SDF VM",
      "SDF Deferred",
      "Mesh",
      "B-Rep",
      "WebGL",
    ]);
  });

  test("render mode persists to localStorage", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("fielddraft.renderMode", "sdf-vm");
    });
    await page.goto("/");
    await page.waitForTimeout(500);

    // Open settings and verify sdf-vm is selected
    const select = await openRenderModeSelect(page);
    expect(await select.inputValue()).toBe("sdf-vm");

    // Switch to sdf-deferred
    await select.selectOption("sdf-deferred");
    await page.waitForTimeout(200);

    // Verify localStorage updated
    const stored = await page.evaluate(() =>
      localStorage.getItem("fielddraft.renderMode"),
    );
    expect(stored).toBe("sdf-deferred");
  });

  test("switching renderer live preserves scene", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("fielddraft.renderMode", "sdf-deferred");
    });
    await page.goto("/");
    await page.waitForTimeout(500);

    // Add a sphere
    await page.evaluate(() => {
      const store = (window as unknown as {
        __fdStore?: { addPrimitive: (k: string) => void };
      }).__fdStore;
      store?.addPrimitive("sphere");
    });
    await waitForShaderCompile(page);
    await page.waitForTimeout(500);

    const countBefore = await page.evaluate(() => {
      return (window as unknown as {
        __fdStore?: { kernel: { bodyCount: () => number } };
      }).__fdStore?.kernel.bodyCount() ?? 0;
    });
    expect(countBefore).toBe(1);

    // Switch to sdf-vm
    const select = await openRenderModeSelect(page);
    await select.selectOption("sdf-vm");
    await page.waitForTimeout(2000);

    // Body count must be preserved
    const countAfter = await page.evaluate(() => {
      return (window as unknown as {
        __fdStore?: { kernel: { bodyCount: () => number } };
      }).__fdStore?.kernel.bodyCount() ?? 0;
    });
    expect(countAfter).toBe(countBefore);
  });
});

test.describe("SDF Renderer: complex scene", () => {
  test("deferred handles mixed primitive types", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("fielddraft.renderMode", "sdf-deferred");
      localStorage.removeItem("fielddraft.project.v1");
    });
    await page.goto("/");
    await page.waitForTimeout(500);

    const gpuErrors: string[] = [];
    page.on("pageerror", (err) => gpuErrors.push(err.message));

    // Add several different primitive types
    await page.evaluate(() => {
      const store = (window as unknown as {
        __fdStore?: {
          addPrimitive: (k: string) => void;
          setNextOp: (op: number) => void;
        };
      }).__fdStore;
      if (!store) throw new Error("no store");
      store.addPrimitive("sphere");
      store.addPrimitive("box");
      store.addPrimitive("cylinder");
      store.addPrimitive("torus");
    });

    await waitForShaderCompile(page);
    await page.waitForTimeout(1000);

    const bodyCount = await page.evaluate(() => {
      return (window as unknown as {
        __fdStore?: { kernel: { bodyCount: () => number } };
      }).__fdStore?.kernel.bodyCount() ?? 0;
    });
    expect(bodyCount).toBeGreaterThanOrEqual(1);

    const blocking = gpuErrors.filter(
      (e) => !e.includes("THREE.TSL") && !e.includes("storageTexture") &&
        !e.includes("WebGPU") && !e.includes("webgl") && !e.includes("THREE.Clock"),
    );
    expect(blocking).toEqual([]);
  });

  test("sdf-vm handles mixed primitive types", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("fielddraft.renderMode", "sdf-vm");
      localStorage.removeItem("fielddraft.project.v1");
    });
    await page.goto("/");
    await page.waitForTimeout(500);

    const gpuErrors: string[] = [];
    page.on("pageerror", (err) => gpuErrors.push(err.message));

    await page.evaluate(() => {
      const store = (window as unknown as {
        __fdStore?: {
          addPrimitive: (k: string) => void;
          setNextOp: (op: number) => void;
        };
      }).__fdStore;
      if (!store) throw new Error("no store");
      store.addPrimitive("sphere");
      store.addPrimitive("box");
      store.addPrimitive("cylinder");
      store.addPrimitive("torus");
    });

    await page.waitForTimeout(3000);

    const bodyCount = await page.evaluate(() => {
      return (window as unknown as {
        __fdStore?: { kernel: { bodyCount: () => number } };
      }).__fdStore?.kernel.bodyCount() ?? 0;
    });
    expect(bodyCount).toBeGreaterThanOrEqual(1);

    const blocking = gpuErrors.filter(
      (e) => !e.includes("THREE.TSL") && !e.includes("storageTexture") &&
        !e.includes("WebGPU") && !e.includes("webgl") && !e.includes("THREE.Clock"),
    );
    expect(blocking).toEqual([]);
  });
});
