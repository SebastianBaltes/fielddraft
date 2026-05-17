/**
 * 3MF roundtrip: import a .3mf file preserving slicer metadata,
 * then re-export with FieldDraft's modified geometry while keeping
 * all original settings (print profiles, filament, AMS, plate layout).
 *
 * Uses fflate for ZIP handling (STORE + DEFLATE).
 */

import { unzipSync, zipSync, type Zippable } from "fflate";

export interface ThreeMfMesh {
  positions: Float32Array;
  indices: Uint32Array;
}

export interface ThreeMfImportResult {
  meshes: ThreeMfMesh[];
  stash: ThreeMfStash;
}

export interface ThreeMfStash {
  entries: Record<string, Uint8Array>;
}

const MODEL_PATH = "3D/3dmodel.model";
const CONTENT_TYPES_PATH = "[Content_Types].xml";
const RELS_PATH = "_rels/.rels";

export function import3mf(data: ArrayBuffer): ThreeMfImportResult {
  const files = unzipSync(new Uint8Array(data));

  let modelXml: string | null = null;
  const stashEntries: Record<string, Uint8Array> = {};

  for (const [path, content] of Object.entries(files)) {
    const normalized = path.replace(/\\/g, "/");
    if (normalized === MODEL_PATH) {
      modelXml = new TextDecoder().decode(content);
    } else if (normalized !== CONTENT_TYPES_PATH && normalized !== RELS_PATH) {
      stashEntries[normalized] = content;
    }
  }

  if (!modelXml) {
    throw new Error("3MF archive missing 3D/3dmodel.model");
  }

  const meshes = parseModelXml(modelXml);
  return { meshes, stash: { entries: stashEntries } };
}

function parseModelXml(xml: string): ThreeMfMesh[] {
  const parser = new DOMParser();
  const doc = parser.parseFromString(xml, "application/xml");
  const ns = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

  const objects = doc.getElementsByTagNameNS(ns, "object");
  const meshes: ThreeMfMesh[] = [];

  for (let oi = 0; oi < objects.length; oi++) {
    const obj = objects[oi];
    const meshEl = obj.getElementsByTagNameNS(ns, "mesh")[0];
    if (!meshEl) continue;

    const verticesEl = meshEl.getElementsByTagNameNS(ns, "vertices")[0];
    const trianglesEl = meshEl.getElementsByTagNameNS(ns, "triangles")[0];
    if (!verticesEl || !trianglesEl) continue;

    const vertexEls = verticesEl.getElementsByTagNameNS(ns, "vertex");
    const triEls = trianglesEl.getElementsByTagNameNS(ns, "triangle");

    const positions = new Float32Array(vertexEls.length * 3);
    for (let i = 0; i < vertexEls.length; i++) {
      const v = vertexEls[i];
      positions[i * 3] = parseFloat(v.getAttribute("x") ?? "0");
      positions[i * 3 + 1] = parseFloat(v.getAttribute("y") ?? "0");
      positions[i * 3 + 2] = parseFloat(v.getAttribute("z") ?? "0");
    }

    const indices = new Uint32Array(triEls.length * 3);
    for (let i = 0; i < triEls.length; i++) {
      const t = triEls[i];
      indices[i * 3] = parseInt(t.getAttribute("v1") ?? "0", 10);
      indices[i * 3 + 1] = parseInt(t.getAttribute("v2") ?? "0", 10);
      indices[i * 3 + 2] = parseInt(t.getAttribute("v3") ?? "0", 10);
    }

    meshes.push({ positions, indices });
  }

  return meshes;
}

export function extractModelXml(zipBytes: Uint8Array): Uint8Array {
  const files = unzipSync(zipBytes);
  return files["3D/3dmodel.model"] ?? new Uint8Array(0);
}

export function export3mfRoundtrip(
  modelXmlBytes: Uint8Array,
  stash: ThreeMfStash,
): Uint8Array {
  const contentTypes = new TextEncoder().encode(
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
      '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">\n' +
      '  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml" />\n' +
      '  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml" />\n' +
      "</Types>\n",
  );

  const rels = new TextEncoder().encode(
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
      '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\n' +
      '  <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" />\n' +
      "</Relationships>\n",
  );

  const zipData: Zippable = {
    [CONTENT_TYPES_PATH]: contentTypes,
    [RELS_PATH]: rels,
    [MODEL_PATH]: modelXmlBytes,
  };

  for (const [path, content] of Object.entries(stash.entries)) {
    zipData[path] = content;
  }

  return zipSync(zipData);
}
