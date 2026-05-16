import { expect, test } from "@playwright/test";

test("Sprengfass via kernel importStepFile (with voxel repair)", async ({ page }) => {
  test.setTimeout(300_000);

  const consoleMessages: string[] = [];
  page.on("console", (msg) => consoleMessages.push(`[${msg.type()}] ${msg.text()}`));

  let alertMsg = "";
  page.on("dialog", async (dialog) => {
    alertMsg = dialog.message();
    await dialog.accept();
  });

  await page.goto("/");
  await page.waitForTimeout(1000);

  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const stepFile = path.resolve("/home/sorokan/ClaudeDraft/fielddraft/assets/models/Sprengfass.step");
  const stepBytes = await fs.readFile(stepFile);
  expect(stepBytes.length).toBeGreaterThan(1000);

  const result = await page.evaluate(async (b64: string) => {
    const binary = atob(b64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    const store = (window as unknown as { __fdStore?: { importStepFile: (buf: ArrayBuffer) => Promise<number | null> } }).__fdStore;
    if (!store) throw new Error("no store");
    const startTime = Date.now();
    const featureId = await store.importStepFile(bytes.buffer);
    const elapsed = Date.now() - startTime;
    return { featureId, elapsed };
  }, stepBytes.toString("base64"));

  console.log(`Alert: "${alertMsg}"`);
  console.log(`Console messages:\n${consoleMessages.filter(m => m.includes("error") || m.includes("Error") || m.includes("STEP") || m.includes("import")).join("\n")}`);
  console.log(`Result: featureId=${result.featureId}, elapsed=${result.elapsed}ms`);

  expect(alertMsg, `import failed with alert: ${alertMsg}`).toBe("");
  expect(result.featureId).not.toBeNull();
});
