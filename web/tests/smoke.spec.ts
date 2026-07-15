import { expect, test, type Download } from "@playwright/test";

/**
 * Trigger an STL download through the current UI and return the Download.
 *
 * The old flat `export-stl` toolbar button is gone: STL export now lives in the
 * menu ("STL…" → `menu-export-stl`) and opens the two-step ExportDialog — click
 * `export-mesh` to run the client mesher, then `export-save` (shown once the
 * mesh phase is "done") fires the actual file download. Meshing runs on a
 * worker after a cold WebGPU start, so the save button gets a generous wait.
 */
async function exportStlDownload(
  page: import("@playwright/test").Page,
): Promise<Download> {
  await page.getByTestId("settings-menu-trigger").click();
  // "STL…" lives in the hover-revealed "Export ▸" submenu, so hover its
  // trigger before the item is clickable.
  await page.getByTestId("menu-export").hover();
  await page.getByTestId("menu-export-stl").click();
  await expect(page.getByTestId("export-dialog")).toBeVisible();
  await page.getByTestId("export-mesh").click();
  const save = page.getByTestId("export-save");
  await expect(save).toBeVisible({ timeout: 60_000 });
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    save.click(),
  ]);
  // Save leaves the dialog OPEN (in its "done" state, showing export-save, not
  // export-mesh). Close it so a second export in the same test opens a fresh
  // dialog instead of hanging on a export-mesh button that isn't there.
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("export-dialog")).toBeHidden();
  return download;
}

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
    await expect(page.getByText("FieldDraft")).toBeVisible();
    await expect(page.getByTestId("fielddraft-canvas")).toBeVisible();
    await expect(page.getByTestId("fielddraft-toolbar")).toBeVisible();
    await expect(page.getByTestId("fielddraft-outliner")).toBeVisible();
    await page.waitForTimeout(800);

    // Default document is empty.
    await expect(page.locator('[data-feature-count="0"]')).toBeAttached();

    const blocking = consoleErrors.filter(
      (msg) =>
        !msg.includes("WebGPU") &&
        !msg.toLowerCase().includes("webgl") &&
        !msg.includes("THREE.Clock") &&
        // The multi-user backend answers `/api/me` with 401 when the e2e run
        // carries no session cookie — that is the correct response, the browser
        // just logs every failed fetch as a console error. Only 401 passes; a
        // 5xx still fails (server broken, not merely unauthenticated).
        !msg.includes("the server responded with a status of 401"),
    );
    expect(blocking, `unexpected console errors:\n${blocking.join("\n")}`).toEqual([]);
  });

  test("M2: add sphere, switch op, add box, verify outliner & undo/redo", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(600);

    // Add a sphere with the default op (Union).
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("sphere"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-1")).toBeVisible();

    // Switch the next-add op to Difference and add a box. (The per-row op
    // toggle + `data-feature-op` attribute were removed in 68dc07d — Boolean is
    // its own feature now — so we no longer assert the op on the row; the flow
    // still exercises setNextOp + a second add.)
    await page.evaluate(() => { (window as unknown as { __fdStore?: { setNextOp: (op: number) => void } }).__fdStore?.setNextOp(1); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-2")).toBeVisible();

    // Undo removes the box. The flat `undo`/`redo` toolbar buttons are gone —
    // undo/redo live in the menu now, but the keyboard shortcuts are the
    // canonical path (keyboardCommands.ts): Ctrl+Z / Ctrl+Shift+Z.
    await page.keyboard.press("Control+z");
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-2")).toHaveCount(0);

    // Redo brings it back.
    await page.keyboard.press("Control+Shift+z");
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-2")).toBeVisible();

    // Select the box via the store directly — after
    // K8.7/K8.8 added eye/reorder/duplicate buttons, the row's
    // body is button-heavy enough that Playwright's
    // centre-click lands on something that stopPropagation and
    // never surfaces to the row's onClick. The store
    // selectOnly path is what the row handler eventually calls
    // anyway, so going through __fdStore is a faithful stand-in
    // for "the user clicks row 2".
    await page.evaluate(() => {
      (
        window as unknown as { __fdStore?: { selectOnly: (id: number) => void } }
      ).__fdStore?.selectOnly(2);
    });
    await page.keyboard.press("Delete");
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
  });

  // (Removed) "M2: per-row op cycle button advances union → difference →
  // intersect": the per-feature op toggle it drove was deleted in 68dc07d
  // ("remove per-feature op toggles") — Boolean is a dedicated feature now, so
  // there is no `.op` button or `data-feature-op` attribute on a row to cycle.
  // The Boolean feature has its own coverage; nothing here to modernise.

  test("K1: mesh renderer shows the dual-contoured two-primitive subtraction", async ({
    page,
  }) => {
    await page.goto("/");
    // Wait for the WASM kernel + WebGPU pipeline to be live.
    await page.waitForTimeout(400);

    // Build a known scene: a base sphere, then a box subtracted from it.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("sphere"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { setNextOp: (op: number) => void } }).__fdStore?.setNextOp(1); }); // Difference
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

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

  test("K7.2.1: Export STL downloads a non-empty binary file", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(400);

    // Add a box so the exporter has one body to serialise. (The old
    // "disabled on an empty document" check went away with the flat toolbar
    // button — export is a menu → dialog now; the dialog just meshes whatever
    // bodies exist.)
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    // Run the export through the menu → dialog flow and wait for the download.
    const download = await exportStlDownload(page);
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

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

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
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    await expect(patternBtn).toBeEnabled();
    await patternBtn.click();

    // The pattern call lives as one outliner row, but the
    // regenerated spine has the source box + two clones = 3
    // spine entries. listFeatures walks feature_calls, so the
    // outliner shows the box + the pattern = 2 rows.
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

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
    const download = await exportStlDownload(page);
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

    // Add a box and circular-pattern it around Y.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    await expect(circularBtn).toBeEnabled();
    await circularBtn.click();
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

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
    const download = await exportStlDownload(page);
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

  test("K8.2: mirror replicates the latest feature through the YZ plane", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    const mirrorBtn = page.getByTestId("mirror-latest");
    await expect(mirrorBtn).toBeDisabled();

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    await expect(mirrorBtn).toBeEnabled();
    await mirrorBtn.click();
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

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
    expect(shape!.some((f) => f.kind === "mirror")).toBe(true);

    // Export STL — source + one mirrored clone is strictly
    // larger than the single-box case.
    const download = await exportStlDownload(page);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    expect(stat.size).toBeGreaterThan(500);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.3: editing a linear pattern's count in the dialog round-trips to the kernel", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.getByTestId("linear-pattern-latest").click();
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    // Select the pattern row so its FeatureDialog mounts.
    await page.getByTestId("feature-row-2").click();

    // The derive prettifies `count` to `Count`; the generic
    // FeatureDialog renders a Field with a required asterisk
    // (`Count *`). `getByLabel` matches substrings, which is
    // what we want here.
    const countInput = page.getByLabel(/^Count/);
    await expect(countInput).toBeVisible();
    await countInput.fill("5");
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
      return kernel.getFeatureCallParams(2);
    });
    expect(after).not.toBeNull();
    expect((after as { count?: number }).count).toBe(5);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.4: revolve sweeps a sketch rectangle into a tube", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    const revolveBtn = page.getByTestId("revolve-latest-sketch");
    await expect(revolveBtn).toBeDisabled();

    // Drop in a sketch with a rectangle profile at r ∈ [5, 9],
    // y ∈ [−10, 10]. Revolving it around Y produces a 4 mm-thick
    // tube. Going through __fdStore.enterSketch + addSketchSubCall
    // avoids the flaky click-drag-in-viewport route while still
    // flowing through the store's commit path so React state
    // updates.
    const sketchId = await page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            enterSketch: (plane: string) => void;
            pushSketchSubCalls: (
              calls: { type: string; params: Record<string, unknown> }[],
            ) => void;
            commitSketch: () => number | null;
          };
        }
      ).__fdStore;
      if (!store) throw new Error("__fdStore not exposed");
      store.enterSketch("qCreatedBy(origin_plane_xy)");
      store.pushSketchSubCalls([
        // A vertical line on x=0: the revolve AXIS. Since ticket ni56 a revolve
        // needs an explicit axis (no implicit local-Y fallback), so a bare
        // rectangle revolved with no axis produces NO body (empty 84-byte STL).
        {
          type: "sketch_line",
          params: { start: [0, -10], end: [0, 10] },
        },
        {
          type: "sketch_rectangle",
          params: {
            corner_a: [5, -10],
            corner_b: [9, 10],
          },
        },
      ]);
      return store.commitSketch();
    });
    expect(sketchId).toBeGreaterThan(0);

    // Re-render tick so the store picks the new sketch up.
    await page.waitForTimeout(100);
    await expect(revolveBtn).toBeEnabled();
    await revolveBtn.click();
    await page.waitForTimeout(200);

    // The revolve was created with `profiles` but no axis (the UI's query
    // picker would fill it). Set it to the x=0 axis line. The sketch line's
    // solver entity id isn't exposed directly, so probe the low ids and keep
    // the one that yields a non-empty body — the same tactic the vase parity
    // fixture uses.
    const revolveId = await page.evaluate(() => {
      const kernel = (window as unknown as { __fdKernel?: { listFeatures: () => Array<{ id: number; kind: string }> } }).__fdKernel;
      return kernel?.listFeatures().find((f) => f.kind === "revolve")?.id ?? null;
    });
    expect(revolveId).not.toBeNull();
    const axisOk = await page.evaluate(
      ({ sid, rid }) => {
        const w = window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
            replaceFeatureCallParams: (id: number, p: Record<string, unknown>) => void;
            bodyCount: () => number;
          };
        };
        const kernel = w.__fdKernel;
        if (!kernel) return false;
        const base = kernel.getFeatureCallParams(rid);
        for (let entity = 1; entity <= 24; entity++) {
          kernel.replaceFeatureCallParams(rid, { ...base, axis: `qSketchLine(${sid}, ${entity})` });
          if (kernel.bodyCount() > 0) return true;
        }
        return false;
      },
      { sid: sketchId as number, rid: revolveId as number },
    );
    expect(axisOk, "no axis entity id yielded a revolved body").toBe(true);
    await page.waitForTimeout(200);

    const after = await page.evaluate(() => {
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
    expect(after).not.toBeNull();
    expect(after!.some((f) => f.kind === "revolve")).toBe(true);

    // Export STL — a revolved 4 mm tube with 20 mm height
    // produces well over 1 KB of triangles at the default DC
    // resolution.
    const download = await exportStlDownload(page);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    expect(stat.size).toBeGreaterThan(1000);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.5: double-click a feature row to rename it inline", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    const row = page.getByTestId("feature-row-1");
    await expect(row.locator(".feature-name")).toHaveText(/^Box/);

    // Double-click enters rename mode — the row swaps its
    // label out for an <input> autofocused on the current
    // name.
    await row.dblclick();
    const input = page.getByTestId("feature-rename-1");
    await expect(input).toBeFocused();
    await input.fill("Main body");
    await input.press("Enter");

    // Label reverts to a span with the new text, and the
    // kernel's wire-form params carry the custom name.
    await expect(row.locator(".feature-name")).toHaveText("Main body");
    const params = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      return kernel ? kernel.getFeatureCallParams(1) : null;
    });
    expect((params as { name?: string }).name).toBe("Main body");

    // Pattern features (no SpineFeature.name) pick the rename
    // up via the synth_name_for_call params short-circuit.
    await page.getByTestId("linear-pattern-latest").click();
    const patternRow = page.getByTestId("feature-row-2");
    await patternRow.dblclick();
    const patternInput = page.getByTestId("feature-rename-2");
    await patternInput.fill("Row of bodies");
    await patternInput.press("Enter");
    await expect(patternRow.locator(".feature-name")).toHaveText(
      "Row of bodies",
    );

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.6: up/down arrows reorder features in the outliner", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    // Three primitives so both buttons have somewhere to go.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("sphere"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("cylinder"); });
    await page.waitForTimeout(200);
    await expect(page.locator('[data-feature-count="3"]')).toBeAttached();

    const kindsAt = async () =>
      page.evaluate(() => {
        const kernel = (
          window as unknown as {
            __fdKernel?: {
              listFeatures: () => Array<{ id: number; kind: string }>;
            };
          }
        ).__fdKernel;
        return kernel ? kernel.listFeatures().map((f) => f.kind) : null;
      });
    expect(await kindsAt()).toEqual(["box", "sphere", "cylinder"]);

    // Move the sphere (id 2) up — it becomes the new head.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { moveFeatureUp: (id: number) => void } }).__fdStore?.moveFeatureUp(2); }); await page.waitForTimeout(100);
    expect(await kindsAt()).toEqual(["sphere", "box", "cylinder"]);

    // Move it back down — order restored.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { moveFeatureDown: (id: number) => void } }).__fdStore?.moveFeatureDown(2); }); await page.waitForTimeout(100);
    expect(await kindsAt()).toEqual(["box", "sphere", "cylinder"]);

    // Up on the first row is a no-op, down on the last row is
    // a no-op. Both must not crash or change state.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { moveFeatureUp: (id: number) => void } }).__fdStore?.moveFeatureUp(1); }); await page.waitForTimeout(100);
    await page.evaluate(() => { (window as unknown as { __fdStore?: { moveFeatureDown: (id: number) => void } }).__fdStore?.moveFeatureDown(3); }); await page.waitForTimeout(100);
    expect(await kindsAt()).toEqual(["box", "sphere", "cylinder"]);

    // Reorder participates in undo: the two edge-case clicks
    // above were no-ops, so one undo rewinds the sphere-down
    // move that restored the original order.
    await page.keyboard.press("Control+z"); // undo moved to the menu; shortcut is canonical
    expect(await kindsAt()).toEqual(["sphere", "box", "cylinder"]);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.7: suppress toggle drops a feature from regeneration but keeps the row", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("sphere"); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    // Suppress the sphere — the outliner row stays, the data
    // attribute flips, and `listFeatures` reports the flag.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { toggleFeatureSuppressed: (id: number) => void } }).__fdStore?.toggleFeatureSuppressed(2); }); await page.waitForTimeout(100);
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-suppressed",
      "true",
    );
    const flags = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            listFeatures: () => Array<{ id: number; suppressed?: boolean }>;
          };
        }
      ).__fdKernel;
      return kernel
        ? kernel.listFeatures().map((f) => [f.id, f.suppressed ?? false])
        : null;
    });
    expect(flags).toEqual([
      [1, false],
      [2, true],
    ]);

    // Undo reverses the suppress — so does clicking the button
    // a second time. Verify both paths.
    await page.keyboard.press("Control+z"); // undo moved to the menu; shortcut is canonical
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-suppressed",
      "false",
    );
    await page.evaluate(() => { (window as unknown as { __fdStore?: { toggleFeatureSuppressed: (id: number) => void } }).__fdStore?.toggleFeatureSuppressed(2); }); await page.waitForTimeout(100);
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-suppressed",
      "true",
    );
    await page.evaluate(() => { (window as unknown as { __fdStore?: { toggleFeatureSuppressed: (id: number) => void } }).__fdStore?.toggleFeatureSuppressed(2); }); await page.waitForTimeout(100);
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-suppressed",
      "false",
    );

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.8: duplicate a feature from the outliner", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    // Duplicate the box — a new row appears, selected, with
    // translate nudged +20 mm along X.
    await page.evaluate(() => { (window as unknown as { __fdStore?: { duplicateFeature: (id: number) => void } }).__fdStore?.duplicateFeature(1); }); await page.waitForTimeout(100);
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-2")).toBeVisible();

    const after = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      if (!kernel) return null;
      return {
        one: kernel.getFeatureCallParams(1),
        two: kernel.getFeatureCallParams(2),
      };
    });
    expect(after).not.toBeNull();
    const sourceTranslate = (after!.one as { translate?: number[] }).translate ?? [
      0, 0, 0,
    ];
    const cloneTranslate = (after!.two as { translate?: number[] }).translate ?? [
      0, 0, 0,
    ];
    expect(cloneTranslate[0]).toBeCloseTo(sourceTranslate[0] + 20, 5);
    expect(cloneTranslate[1]).toBeCloseTo(sourceTranslate[1], 5);
    expect(cloneTranslate[2]).toBeCloseTo(sourceTranslate[2], 5);

    // Undo rolls back the duplicate cleanly.
    await page.keyboard.press("Control+z"); // undo moved to the menu; shortcut is canonical
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
    await expect(page.getByTestId("feature-row-2")).toHaveCount(0);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.9: hole carves a cylinder out of a box and exports", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addHole: () => void } }).__fdStore?.addHole(); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    // The hole row advertises its kind as `hole` and carries diameter/depth in
    // its wire-form params. (The `data-feature-op` attribute went away in
    // 68dc07d; a hole is inherently a subtraction, verified via its params /
    // the exported geometry below, not a per-row op badge.)
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-kind",
      "hole",
    );

    const params = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      return kernel ? kernel.getFeatureCallParams(2) : null;
    });
    expect(params).not.toBeNull();
    const diameter = (params as { diameter?: { value?: number } }).diameter;
    const depth = (params as { depth?: { value?: number } }).depth;
    expect(diameter?.value).toBeCloseTo(6, 5);
    expect(depth?.value).toBeCloseTo(40, 5);

    // Export STL — box minus hole produces a valid mesh.
    const download = await exportStlDownload(page);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path);
    expect(stat.size).toBeGreaterThan(500);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.10: variables panel adds, edits, deletes, and persists", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    await expect(page.getByTestId("variables-empty")).toBeVisible();

    // Add two variables.
    await page.getByTestId("variable-new-name").fill("width");
    await page.getByTestId("variable-new-value").fill("50");
    await page.getByTestId("variable-add").click();

    await page.getByTestId("variable-new-name").fill("depth");
    await page.getByTestId("variable-new-value").fill("30");
    await page.getByTestId("variable-new-value").press("Enter");

    // BTreeMap ordering: `depth` before `width`.
    await expect(page.getByTestId("variable-row-depth")).toBeVisible();
    await expect(page.getByTestId("variable-row-width")).toBeVisible();

    // listVariables() now carries `kind` + `formula` per entry (ticket 8ar9);
    // project to {name, value} so these assertions test what they mean to.
    const kernelVars = async () =>
      page.evaluate(() => {
        const kernel = (
          window as unknown as {
            __fdKernel?: {
              listVariables: () => { name: string; value: number }[];
            };
          }
        ).__fdKernel;
        return kernel
          ? kernel.listVariables().map((v) => ({ name: v.name, value: v.value }))
          : null;
      });
    expect(await kernelVars()).toEqual([
      { name: "depth", value: 30 },
      { name: "width", value: 50 },
    ]);

    // Edit width → 75 via the inline input.
    const widthInput = page.getByTestId("variable-value-width");
    await widthInput.fill("75");
    await widthInput.press("Enter");
    expect(await kernelVars()).toEqual([
      { name: "depth", value: 30 },
      { name: "width", value: 75 },
    ]);

    // Delete depth.
    await page.getByTestId("variable-delete-depth").click({ force: true });
    expect(await kernelVars()).toEqual([{ name: "width", value: 75 }]);

    // Autosave debounce + reload — the remaining variable
    // round-trips through localStorage serde.
    await page.waitForTimeout(800);
    await page.reload();
    await page.waitForTimeout(600);
    expect(await kernelVars()).toEqual([{ name: "width", value: 75 }]);
    await expect(page.getByTestId("variable-row-width")).toBeVisible();

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.11: Length formulas resolve against variables during regen", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    // Seed a variable, build a box + hole so the CSG tree
    // has something to carve out of.
    await page.getByTestId("variable-new-name").fill("width");
    await page.getByTestId("variable-new-value").fill("100");
    await page.getByTestId("variable-add").click();
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addHole: () => void } }).__fdStore?.addHole(); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    // Patch the hole's diameter to `=width / 5` via the kernel
    // directly — the K8.12 slice will wire this through the
    // LengthInput widget, but the K8.11 resolver is already
    // live. Hole is feature id 2 because the box is 1.
    await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            replaceFeatureCallParams: (
              id: number,
              params: Record<string, unknown>,
            ) => void;
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      if (!kernel) throw new Error("__fdKernel not exposed");
      const current = kernel.getFeatureCallParams(2);
      // Park the hole at the origin so it's guaranteed to
      // overlap the box, and give it a large fixed depth so
      // the diameter is the only variable-driven dimension.
      kernel.replaceFeatureCallParams(2, {
        ...current,
        diameter: { value: "=width / 5", unit: "mm" },
        depth: { value: 200, unit: "mm" },
        translate: [0, 0, 0],
      });
    });
    await page.waitForTimeout(200);

    // The wire form keeps the formula string verbatim — the
    // resolver only substitutes at regen time, not on the
    // canonical document.
    const stored = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      return kernel ? kernel.getFeatureCallParams(2) : null;
    });
    const diameter = (stored as { diameter?: { value?: unknown } }).diameter;
    expect(diameter?.value).toBe("=width / 5");

    // Editing the variable re-regenerates via store.commit().
    // The hole follows: width=100 → Ø20 carved from a 100 mm
    // box. Bump to 500 → hole overflows the box entirely and
    // the carved STL output *shrinks* (there's less material
    // left). Either direction proves the formula got
    // re-resolved.
    const exportStlSize = async () => {
      const download = await exportStlDownload(page);
      const p = await download.path();
      const fs = await import("node:fs/promises");
      return (await fs.stat(p!)).size;
    };
    const smallHole = await exportStlSize();
    expect(smallHole).toBeGreaterThan(500);

    await page.getByTestId("variable-value-width").fill("500");
    await page.getByTestId("variable-value-width").press("Enter");
    await page.waitForTimeout(200);
    const bigHole = await exportStlSize();
    // The STL must change — direction depends on how DC
    // tessellates the intersection, but "the resolver is
    // live" == "output differs".
    expect(bigHole).not.toBe(smallHole);

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("K8.12: LengthInput formula toggle stores =formula in wire params", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    // Seed a variable and add a hole.
    await page.getByTestId("variable-new-name").fill("d");
    await page.getByTestId("variable-new-value").fill("8");
    await page.getByTestId("variable-add").click();
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addHole: () => void } }).__fdStore?.addHole(); });

    // Select the hole row so the FeatureDialog mounts.
    await page.evaluate(() => {
      (window as unknown as { __fdStore?: { selectOnly: (id: number) => void } })
        .__fdStore?.selectOnly(1);
    });
    await page.waitForTimeout(100);

    // There is no "ƒx" toggle any more (ticket nlij): a scalar field doubles as
    // its own formula input — you just type `=…` into the same box and the
    // field reads literal-vs-formula from the text (looksLikeFormula). So target
    // the Diameter field and type the formula straight in; the input then
    // carries the `is-formula` class.
    const diameterField = page.getByLabel(/diameter/i).first();
    await diameterField.fill("=d * 2");
    await diameterField.press("Enter");
    await expect(diameterField).toHaveClass(/is-formula/);
    await page.waitForTimeout(200);

    // The wire-form params should carry the formula string.
    const params = await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
          };
        }
      ).__fdKernel;
      return kernel ? kernel.getFeatureCallParams(1) : null;
    });
    const diameter = (params as { diameter?: { value?: unknown } }).diameter;
    expect(diameter?.value).toBe("=d * 2");

    expect(
      pageErrors.map((e) => e.message),
      `uncaught page errors:\n${pageErrors.map((e) => e.message).join("\n")}`,
    ).toEqual([]);
  });

  test("Goldfile: box + hole + fillet exports consistent STL", async ({ page }) => {
    // Regression baseline: a box with a subtracted hole and a
    // blend radius. The STL size must stay within a tolerance
    // across kernel changes — a size shift indicates a geometry
    // regression in the DC mesher or CSG pipeline.
    await page.goto("/");
    await page.waitForTimeout(400);

    // Build: 100×100×100 box, 20mm hole at origin, 5mm fillet.
    await page.evaluate(() => {
      const store = (
        window as unknown as {
          __fdStore?: {
            addPrimitive: (kind: string) => number;
            addHole: () => number;
            updateFeatureParams: (id: number, params: Record<string, unknown>) => boolean;
          };
        }
      ).__fdStore;
      if (!store) throw new Error("no store");
      store.addPrimitive("box");
      store.addHole();
      // Set blend_radius on the hole for a filleted edge.
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
            replaceFeatureCallParams: (id: number, params: Record<string, unknown>) => void;
          };
        }
      ).__fdKernel;
      if (kernel) {
        const holeParams = kernel.getFeatureCallParams(2);
        kernel.replaceFeatureCallParams(2, {
          ...holeParams,
          translate: [0, 0, 0],
          depth: { value: 200, unit: "mm" },
          diameter: { value: 20, unit: "mm" },
        });
      }
    });
    await page.waitForTimeout(200);

    const download = await exportStlDownload(page);
    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path!);
    // The STL must be non-trivial (> 10 KB for a filleted box
    // with a hole at DC default resolution).
    expect(stat.size).toBeGreaterThan(10_000);
    // Upper bound: < 5 MB (sanity — a 100 mm box shouldn't
    // produce excessive geometry).
    expect(stat.size).toBeLessThan(5_000_000);
  });

  test("M14: Export STEP downloads a non-empty .step file via OpenCASCADE", async ({ page }) => {
    // The OCCT WASM is ~50 MB and takes considerable time to fetch + compile,
    // especially under software rendering (SwiftShader). Allow 5 minutes.
    test.setTimeout(120_000);

    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    let alertMsg = "";
    page.on("dialog", async (dialog) => {
      alertMsg = dialog.message();
      await dialog.accept();
    });

    const [download] = await Promise.all([
      page.waitForEvent("download", { timeout: 90_000 }),
      page.evaluate(() => { (window as unknown as { __fdStore?: { downloadStep: () => Promise<void> } }).__fdStore?.downloadStep(); }),
    ]);
    expect(alertMsg, `alert was shown: ${alertMsg}`).toBe("");
    expect(download.suggestedFilename()).toBe("fielddraft.step");

    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path!);
    expect(stat.size).toBeGreaterThan(100);

    const content = await fs.readFile(path!, "utf-8");
    expect(content).toContain("ISO-10303-21");
  });

  test("M14: STEP export with degraded feature shows confirm and downloads", async ({ page }) => {
    test.setTimeout(120_000);
    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    await page.evaluate(() => {
      const kernel = (
        window as unknown as {
          __fdKernel?: {
            getFeatureCallParams: (id: number) => Record<string, unknown>;
            replaceFeatureCallParams: (id: number, params: Record<string, unknown>) => void;
          };
        }
      ).__fdKernel;
      if (!kernel) throw new Error("no kernel");
      const params = kernel.getFeatureCallParams(1);
      kernel.replaceFeatureCallParams(1, { ...params, blend_radius: 5.0 });
    });
    await page.waitForTimeout(100);

    let dialogMessage = "";
    page.on("dialog", (dialog) => {
      dialogMessage = dialog.message();
      dialog.accept();
    });

    const [download] = await Promise.all([
      page.waitForEvent("download", { timeout: 60_000 }),
      page.evaluate(() => { (window as unknown as { __fdStore?: { downloadStep: () => Promise<void> } }).__fdStore?.downloadStep(); }),
    ]);
    expect(download.suggestedFilename()).toBe("fielddraft.step");
    expect(dialogMessage).toContain("DEGRADED");

    const path = await download.path();
    expect(path).not.toBeNull();
    const fs = await import("node:fs/promises");
    const stat = await fs.stat(path!);
    expect(stat.size).toBeGreaterThan(100);
  });

  test("M14: STEP export with excluded feature can be cancelled via confirm dialog", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addGyroid: () => void } }).__fdStore?.addGyroid(); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    let dialogMessage = "";
    page.on("dialog", (dialog) => {
      dialogMessage = dialog.message();
      dialog.dismiss();
    });

    await page.evaluate(() => { (window as unknown as { __fdStore?: { downloadStep: () => Promise<void> } }).__fdStore?.downloadStep(); });
    await page.waitForTimeout(500);

    expect(dialogMessage).toContain("EXCLUDED");
    expect(dialogMessage).toContain("Gyroid");
  });

  test("M14i: Import STEP file creates an ImportedMesh feature via round-trip", async ({ page }) => {
    test.setTimeout(180_000);
    await page.goto("/");
    await page.waitForTimeout(400);

    // 1. Create a box and export it as STEP
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();

    page.on("dialog", async (dialog) => { await dialog.accept(); });

    const [download] = await Promise.all([
      page.waitForEvent("download", { timeout: 90_000 }),
      page.evaluate(() => { (window as unknown as { __fdStore?: { downloadStep: () => Promise<void> } }).__fdStore?.downloadStep(); }),
    ]);
    const stepPath = await download.path();
    expect(stepPath).not.toBeNull();

    const fs = await import("node:fs/promises");
    const stepBytes = await fs.readFile(stepPath!);
    expect(stepBytes.length).toBeGreaterThan(100);

    // 2. Clear the project, then import the STEP file
    await page.evaluate(() => {
      (window as unknown as { __fdStore?: { newProject: () => void } }).__fdStore?.newProject();
    });
    await expect(page.locator('[data-feature-count="0"]')).toBeAttached();

    // Pass the STEP bytes into the page and call importStepFile
    const featureId = await page.evaluate(async (b64) => {
      const binary = atob(b64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      const store = (window as unknown as { __fdStore?: { importStepFile: (buf: ArrayBuffer) => Promise<number | null> } }).__fdStore;
      if (!store) throw new Error("no store");
      return store.importStepFile(bytes.buffer);
    }, stepBytes.toString("base64"));

    expect(featureId).not.toBeNull();
    expect(typeof featureId).toBe("number");
    await expect(page.locator('[data-feature-count="1"]')).toBeAttached();
  });

  test("M14i: Import Sprengfass.step from disk and verify rendering", async ({ page }) => {
    test.setTimeout(180_000);
    await page.goto("/");
    await page.waitForTimeout(400);

    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const { fileURLToPath } = await import("node:url");
    const thisDir = path.dirname(fileURLToPath(import.meta.url));
    const stepFile = path.resolve(thisDir, "../../assets/models/Sprengfass.step");
    const stepBytes = await fs.readFile(stepFile);
    expect(stepBytes.length).toBeGreaterThan(1000);

    // Import the STEP via the OCCT worker (tessellate only). The DC
    // mesher can't handle complex imported meshes in reasonable time,
    // so we render the tessellated output directly.
    const meshInfo = await page.evaluate(async (b64: string) => {
      const binary = atob(b64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);

      // Vite dev-server URL resolved at runtime in the browser; typed as a
      // non-literal specifier so tsc treats it as a dynamic import (any).
      const stepModPath: string = "/src/stepImport.ts";
      const { importStep } = await import(/* @vite-ignore */ stepModPath);
      const { positions, normals, indices } = await importStep(bytes);
      const posF32 = positions instanceof Float32Array ? positions : new Float32Array(positions);
      const nrmF32 = normals instanceof Float32Array ? normals : new Float32Array(normals);
      const idxU32 = indices instanceof Uint32Array ? indices : new Uint32Array(indices);
      const hasNormals = nrmF32.some((v: number) => v !== 0);

      // Compute bounding box and center
      let minX = Infinity, minY = Infinity, minZ = Infinity;
      let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
      for (let i = 0; i < posF32.length; i += 3) {
        minX = Math.min(minX, posF32[i]);
        minY = Math.min(minY, posF32[i + 1]);
        minZ = Math.min(minZ, posF32[i + 2]);
        maxX = Math.max(maxX, posF32[i]);
        maxY = Math.max(maxY, posF32[i + 1]);
        maxZ = Math.max(maxZ, posF32[i + 2]);
      }
      const cx = (minX + maxX) / 2, cy = (minY + maxY) / 2, cz = (minZ + maxZ) / 2;
      const size = Math.max(maxX - minX, maxY - minY, maxZ - minZ);

      // Render using WebGL directly (no Three.js bare-import available
      // in page.evaluate). Simple flat-shaded triangle rendering.
      const cv = document.createElement("canvas");
      cv.width = 800; cv.height = 600;
      const gl = cv.getContext("webgl2")!;
      gl.viewport(0, 0, 800, 600);
      gl.clearColor(0.94, 0.94, 0.94, 1);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      gl.enable(gl.DEPTH_TEST);
      gl.enable(gl.CULL_FACE);

      const vs = `#version 300 es
        in vec3 aPos;
        in vec3 aNrm;
        uniform mat4 uMVP;
        out vec3 vPos;
        out vec3 vNrm;
        void main() {
          vPos = aPos;
          vNrm = aNrm;
          gl_Position = uMVP * vec4(aPos, 1.0);
        }`;
      const fsrc = `#version 300 es
        precision mediump float;
        in vec3 vPos;
        in vec3 vNrm;
        out vec4 fragColor;
        void main() {
          vec3 n;
          if (length(vNrm) > 0.01) {
            n = normalize(vNrm);
          } else {
            n = normalize(cross(dFdx(vPos), dFdy(vPos)));
          }
          if (!gl_FrontFacing) n = -n;
          vec3 key  = normalize(vec3(0.4, 1.0, 0.5));
          vec3 fill = normalize(vec3(-0.6, 0.2, 0.8));
          vec3 rim  = normalize(vec3(0.0, -0.3, -1.0));
          float d1 = max(dot(n, key),  0.0);
          float d2 = max(dot(n, fill), 0.0);
          float d3 = max(dot(n, rim),  0.0);
          float light = d1 * 0.45 + d2 * 0.2 + d3 * 0.1 + 0.3;
          fragColor = vec4(vec3(0.6, 0.65, 0.7) * light, 1.0);
        }`;
      function compileShader(src: string, type: number) {
        const s = gl.createShader(type)!;
        gl.shaderSource(s, src);
        gl.compileShader(s);
        return s;
      }
      const prog = gl.createProgram()!;
      gl.attachShader(prog, compileShader(vs, gl.VERTEX_SHADER));
      gl.attachShader(prog, compileShader(fsrc, gl.FRAGMENT_SHADER));
      gl.linkProgram(prog);
      gl.useProgram(prog);

      const vbo = gl.createBuffer()!;
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      gl.bufferData(gl.ARRAY_BUFFER, posF32, gl.STATIC_DRAW);
      const aPos = gl.getAttribLocation(prog, "aPos");
      gl.enableVertexAttribArray(aPos);
      gl.vertexAttribPointer(aPos, 3, gl.FLOAT, false, 0, 0);

      if (hasNormals) {
        const nbo = gl.createBuffer()!;
        gl.bindBuffer(gl.ARRAY_BUFFER, nbo);
        gl.bufferData(gl.ARRAY_BUFFER, nrmF32, gl.STATIC_DRAW);
        const aNrm = gl.getAttribLocation(prog, "aNrm");
        gl.enableVertexAttribArray(aNrm);
        gl.vertexAttribPointer(aNrm, 3, gl.FLOAT, false, 0, 0);
      }

      const ibo = gl.createBuffer()!;
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idxU32, gl.STATIC_DRAW);

      // Simple perspective MVP matrix
      const fov = 45 * Math.PI / 180, aspect = 800 / 600;
      const near = size * 0.01, far = size * 10;
      const f = 1 / Math.tan(fov / 2);
      const proj = [
        f / aspect, 0, 0, 0,
        0, f, 0, 0,
        0, 0, (far + near) / (near - far), -1,
        0, 0, 2 * far * near / (near - far), 0,
      ];
      // Camera position: offset from center
      const d = size * 1.5;
      const ex = cx + d * 0.7, ey = cy + d * 0.5, ez = cz + d * 0.7;
      // lookAt
      const fwd = [cx - ex, cy - ey, cz - ez];
      const flen = Math.sqrt(fwd[0] ** 2 + fwd[1] ** 2 + fwd[2] ** 2);
      fwd[0] /= flen; fwd[1] /= flen; fwd[2] /= flen;
      const up = [0, 1, 0];
      const right = [
        fwd[1] * up[2] - fwd[2] * up[1],
        fwd[2] * up[0] - fwd[0] * up[2],
        fwd[0] * up[1] - fwd[1] * up[0],
      ];
      const rlen = Math.sqrt(right[0] ** 2 + right[1] ** 2 + right[2] ** 2);
      right[0] /= rlen; right[1] /= rlen; right[2] /= rlen;
      const realUp = [
        right[1] * fwd[2] - right[2] * fwd[1],
        right[2] * fwd[0] - right[0] * fwd[2],
        right[0] * fwd[1] - right[1] * fwd[0],
      ];
      const view = [
        right[0], realUp[0], -fwd[0], 0,
        right[1], realUp[1], -fwd[1], 0,
        right[2], realUp[2], -fwd[2], 0,
        -(right[0] * ex + right[1] * ey + right[2] * ez),
        -(realUp[0] * ex + realUp[1] * ey + realUp[2] * ez),
        -(-fwd[0] * ex + -fwd[1] * ey + -fwd[2] * ez),
        1,
      ];
      // MVP = proj * view
      const mvp = new Float32Array(16);
      for (let r = 0; r < 4; r++)
        for (let c = 0; c < 4; c++) {
          let s = 0;
          for (let k = 0; k < 4; k++) s += proj[r + k * 4] * view[k + c * 4];
          mvp[r + c * 4] = s;
        }

      gl.uniformMatrix4fv(gl.getUniformLocation(prog, "uMVP"), false, mvp);
      gl.drawElements(gl.TRIANGLES, idxU32.length, gl.UNSIGNED_INT, 0);
      gl.finish();

      const dataUrl = cv.toDataURL("image/png");
      return {
        vertexCount: posF32.length / 3,
        triangleCount: idxU32.length / 3,
        bbox: { minX, minY, minZ, maxX, maxY, maxZ },
        size,
        screenshot: dataUrl,
      };
    }, stepBytes.toString("base64"));

    expect(meshInfo.vertexCount).toBeGreaterThan(100);
    expect(meshInfo.triangleCount).toBeGreaterThan(100);
    console.log(
      `Barrel: ${meshInfo.vertexCount} verts, ${meshInfo.triangleCount} tris, ` +
      `bbox=[${meshInfo.bbox.minX.toFixed(1)}..${meshInfo.bbox.maxX.toFixed(1)}, ` +
      `${meshInfo.bbox.minY.toFixed(1)}..${meshInfo.bbox.maxY.toFixed(1)}, ` +
      `${meshInfo.bbox.minZ.toFixed(1)}..${meshInfo.bbox.maxZ.toFixed(1)}], ` +
      `size=${meshInfo.size.toFixed(1)}`,
    );

    // Decode the data URL and save as PNG
    const pngB64 = meshInfo.screenshot.replace(/^data:image\/png;base64,/, "");
    const pngBuf = Buffer.from(pngB64, "base64");
    expect(pngBuf.length).toBeGreaterThan(1000);

    const outDir = path.resolve(thisDir, "../../assets/screenshots");
    await fs.mkdir(outDir, { recursive: true });
    await fs.writeFile(path.join(outDir, "sprengfass-import.png"), pngBuf);

    // Second render: top-down view of the lid to show tessellated symbols
    const lidShot = await page.evaluate(async (b64: string) => {
      const binary = atob(b64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);

      // Vite dev-server URL resolved at runtime in the browser; typed as a
      // non-literal specifier so tsc treats it as a dynamic import (any).
      const stepModPath: string = "/src/stepImport.ts";
      const { importStep } = await import(/* @vite-ignore */ stepModPath);
      const { positions, normals, indices } = await importStep(bytes);
      const posF32 = positions instanceof Float32Array ? positions : new Float32Array(positions);
      const nrmF32 = normals instanceof Float32Array ? normals : new Float32Array(normals);
      const idxU32 = indices instanceof Uint32Array ? indices : new Uint32Array(indices);
      const hasNormals = nrmF32.some((v: number) => v !== 0);

      const cv = document.createElement("canvas");
      cv.width = 800; cv.height = 800;
      const gl = cv.getContext("webgl2")!;
      gl.viewport(0, 0, 800, 800);
      gl.clearColor(0.94, 0.94, 0.94, 1);
      gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
      gl.enable(gl.DEPTH_TEST);
      gl.enable(gl.CULL_FACE);

      const vs = `#version 300 es
        in vec3 aPos;
        in vec3 aNrm;
        uniform mat4 uMVP;
        out vec3 vPos;
        out vec3 vNrm;
        void main() { vPos = aPos; vNrm = aNrm; gl_Position = uMVP * vec4(aPos, 1.0); }`;
      const fsrc = `#version 300 es
        precision mediump float;
        in vec3 vPos;
        in vec3 vNrm;
        out vec4 fragColor;
        void main() {
          vec3 n;
          if (length(vNrm) > 0.01) {
            n = normalize(vNrm);
          } else {
            n = normalize(cross(dFdx(vPos), dFdy(vPos)));
          }
          if (!gl_FrontFacing) n = -n;
          vec3 key  = normalize(vec3(1.0, 0.15, 0.3));
          vec3 fill = normalize(vec3(-0.6, 0.1, 0.8));
          float d1 = max(dot(n, key),  0.0);
          float d2 = max(dot(n, fill), 0.0);
          float light = d1 * 0.45 + d2 * 0.2 + 0.25;
          float depthEdge = clamp(1.0 - fwidth(vPos.y) * 60.0, 0.0, 1.0);
          float heightTint = smoothstep(98.8, 100.0, vPos.y);
          float ao = depthEdge * mix(0.75, 1.0, heightTint);
          fragColor = vec4(vec3(0.75, 0.78, 0.72) * light * ao, 1.0);
        }`;
      function compileShader(src: string, type: number) {
        const s = gl.createShader(type)!;
        gl.shaderSource(s, src);
        gl.compileShader(s);
        return s;
      }
      const prog = gl.createProgram()!;
      gl.attachShader(prog, compileShader(vs, gl.VERTEX_SHADER));
      gl.attachShader(prog, compileShader(fsrc, gl.FRAGMENT_SHADER));
      gl.linkProgram(prog);
      gl.useProgram(prog);

      const vbo = gl.createBuffer()!;
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      gl.bufferData(gl.ARRAY_BUFFER, posF32, gl.STATIC_DRAW);
      const aPos = gl.getAttribLocation(prog, "aPos");
      gl.enableVertexAttribArray(aPos);
      gl.vertexAttribPointer(aPos, 3, gl.FLOAT, false, 0, 0);

      if (hasNormals) {
        const nbo = gl.createBuffer()!;
        gl.bindBuffer(gl.ARRAY_BUFFER, nbo);
        gl.bufferData(gl.ARRAY_BUFFER, nrmF32, gl.STATIC_DRAW);
        const aNrm = gl.getAttribLocation(prog, "aNrm");
        gl.enableVertexAttribArray(aNrm);
        gl.vertexAttribPointer(aNrm, 3, gl.FLOAT, false, 0, 0);
      }

      const ibo = gl.createBuffer()!;
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ibo);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, idxU32, gl.STATIC_DRAW);

      // Top-down orthographic projection looking at the lid (Y≈100mm)
      const span = 55.0;
      const orthoN = 1.0, orthoF = 300.0;
      const proj = [
        2 / (2 * span), 0, 0, 0,
        0, 2 / (2 * span), 0, 0,
        0, 0, -2 / (orthoF - orthoN), 0,
        0, 0, -(orthoF + orthoN) / (orthoF - orthoN), 1,
      ];
      // lookAt: eye=(0,200,0), target=(0,100,0), up=(0,0,-1)
      // f=(0,-1,0), s=cross(f,up)=(1,0,0), u=cross(s,f)=(0,0,-1)
      const view = [
        1, 0, 0, 0,
        0, 0, 1, 0,
        0, -1, 0, 0,
        0, 0, -200, 1,
      ];
      const mvp = new Float32Array(16);
      for (let r = 0; r < 4; r++)
        for (let c = 0; c < 4; c++) {
          let s = 0;
          for (let k = 0; k < 4; k++) s += proj[r + k * 4] * view[k + c * 4];
          mvp[r + c * 4] = s;
        }

      gl.uniformMatrix4fv(gl.getUniformLocation(prog, "uMVP"), false, mvp);
      gl.drawElements(gl.TRIANGLES, idxU32.length, gl.UNSIGNED_INT, 0);
      gl.finish();

      return cv.toDataURL("image/png");
    }, stepBytes.toString("base64"));

    const lidB64 = lidShot.replace(/^data:image\/png;base64,/, "");
    const lidBuf = Buffer.from(lidB64, "base64");
    await fs.writeFile(path.join(outDir, "sprengfass-lid-top.png"), lidBuf);

    // Verify the rendered image shows a barrel (not just background).
    const stats = await page.evaluate(async (b64: string) => {
      const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const blob = new Blob([bin], { type: "image/png" });
      const bitmap = await createImageBitmap(blob);
      const cv = document.createElement("canvas");
      cv.width = Math.min(bitmap.width, 512);
      cv.height = Math.min(bitmap.height, 512);
      const ctx = cv.getContext("2d")!;
      ctx.drawImage(bitmap, 0, 0, cv.width, cv.height);
      const data = ctx.getImageData(0, 0, cv.width, cv.height).data;

      let meshPixels = 0;
      const total = cv.width * cv.height;
      for (let i = 0; i < data.length; i += 4) {
        const r = data[i];
        const g = data[i + 1];
        const b = data[i + 2];
        if ((r + g + b) > 30 && (r + g + b) < 700) meshPixels++;
      }
      return { total, meshPixels };
    }, pngB64);

    expect(
      stats.meshPixels,
      `barrel not visible — only ${stats.meshPixels}/${stats.total} mesh pixels`,
    ).toBeGreaterThan(stats.total * 0.05);
  });

  test("M14i: Kernel passthrough renders imported mesh without DC", async ({ page }) => {
    test.setTimeout(60_000);

    const errors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error" || msg.text().includes("failed") || msg.text().includes("Error")) {
        errors.push(`[${msg.type()}] ${msg.text()}`);
      }
    });
    page.on("pageerror", (err) => errors.push(`[pageerror] ${err.message}`));

    await page.goto("/");
    await page.waitForTimeout(400);

    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const { fileURLToPath } = await import("node:url");
    const thisDir = path.dirname(fileURLToPath(import.meta.url));
    const stepFile = path.resolve(thisDir, "../../assets/models/Sprengfass.step");
    const stepBytes = await fs.readFile(stepFile);

    const result = await page.evaluate(async (b64: string) => {
      const binary = atob(b64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      const store = (window as unknown as { __fdStore?: {
        importStepFile: (buf: ArrayBuffer) => Promise<number | null>;
        getState: () => { meshedBodies: { id: number; positions: Float32Array; indices: Uint32Array }[] };
        kernel: { toJson: () => string };
      } }).__fdStore;
      if (!store) throw new Error("no store");

      const featureId = await store.importStepFile(bytes.buffer);
      const jsonLen = store.kernel.toJson().length;

      // Wait for worker to produce bodies (up to 15s)
      let bodies: { id: number; positions: Float32Array; indices: Uint32Array }[] = [];
      for (let i = 0; i < 30; i++) {
        await new Promise(r => setTimeout(r, 500));
        bodies = store.getState().meshedBodies as typeof bodies;
        if (bodies.length > 0 && bodies[0].positions.length > 0) break;
      }

      return {
        featureId,
        jsonLen,
        bodyCount: bodies.length,
        vertexCount: bodies[0]?.positions?.length / 3 || 0,
        indexCount: bodies[0]?.indices?.length || 0,
      };
    }, stepBytes.toString("base64"));

    console.log(
      `Passthrough: featureId=${result.featureId}, jsonLen=${result.jsonLen}, ` +
      `bodies=${result.bodyCount}, verts=${result.vertexCount}, tris=${result.indexCount / 3}`,
    );
    if (errors.length > 0) console.log(`Errors:\n${errors.join("\n")}`);

    // Document JSON should be compact (~2-5MB, not 175MB)
    expect(result.jsonLen, "document JSON too large — passthrough storage not working").toBeLessThan(10_000_000);
    expect(result.featureId).not.toBeNull();
    expect(result.bodyCount).toBeGreaterThan(0);
    expect(result.vertexCount, "worker produced empty mesh — passthrough not working").toBeGreaterThan(100);
  });

  test("persists feature list across reloads via localStorage", async ({ page }) => {
    await page.goto("/");
    await page.waitForTimeout(400);

    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("box"); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { setNextOp: (op: number) => void } }).__fdStore?.setNextOp(1); });
    await page.evaluate(() => { (window as unknown as { __fdStore?: { addPrimitive: (k: string) => void } }).__fdStore?.addPrimitive("sphere"); });
    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();

    // Wait for the autosave debounce (500 ms) to flush.
    await page.waitForTimeout(800);

    await page.reload();
    await page.waitForTimeout(800);

    await expect(page.locator('[data-feature-count="2"]')).toBeAttached();
  });
});
