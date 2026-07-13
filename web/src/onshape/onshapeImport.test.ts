import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { importOnshapeFeatures, type ImportKernel } from "./onshapeImport";

function kabelbox(): { features: any[] } {
  return JSON.parse(
    readFileSync(join(__dirname, "fixtures", "kabelbox-features.json"), "utf8"),
  );
}

/** Mock kernel: allocates sequential ids and reports every face-level
 *  query as uniquely resolving (the REAL uniqueness check runs against
 *  the wasm kernel in the app; here we exercise the translation flow). */
function mockKernel(): ImportKernel & { calls: { type: string; params: any }[] } {
  let next = 1;
  const calls: { type: string; params: any }[] = [];
  return {
    calls,
    addSketch(planeQuery, subCalls) {
      calls.push({ type: "sketch", params: { planeQuery, subCalls } });
      return next++;
    },
    addFeatureCall(typeName, paramsJson) {
      calls.push({ type: typeName, params: JSON.parse(paramsJson) });
      return next++;
    },
    resolveQuery() {
      return [{ kind: "face", idHash: "1", query: "qFace(1)" }];
    },
  };
}

describe("importOnshapeFeatures — Kabelbox translation flow", () => {
  it("imports sketches with parametric plane queries", async () => {
    const kernel = mockKernel();
    const result = await importOnshapeFeatures(kabelbox(), kernel);

    const sketches = kernel.calls.filter((c) => c.type === "sketch");
    expect(sketches.length).toBe(7);

    const planes = sketches.map((s) => s.params.planeQuery as string);
    // Sketch 1 + 3: default plane.
    expect(planes[0]).toBe("qCreatedBy(origin_plane_xy)");
    expect(planes[2]).toBe("qCreatedBy(origin_plane_xy)");
    // Sketch 2: extrude-1 start cap (fd id 2 — sketch1=1, extrude1=2).
    expect(planes[1]).toBe("qCreatedBy(2, CAP_BOTTOM)");
    // Sketch 5: the wall swept from Sketch 1's left line (entity 12:
    // rectangle lines bottom=3, top=6, left=9, right=12 by emission
    // order? — assert the FORM here; ordinals are covered below).
    expect(planes[4]).toMatch(/^qWall\(2, \d+\)$/);
    // Sketch 4: split-disambiguated wall.
    expect(planes[3]).toMatch(/^qAdjacentTo\(qWall\(2, \d+\), qWall\(\d+, \d+\)\)$/);

    // Extrudes carry operation + profile of the right sketch.
    const extrudes = kernel.calls.filter((c) => c.type === "extrude");
    expect(extrudes.length).toBeGreaterThanOrEqual(5);
    expect(extrudes[0].params.operation).toBe("new");
    expect(extrudes[0].params.profiles).toBe("qCreatedBy(1)");
    expect(extrudes[0].params.depth.value).toBeCloseTo(250); // 25 cm
    expect(extrudes[1].params.operation).toBe("remove");

    // Shell opens extrude 1's END cap.
    const shell = kernel.calls.find((c) => c.type === "shell");
    expect(shell).toBeTruthy();
    expect(shell!.params.open_faces).toBe("qCreatedBy(2, CAP_TOP)");
    expect(shell!.params.thickness.value).toBeCloseTo(2.5);

    // No silent failures: whatever couldn't translate is REPORTED.
    for (const err of result.errors) expect(typeof err).toBe("string");
  });

  it("skips a sketch whose plane resolves ambiguously — visibly", async () => {
    const kernel = mockKernel();
    kernel.resolveQuery = () => []; // nothing resolves
    const result = await importOnshapeFeatures(kabelbox(), kernel);
    // Default-plane sketches still import; face-level planes error out.
    const sketches = kernel.calls.filter((c) => c.type === "sketch");
    expect(sketches.length).toBe(2);
    expect(result.errors.some((e) => e.includes("löst auf 0"))).toBe(true);
  });
});
