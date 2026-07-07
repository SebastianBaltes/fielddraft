// parseBinaryStl — the STL→preview-buffer parser of the server mesh route.

import { describe, expect, it } from "vitest";
import { parseBinaryStl } from "./serverMesh";

/** Build a binary STL with the given triangles (xyz per vertex). */
function makeStl(tris: number[][][]): Uint8Array {
  const bytes = new Uint8Array(84 + tris.length * 50);
  const view = new DataView(bytes.buffer);
  view.setUint32(80, tris.length, true);
  tris.forEach((tri, t) => {
    let off = 84 + t * 50 + 12; // normal stays zero
    for (const v of tri) {
      for (const c of v) {
        view.setFloat32(off, c, true);
        off += 4;
      }
    }
  });
  return bytes;
}

describe("parseBinaryStl", () => {
  it("extracts positions and trivial indices", () => {
    const stl = makeStl([
      [
        [0, 0, 0],
        [1, 0, 0],
        [0, 1, 0],
      ],
      [
        [1, 0, 0],
        [1, 1, 0],
        [0, 1, 0.5],
      ],
    ]);
    const { positions, indices } = parseBinaryStl(stl);
    expect(positions).toHaveLength(18);
    expect(indices).toEqual(new Uint32Array([0, 1, 2, 3, 4, 5]));
    expect(Array.from(positions.slice(0, 9))).toEqual([
      0, 0, 0, 1, 0, 0, 0, 1, 0,
    ]);
    expect(positions[17]).toBeCloseTo(0.5);
  });

  it("rejects truncated files", () => {
    expect(() => parseBinaryStl(new Uint8Array(10))).toThrow("too short");
    const stl = makeStl([
      [
        [0, 0, 0],
        [1, 0, 0],
        [0, 1, 0],
      ],
    ]);
    expect(() => parseBinaryStl(stl.slice(0, 100))).toThrow("truncated");
  });

  it("handles an STL with a non-zero buffer offset", () => {
    const stl = makeStl([
      [
        [2, 0, 0],
        [0, 2, 0],
        [0, 0, 2],
      ],
    ]);
    // Embed at offset 3 in a larger buffer — parse must honor byteOffset.
    const padded = new Uint8Array(stl.length + 3);
    padded.set(stl, 3);
    const view = padded.subarray(3);
    const { positions } = parseBinaryStl(view);
    expect(positions[0]).toBe(2);
    expect(positions[8]).toBe(2);
  });
});
