import { expect, test } from "@playwright/test";

/**
 * v0.6.568 — visual acceptance for the Displace march-relax fix.
 *
 * The displace field `d − depth·sin(xf)·sin(1.3yf)·sin(0.7zf)` is not
 * 1-Lipschitz (L ≈ 1 + |depth·freq|·√(1+1.3²+0.7²) ≈ 1.80 at depth 0.3 /
 * freq 1.5). Before the fix, sphere-marching overshot the bump surface at
 * grazing incidence — background-coloured HOLES inside the silhouette.
 * The fix computes a per-body step damping (`march_relax` in
 * sdf_compiler.rs) and feeds it via bodySdfMarchRelax → marchRelaxBuf →
 * emitUnionMarch into both SDF renderers.
 *
 * This spec is a permanent headful-WebGPU anchor (real GPU: run with
 * DISPLAY=:1 + the mutter Xwayland XAUTHORITY cookie, e.g.
 *   DISPLAY=:1 XAUTHORITY=/run/user/1000/.mutter-Xwaylandauth.<cookie> \
 *     npx playwright test --config e2e/playwright.config.ts
 * from `web/`). It asserts:
 *  1. Regression anchor: bodySdfMarchRelax == 1.0 for a plain sphere and
 *     ≈ 1/1.80 ≈ 0.555 once displace_depth 0.3 / displace_freq 1.5 is set.
 *  2. No marching holes: front AND iso views of the displaced sphere show
 *     zero background-pixel clusters inside the silhouette (the rim is
 *     where every ray grazes the bump field).
 *  3. Silhouette integrity: front rim compared per-direction against a
 *     plain-sphere baseline at the same camera; iso rim against its own
 *     median. Allowed band = bump amplitude (0.3 mm on r=30 ≈ 1 %) + 8 px;
 *     an iteration-cap fray at the rim would blow past that.
 *
 * Measurement design (each point below was learned the hard way):
 *  - TRUE background frames: the body is translated 1000 mm off-screen and
 *    back; body mask + hole test are per-pixel diffs against that frame.
 *    Colour classifiers fail — the displaced surface shades pale/white
 *    (bump normals tilt up to ~40°, killing the saturated body blue), and
 *    the sky is itself a white-blob gradient.
 *  - Datum widgets (origin dot + Top/Front/Right plane cards) draw OVER
 *    the body at the canvas centre and would read as fake holes → hidden
 *    via store.toggleDatumVisible. Likewise the xray B-Rep edge overlay
 *    draws seam lines across the body → disabled via localStorage.
 *  - NO camera interaction of any kind: view presets "1"/"0" fit the scene
 *    to a usable ~130 px radius on their own, and every mutation is expected
 *    to repaint by itself. This spec USED to force repaints with a
 *    camera-neutral −2/+2 wheel nudge and to tolerate frames in which the
 *    body was missing entirely. Both were crutches for ONE renderer bug,
 *    fixed in v0.6.574 (see e2e/sdf-invalidate.spec.ts): the deferred
 *    pipeline re-blitted its cached shade target after an SDF-only commit,
 *    so the canvas kept showing the PREVIOUS image until the camera moved.
 *    That is also what made the body look like it "vanished during regen" —
 *    the stale blit being shown was the background frame captured with the
 *    body translated off-screen. The nudge and the tolerance are gone; the
 *    spec now waits for a settled frame and asserts the body is THERE.
 *  - Settling: a frame counts only when the "Building SDF…" curtain is down
 *    and two consecutive screenshots are byte-identical (the renderer is
 *    deterministic: identical scene ⇒ byte-identical PNG).
 *
 * Screenshots land in /tmp/displace-march-{front-baseline,front,iso}.png
 * (+ *-bg.png background frames).
 */

const EXPECTED_RELAX = 1 / (1 + 0.3 * 1.5 * Math.sqrt(1 + 1.3 * 1.3 + 0.7 * 0.7));

type Silhouette = {
  width: number;
  height: number;
  radii: number[]; // rim radius per degree (360 samples), from canvas centre
  rMin: number;
  rMax: number;
  rMedian: number;
  holePixels: number; // pixels matching the empty-scene frame inside 0.85·rMin
  holeClusters: number; // 4-connected clusters (size ≥ 2) of such pixels
  holeSamples: Array<[number, number]>; // up to 12 hole positions for diagnosis
};

