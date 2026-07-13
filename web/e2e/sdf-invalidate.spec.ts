import { expect, test } from "@playwright/test";

/**
 * v0.6.574 — the demand-frameloop invalidation gap for SDF-only commits.
 *
 * BUG: a commit that changes only the SDF program / body uniforms (displace
 * on-off, blend radius, shell thickness, twist rate, a primitive being added
 * …) repainted the canvas with the PREVIOUS image. It only became visible
 * once the user moved the camera.
 *
 * Root cause (web/src/viewportSdfOverlays.tsx, useSdfPipelineOverlay):
 * the frame that DETECTED the change wrote all the "seen" refs
 * (rebuildFpRef / dataFpRef / matFeaturesRef / selectionRef) and then bailed
 * out at `if (!readyRef.current || compilingRef.current) return` without
 * rendering — the rebuild+shader-compile is async. The frame that the compile
 * completion invalidated afterwards recomputed the fingerprints, found them
 * all equal to those refs, and passed `sceneDirty = false` into
 * renderDeferredFrame — which, with an unchanged camera `frameKey`, skipped
 * every pass and re-blitted the STALE cached shadeRT (ticket 40001a3a's
 * overlay-only fast path). A camera nudge changed the frameKey and that is
 * what made the new geometry pop in.
 *
 * The fix accumulates the dirt in a `sceneDirtyRef` that is cleared only after
 * a frame actually rendered, so the dirt survives the compile gap.
 *
 * This spec is the anchor: it performs NO camera move, NO wheel, NO mouse and
 * NO keyboard input after load — every mutation goes through the kernel/store
 * API and the canvas must update on its own. It also asserts the demand loop
 * comes back to REST after each mutation (no endless repaint loop):
 * `__fdSdfFrames`
 * (DEV-only counter in the SDF overlay's useFrame) must stop advancing.
 *
 * Headful, real WebGPU — run from `web/`:
 *   DISPLAY=:1 XAUTHORITY=/run/user/1000/.mutter-Xwaylandauth.<cookie> \
 *     npx playwright test --config e2e/playwright.config.ts sdf-invalidate
 */

