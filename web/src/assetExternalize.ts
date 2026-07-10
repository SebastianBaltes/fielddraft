/**
 * assetExternalize — pure document ⇄ blob transforms (Phase 1a).
 *
 * IO-free companions to {@link ./assetStore}. They move large binary payloads
 * out of the persisted kernel document (and back), so the document stays
 * KB-small (docs/PROJECT-ASSET-STORAGE-PLAN §5.2). `externalizeAssets` returns
 * the lean document plus a hash→bytes map; `inlineAssets` takes a blob
 * resolver. Wiring them into serialize/load/autosave is P1b.
 *
 * ── What gets externalized ────────────────────────────────────────────────
 *  1. **imported_mesh vertex data.** In the kernel document an `imported_mesh`
 *     feature call carries its geometry inline in `params`
 *     (crates/kernel/src/features/imported_mesh.rs, crates/kernel/src/api/
 *     features.rs `add_imported_mesh`):
 *       - v2 (current): `__positions` (number[]) + `__indices` (number[])
 *       - v1 (legacy):  `__mesh` = { tris:number[], bboxes:number[],
 *                        aabb_min:[3], aabb_max:[3] } (a serialized MeshSdf)
 *  2. **Fonts** embedded as base64 under `__fontData` (string).
 *     NOTE: today the kernel document does NOT carry fonts — uploaded font
 *     bytes live in a JS-side cache and ride in the .fdraft ZIP separately
 *     (sketchTextFont.ts / project.ts `collectProjectAssets`). `__fontData`
 *     is the agreed embedding convention that lets P1b route ALL binaries
 *     through one content-addressed path. See the report for the P1b hook.
 *
 * Each externalized site loses its inline payload and gains an asset ref:
 *     { __asset: "<sha256hex>", __assetKind: "mesh" | "meshLegacy" | "font" }
 * The `__assetKind` marker lets {@link inlineAssets} rebuild the right inline
 * shape without inspecting blob bytes first.
 *
 * Round-trip guarantee: `inlineAssets(externalizeAssets(doc).doc, resolve)` is
 * structurally equal to `doc` (unit-tested). Floats round-trip exactly because
 * mesh values are already f32 (kernel-emitted), which is lossless through the
 * Float32 blob layout below.
 *
 * ── Blob byte layout (little-endian, deterministic ⇒ stable hash / dedup) ──
 * Little-endianness is enforced regardless of host, so the same mesh hashes
 * identically on every machine (cross-machine server dedup, P2).
 *
 *   MESH v2  ("mesh")       — magic "FDM1"
 *     [0..4)   4 bytes  ASCII "FDM1"
 *     [4..8)   u32      posLen  (float count in positions = 3·vertexCount)
 *     [8..12)  u32      idxLen  (uint count in indices    = 3·triCount)
 *     [12..)   f32×posLen   positions
 *     [.. ..)  u32×idxLen   indices
 *
 *   MESH v1  ("meshLegacy") — magic "FDL1"  (serialized MeshSdf)
 *     [0..4)   4 bytes  ASCII "FDL1"
 *     [4..8)   u32      trisLen    (float count in tris   = 9·triCount)
 *     [8..12)  u32      bboxesLen  (float count in bboxes = 6·triCount)
 *     [12..36) f32×6    aabb_min[3] then aabb_max[3]
 *     [36..)   f32×trisLen    tris
 *     [.. ..)  f32×bboxesLen  bboxes
 *
 *   FONT ("font") — raw file bytes, no header (plan §5.1: "hash über die
 *     Datei-Bytes"), so a font's blob hash matches the same bytes anywhere.
 */

import { sha256Hex } from "./assetStore";

type AssetKind = "mesh" | "meshLegacy" | "font";

const MAGIC_MESH = "FDM1";
const MAGIC_LEGACY = "FDL1";

/** Keys that carry a raw binary payload — never recursed into during the walk
 *  (a million-element `__positions` array must not be traversed element-wise). */
const PAYLOAD_KEYS = new Set([
  "__positions",
  "__indices",
  "__mesh",
  "__fontData",
  "__asset",
]);

type JsonObject = Record<string, unknown>;

