import { expect, test } from "@playwright/test";
import {
  freshLoad,
  subCallTypes,
  committedSketchTypes,
} from "./sketchHarness";

/**
 * F22: SVG / DXF vector import. Each test enters a fresh sketch, feeds a
 * small in-memory file to the hidden `sketch-import-input` via Playwright's
 * `setInputFiles` (no fixture file needed), and asserts the parsed
 * geometry lands as the expected sub-calls — both live in the session and,
 * after Finish, in the committed sketch. Cold WebGPU first-paint can be
 * slow, so the per-test timeout is generous.
 */
test.describe.configure({ timeout: 120_000 });

test.describe("Sketch vector import", () => {
  test("imports an SVG rect + circle as lines + a circle", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-mode-indicator").waitFor();

    const svg = `<svg xmlns="http://www.w3.org/2000/svg">
      <rect x="0" y="0" width="40" height="20"/>
      <circle cx="60" cy="10" r="8"/>
    </svg>`;

    await page.getByTestId("sketch-import-input").setInputFiles({
      name: "shapes.svg",
      mimeType: "image/svg+xml",
      buffer: Buffer.from(svg),
    });

    await expect
      .poll(async () => subCallTypes(page))
      .toContain("sketch_circle");

    const types = await subCallTypes(page);
    expect(types.filter((t) => t === "sketch_line").length).toBeGreaterThanOrEqual(4);
    expect(types).toContain("sketch_circle");

    await page.getByTestId("sketch-detail-accept").click();
    await expect
      .poll(async () => committedSketchTypes(page))
      .not.toBeNull();
    const committed = await committedSketchTypes(page);
    expect(committed!.filter((t) => t === "sketch_line").length).toBeGreaterThanOrEqual(4);
    expect(committed).toContain("sketch_circle");

    expect(pageErrors).toEqual([]);
  });

  test("imports a DXF LINE + CIRCLE", async ({ page }) => {
    const pageErrors: Error[] = [];
    page.on("pageerror", (err) => pageErrors.push(err));

    await freshLoad(page);
    await page.getByTestId("enter-sketch").click();
    await page.getByTestId("sketch-mode-indicator").waitFor();

    const dxf = [
      "0", "SECTION", "2", "ENTITIES",
      "0", "LINE", "10", "0", "20", "0", "11", "30", "21", "10",
      "0", "CIRCLE", "10", "50", "20", "20", "40", "8",
      "0", "ENDSEC", "0", "EOF", "",
    ].join("\n");

    await page.getByTestId("sketch-import-input").setInputFiles({
      name: "shapes.dxf",
      mimeType: "application/dxf",
      buffer: Buffer.from(dxf),
    });

    await expect
      .poll(async () => subCallTypes(page))
      .toContain("sketch_circle");

    const types = await subCallTypes(page);
    expect(types).toContain("sketch_line");
    expect(types).toContain("sketch_circle");

    await page.getByTestId("sketch-detail-accept").click();
    await expect
      .poll(async () => committedSketchTypes(page))
      .not.toBeNull();
    const committed = await committedSketchTypes(page);
    expect(committed).toContain("sketch_line");
    expect(committed).toContain("sketch_circle");

    expect(pageErrors).toEqual([]);
  });
});