/** Decode both PNGs in-page (Node has no PNG decoder) and measure the body
 *  silhouette centred on the canvas. `bodyB64` = frame with the body,
 *  `bgB64` = same camera, body translated off-screen. */
async function analyzeSilhouette(
  page: import("@playwright/test").Page,
  bodyB64: string,
  bgB64: string,
): Promise<Silhouette> {
  return page.evaluate(
    async ({ bodyB64, bgB64 }: { bodyB64: string; bgB64: string }) => {
      const decode = async (b64: string) => {
        const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
        const bitmap = await createImageBitmap(new Blob([bin], { type: "image/png" }));
        const cv = document.createElement("canvas");
        cv.width = bitmap.width;
        cv.height = bitmap.height;
        const ctx = cv.getContext("2d")!;
        ctx.drawImage(bitmap, 0, 0);
        return { data: ctx.getImageData(0, 0, cv.width, cv.height).data, W: cv.width, H: cv.height };
      };
      const body = await decode(bodyB64);
      const bg = await decode(bgB64);
      const W = Math.min(body.W, bg.W);
      const H = Math.min(body.H, bg.H);

      const dist2 = (x: number, y: number): number => {
        const i = (y * body.W + x) * 4;
        const j = (y * bg.W + x) * 4;
        const dr = body.data[i] - bg.data[j];
        const dg = body.data[i + 1] - bg.data[j + 1];
        const db = body.data[i + 2] - bg.data[j + 2];
        return dr * dr + dg * dg + db * db;
      };
      // Body = clearly deviates from the empty-scene frame. Hole = matches
      // it almost exactly (the renderer is deterministic, so a marching
      // miss reproduces the bg pixel bit-near-exactly). The gap between the
      // thresholds absorbs AA edge pixels without flapping either way.
      const isBody = (x: number, y: number) => dist2(x, y) > 30 * 30;
      const isBgExact = (x: number, y: number) => dist2(x, y) < 8 * 8;

      // Rim radius per direction: walk INWARD from the outside until a
      // sustained body run begins (4 px). Walking outward from the centre
      // fails on the displaced body: its bright bump glints can match the
      // pale sky for several pixels and fake an early rim exit.
      const cx = W / 2;
      const cy = H / 2;
      const N = 360;
      const radii: number[] = [];
      const maxR = Math.min(W, H) / 2 - 2;
      for (let a = 0; a < N; a++) {
        const dx = Math.cos((2 * Math.PI * a) / N);
        const dy = Math.sin((2 * Math.PI * a) / N);
        let run = 0;
        let rim = 0;
        for (let r = maxR; r >= 2; r--) {
          const x = Math.round(cx + dx * r);
          const y = Math.round(cy + dy * r);
          if (x < 0 || y < 0 || x >= W || y >= H) continue;
          if (isBody(x, y)) {
            run++;
            if (run >= 4) {
              rim = r + run - 1;
              break;
            }
          } else run = 0;
        }
        radii.push(rim);
      }
      const sorted = radii.slice().sort((a, b) => a - b);
      const rMedian = sorted[N >> 1];
      const rMin = sorted[0];
      const rMax = sorted[N - 1];

      // Holes: exact-bg pixels strictly inside 0.85·rMin (clear of rim+bumps).
      const holeR = 0.85 * rMin;
      const holeSet = new Set<number>();
      const holes: Array<[number, number]> = [];
      const x0 = Math.max(0, Math.floor(cx - holeR));
      const x1 = Math.min(W - 1, Math.ceil(cx + holeR));
      const y0 = Math.max(0, Math.floor(cy - holeR));
      const y1 = Math.min(H - 1, Math.ceil(cy + holeR));
      for (let y = y0; y <= y1; y++) {
        for (let x = x0; x <= x1; x++) {
          const ddx = x - cx;
          const ddy = y - cy;
          if (ddx * ddx + ddy * ddy > holeR * holeR) continue;
          if (isBgExact(x, y)) {
            holeSet.add(y * W + x);
            if (holes.length < 12) holes.push([x, y]);
          }
        }
      }
      // 4-connected clusters of size ≥ 2 — a marching hole is never a lone
      // pixel; single-pixel coincidences don't fail the run.
      let holeClusters = 0;
      const seen = new Set<number>();
      for (const start of holeSet) {
        if (seen.has(start)) continue;
        const stack = [start];
        seen.add(start);
        let size = 0;
        while (stack.length) {
          const p = stack.pop()!;
          size++;
          const px = p % W;
          for (const q of [p - W, p + W, px > 0 ? p - 1 : -1, px < W - 1 ? p + 1 : -1]) {
            if (q >= 0 && holeSet.has(q) && !seen.has(q)) {
              seen.add(q);
              stack.push(q);
            }
          }
        }
        if (size >= 2) holeClusters++;
      }

      return {
        width: W,
        height: H,
        radii,
        rMin,
        rMax,
        rMedian,
        holePixels: holeSet.size,
        holeClusters,
        holeSamples: holes,
      };
    },
    { bodyB64, bgB64 },
  );
}