function isObject(v: unknown): v is JsonObject {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * Externalize every inline binary asset in `docJson`. Returns the lean
 * document (payloads replaced by `{ __asset, __assetKind }` refs) plus a
 * hash→bytes map. Identical payloads collapse to one map entry (dedup).
 */
export async function externalizeAssets(
  docJson: string,
): Promise<{ doc: string; blobs: Map<string, Uint8Array> }> {
  const doc: unknown = JSON.parse(docJson);
  const blobs = new Map<string, Uint8Array>();

  // Collect sites synchronously (no mutation mid-walk), then hash + patch.
  const sites: { node: JsonObject; bytes: Uint8Array; kind: AssetKind }[] = [];
  walk(doc, (node) => {
    if (Array.isArray(node.__positions) && Array.isArray(node.__indices)) {
      sites.push({
        node,
        bytes: encodeMeshV2(
          node.__positions as number[],
          node.__indices as number[],
        ),
        kind: "mesh",
      });
    } else if (isObject(node.__mesh)) {
      sites.push({ node, bytes: encodeMeshLegacy(node.__mesh), kind: "meshLegacy" });
    }
    if (typeof node.__fontData === "string") {
      sites.push({ node, bytes: base64ToBytes(node.__fontData), kind: "font" });
    }
  });

  for (const { node, bytes, kind } of sites) {
    const hash = await sha256Hex(bytes);
    if (!blobs.has(hash)) blobs.set(hash, bytes);
    // Drop the inline payload; leave sibling metadata (e.g. __watertight, op).
    if (kind === "mesh" || kind === "meshLegacy") {
      delete node.__positions;
      delete node.__indices;
      delete node.__mesh;
    } else {
      delete node.__fontData;
    }
    node.__asset = hash;
    node.__assetKind = kind;
  }

  return { doc: JSON.stringify(doc), blobs };
}

/**
 * Inverse of {@link externalizeAssets}: rebuild inline payloads from blobs so
 * the result feeds `kernel.loadJson` directly. Missing blobs (resolver returns
 * `null`) are left as refs with a console warning — never a crash.
 */
export async function inlineAssets(
  docJson: string,
  resolveBlob: (hash: string) => Promise<Uint8Array | null>,
): Promise<string> {
  const doc: unknown = JSON.parse(docJson);

  const sites: { node: JsonObject; kind: AssetKind; hash: string }[] = [];
  walk(doc, (node) => {
    if (typeof node.__asset === "string" && typeof node.__assetKind === "string") {
      sites.push({
        node,
        kind: node.__assetKind as AssetKind,
        hash: node.__asset,
      });
    }
  });

  for (const { node, kind, hash } of sites) {
    const bytes = await resolveBlob(hash);
    if (!bytes) {
      console.warn(
        `fielddraft: asset blob missing (${hash}) — leaving __asset ref inline`,
      );
      continue;
    }
    if (kind === "mesh") {
      const { positions, indices } = decodeMeshV2(bytes);
      node.__positions = positions;
      node.__indices = indices;
    } else if (kind === "meshLegacy") {
      node.__mesh = decodeMeshLegacy(bytes);
    } else if (kind === "font") {
      node.__fontData = bytesToBase64(bytes);
    } else {
      console.warn(`fielddraft: unknown __assetKind "${kind}" — leaving ref`);
      continue;
    }
    delete node.__asset;
    delete node.__assetKind;
  }

  return JSON.stringify(doc);
}

// ── tree walk ────────────────────────────────────────────────────────────

/** Depth-first visit of every object node in a parsed JSON value. Skips
 *  recursion into binary payload keys (perf). */
function walk(value: unknown, visit: (node: JsonObject) => void): void {
  if (Array.isArray(value)) {
    for (const v of value) walk(v, visit);
    return;
  }
  if (isObject(value)) {
    visit(value);
    for (const [k, v] of Object.entries(value)) {
      if (PAYLOAD_KEYS.has(k)) continue;
      walk(v, visit);
    }
  }
}

// ── binary (de)serialization ───────────────────────────────────────────────
//
// Host-endianness: typed-array views are host-endian, so the numeric writes go
// through DataView (always little-endian) for a deterministic, cross-machine
// hash. A fast bulk-copy path is taken on the (universal) little-endian host.

const HOST_LE = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

function writeMagic(dv: DataView, offset: number, magic: string): void {
  for (let i = 0; i < 4; i++) dv.setUint8(offset + i, magic.charCodeAt(i));
}

function checkMagic(dv: DataView, magic: string): void {
  for (let i = 0; i < 4; i++) {
    if (dv.getUint8(i) !== magic.charCodeAt(i)) {
      throw new Error(`fielddraft: bad asset blob magic (expected ${magic})`);
    }
  }
}

/** Write f32 array at `offset`; returns the new offset. `offset` must be
 *  4-byte aligned within the buffer for the fast path (it always is here). */
function packF32(buf: ArrayBuffer, offset: number, arr: ArrayLike<number>): number {
  if (HOST_LE) {
    new Float32Array(buf, offset, arr.length).set(arr);
  } else {
    const dv = new DataView(buf);
    for (let i = 0; i < arr.length; i++) dv.setFloat32(offset + i * 4, arr[i], true);
  }
  return offset + arr.length * 4;
}

function packU32(buf: ArrayBuffer, offset: number, arr: ArrayLike<number>): number {
  if (HOST_LE) {
    new Uint32Array(buf, offset, arr.length).set(arr);
  } else {
    const dv = new DataView(buf);
    for (let i = 0; i < arr.length; i++) dv.setUint32(offset + i * 4, arr[i], true);
  }
  return offset + arr.length * 4;
}

function readF32(buf: ArrayBuffer, offset: number, len: number): number[] {
  if (HOST_LE) return Array.from(new Float32Array(buf, offset, len));
  const dv = new DataView(buf);
  const out = new Array<number>(len);
  for (let i = 0; i < len; i++) out[i] = dv.getFloat32(offset + i * 4, true);
  return out;
}

function readU32(buf: ArrayBuffer, offset: number, len: number): number[] {
  if (HOST_LE) return Array.from(new Uint32Array(buf, offset, len));
  const dv = new DataView(buf);
  const out = new Array<number>(len);
  for (let i = 0; i < len; i++) out[i] = dv.getUint32(offset + i * 4, true);
  return out;
}

/** A fresh, 0-offset ArrayBuffer copy so typed-array views are word-aligned
 *  (an incoming blob may be a view with a non-aligned byteOffset). */
function alignedBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.slice().buffer;
}

