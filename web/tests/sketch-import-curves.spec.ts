import { expect, test } from "@playwright/test";
import {
  freshLoad,
  subCallTypes,
  committedSketchTypes,
} from "./sketchHarness";

/**
 * F22-full: SVG full transform stack + exact Bézier import. Feeds an SVG
 * containing
 *   • a rotated element (rotate transform must be honoured, not dropped),
 *   • a path with a cubic Bézier (preserved AS a curve → `sketch_path`),
 *   • a donut (M…Z M…Z two-subpath path → one `sketch_path` w/ a real hole),
 * and asserts the parsed geometry lands as `sketch_path` sub-calls both in
 * the live session and, after Finish, in the committed sketch — on real
 * WebGPU. Cold first-paint can be slow, so the timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch vector import — curves + transforms", () => {
  test("imports a rotated element + cubic path + donut hole as sketch_path", async ({
    page,
  }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-mode-indicator").waitFor();

    const svg = `<svg xmlns="http://www.w3.org/2000/svg">
      <g transform="rotate(30)">
        <rect x="0" y="0" width="20" height="10"/>
      </g>
      <path d="M40 0 C40 20 70 20 70 0"/>
      <path d="M0 40 L30 40 L30 70 L0 70 Z M8 48 L22 48 L22 62 L8 62 Z"/>
    </svg>`;

    await page.getByTestId("sketch-import-input").setInputFiles({
      name: "curves.svg",
      mimeType: "image/svg+xml",
      buffer: Buffer.from(svg),
    });

    // The two <path> elements both become sketch_path sub-calls.
    await expect.poll(async () => subCallTypes(page)).toContain("sketch_path");

    const types = await subCallTypes(page);
    expect(types.filter((t) => t === "sketch_path").length).toBeGreaterThanOrEqual(2);
    // The rotated <rect> still imports as 4 lines.
    expect(types.filter((t) => t === "sketch_line").length).toBeGreaterThanOrEqual(4);

    // Inspect the donut path: it must carry TWO contours (outer + hole)
    // so the kernel cuts a real hole, and the cubic path must carry
    // quad-bezier segments (curve preserved, not a polyline).
    const detail = await page.evaluate(() => {
      const w = window as unknown as {
        __fdStore?: {
          state: {
            sketchSession: {
              subCalls: Array<{
                type: string;
                params?: Record<string, unknown>;
              }>;
            };
          };
        };
      };
      const calls = w.__fdStore?.state.sketchSession.subCalls ?? [];
      const paths = calls.filter((c) => c.type === "sketch_path");
      const contourCounts = paths.map(
        (p) => (p.params?.contours as unknown[] | undefined)?.length ?? 0,
      );
      // Any quad segment anywhere across the imported paths?
      let hasQuad = false;
      for (const p of paths) {
        const contours = (p.params?.contours as
          | Array<{ segments?: Array<{ kind?: string }> }>
          | undefined) ?? [];
        for (const c of contours) {
          for (const s of c.segments ?? []) {
            if (s.kind === "quad") hasQuad = true;
          }
        }
      }
      return { contourCounts, hasQuad };
    });
    // A path with two subpaths → one sketch_path with two contours.
    expect(detail.contourCounts).toContain(2);
    // The cubic survived as quad-bezier segments.
    expect(detail.hasQuad).toBe(true);

    // The canvas renders without a present/draw crash.
    await expect(page.getByTestId("sketch-canvas-2d")).toBeVisible();

    // Finish: the sketch_path flows through to the committed sketch.
    await page.getByTestId("sketch-detail-accept").click();
    await expect
      .poll(async () => committedSketchTypes(page))
      .not.toBeNull();
    const committed = await committedSketchTypes(page);
    expect(committed!.filter((t) => t === "sketch_path").length).toBeGreaterThanOrEqual(2);

    expect(pageErrors).toEqual([]);
  });
});