test("v0.6.574: SDF-only commits repaint the canvas without a camera move", async ({
  page,
}) => {
  test.setTimeout(300_000);
  // Renderer/app errors only. Plain resource 401/404s are backend noise (the
  // optional Go server isn't up in this run) and say nothing about the SDF.
  const logs: string[] = [];
  page.on("console", (m) => {
    const t = m.text();
    if (m.type() === "error" && !t.startsWith("Failed to load resource")) {
      logs.push(t);
    }
  });

  await page.goto("/");
  await page.evaluate(() => {
    try {
      for (const k of Object.keys(window.localStorage))
        if (k.startsWith("fielddraft")) window.localStorage.removeItem(k);
      // Keep the census on pure SDF faces: no B-Rep edge overlay lines.
      window.localStorage.setItem("fielddraft.brepEdgeMode", "none");
      window.localStorage.setItem("fielddraft.theme", "dark");
    } catch {
      /* ignore */
    }
  });
  await page.reload();
  const canvas = page.getByTestId("fielddraft-canvas");
  await expect(canvas).toBeVisible();
  await page.waitForTimeout(1200);

  // Datum widgets draw over the body; hide them so the frame diff is body-only.
  await page.evaluate(() => {
    const store = (window as any).__fdStore;
    for (const d of ["origin", "top", "front", "right"]) store.toggleDatumVisible(d);
  });

  const shot = async () => (await canvas.screenshot()).toString("base64");
  /** DEV instrumentation: `marches` counts full G-buffer + sphere-trace passes
   *  (sdfDeferredRenderer.renderDeferredFrame), `frames` counts overlay ticks. */
  const counters = () =>
    page.evaluate(() => {
      const w = window as any;
      return {
        frames: (w.__fdSdfFrames ?? 0) as number,
        marches: (w.__fdSdfMarches ?? 0) as number,
      };
    });

  /** The "Building SDF…" curtain (.shader-compiling) is an opaque DOM layer
   *  over the canvas while a rebuild/compile runs — an element screenshot
   *  composites it, so a frame taken under it says nothing about the SDF
   *  image. Settle = curtain down AND two identical consecutive frames (the
   *  renderer is deterministic: an unchanged scene ⇒ byte-identical PNG). */
  const settle = async (what: string): Promise<string> => {
    let last = "";
    for (let i = 0; i < 120; i++) {
      await page.waitForTimeout(500);
      const busy = await page.evaluate(
        () =>
          document.getElementById("fd-shader-compiling")?.style.display !== "none",
      );
      if (busy) {
        last = "";
        continue;
      }
      const s = await shot();
      if (s === last) return s;
      last = s;
    }
    throw new Error(`frame never settled: ${what}`);
  };

  /**
   * After settling, the SDF must be at REST: a scene with no edit and no
   * camera move must not re-march. This is the counterpart guard to the fix —
   * `sceneDirtyRef` is cleared after a presented frame, so a stuck-true flag
   * (i.e. "just repaint everything forever", which is NOT the fix we want)
   * would show up here as a march counter climbing at frame rate.
   *
   * Asserted on MARCHES, not on raw frame ticks: a bare overlay tick that
   * replays the cached shade blit costs nothing and is the intended fast path
   * (ticket 40001a3a). Stray invalidates from other overlays therefore must
   * not fail this — GPU work must.
   */
  const expectQuiescent = async (what: string): Promise<void> => {
    const before = await counters();
    expect(before.marches, "DEV instrumentation must be installed").toBeGreaterThan(0);
    await page.waitForTimeout(2000);
    const after = await counters();
    expect(
      after.marches - before.marches,
      `${what}: the SDF must stop re-marching once the scene is at rest, but it re-marched ${after.marches - before.marches}× in 2 s (frames ticked ${after.frames - before.frames}×)`,
    ).toBe(0);
  };

  const sphereId = (await page.evaluate(() =>
    (window as any).__fdStore?.addPrimitive("sphere"),
  )) as number;
  expect(sphereId).toBeGreaterThan(0);

  const withSphere = await settle("addPrimitive(sphere)");
  await expectQuiescent("after addPrimitive");

  const setParams = (patch: Record<string, unknown>) =>
    page.evaluate(
      ({ sphereId, patch }) => {
        const kernel = (window as any).__fdKernel;
        const store = (window as any).__fdStore;
        const params = kernel.getFeatureCallParams(sphereId) as Record<string, unknown>;
        for (const [key, v] of Object.entries(patch)) {
          const cur = params[key];
          params[key] =
            cur !== null &&
            typeof cur === "object" &&
            !Array.isArray(cur) &&
            "value" in (cur as object)
              ? { ...(cur as object), value: v }
              : v;
        }
        return store.updateFeatureParams(sphereId, params) as boolean;
      },
      { sphereId, patch },
    );

  /** Commit an SDF-only change and require the canvas to show it WITHOUT any
   *  camera interaction. `prev` is the settled frame from before the commit. */
  const commitMustRepaint = async (
    prev: string,
    patch: Record<string, unknown>,
    what: string,
  ): Promise<string> => {
    expect(await setParams(patch), `${what}: params accepted`).toBeTruthy();
    const next = await settle(what);
    expect(
      next,
      `${what}: the canvas still shows the PREVIOUS image — the SDF-only commit missed the demand-frameloop repaint (no camera move was made, which is the point)`,
    ).not.toBe(prev);
    await expectQuiescent(`after ${what}`);
    return next;
  };

  // 1) Pure uniform change (radius) — the data-update path.
  const r30 = await commitMustRepaint(withSphere, { radius: 30 }, "radius 50 → 30");

  // 2) SDF PROGRAM change (displace on) — the opcode layout moves, so this
  //    takes the rebuild + async shader-compile path: the one the stale-cache
  //    blit used to swallow.
  const displaced = await commitMustRepaint(
    r30,
    { displace_depth: 0.3, displace_freq: 1.5 },
    "displace 0.3/1.5 on",
  );

  // 3) …and back off again — the same gap in the other direction.
  await commitMustRepaint(displaced, { displace_depth: 0, displace_freq: 0 }, "displace off");

  // 4) Second SDF-program class: a blend (smin) radius change on a second body.
  const boxId = (await page.evaluate(() =>
    (window as any).__fdStore?.addPrimitive("box"),
  )) as number;
  expect(boxId).toBeGreaterThan(0);
  const withBox = await settle("addPrimitive(box)");
  await expectQuiescent("after addPrimitive(box)");

  await page.evaluate(
    ({ boxId }) => {
      const kernel = (window as any).__fdKernel;
      const store = (window as any).__fdStore;
      const params = kernel.getFeatureCallParams(boxId) as Record<string, unknown>;
      const cur = params["translate"];
      params["translate"] =
        cur !== null && typeof cur === "object" && !Array.isArray(cur) && "value" in (cur as object)
          ? { ...(cur as object), value: [20, 0, 0] }
          : [20, 0, 0];
      store.updateFeatureParams(boxId, params);
    },
    { boxId },
  );
  const boxMoved = await settle("box translate");
  expect(boxMoved, "box translate must repaint").not.toBe(withBox);
  await expectQuiescent("after box translate");

  // 5) Suppress toggle — the same class. A suppressed feature is skipped by the
  //    kernel entirely (there is NO translucent-ghost draw path), so the body
  //    must be GONE from the very next frame. Before the fix this repainted the
  //    stale cached shade blit, i.e. the body stayed fully on screen with no
  //    camera move — which is what earlier acceptance runs misread as a "ghost".
  expect(
    await page.evaluate(
      ({ sphereId }) =>
        (window as any).__fdStore.toggleFeatureSuppressed(sphereId) as boolean,
      { sphereId },
    ),
    "suppress accepted",
  ).toBeTruthy();
  const suppressed = await settle("suppress sphere");
  expect(
    suppressed,
    "suppressing the sphere must repaint immediately (no camera move) — a stale blit would still show it",
  ).not.toBe(boxMoved);
  await expectQuiescent("after suppress");

  // Un-suppress: it must come back, again with no camera move.
  expect(
    await page.evaluate(
      ({ sphereId }) =>
        (window as any).__fdStore.toggleFeatureSuppressed(sphereId) as boolean,
      { sphereId },
    ),
    "un-suppress accepted",
  ).toBeTruthy();
  const unsuppressed = await settle("un-suppress sphere");
  expect(unsuppressed, "un-suppressing must repaint immediately").not.toBe(suppressed);
  await expectQuiescent("after un-suppress");

  expect(logs, `console errors: ${logs.join(" | ")}`).toEqual([]);
});
