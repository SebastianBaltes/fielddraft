/**
 * K9.2: GLB/glTF → one merged mesh.
 *
 * Parses a GLB/glTF buffer with Three.js' GLTFLoader (lazy-imported —
 * it's only needed on an actual import) and concatenates every mesh
 * geometry in the scene into a single positions/normals/indices set.
 * Vertex indices are rebased per mesh so the merged index buffer stays
 * valid. Returns `null` when the scene contains no usable mesh.
 */

export interface ImportedMeshBuffers {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
}

export async function parseGlbToMesh(
  buffer: ArrayBuffer,
): Promise<ImportedMeshBuffers | null> {
  const t0 = performance.now();
  console.log(
    `fielddraft: importing GLB reference — ${(buffer.byteLength / 1024).toFixed(0)} KB`,
  );
  const { GLTFLoader } =
    await import("three/examples/jsm/loaders/GLTFLoader.js");
  const loader = new GLTFLoader();
  const gltf = await new Promise<{
    scene: { traverse: (cb: (child: unknown) => void) => void };
  }>((resolve, reject) => {
    loader.parse(buffer, "", resolve as never, reject);
  });

  type GeoLike = {
    getAttribute: (
      n: string,
    ) => { array: ArrayLike<number>; count: number } | null;
    getIndex: () => { array: ArrayLike<number> } | null;
  };
  const allPos: Float32Array[] = [];
  const allNor: Float32Array[] = [];
  const allIdx: Uint32Array[] = [];
  let vertexOffset = 0;

  gltf.scene.traverse((child: unknown) => {
    const mesh = child as { isMesh?: boolean; geometry?: GeoLike };
    if (!mesh.isMesh || !mesh.geometry) return;
    const posAttr = mesh.geometry.getAttribute("position");
    if (!posAttr || posAttr.count === 0) return;

    const pos = new Float32Array(posAttr.array as ArrayLike<number>);
    const normalAttr = mesh.geometry.getAttribute("normal");
    const nor = normalAttr
      ? new Float32Array(normalAttr.array as ArrayLike<number>)
      : new Float32Array(pos.length);
    const indexAttr = mesh.geometry.getIndex();
    const rawIdx = indexAttr
      ? new Uint32Array(indexAttr.array as ArrayLike<number>)
      : new Uint32Array(Array.from({ length: posAttr.count }, (_, i) => i));

    if (vertexOffset > 0) {
      const offset = new Uint32Array(rawIdx.length);
      for (let i = 0; i < rawIdx.length; i++)
        offset[i] = rawIdx[i] + vertexOffset;
      allIdx.push(offset);
    } else {
      allIdx.push(rawIdx);
    }
    allPos.push(pos);
    allNor.push(nor);
    vertexOffset += posAttr.count;
  });

  if (allPos.length === 0) return null;

  const totalPosLen = allPos.reduce((s, a) => s + a.length, 0);
  const totalIdxLen = allIdx.reduce((s, a) => s + a.length, 0);
  const positions = new Float32Array(totalPosLen);
  const normals = new Float32Array(totalPosLen);
  const indices = new Uint32Array(totalIdxLen);
  let pOff = 0;
  for (let i = 0; i < allPos.length; i++) {
    positions.set(allPos[i], pOff);
    normals.set(allNor[i], pOff);
    pOff += allPos[i].length;
  }
  let iOff = 0;
  for (const idx of allIdx) {
    indices.set(idx, iOff);
    iOff += idx.length;
  }

  console.log(
    `fielddraft: GLB parsed — ${allPos.length} mesh(es), ${vertexOffset} vertices in ${(performance.now() - t0).toFixed(0)} ms`,
  );
  return { positions, normals, indices };
}