function encodeMeshV2(positions: number[], indices: number[]): Uint8Array {
  const buf = new ArrayBuffer(12 + positions.length * 4 + indices.length * 4);
  const dv = new DataView(buf);
  writeMagic(dv, 0, MAGIC_MESH);
  dv.setUint32(4, positions.length, true);
  dv.setUint32(8, indices.length, true);
  let off = packF32(buf, 12, positions);
  packU32(buf, off, indices);
  return new Uint8Array(buf);
}

function decodeMeshV2(bytes: Uint8Array): { positions: number[]; indices: number[] } {
  const buf = alignedBuffer(bytes);
  const dv = new DataView(buf);
  checkMagic(dv, MAGIC_MESH);
  const posLen = dv.getUint32(4, true);
  const idxLen = dv.getUint32(8, true);
  const positions = readF32(buf, 12, posLen);
  const indices = readU32(buf, 12 + posLen * 4, idxLen);
  return { positions, indices };
}

function encodeMeshLegacy(mesh: JsonObject): Uint8Array {
  const tris = (mesh.tris as number[]) ?? [];
  const bboxes = (mesh.bboxes as number[]) ?? [];
  const aabbMin = (mesh.aabb_min as number[]) ?? [0, 0, 0];
  const aabbMax = (mesh.aabb_max as number[]) ?? [0, 0, 0];
  const buf = new ArrayBuffer(12 + 24 + tris.length * 4 + bboxes.length * 4);
  const dv = new DataView(buf);
  writeMagic(dv, 0, MAGIC_LEGACY);
  dv.setUint32(4, tris.length, true);
  dv.setUint32(8, bboxes.length, true);
  let off = packF32(buf, 12, [aabbMin[0], aabbMin[1], aabbMin[2], aabbMax[0], aabbMax[1], aabbMax[2]]);
  off = packF32(buf, off, tris);
  packF32(buf, off, bboxes);
  return new Uint8Array(buf);
}

function decodeMeshLegacy(bytes: Uint8Array): JsonObject {
  const buf = alignedBuffer(bytes);
  const dv = new DataView(buf);
  checkMagic(dv, MAGIC_LEGACY);
  const trisLen = dv.getUint32(4, true);
  const bboxesLen = dv.getUint32(8, true);
  const aabb = readF32(buf, 12, 6);
  const tris = readF32(buf, 36, trisLen);
  const bboxes = readF32(buf, 36 + trisLen * 4, bboxesLen);
  return {
    tris,
    bboxes,
    aabb_min: [aabb[0], aabb[1], aabb[2]],
    aabb_max: [aabb[3], aabb[4], aabb[5]],
  };
}

// ── base64 (font payloads) ──────────────────────────────────────────────────
// Self-contained (no dependency on the heavy reportTicket.ts module) so this
// transform stays framework-free. Chunked to dodge the arg-count limit of
// `String.fromCharCode(...spread)` on large buffers.

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}
