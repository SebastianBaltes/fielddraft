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
    const opButton = row.locator(".op");
    await opButton.click();
    await expect(row).toHaveAttribute("data-feature-op", "difference");
    await opButton.click();
    await expect(row).toHaveAttribute("data-feature-op", "intersect");
    await opButton.click();
    await expect(row).toHaveAttribute("data-feature-op", "union");
  });

  test("K1: mesh renderer shows the dual-contoured two-primitive subtraction", async ({
    page,
  }) => {
    await page.goto("/");
    // Wait for the WASM kernel + WebGPU pipeline to be live.
    await page.waitForTimeout(400);

    // Build a known scene: a base sphere, then a box subtracted from it.
    await page.getByTestId("add-sphere").click();
    await page.getByTestId("op-1").click(); // Difference
    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    // K1 pipeline: the kernel re-meshed on each mutation; the viewport
    // polls `meshVersion` once per frame and rebinds BufferGeometry
    // attributes. A short wait lets the first post-mutation frame land.
    await page.waitForTimeout(600);

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
    // camera framing. The point is "the DC mesh actually drew shaded
    // pixels into the canvas, against a light-mode background".
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
    // And the mesh pass must have drawn at least *some* lit primitive
    // pixels — proves the kernel produced a DC mesh and the WebGPU
    // pipeline rasterised it.
    expect(
      stats.bluePixels,
      `no lit primitive pixels: ${JSON.stringify(stats)}`,
    ).toBeGreaterThan(0);
  });

  test("K7.2.1: Export STL button downloads a non-empty binary file", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(400);

    // Export button is disabled on the empty document.
    const button = page.getByTestId("export-stl");
    await expect(button).toBeDisabled();

    // Add a box so the exporter has one body to serialise.
    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    await expect(button).toBeEnabled();

    // The mesher runs on a worker so the kernel's own body
    // cache stays empty until the exporter forces a sync
    // rebuild (which `exportStlBinary` does internally, see
    // K7.2 kernel side). No client-side wait needed.

    // Kick off the download and wait for Playwright to notice
    // the file save event.
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      button.click(),
    ]);
    expect(download.suggestedFilename()).toBe("fielddraft.stl");

    // Peek the file size via Playwright's download path helper —
    // any non-trivial mesh produces at least a few hundred bytes
    // (80-byte header + 4-byte count + 50 bytes per triangle).
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    expect(stat.size).toBeGreaterThan(100);
  });

  test("K7.5: editing a box's Half extents in the outliner dialog round-trips to the kernel", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    // Select the box row so the FeatureDialog mounts on top.
    await page.getByTestId("feature-row-1").click();

    // Drive the Vec3 Half extents widget via its per-axis
    // aria-labels. The default add-box size is 50 × 50 × 50;
    // we'll grow the X half-extent to 75.
    const xInput = page.getByLabel("Half extents x");
    await expect(xInput).toBeVisible();
    await xInput.fill("75");
    // The TS commit path runs synchronously off the onChange,
    // but give React one tick to propagate before we peek the
    // kernel.
    await page.waitForTimeout(100);

    const after = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      return kernel.getFeatureCallParams(1);
    });
    expect(after).not.toBeNull();
    const halfExtents = (after as { half_extents?: number[] }).half_extents;
    expect(halfExtents?.[0]).toBeCloseTo(75, 5);
    // Y and Z stay at the add-box default.
    expect(halfExtents?.[1]).toBeCloseTo(50, 5);
    expect(halfExtents?.[2]).toBeCloseTo(50, 5);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.0: linear pattern replicates the latest feature into multiple bodies", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    // Pattern button starts disabled — nothing to pattern.
    const patternBtn = page.getByTestId("linear-pattern-latest");
    await expect(patternBtn).toBeDisabled();

    // Add a box and pattern it.
    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    await expect(patternBtn).toBeEnabled();
    await patternBtn.click();

    // The pattern call lives as one outliner row, but the
    // regenerated spine has the source box + two clones = 3
    // spine entries. listFeatures walks feature_calls, so the
    // outliner shows the box + the pattern = 2 rows.
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    const shape = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
            meshTriangleCount?: () => number;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      return {
        features: kernel.listFeatures(),
      };
    });
    expect(shape).not.toBeNull();
    expect(shape!.features.some((f) => f.kind === "linear_pattern")).toBe(true);

    // Export STL — a patterned scene should produce a
    // meaningfully larger mesh than the single-box case.
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
    ]);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    // Three boxes at the DC mesher's default resolution
    // produce well over 1 KB of STL triangles.
    expect(stat.size).toBeGreaterThan(1000);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.1: circular pattern replicates the latest feature around the Y axis", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    const circularBtn = page.getByTestId("circular-pattern-latest");
    await expect(circularBtn).toBeDisabled();

    // Add a box (the default add-box stagger offsets it
    // along +X) and circular-pattern it around Y.
    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    await expect(circularBtn).toBeEnabled();
    await circularBtn.click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    const shape = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; kind: string }>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      return kernel.listFeatures();
    });
    expect(shape).not.toBeNull();
    expect(shape!.some((f) => f.kind === "circular_pattern")).toBe(true);

    // Export STL — six boxes distributed around a ring
    // produce a mesh strictly larger than a single box at
    // the default DC resolution.
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
    ]);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    expect(stat.size).toBeGreaterThan(2000);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
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
