import { describe, it, expect } from "vitest";
import { zipSync } from "fflate";
import { import3mf, export3mfRoundtrip, extractModelXml } from "./threemf";

const MINIMAL_MODEL_XML = `<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US"
       xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <object id="1" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0" />
          <vertex x="10" y="0" z="0" />
          <vertex x="5" y="10" z="0" />
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2" />
        </triangles>
      </mesh>
    </object>
  </resources>
  <build><item objectid="1" /></build>
</model>`;

function makeTestZip(extras?: Record<string, Uint8Array>): ArrayBuffer {
  const enc = new TextEncoder();
  const data: Record<string, Uint8Array> = {
    "[Content_Types].xml": enc.encode("<Types/>"),
    "_rels/.rels": enc.encode("<Relationships/>"),
    "3D/3dmodel.model": enc.encode(MINIMAL_MODEL_XML),
    ...extras,
  };
  return zipSync(data).buffer;
}

describe("import3mf", () => {
  it("extracts mesh positions and indices from a single-object 3MF", () => {
    const result = import3mf(makeTestZip());
    expect(result.meshes).toHaveLength(1);
    const m = result.meshes[0];
    expect(m.positions).toBeInstanceOf(Float32Array);
    expect(m.positions.length).toBe(9);
    expect(m.positions[0]).toBe(0);
    expect(m.positions[3]).toBe(10);
    expect(m.positions[7]).toBe(10);
    expect(m.indices).toBeInstanceOf(Uint32Array);
    expect(Array.from(m.indices)).toEqual([0, 1, 2]);
  });

  it("stashes non-model entries for roundtrip", () => {
    const enc = new TextEncoder();
    const extras = {
      "Metadata/plate_1.config": enc.encode("speed=60\ntemp=210"),
      "Metadata/model_settings.config": enc.encode("infill=20%"),
    };
    const result = import3mf(makeTestZip(extras));
    expect(Object.keys(result.stash.entries)).toHaveLength(2);
    expect(result.stash.entries["Metadata/plate_1.config"]).toBeDefined();
    expect(new TextDecoder().decode(result.stash.entries["Metadata/plate_1.config"])).toBe(
      "speed=60\ntemp=210",
    );
  });

  it("throws when model XML is missing", () => {
    const enc = new TextEncoder();
    const data: Record<string, Uint8Array> = {
      "[Content_Types].xml": enc.encode("<Types/>"),
    };
    const buf = zipSync(data).buffer;
    expect(() => import3mf(buf)).toThrow("3MF archive missing 3D/3dmodel.model");
  });
});

describe("export3mfRoundtrip", () => {
  it("produces a valid ZIP with model XML and stashed entries", () => {
    const enc = new TextEncoder();
    const modelBytes = enc.encode(MINIMAL_MODEL_XML);
    const stash = {
      entries: {
        "Metadata/print_settings.config": enc.encode("layer_height=0.2"),
      },
    };

    const zip = export3mfRoundtrip(modelBytes, stash);
    const reimported = import3mf(zip.buffer);

    expect(reimported.meshes).toHaveLength(1);
    expect(reimported.meshes[0].positions.length).toBe(9);
    expect(reimported.stash.entries["Metadata/print_settings.config"]).toBeDefined();
    expect(
      new TextDecoder().decode(reimported.stash.entries["Metadata/print_settings.config"]),
    ).toBe("layer_height=0.2");
  });
});

describe("extractModelXml", () => {
  it("extracts 3D/3dmodel.model from a ZIP", () => {
    const enc = new TextEncoder();
    const data: Record<string, Uint8Array> = {
      "3D/3dmodel.model": enc.encode("<model/>"),
      "other.txt": enc.encode("hello"),
    };
    const zip = zipSync(data);
    const xml = extractModelXml(zip);
    expect(new TextDecoder().decode(xml)).toBe("<model/>");
  });
});