const brief = (s: Silhouette) =>
  JSON.stringify({
    rMin: s.rMin,
    rMax: s.rMax,
    rMedian: s.rMedian,
    holePixels: s.holePixels,
    holeClusters: s.holeClusters,
  });

test("v0.6.568: displace sphere marches hole-free at grazing angles (march_relax)", async ({
  page,
}) => {
  test.setTimeout(540_000); // headful WebGPU shader compile + displaced-body regen can block for minutes
  const logs: string[] = [];
  page.on("console", (m) => logs.push(`[${m.type()}] ${m.text()}`));

  await page.goto("/");
  await page.evaluate(() => {
    try {
      for (const k of Object.keys(window.localStorage))
        if (k.startsWith("fielddraft")) window.localStorage.removeItem(k);
      // The xray B-Rep edge overlay draws seam lines across the body that
      // are not body-coloured — off, so the pixel census sees pure faces.
      window.localStorage.setItem("fielddraft.brepEdgeMode", "none");
      // Dark theme: the light theme's near-white viewport background is
      // byte-close to the body's specular highlights, which then read as
      // fake holes in the per-pixel diff. Against the dark background the
      // lit body differs everywhere.
      window.localStorage.setItem("fielddraft.theme", "dark");
    } catch {
      /* ignore */
    }
  });
  await page.reload();
  const canvas = page.getByTestId("fielddraft-canvas");
  await expect(canvas).toBeVisible();
  await page.waitForTimeout(1200);

  // Hide the datum widgets (origin dot + plane cards) — they draw OVER the
  // body at the canvas centre and would register as fake holes.
  await page.evaluate(() => {
    const store = (window as any).__fdStore;
    for (const d of ["origin", "top", "front", "right"]) store.toggleDatumVisible(d);
  });
  await page.waitForTimeout(400);

  const shot = async () => (await canvas.screenshot()).toString("base64");
  const canvasBox = (await canvas.boundingBox())!;
  const midPt = {
    x: canvasBox.x + canvasBox.width / 2,
    y: canvasBox.y + canvasBox.height / 2,
  };
  /** A frame is usable once the "Building SDF…" curtain is down (it is an
   *  opaque DOM layer over the canvas, and an element screenshot composites
   *  it) AND two consecutive screenshots are byte-identical. No nudging: an
   *  SDF-only commit is required to repaint on its own (v0.6.574). */
  const settle = async (what: string): Promise<string> => {
    let last = "";
    for (let i = 0; i < 90; i++) {
      await page.waitForTimeout(400);
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

  // ── 1) Plain sphere (radius 30): march_relax must be exactly 1 — the
  //       damping must NOT leak onto eikonal bodies. ─────────────────────
  const preAdd = await shot();
  const sphereId = (await page.evaluate(() =>
    (window as any).__fdStore?.addPrimitive("sphere"),
  )) as number;
  expect(sphereId).toBeGreaterThan(0);
  expect(
    await settle("addPrimitive(sphere)"),
    "adding the sphere must repaint the canvas by itself",
  ).not.toBe(preAdd);

  const setParams = (patch: Record<string, unknown>) =>
    page.evaluate(
      ({ sphereId, patch }) => {
        const kernel = (window as any).__fdKernel;
        const store = (window as any).__fdStore;
        const params = kernel.getFeatureCallParams(sphereId) as Record<string, unknown>;
        for (const [key, v] of Object.entries(patch)) {
          const cur = params[key];
          params[key] =
            cur !== null && typeof cur === "object" && !Array.isArray(cur) && "value" in (cur as object)
              ? { ...(cur as object), value: v }
              : v;
        }
        return store.updateFeatureParams(sphereId, params) as boolean;
      },
      { sphereId, patch },
    );
  /** Patch params and require the canvas to show the change on its own — no
   *  camera move, no nudge (v0.6.574). */
  const patchAndRender = async (patch: Record<string, unknown>, what: string) => {
    const pre = await shot();
    expect(await setParams(patch), `${what}: params accepted`).toBeTruthy();
    expect(
      await settle(what),
      `${what}: the canvas still shows the previous image — an SDF-only commit must repaint without a camera move`,
    ).not.toBe(pre);
  };

  await patchAndRender({ radius: 30 }, "radius 50 → 30");

  const relaxOf = () =>
    page.evaluate(() => (window as any).__fdKernel?.bodySdfMarchRelax(0) as number);
  const relaxPlain = await relaxOf();
  logs.push(`marchRelax plain sphere = ${relaxPlain}`);
  expect(relaxPlain, "plain sphere must march at full step").toBeCloseTo(1.0, 3);

  /** Count pixels (sampled every 3rd) in the central half of the frame that
   *  clearly deviate from the empty-scene frame — "is a body on screen?". */
  const bodyPixelCount = (bodyB64: string, bgB64: string) =>
    page.evaluate(
      async ({ bodyB64, bgB64 }: { bodyB64: string; bgB64: string }) => {
        const decode = async (b64: string) => {
          const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
          const bitmap = await createImageBitmap(new Blob([bin], { type: "image/png" }));
          const cv = document.createElement("canvas");
          cv.width = bitmap.width;
          cv.height = bitmap.height;
          const ctx = cv.getContext("2d")!;
          ctx.drawImage(bitmap, 0, 0);
          return { data: ctx.getImageData(0, 0, cv.width, cv.height).data, W: cv.width, H: cv.height };
        };
        const a = await decode(bodyB64);
        const b = await decode(bgB64);
        const W = Math.min(a.W, b.W);
        const H = Math.min(a.H, b.H);
        let n = 0;
        for (let y = H >> 2; y < H - (H >> 2); y += 3) {
          for (let x = W >> 2; x < W - (W >> 2); x += 3) {
            const i = (y * a.W + x) * 4;
            const j = (y * b.W + x) * 4;
            const dr = a.data[i] - b.data[j];
            const dg = a.data[i + 1] - b.data[j + 1];
            const db = a.data[i + 2] - b.data[j + 2];
            if (dr * dr + dg * dg + db * db > 30 * 30) n++;
          }
        }
        return n;
      },
      { bodyB64, bgB64 },
    );

  // Differential measurement at the CURRENT camera: move the body 1 m off
  // screen for a true background frame, move it back. The bg frame is
  // camera-only, so one capture serves every measurement at that camera.
  const captureBg = async (bgPath: string): Promise<string> => {
    await patchAndRender({ translate: [1000, 0, 0] }, "translate off-screen (bg frame)");
    const bgB64 = await shot();
    await canvas.screenshot({ path: bgPath });
    await patchAndRender({ translate: [0, 0, 0] }, "translate back");
    return bgB64;
  };
  /** Settle, then require the body to actually BE on screen. Strict on
   *  purpose (v0.6.574): the body must stay visible through a regen — the
   *  old "poll until some frame happens to contain a body" tolerance was
   *  hiding the stale-blit bug, whose symptom was exactly a settled frame
   *  with no body in it. */
  const measure = async (bgB64: string, shotPath: string): Promise<Silhouette> => {
    const bodyB64 = await settle(shotPath);
    const px = await bodyPixelCount(bodyB64, bgB64);
    expect(
      px,
      `${shotPath}: the settled frame contains no body (${px} body px) — the body must remain visible across a regeneration`,
    ).toBeGreaterThan(300);
    await canvas.screenshot({ path: shotPath });
    return analyzeSilhouette(page, bodyB64, bgB64);
  };

  // ── 2) Front view (preset fits the scene — no wheel, see header) and
  //       plain-sphere BASELINE at this camera. ──────────────────────────
  await page.mouse.click(midPt.x, midPt.y); // focus viewport
  await page.keyboard.press("Escape"); // drop any selection the click made
  await page.keyboard.press("1"); // front view
  await page.waitForTimeout(800);

  const bgFront = await captureBg("/tmp/displace-march-front-bg.png");
  const baseline = await measure(bgFront, "/tmp/displace-march-front-baseline.png");
  logs.push(`baseline (plain, front): ${brief(baseline)}`);
  expect(
    baseline.rMedian,
    "front preset must show the sphere at ≥ 100 px radius for a meaningful pixel census",
  ).toBeGreaterThan(100);
  expect(
    baseline.holeClusters,
    `plain sphere shows bg clusters inside its own silhouette — renderer baseline broken, samples ${JSON.stringify(baseline.holeSamples)}`,
  ).toBe(0);

  // ── 3) Displace it (depth 0.3 / freq 1.5 — the Lipschitz case from
  //       sdf_compiler.rs::displace_march_relax_dampens_by_lipschitz_bound)
  //       WITHOUT touching the camera. ───────────────────────────────────
  await patchAndRender(
    { displace_depth: 0.3, displace_freq: 1.5 },
    "displace 0.3/1.5 applied",
  );

  const relaxDisplace = await relaxOf();
  logs.push(
    `marchRelax displace sphere = ${relaxDisplace} (expected ≈ ${EXPECTED_RELAX.toFixed(4)})`,
  );
  expect(
    Math.abs(relaxDisplace - EXPECTED_RELAX),
    `displace sphere must damp the march step to ≈ ${EXPECTED_RELAX.toFixed(4)}, got ${relaxDisplace}`,
  ).toBeLessThan(5e-3);

  // Camera unchanged since the baseline — the front bg frame is still valid.
  const front = await measure(bgFront, "/tmp/displace-march-front.png");
  logs.push(`displaced (front): ${brief(front)}`);

  // Acceptance 1: no background clusters inside the silhouette.
  expect(
    front.holeClusters,
    `front view: ${front.holeClusters} marching-hole cluster(s), ${front.holePixels} bg px inside r=${(0.85 * front.rMin).toFixed(0)}px, samples ${JSON.stringify(front.holeSamples)}`,
  ).toBe(0);

  // Acceptance 2: rim integrity vs. the plain baseline at the same camera.
  const allow = 0.01 * baseline.rMedian + 8;
  const deviations = front.radii.map((r, i) => Math.abs(r - baseline.radii[i]));
  const rimOutliers = deviations.filter((d) => d > allow).length;
  const dMax = Math.max(...deviations);
  logs.push(
    `front rim vs baseline: max deviation ${dMax}px, allow ${allow.toFixed(1)}px, outliers ${rimOutliers}/360`,
  );
  expect(
    rimOutliers,
    `front rim deviates from the plain-sphere baseline at ${rimOutliers}/360 angles (max ${dMax}px, allowed ${allow.toFixed(1)}px) — silhouette artefact`,
  ).toBeLessThanOrEqual(4);

  // ── 4) Iso view — a second, oblique grazing geometry over the bumps. ──
  await page.keyboard.press("0");
  await page.waitForTimeout(800);

  const bgIso = await captureBg("/tmp/displace-march-iso-bg.png");
  const iso = await measure(bgIso, "/tmp/displace-march-iso.png");
  logs.push(`displaced (iso): ${brief(iso)}`);
  expect(iso.rMedian, "iso view sphere large enough").toBeGreaterThan(100);
  expect(
    iso.holeClusters,
    `iso view: ${iso.holeClusters} marching-hole cluster(s), ${iso.holePixels} bg px inside, samples ${JSON.stringify(iso.holeSamples)}`,
  ).toBe(0);
  // Self-band on the iso rim (no plain baseline here): median ± (bump+8px).
  const isoAllow = 0.01 * iso.rMedian + 8;
  const isoOutliers = iso.radii.filter((r) => Math.abs(r - iso.rMedian) > isoAllow).length;
  logs.push(`iso rim: ${isoOutliers}/360 outside median±${isoAllow.toFixed(1)}px`);
  expect(
    isoOutliers,
    `iso rim: ${isoOutliers}/360 radius samples outside median±${isoAllow.toFixed(1)}px`,
  ).toBeLessThanOrEqual(4);

  console.log("=== DISPLACE MARCH PROBE ===\n" + logs.join("\n"));
});
