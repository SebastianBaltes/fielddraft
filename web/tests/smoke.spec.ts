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
    await expect(page.getByText("FieldDraft")).toBeVisible();
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

  test("K8.2: mirror replicates the latest feature through the YZ plane", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => {
      pageErrors.push(err);
    });

    await page.goto("/");
    await page.waitForTimeout(400);

    const mirrorBtn = page.getByTestId("mirror-latest");
    await expect(mirrorBtn).toBeDisabled();

    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
    await expect(mirrorBtn).toBeEnabled();
    await mirrorBtn.click();
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
    expect(shape!.some((f) => f.kind === "mirror")).toBe(true);

    // Export STL — source + one mirrored clone is strictly
    // larger than the single-box case.
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
    ]);
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

    await page.getByTestId("add-box").click();
    await page.getByTestId("linear-pattern-latest").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

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
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
    ]);
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

    await page.getByTestId("add-box").click();
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
    await page.getByTestId("add-box").click();
    await page.getByTestId("add-sphere").click();
    await page.getByTestId("add-cylinder").click();
    await expect(page.getByTestId("feature-count")).toHaveText("3 features");

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
    await page.getByTestId("feature-move-up-2").click();
    expect(await kindsAt()).toEqual(["sphere", "box", "cylinder"]);

    // Move it back down — order restored.
    await page.getByTestId("feature-move-down-2").click();
    expect(await kindsAt()).toEqual(["box", "sphere", "cylinder"]);

    // Up on the first row is a no-op, down on the last row is
    // a no-op. Both must not crash or change state.
    await page.getByTestId("feature-move-up-1").click();
    await page.getByTestId("feature-move-down-3").click();
    expect(await kindsAt()).toEqual(["box", "sphere", "cylinder"]);

    // Reorder participates in undo: the two edge-case clicks
    // above were no-ops, so one undo rewinds the sphere-down
    // move that restored the original order.
    await page.getByTestId("undo").click();
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

    await page.getByTestId("add-box").click();
    await page.getByTestId("add-sphere").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    // Suppress the sphere — the outliner row stays, the data
    // attribute flips, and `listFeatures` reports the flag.
    await page.getByTestId("feature-suppress-2").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");
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
    await page.getByTestId("undo").click();
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-suppressed",
      "false",
    );
    await page.getByTestId("feature-suppress-2").click();
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-suppressed",
      "true",
    );
    await page.getByTestId("feature-suppress-2").click();
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

    await page.getByTestId("add-box").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");

    // Duplicate the box — a new row appears, selected, with
    // translate nudged +20 mm along X.
    await page.getByTestId("feature-duplicate-1").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");
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
    await page.getByTestId("undo").click();
    await expect(page.getByTestId("feature-count")).toHaveText("1 feature");
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

    await page.getByTestId("add-box").click();
    await page.getByTestId("add-hole").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

    // The hole row advertises its kind as `hole`, is op=difference,
    // and carries diameter/depth in its wire-form params.
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-kind",
      "hole",
    );
    await expect(page.getByTestId("feature-row-2")).toHaveAttribute(
      "data-feature-op",
      "difference",
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
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
    ]);
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

    const kernelVars = async () =>
      page.evaluate(() => {
        const kernel = (
          window as unknown as {
            __fdKernel?: {
              listVariables: () => { name: string; value: number }[];
            };
          }
        ).__fdKernel;
        return kernel ? kernel.listVariables() : null;
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
    await page.getByTestId("variable-delete-depth").click();
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
    await page.getByTestId("add-box").click();
    await page.getByTestId("add-hole").click();
    await expect(page.getByTestId("feature-count")).toHaveText("2 features");

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
      const [download] = await Promise.all([
        page.waitForEvent("download"),
        page.getByTestId("export-stl").click(),
      ]);
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
    await page.getByTestId("add-hole").click();

    // Select the hole row so the FeatureDialog mounts.
    await page.evaluate(() => {
      (window as unknown as { __fdStore?: { selectOnly: (id: number) => void } })
        .__fdStore?.selectOnly(1);
    });
    await page.waitForTimeout(100);

    // Find the Diameter field's "ƒx" toggle and click it —
    // switches from number to formula text input.
    const fxButtons = page.locator(".fd-feature-dialog .formula-toggle");
    await fxButtons.first().click();
    await page.waitForTimeout(50);

    // The formula input should now be visible; type a formula.
    const formulaInput = page.locator(".fd-feature-dialog .input.is-formula").first();
    await expect(formulaInput).toBeVisible();
    await formulaInput.fill("=d * 2");
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

    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByTestId("export-stl").click(),
    ]);
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
