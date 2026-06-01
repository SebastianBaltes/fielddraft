export type MeshOverlayData = {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
};

export function canTriggerDownload(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof document !== "undefined" &&
    typeof URL !== "undefined" &&
    typeof URL.createObjectURL === "function"
  );
}

export function triggerDownload(
  data: BlobPart,
  mimeType: string,
  filename: string,
): void {
  const blob = new Blob([data], { type: mimeType });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

export function parseStlBinary(buffer: ArrayBuffer): MeshOverlayData | null {
  console.log(
    `fielddraft: importing STL — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
  );

  const view = new DataView(buffer);
  if (buffer.byteLength < 84) {
    console.error(
      "fielddraft: STL import failed — file too small (< 84 bytes), possibly not a valid STL",
    );
    return null;
  }
  const triCount = view.getUint32(80, true);
  const expected = 84 + triCount * 50;
  if (buffer.byteLength < expected) {
    console.error(
      `fielddraft: STL import failed — header says ${triCount} triangles (needs ${expected} bytes) but file is ${buffer.byteLength} bytes. Possibly ASCII STL (not supported) or corrupt file.`,
    );
    return null;
  }

  console.log(`fielddraft: parsing ${triCount} triangles…`);
  const positions = new Float32Array(triCount * 9);
  const normals = new Float32Array(triCount * 9);
  const indices = new Uint32Array(triCount * 3);

  let offset = 84;
  for (let t = 0; t < triCount; t++) {
    const nx = view.getFloat32(offset, true);
    const ny = view.getFloat32(offset + 4, true);
    const nz = view.getFloat32(offset + 8, true);
    offset += 12;
    for (let v = 0; v < 3; v++) {
      const base = t * 9 + v * 3;
      positions[base] = view.getFloat32(offset, true);
      positions[base + 1] = view.getFloat32(offset + 4, true);
      positions[base + 2] = view.getFloat32(offset + 8, true);
      normals[base] = nx;
      normals[base + 1] = ny;
      normals[base + 2] = nz;
      indices[t * 3 + v] = t * 3 + v;
      offset += 12;
    }
    offset += 2;
  }

  return { positions, normals, indices };
}

export function parseObjText(text: string): MeshOverlayData | null {
  const t0 = performance.now();
  console.log(
    `fielddraft: importing OBJ reference — ${(text.length / 1024).toFixed(0)} KB`,
  );
  const verts: number[] = [];
  const positions: number[] = [];
  const normals: number[] = [];

  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("v ")) {
      const parts = line.split(/\s+/);
      verts.push(
        parseFloat(parts[1]),
        parseFloat(parts[2]),
        parseFloat(parts[3]),
      );
    } else if (line.startsWith("f ")) {
      const parts = line.split(/\s+/).slice(1);
      const vertexCount = verts.length / 3;
      const idxs = parts.map((p) => {
        const raw = parseInt(p.split("/")[0], 10);
        // OBJ allows negative indices that count back from the most recently
        // seen vertex (-1 = last). Resolve those relative to the current count.
        return raw < 0 ? vertexCount + raw : raw - 1;
      });
      for (let i = 1; i < idxs.length - 1; i++) {
        for (const idx of [idxs[0], idxs[i], idxs[i + 1]]) {
          positions.push(
            verts[idx * 3] ?? 0,
            verts[idx * 3 + 1] ?? 0,
            verts[idx * 3 + 2] ?? 0,
          );
          normals.push(0, 0, 0);
        }
      }
    }
  }

  if (positions.length === 0) {
    console.error("fielddraft: OBJ import failed — no geometry found");
    return null;
  }

  console.log(
    `fielddraft: OBJ parsed — ${verts.length / 3} vertices, ${positions.length / 9} triangles`,
  );
  for (let i = 0; i < positions.length; i += 9) {
    const ax = positions[i + 3] - positions[i];
    const ay = positions[i + 4] - positions[i + 1];
    const az = positions[i + 5] - positions[i + 2];
    const bx = positions[i + 6] - positions[i];
    const by = positions[i + 7] - positions[i + 1];
    const bz = positions[i + 8] - positions[i + 2];
    let nx = ay * bz - az * by;
    let ny = az * bx - ax * bz;
    let nz = ax * by - ay * bx;
    const len = Math.hypot(nx, ny, nz) || 1;
    nx /= len;
    ny /= len;
    nz /= len;
    for (let v = 0; v < 3; v++) {
      normals[i + v * 3] = nx;
      normals[i + v * 3 + 1] = ny;
      normals[i + v * 3 + 2] = nz;
    }
  }

  const posArr = new Float32Array(positions);
  const normArr = new Float32Array(normals);
  const idxArr = new Uint32Array(
    Array.from({ length: positions.length / 3 }, (_, i) => i),
  );

  console.log(
    `fielddraft: OBJ parsed — ${positions.length / 9} triangles in ${(performance.now() - t0).toFixed(0)} ms`,
  );
  return { positions: posArr, normals: normArr, indices: idxArr };
}

export function buildObjExportText(
  bodies: readonly { positions: Float32Array; normals: Float32Array; indices: Uint32Array }[],
): string {
  const lines: string[] = ["# FieldDraft OBJ export"];
  let vertexOffset = 0;
  for (const body of bodies) {
    if (body.positions.length === 0) continue;
    const vertCount = body.positions.length / 3;
    for (let i = 0; i < body.positions.length; i += 3) {
      lines.push(
        `v ${body.positions[i].toFixed(6)} ${body.positions[i + 1].toFixed(6)} ${body.positions[i + 2].toFixed(6)}`,
      );
    }
    for (let i = 0; i < body.normals.length; i += 3) {
      lines.push(
        `vn ${body.normals[i].toFixed(6)} ${body.normals[i + 1].toFixed(6)} ${body.normals[i + 2].toFixed(6)}`,
      );
    }
    for (let i = 0; i < body.indices.length; i += 3) {
      const a = body.indices[i] + 1 + vertexOffset;
      const b = body.indices[i + 1] + 1 + vertexOffset;
      const c = body.indices[i + 2] + 1 + vertexOffset;
      lines.push(`f ${a}//${a} ${b}//${b} ${c}//${c}`);
    }
    vertexOffset += vertCount;
  }
  return lines.join("\n") + "\n";
}
