/**
 * Thin TypeScript wrapper around the Rust WASM kernel.
 *
 * The central concern here is the **zero-copy memory bridge**: the kernel
 * owns a packed `Vec<f32>` of feature parameters in WASM linear memory;
 * JS reads it through a `Float32Array` view that aliases that same memory,
 * with no intermediate JSON serialization. The view must be re-acquired
 * whenever the underlying `Vec` could have been reallocated (every
 * mutating call), since WASM memory growth invalidates old views.
 */

// eslint-disable-next-line import/no-unresolved
import initWasm, {
  Kernel as WasmKernel,
  wasmVersion,
} from "./wasm/fielddraft_kernel.js";
import type { FieldSkriptRecord } from "./fieldskript/record";
import { installMainScriptHost, syncMainScriptHost } from "./mainScriptHost";
import { scriptRecordsFrom } from "./workerScriptHost";

interface WasmExports {
  memory: WebAssembly.Memory;
}

/** Number of contributor id slots per mesh vertex. Mirrors
 *  `csg::MAX_CONTRIBUTORS` on the Rust side. */
export const MESH_CONTRIBUTOR_STRIDE = 4;

/** Sentinel written into unused contributor slots (= `u32::MAX`). */
export const CONTRIBUTOR_SENTINEL = 0xffffffff;

/** B-Rep proxy element classification returned by `pick()` and stored
 *  per mesh vertex. */
export type ProxyElement = "face" | "edge" | "vertex";

/** What a document variable represents (ticket 8ar9). Drives its display
 *  unit (mm / ° / none) and the type tab in the variables panel. */
export type VarKind = "number" | "length" | "angle";

/** A document-level variable as listed by {@link Kernel.listVariables}. */
export interface VariableEntry {
  name: string;
  value: number;
  kind: VarKind;
  /** The raw source text the value was resolved from (e.g. `"10 cm"` or
   *  `"sin(Breite) * 2"`), or undefined for a plain literal whose display
   *  equals `value`. Drives what the variables panel shows in its input. */
  formula?: string;
}

export interface PickHit {
  hit: true;
  point: [number, number, number];
  normal: [number, number, number];
  t: number;
  element: ProxyElement;
  /** Flat root *feature* ids of the colliding branches — what selection,
   *  query-picking and edge fillets map back to features in the tree. */
  contributors: number[];
  /** Packed BranchIds (feature << SUB_BITS | sub) of the same branches, in
   *  the same order. Used by the analytic B-Rep edge overlay to match a
   *  hovered edge by its colliding-face signature. */
  branches: number[];
}
export interface PickMiss {
  hit: false;
}
export type PickResult = PickHit | PickMiss;

/** Sentinel written into a mesh triangle's face tag when no B-Rep face could
 *  be correlated (= `u32::MAX`). Mirrors `face_export::FACE_INDEX_SENTINEL`. */
export const FACE_INDEX_SENTINEL = 0xffffffff;

/** Rigid + uniform-scale placement of a body. B-Reps are stored body-LOCAL
 *  (DL-56); world coordinates are obtained by composing this placement back in
 *  via {@link localToWorld}. */
export interface Placement {
  translate: [number, number, number];
  /** Unit quaternion `(x, y, z, w)`. */
  rotation: [number, number, number, number];
  scale: [number, number, number];
}

/** One analytic B-Rep edge of a body: a world-space polyline plus its
 *  canonical colliding-branch signature `{a, b}` (packed BranchIds) and the
 *  stable `TopoRef` edge id as a decimal string. `idHash` becomes available
 *  once the kernel exports it (Brocken B-K); until the wasm is regenerated it
 *  may be absent at runtime, so consumers must treat a missing `idHash` as
 *  "edge not yet selectable". */
export interface BRepEdge {
  a: number;
  b: number;
  /** Dense tessellation polyline (world space, flat `[x,y,z, …]`) — the
   *  wireframe geometry. */
  points: number[];
  /** Sparse **structural** control points (world space, flat `[x,y,z, …]`) for
   *  the debug points overlay: the few points that define the curve. On-curve
   *  nodes sit at every {@link degree}-th control point; the rest are off-curve
   *  Bézier handles. */
  controlPoints: number[];
  /** Curve degree. Clamped-Bézier-chain NURBS report 2 (rational quadratic,
   *  e.g. exact circles) or 3 (generic cubic); polyline/Hermite edges report
   *  1 (every control point is an on-curve node). */
  degree: number;
  idHash: string;
  /** Canonical fingerprint-carrying element query (`qEdge(<hash> @ …)`) —
   *  what pickers should persist so the reference can heal across
   *  topological edits (PERSISTENT-NAMING-PLAN N2). Absent until the wasm
   *  exporting it is regenerated. */
  query?: string;
  /** Arena indices (== `BRepFace.faceIndex`) of the faces this edge bounds.
   *  Face-precise — unlike the `a`/`b` branch signature, which coplanar
   *  split faces share (ticket b91c). */
  faces: number[];
}

/** One resolved edge-fillet zone (debug overlay, #350). The `polyline` is the
 *  world-space edge the fillet rounds; `maskRadius` (= `blendRadius ×
 *  FILLET_MASK_SCALE`) is the radius of the capsule/pill the blend is masked
 *  to — exactly the region that shapes the SDF. */
export interface FilletZone {
  filletId: number | null;
  polyline: [number, number, number][];
  maskRadius: number;
  blendRadius: number;
}

/** One topology-octree leaf cell (debug overlay). `min`/`max` are the cell's
 *  axis-aligned bounds in the **body-local** frame; `class` is 0=Face, 1=Edge,
 *  2=Vertex. Fold to world via the parent {@link OctreeCells} placement. */
export interface OctreeCell {
  min: [number, number, number];
  max: [number, number, number];
  class: number;
}

/** Debug octree dump for one body: its local-frame leaf {@link OctreeCell}s
 *  plus the rigid+uniform-scale placement that folds them to world. */
export interface OctreeCells {
  translate: [number, number, number];
  rotation: [number, number, number, number];
  scale: number;
  cells: OctreeCell[];
}

/** One selectable B-Rep face of a body, in the body-LOCAL frame. `faceIndex`
 *  indexes the same ordering as the per-triangle face tags
 *  ({@link KernelHandle.bodyFaceIndexView}). `idHash` is the
 *  regeneration-stable `TopoRef` id as a decimal string (a `u64` is unsafe as a
 *  JS number). */
export interface BRepFace {
  faceIndex: number;
  idHash: string;
  /** Canonical fingerprint-carrying element query (`qFace(<hash> @ …)`);
   *  absent until the wasm exporting it is regenerated. */
  query?: string;
  kind: "face";
  /** Packed BranchId (feature << SUB_BITS | sub) this face lies on. */
  branchId: number;
  /** Flat feature id (sub stripped). */
  featureId: number;
  surfaceKind:
    "plane" | "cylinder" | "sphere" | "cone" | "torus" | "tessellated";
  /** Body-local analytic surface parameters for the hybrid renderer's
   *  parametric tessellation; absent (`null`) for planar (uses
   *  centroid/normal) and tessellated (gyroid/revolved) faces. The axis of
   *  every analytic primitive is its local +Y; the caller folds the body
   *  placement in, like {@link centroidLocal}/{@link normalLocal}. */
  surfaceParams?: SurfaceParams | null;
  centroidLocal: [number, number, number];
  /** Where a SKETCH on this face has its origin, body-local — the kernel's own
   *  `face_plane_origin_local` (#442). NOT the same point as
   *  {@link centroidLocal} on a face without symmetry: that one is the mean of
   *  the outer loop's edge samples, this one the fingerprint centroid projected
   *  onto the exact carrier plane, and only this one is what the kernel builds
   *  the sketch frame on. Absent until the wasm exporting it is regenerated. */
  planeOriginLocal?: [number, number, number];
  normalLocal: [number, number, number];
}

export type SurfaceParams =
  | {
      kind: "cylinder";
      origin: [number, number, number];
      axis: [number, number, number];
      radius: number;
    }
  | { kind: "sphere"; origin: [number, number, number]; radius: number }
  | {
      kind: "cone";
      origin: [number, number, number];
      axis: [number, number, number];
      halfAngle: number;
    }
  | {
      kind: "torus";
      origin: [number, number, number];
      axis: [number, number, number];
      majorRadius: number;
      minorRadius: number;
    };

/** One selectable B-Rep vertex of a body, in the body-LOCAL frame. `idHash` is
 *  the stable `TopoRef` vertex id as a decimal string. */
export interface BRepVertex {
  vertexIndex: number;
  idHash: string;
  /** Canonical fingerprint-carrying element query (`qVertex(<hash> @ …)`);
   *  absent until the wasm exporting it is regenerated. */
  query?: string;
  kind: "vertex";
  positionLocal: [number, number, number];
  /** Packed BranchIds of the ≥3 branch surfaces colliding at this vertex. */
  branches: number[];
}

/** Compose a body placement back onto a body-local point to get world
 *  coordinates: `world = R·(s·p) + t`. The placement scale is uniform under
 *  the rigid + uniform-scale restriction (DL-56), but this helper applies each
 *  component independently so a (defensively) non-uniform scale still works. */
export function localToWorld(
  p: [number, number, number],
  pl: Placement,
): [number, number, number] {
  // s·p
  const s: [number, number, number] = [
    p[0] * pl.scale[0],
    p[1] * pl.scale[1],
    p[2] * pl.scale[2],
  ];
  // R·(s·p) — quaternion rotation of a vector.
  const [qx, qy, qz, qw] = pl.rotation;
  const tx = 2 * (qy * s[2] - qz * s[1]);
  const ty = 2 * (qz * s[0] - qx * s[2]);
  const tz = 2 * (qx * s[1] - qy * s[0]);
  const r: [number, number, number] = [
    s[0] + qw * tx + qy * tz - qz * ty,
    s[1] + qw * ty + qz * tx - qx * tz,
    s[2] + qw * tz + qx * ty - qy * tx,
  ];
  // + t
  return [
    r[0] + pl.translate[0],
    r[1] + pl.translate[1],
    r[2] + pl.translate[2],
  ];
}

/** Numeric tags for primitive kinds. Mirror `Primitive::KIND_*` in protocol. */
export const PrimitiveKind = {
  Box: 0,
  Sphere: 1,
  Cylinder: 2,
  Torus: 6,
  Thread: 7,
  Cone: 8,
  Capsule: 9,
  HexPrism: 10,
  Ellipsoid: 12,
} as const;
export type PrimitiveKindTag =
  (typeof PrimitiveKind)[keyof typeof PrimitiveKind];

/** Numeric tags for boolean ops (Onshape result-body vocabulary).
 *  Mirror `BoolOp::TAG_*` in protocol: Add merges into the touching
 *  body, New always seeds its own, Remove subtracts, Intersect clips. */
export const BoolOpTag = {
  Add: 0,
  New: 1,
  Remove: 2,
  Intersect: 3,
} as const;
export type BoolOpTagValue = (typeof BoolOpTag)[keyof typeof BoolOpTag];

export type BoolOpName = "add" | "new" | "remove" | "intersect";
export type PrimitiveKindName =
  | "box"
  | "sphere"
  | "cylinder"
  | "torus"
  | "thread"
  | "cone"
  | "capsule"
  | "hex_prism"
  | "ellipsoid"
  | "hole"
  | "linear_pattern"
  | "circular_pattern";
/** K6b.3a: the kernel's `listFeatures` can now return entries
 *  whose kind is a non-primitive feature type like "sketch" or
 *  "extrude". The outliner handles them generically by keying
 *  glyphs / labels off the string. */
export type FeatureKindName =
  | PrimitiveKindName
  | "sketch"
  | "extrude"
  | "revolve"
  | "mirror"
  | "mate_connector"
  | "fillet_edge"
  | "chamfer_edge"
  | "round_all"
  | "shell"
  | "offset"
  | "sweep"
  | "loft"
  | "imported_mesh"
  | "translate"
  | "rotate"
  | "scale"
  | "boolean"
  | "material"
  | "unknown";

export interface TopoFaceData {
  index: number;
  contributor: number;
  area: number;
  centroid: [number, number, number];
  normal: [number, number, number];
  triangleCount: number;
}

export interface TopoEdgeData {
  index: number;
  contributorA: number;
  contributorB: number;
  pointCount: number;
}

export interface ContourData {
  outer: [number, number][];
  holes: [number, number][][];
}

/** A non-rigid domain warp the web mesher applies to a leaf's WORLD-space
 *  geometry (Twist about Y, Bend about Z). Emitted by the kernel on every
 *  descendant leaf of a Twist/Bend node; see `bvh.rs` `LeafWarp`. */
export type LeafWarp =
  { kind: "twist"; rate: number } | { kind: "bend"; rate: number };

/** Spine payload for a parametric `"sweep"` leaf: `buildSweepGeometry` sweeps
 *  the leaf's `contour` along these (local-space) points; `transform` places
 *  it. The web mesher re-derives the same RMF frames the kernel SDF uses. */
export interface SweepLeafData {
  points: [number, number, number][];
  closed: boolean;
  /** Total profile twist (radians), distributed by arclength. */
  twist: number;
}

export interface HybridLeafInfo {
  kind: string;
  aabb: [number, number, number, number, number, number];
  transform: [
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
  ];
  params: number[];
  role: "mesh" | "skip";
  contour?: ContourData;
  warp?: LeafWarp;
  sweep?: SweepLeafData;
}

export interface SdfRegionInfo {
  aabb: [number, number, number, number, number, number];
}

export interface HybridInfo {
  leaves: HybridLeafInfo[];
  sdf_regions: SdfRegionInfo[];
  needs_full_sdf: boolean;
  is_union_only: boolean;
}

export interface FeatureSummary {
  id: number;
  name: string;
  op: BoolOpName;
  kind: FeatureKindName;
  translate: [number, number, number];
  rotation: [number, number, number, number];
  scale: [number, number, number];
  /** K8.7: mirrors the kernel's `FeatureCall.suppressed`
   *  flag. Undefined in JSON when false — upstream
   *  `listFeatures` omits the field via a
   *  `skip_serializing_if` guard, so the TS layer has to
   *  treat the absence as `false`. */
  suppressed?: boolean;
  rolled_back?: boolean;
  /** Regen error of a failing feature (ticket sx5a, Onshape semantics):
   *  the feature stays in the document but contributes no geometry; the
   *  outliner paints it red and shows this message as the tooltip.
   *  Omitted from the JSON when the feature regenerated cleanly. */
  error?: string;
  /** Non-fatal merge-scope warning: a feature-implicit boolean
   *  (Add/Remove/Intersect) with no resolved merge scope. Geometry still
   *  regenerates (the body shows as a translucent red preview); the outliner
   *  paints the row in the warning colour and shows this as the tooltip.
   *  Omitted when the feature has a valid scope. */
  warning?: string;
  /** #384: for an `offset`/`shell` feature, the capability contract of its
   *  regen-time expansion. Omitted for every other feature kind. */
  capability?: FeatureCapability;
}

/** #384 offset_capability: how dimension-true an Offset/Shell result is.
 *  `exact` — fully analytic; `tolerance_bounded` — arc-spline corrected to a
 *  known tolerance; `conservative` — `f − δ` left standing somewhere (unbounded
 *  local deviation); `unsupported` — rejected (Twist/Bend in scope). */
export type CapabilityStatus =
  | "exact"
  | "tolerance_bounded"
  | "conservative"
  | "unsupported";

export interface FeatureCapability {
  status: CapabilityStatus;
  /** Human-readable reasons for a non-exact status (conservative first). */
  reasons?: string[];
}

let wasmInitPromise: Promise<WasmExports> | null = null;

async function ensureInit(): Promise<WasmExports> {
  if (!wasmInitPromise) {
    wasmInitPromise = initWasm() as Promise<WasmExports>;
  }
  return wasmInitPromise;
}

/**
 * High-level kernel handle owned by the React app. Wraps the wasm-bindgen
 * `Kernel` class.
 */
export class Kernel {
  private readonly inner: WasmKernel;
  private readonly memory: WebAssembly.Memory;
  private mutating = false;

  private guardMut<T>(fn: () => T): T {
    if (this.mutating) {
      throw new Error(
        "Kernel: re-entrant mutation rejected (would cause WASM RefCell panic)",
      );
    }
    this.mutating = true;
    try {
      return fn();
    } finally {
      this.mutating = false;
    }
  }

  private constructor(inner: WasmKernel, memory: WebAssembly.Memory) {
    this.inner = inner;
    this.memory = memory;
  }

  static async create(): Promise<Kernel> {
    const exports = await ensureInit();
    if (!exports?.memory) {
      throw new Error("fielddraft kernel: wasm memory export not found");
    }
    const inner = new WasmKernel();
    inner.setMeshResolution(64);
    inner.setContextMeshResolution(32);

    // Stale-WASM guard: the binary is stamped at build time (see build.rs /
    // `just build-wasm`), which also mirrors the stamp into the
    // `.kernel-version` sidecar that `__KERNEL_EXPECTED__` is defined from.
    // Comparing the loaded module's stamp against the on-disk artifact (NOT
    // the live package.json) means a cosmetic version bump without a rebuild
    // never trips this — only a genuinely stale module does, i.e. a long-open
    // tab whose worker booted before the last `just build-wasm`. Warn loudly
    // so it's never silent (e.g. two meshers returning the same triangle
    // count because the new mesher code never loaded).
    const wasmV = wasmVersion();
    if (wasmV !== __KERNEL_EXPECTED__) {
      console.warn(
        `[fielddraft] stale kernel module: loaded v${wasmV}, on-disk kernel is v${__KERNEL_EXPECTED__}. ` +
          `Reload the page (Ctrl+Shift+R). If it persists, rebuild with \`just build-wasm\`.`,
      );
    } else {
      console.info(`[fielddraft] kernel v${wasmV}`);
    }

    // FieldSkript SI-W: install the build host on THIS thread too. The
    // main-thread regenerate (AABB, picking, outliner, undo) walks
    // `ScriptFeature::execute` and would fail with `NoRunner` without it.
    // `build()` is lightweight here (op-emission only); meshing stays in
    // the worker. Safe to install now — wasm init has completed.
    installMainScriptHost();

    return new Kernel(inner, exports.memory);
  }

  /** App version this kernel's WASM binary was stamped with at build time. */
  version(): string {
    return wasmVersion();
  }

  featureCount(): number {
    return this.inner.featureCount();
  }

  // ── mesh bridge (zero-copy) ─────────────────────────────────────────
  //
  // The kernel re-runs the Dual Contouring pass on every mutation and
  // stores positions / normals / indices as three flat `Vec`s. These
  // views alias those `Vec`s directly — **never cache them** across a
  // mutating call, because `Vec::push` can relocate the backing
  // allocation and invalidate the view's buffer/offset pair.

  // ── aggregate counts (sum over bodies) ──────────────────────────────
  //
  // The aggregate pointer buffer from K3 part 1 is gone — the viewport
  // iterates bodies via the per-body views below. These counts stay
  // around as cheap totals for status bars and tests.

  meshVertexCount(): number {
    return this.inner.meshVertexCount();
  }

  meshIndexCount(): number {
    return this.inner.meshIndexCount();
  }

  meshTriangleCount(): number {
    return this.inner.meshTriangleCount();
  }

  meshVersion(): number {
    return this.inner.meshVersion();
  }

  // ── per-body mesh bridge (K3 part 3) ───────────────────────────────
  //
  // Each body owns its own contiguous positions / normals / indices /
  // classifications / contributors buffers inside WASM linear memory.
  // `bodyCount()` changes only when bodies are added, removed or
  // merged; `bodyVersion(i)` bumps only when body `i` was actually
  // re-meshed, so the viewport can rebind exactly the bodies that
  // changed. All views alias the WASM buffers directly — never cache
  // them across a mutating call.

  bodyCount(): number {
    return this.inner.bodyCount();
  }

  bodyId(i: number): number {
    return this.inner.bodyId(i);
  }

  bodySeedFeatureId(i: number): number {
    return this.inner.bodySeedFeatureId(i);
  }

  bodyVersion(i: number): number {
    return this.inner.bodyVersion(i);
  }

  bodyHasTransformDelta(i: number): boolean {
    return this.inner.bodyHasTransformDelta(i);
  }

  bodyTransformDelta(i: number): Float32Array {
    return this.inner.bodyTransformDelta(i);
  }

  bodyVertexCount(i: number): number {
    return this.inner.bodyVertexCount(i);
  }

  bodyIndexCount(i: number): number {
    return this.inner.bodyIndexCount(i);
  }

  bodyTriangleCount(i: number): number {
    return this.inner.bodyTriangleCount(i);
  }

  bodyPositionsView(i: number): Float32Array {
    const ptr = this.inner.bodyPositionsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Float32Array(this.memory.buffer, ptr, count * 3);
  }

  bodyNormalsView(i: number): Float32Array {
    const ptr = this.inner.bodyNormalsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Float32Array(this.memory.buffer, ptr, count * 3);
  }

  bodyIndicesView(i: number): Uint32Array {
    const ptr = this.inner.bodyIndicesPtr(i);
    const count = this.inner.bodyIndexCount(i);
    return new Uint32Array(this.memory.buffer, ptr, count);
  }

  bodyClassificationsView(i: number): Uint8Array {
    const ptr = this.inner.bodyClassificationsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Uint8Array(this.memory.buffer, ptr, count);
  }

  bodyContributorsView(i: number): Uint32Array {
    const ptr = this.inner.bodyContributorsPtr(i);
    const count = this.inner.bodyVertexCount(i);
    return new Uint32Array(
      this.memory.buffer,
      ptr,
      count * MESH_CONTRIBUTOR_STRIDE,
    );
  }

  bodySdfOpcodesView(i: number): Float32Array {
    const ptr = this.inner.bodySdfOpcodesPtr(i);
    const len = this.inner.bodySdfOpcodesLen(i);
    if (!ptr || len === 0) return new Float32Array(0);
    return new Float32Array(this.memory.buffer, ptr, len);
  }

  bodySdfAabb(i: number): Float32Array {
    return new Float32Array(this.inner.bodySdfAabb(i));
  }

  /** Analytic world AABB `[minx,miny,minz, maxx,maxy,maxz]` of the body that
   *  contains `featureId` (the pipeline-maintained tight `Body.aabb`, ticket
   *  f88u), or `[]` when the feature isn't in any body. Feeds the Debug ▸
   *  Part-AABB overlay for the selected part. */
  bodyAabbForFeature(featureId: number): number[] {
    return Array.from(this.inner.bodyAabbForFeature(featureId));
  }

  /** Conservative raymarch step factor (≤1) for this body — the SDF renderer
   *  multiplies its march step by it so a thin-walled revolve/shell isn't
   *  overshot at grazing angles (Ticket 1wnm). 1.0 = full step. */
  bodySdfMarchRelax(i: number): number {
    return this.inner.bodySdfMarchRelax(i);
  }

  /** CPU-mirror eval of the body's compiled opcode stream — the parity
   *  reference for the GPU probe (`window.__fdSdfVmProbe`). */
  bodySdfEval(i: number, x: number, y: number, z: number): number {
    return this.inner.bodySdfEval(i, x, y, z);
  }

  bodyVolumeCount(i: number): number {
    return this.inner.bodyVolumeCount(i);
  }

  bodyVolumeDataView(i: number, volIdx: number): Float32Array {
    const ptr = this.inner.bodyVolumeDataPtr(i, volIdx);
    const len = this.inner.bodyVolumeDataLen(i, volIdx);
    if (!ptr || len === 0) return new Float32Array(0);
    return new Float32Array(this.memory.buffer, ptr, len);
  }

  bodyVolumeDims(i: number, volIdx: number): [number, number, number] {
    const dims = this.inner.bodyVolumeDims(i, volIdx);
    return [dims[0], dims[1], dims[2]];
  }

  bodyHybridInfo(i: number): HybridInfo {
    return JSON.parse(this.inner.bodyHybridInfoJson(i));
  }

  // ── Per-body analytic B-Rep edges ────────────────────────

  /**
   * Analytic B-Rep edges of body `i`, each a world-space polyline plus its
   * canonical colliding-branch signature `{a, b}`. The two ids match the
   * `contributors` array {@link KernelHandle.pick} reports, so a hovered edge
   * can be looked up by signature. Edges come straight from the woven
   * (cached) B-Rep — independent of mesh resolution.
   */
  bodyBrepEdges(i: number): BRepEdge[] {
    return JSON.parse(this.inner.bodyBrepEdges(i)) as BRepEdge[];
  }

  /** Resolved edge-fillet zones (debug, #350): each active `fillet_edge`'s
   *  masked capsule (edge polyline + `maskRadius`). Drawn by the fillet
   *  debug overlay when the fillet detail checkbox is on. */
  filletZones(): FilletZone[] {
    return JSON.parse(this.inner.filletZones()) as FilletZone[];
  }

  /**
   * Stable body ids (see {@link KernelHandle.bodyId}) whose woven B-Rep
   * is NOT current. The store's async weave pipeline polls this after
   * every commit and posts the listed bodies to the weaver worker.
   * Never weaves.
   */
  bodyBrepStaleIds(): number[] {
    return JSON.parse(this.inner.bodyBrepStaleIds()) as number[];
  }

  /**
   * Install a weaver-worker result (MessagePack `(key, BRep)` from the
   * worker kernel's `weaveBodyBrep`). Validated against the CURRENT
   * placement-invariant cache key — a result that raced a newer edit is
   * rejected. Returns whether anything was installed (the body version
   * bumped, overlays should refetch).
   */
  importBodyBrep(bodyId: number, bytes: Uint8Array): boolean {
    return this.inner.importBodyBrep(bodyId, bytes);
  }

  /**
   * **Debug only.** Topology-octree leaf cells of body `i` for the octree-cell
   * overlay — the adaptive grid the B-Rep weave classifies into face/edge/
   * vertex cells. Cells come in the body-LOCAL frame plus the placement that
   * folds them to world. The octree is rebuilt on every call (not cached), so
   * read it only when the overlay is actually on.
   */
  bodyOctreeCells(i: number): OctreeCells {
    return JSON.parse(this.inner.bodyOctreeCells(i)) as OctreeCells;
  }

  // ── Per-body selectable B-Rep faces / vertices (Brocken A) ────────

  /**
   * Selectable B-Rep faces of body `i`, in the body-LOCAL frame. `faceIndex`
   * matches the per-triangle face tags from {@link bodyFaceIndexView}, so a
   * hovered/clicked triangle resolves to one of these. Fold {@link
   * bodyPlacement} in via {@link localToWorld} for world coordinates.
   */
  bodyBrepFaces(i: number): BRepFace[] {
    return JSON.parse(this.inner.bodyBrepFaces(i)) as BRepFace[];
  }

  /**
   * Selectable B-Rep vertices (sharp corners) of body `i`, body-LOCAL.
   */
  bodyBrepVertices(i: number): BRepVertex[] {
    return JSON.parse(this.inner.bodyBrepVertices(i)) as BRepVertex[];
  }

  /**
   * Ticket dhjn: the `qRegion(sketch_id, i)` query for the atomic imprint face
   * of `sketchId` that contains the sketch-space point `(u, v)`, or `""` when
   * the point is outside every region. Lets a Revolve/Extrude profile picker
   * store just the clicked region instead of the whole sketch. Region index
   * order matches the kernel's arrangement (same as `listSketchRegions`).
   */
  regionQueryAtPoint(sketchId: number, u: number, v: number): string {
    return this.inner.regionQueryAtPoint(sketchId, u, v);
  }

  /**
   * The sketch's atomic imprint faces as raw JSON — `{index, centroid,
   * interior, aabb, area, perimeter}` per face, in the kernel's deterministic
   * arrangement order (#412). `resolution` is accepted for call compatibility
   * but ignored:
   * the arrangement is exact, not grid-sampled.
   */
  listSketchRegions(sketchId: number, resolution = 128): string {
    return this.inner.listSketchRegions(sketchId, resolution);
  }

  /**
   * Rigid + uniform-scale placement of body `i`. B-Reps stay body-local and an
   * affine transform never invalidates them; compose this back in with {@link
   * localToWorld} to get world geometry.
   */
  bodyPlacement(i: number): Placement {
    return JSON.parse(this.inner.bodyPlacement(i)) as Placement;
  }

  /**
   * Per-**triangle** B-Rep face tags of body `i`: one `u32` per mesh triangle
   * (length = triangle count) giving the arena index of the {@link BRepFace}
   * that triangle lies on (matching `faceIndex`), or {@link
   * FACE_INDEX_SENTINEL} when unresolved. Index this with a hovered triangle id
   * to resolve its stable face. Returns an empty view for an un-tagged body.
   */
  bodyFaceIndexView(i: number): Uint32Array {
    const ptr = this.inner.bodyFaceIndexPtr(i);
    const len = this.inner.bodyFaceIndexLen(i);
    if (!ptr || len === 0) return new Uint32Array(0);
    return new Uint32Array(this.memory.buffer, ptr, len);
  }

  /**
   * Cast a world-space ray through the current CSG tree and return
   * the first surface hit, or `{ hit: false }` on a miss. Delegates
   * to the Rust kernel's sphere-tracing raycaster and parses its
   * compact JSON payload.
   */
  pick(
    origin: [number, number, number],
    direction: [number, number, number],
  ): PickResult {
    if (origin.some(Number.isNaN) || direction.some(Number.isNaN)) {
      return { hit: false } as PickResult;
    }
    const json = this.inner.pick(
      origin[0],
      origin[1],
      origin[2],
      direction[0],
      direction[1],
      direction[2],
    );
    try {
      return JSON.parse(json) as PickResult;
    } catch {
      return { hit: false } as PickResult;
    }
  }

  meshResolution(): number {
    return this.inner.meshResolution();
  }

  setMeshResolution(resolution: number): boolean {
    return this.inner.setMeshResolution(resolution);
  }

  // ── LOD (K3 part 4) ─────────────────────────────────────────────────
  //
  // Context bodies — everything that isn't the body the user is
  // currently editing — run at a coarser voxel grid, so a single
  // context re-mesh (for example after a bridging Union that merges
  // two bodies) is a fraction of the cost of an active re-mesh.
  // `activeBodyId === 0` means "no LOD hint set" — every body runs
  // at active resolution, identical to the K3 part 1–3 behaviour.

  contextMeshResolution(): number {
    return this.inner.contextMeshResolution();
  }

  setContextMeshResolution(resolution: number): boolean {
    return this.inner.setContextMeshResolution(resolution);
  }

  activeBodyId(): number {
    return this.inner.activeBodyId();
  }

  /**
   * Mark the body with the given stable id as the active one.
   * Pass `0` (or call `clearActiveBodyId()`) to disable LOD again.
   */
  setActiveBodyId(bodyId: number): boolean {
    return this.inner.setActiveBodyId(bodyId);
  }

  clearActiveBodyId(): boolean {
    return this.inner.clearActiveBodyId();
  }

  /**
   * Convenience: look up whichever body owns the given feature id
   * and mark that body as active. No-op when the feature doesn't
   * exist or already lives in the active body.
   */
  setActiveBodyByFeature(featureId: number): boolean {
    return this.inner.setActiveBodyByFeature(featureId);
  }

  // ── sync meshing toggle (K3 part 5) ─────────────────────────────────
  //
  // The main-thread kernel flips this off once the mesher web worker
  // is ready. After that, mutations still update the document (and
  // the CSG tree used by `pick`) but skip the Dual Contouring pass
  // — a second kernel instance inside the worker owns the meshing.

  syncMeshing(): boolean {
    return this.inner.syncMeshing();
  }

  setSyncMeshing(enabled: boolean): boolean {
    return this.inner.setSyncMeshing(enabled);
  }

  /** Toggle milestone-1 parametric weaving (exact analytic B-Rep for pure
   * grundforms). Returns `true` if the flag actually changed. */
  setParametricWeave(enabled: boolean): boolean {
    return this.inner.setParametricWeave(enabled);
  }

  /** Suppress every B-Rep weave while a gizmo drag streams throttled
   * commits ("B-Rep rebuild while dragging" off): bodies whose geometry
   * changed export empty B-Rep payloads instead of weaving. Turning the
   * flag off weaves the catch-up once and bumps the stale bodies'
   * versions so the overlays refetch. Returns `true` if the flag
   * actually changed. */
  setBrepSuppressed(on: boolean): boolean {
    return this.inner.setBrepSuppressed(on);
  }

  // ── outliner ────────────────────────────────────────────────────────

  listFeatures(): FeatureSummary[] {
    const json = this.inner.listFeatures();
    return JSON.parse(json) as FeatureSummary[];
  }

  // ── K6a: feature-schema lookup ─────────────────────────────────────

  /**
   * List every feature type name the built-in registry knows
   * about. Sorted alphabetically by the Rust side so the result
   * is stable across calls and safe to key React lists on.
   */
  listFeatureTypes(): string[] {
    return JSON.parse(this.inner.listFeatureTypes()) as string[];
  }

  /**
   * Load the JSON schema a feature type declares via the Rust
   * `FeatureSchema` derive. The shape is
   * `{ type: string, params: Array<{ key, name, type, required }> }`
   * — see `FeatureSchemaJson` in `web/src/widgets/schema.ts`
   * for the TypeScript type.
   */
  schemaForType(typeName: string): unknown {
    return JSON.parse(this.inner.schemaForType(typeName));
  }

  /**
   * K6b.2: list every sketch-tool type name the kernel knows.
   * Sketch tools live in a registry disjoint from the top-level
   * feature registry; K6b.1 ships two built-ins (`sketch_point`,
   * `sketch_line`), K6b.3+ grows the set. Sorted alphabetically.
   */
  listSketchToolTypes(): string[] {
    return JSON.parse(this.inner.listSketchToolTypes()) as string[];
  }

  /**
   * K6b.2: load the schema for a single sketch tool. Same JSON
   * shape as [`schemaForType`] so the generic FeatureDialog can
   * render it without knowing whether it's a top-level feature
   * or a sketch-local tool.
   */
  schemaForSketchToolType(typeName: string): unknown {
    return JSON.parse(this.inner.schemaForSketchToolType(typeName));
  }

  /**
   * K6b.3a: commit a sketch from the UI session as a `sketch`
   * FeatureCall on the kernel's canonical wire list. `subCalls`
   * is a JSON-ready array of the sketch-tool FeatureCalls the
   * session accumulated; it's stringified here so the
   * wasm-bindgen surface can stay `String`-in, `u32`-out.
   * Returns the new feature id.
   */
  addSketch(
    planeQuery: string,
    subCalls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): number {
    // The kernel side expects FeatureCall JSON objects with
    // `type`, `id`, `params`. We don't have sketch-tool-level
    // ids yet — K6b.3a allocates synthetic strings so the
    // protocol's `FeatureId::FromStr` accepts them. These are
    // purely internal to the sketch sub-spine and don't collide
    // with the top-level document id space.
    const prepared = subCalls.map((c, i) => ({
      type: c.type,
      id: String(i + 1),
      params: c.params,
    }));
    return this.inner.addSketch(planeQuery, JSON.stringify(prepared));
  }

  /** K6b.3a: read back a sketch's sub-spine by feature id. */
  getSketchSubCalls(
    id: number,
  ): Array<{ type: string; id: string; params: Record<string, unknown> }> {
    const json = this.inner.getSketchSubCalls(id);
    return JSON.parse(json) as Array<{
      type: string;
      id: string;
      params: Record<string, unknown>;
    }>;
  }

  /** K7.5: current wire-form `params` of a feature call by
   *  id. The Outliner FeatureDialog seeds its edit buffer
   *  from this, so changes carry through to the actual
   *  feature rather than a schema-default stub. */
  getFeatureCallParams(id: number): Record<string, unknown> {
    const json = this.inner.getFeatureCallParams(id);
    return JSON.parse(json) as Record<string, unknown>;
  }

  /** K7.5: replace the wire-form params of a feature call in
   *  place and regenerate. Keeps the feature id stable.
   *  `coalesce` skips the undo snapshot so a throttled drag
   *  stays one undo step (see kernel doc). */
  replaceFeatureCallParams(
    id: number,
    params: Record<string, unknown>,
    coalesce = false,
  ): void {
    this.inner.replaceFeatureCallParams(id, JSON.stringify(params), coalesce);
  }

  /** K8.0: append a linear_pattern feature that replicates
   *  `sourceId` along `(dx, dy, dz)` with `count` total
   *  instances at `spacingMm` spacing. Returns the new
   *  pattern feature id. */
  addLinearPattern(
    sourceId: number,
    dx: number,
    dy: number,
    dz: number,
    count: number,
    spacingMm: number,
  ): number {
    return this.inner.addLinearPattern(sourceId, dx, dy, dz, count, spacingMm);
  }

  /** K8.1: append a circular_pattern feature that rotates
   *  `sourceId` around the axis `(ax, ay, az)` through the
   *  pivot `(cx, cy, cz)` with `count` total instances over
   *  `angleDeg` total sweep (360 = full ring). Returns the
   *  new pattern feature id. */
  addCircularPattern(
    sourceId: number,
    ax: number,
    ay: number,
    az: number,
    cx: number,
    cy: number,
    cz: number,
    count: number,
    angleDeg: number,
  ): number {
    return this.inner.addCircularPattern(
      sourceId,
      ax,
      ay,
      az,
      cx,
      cy,
      cz,
      count,
      angleDeg,
    );
  }

  /** K8.2: append a mirror feature that reflects `sourceId`
   *  through a plane. `plane` is a face/datum query (e.g.
   *  `qFace(<hash>)` or `origin_plane_xy`) whose world-space
   *  frame supplies the mirror plane. Returns the new mirror
   *  feature id. */
  addMirror(sourceId: number, plane: string): number {
    return this.inner.addMirror(sourceId, plane);
  }

  /** #404 P1: append an Onshape-style hole feature. Drills one identical cutter
   *  per location. `standardSize` is an ISO key (`"M6"`) or `"custom"`;
   *  `endType` is `"blind"` or `"through_all"`. `locations`/`targets` are
   *  comma-separated MultiQuery strings; diameter/depth are `Length`-typed. */
  addHole(
    standardSize: string,
    diameterMm: number,
    endType: string,
    depthMm: number,
    locations: string,
    targets: string,
  ): number {
    return this.inner.addHole(
      standardSize,
      diameterMm,
      endType,
      depthMm,
      locations,
      targets,
    );
  }

  /** K11.0: append a gyroid TPMS infill feature. */
  addGyroid(
    cellSizeMm: number,
    thicknessMm: number,
    op: BoolOpTagValue,
  ): number {
    return this.inner.addGyroid(cellSizeMm, thicknessMm, op);
  }

  /** Append a custom-SDF feature (user math expression over x,y,z,
   *  clipped to a local box of the given half extent). */
  addCustomSdf(halfExtentMm: number): number {
    return this.inner.addCustomSdf(halfExtentMm);
  }

  addBoolean(): number {
    return this.inner.addBoolean();
  }

  addMaterial(): number {
    return this.inner.addMaterial();
  }

  /** K8.4: append a revolve feature that consumes the sketch
   *  with id `sketchId` and sweeps its profile around the local
   *  Y axis through `(cx, cy, cz)`. Returns the new feature id. */
  addRevolve(sketchId: number, op: BoolOpTagValue): number {
    return this.inner.addRevolve(sketchId, op);
  }

  /** Append a helix feature sweeping the sketch's profile along a
   *  helical path around the sketch-local Y axis (real screw
   *  threads). `pitchMm` is the axial advance per revolution,
   *  `turns` the revolution count. `sketchId === 0` creates an
   *  empty helix the dialog's query picker fills afterwards. */
  addHelix(
    sketchId: number,
    op: BoolOpTagValue,
    pitchMm = 2,
    turns = 8,
  ): number {
    return this.guardMut(() =>
      this.inner.addHelix(sketchId, op, pitchMm, turns),
    );
  }

  /** Append a sweep: `profileSketchId` swept along `pathSketchId` (a planar
   *  path sketch). Returns the new feature id. */
  addSweep(
    profileSketchId: number,
    pathSketchId: number,
    op: BoolOpTagValue,
  ): number {
    return this.guardMut(() =>
      this.inner.addSweep(profileSketchId, pathSketchId, op),
    );
  }

  /** Append a sweep along a default 3D control-point path (Gizmo3d); the
   *  control points are then editable. Returns the new feature id. */
  addSweepGizmo(profileSketchId: number, op: BoolOpTagValue): number {
    return this.guardMut(() => this.inner.addSweepGizmo(profileSketchId, op));
  }

  /** #385: append a loft interpolating between N ≥ 2 parallel profile
   *  cross-sections. `profiles` is a comma-separated MultiQuery of sketch-region
   *  / planar-face queries; empty is allowed (the dialog's query picker fills it
   *  afterwards). Returns the new feature id. */
  addLoft(profiles: string, op: BoolOpTagValue): number {
    return this.guardMut(() => this.inner.addLoft(profiles, op));
  }

  /** K7.4: append an extrude feature that consumes the
   *  sketch with id `sketchId`. `depthMm` is the total slab
   *  depth, `op` is a BoolOpTag and `tz` positions the
   *  extrude plane on the world Z axis. `suppressed` creates
   *  the call as a PENDING ghost (ticket x7af) — the body
   *  keeps its pre-extrude shape until the dialog confirms.
   *  Returns the new feature id. */
  addExtrude(
    sketchId: number,
    depthMm: number,
    op: BoolOpTagValue,
    tz = 0,
    suppressed = false,
  ): number {
    return this.guardMut(() =>
      this.inner.addExtrude(sketchId, depthMm, op, tz, suppressed),
    );
  }

  // ── Meshing Phase 0: tolerance-driven export meshing session ──
  // Protocol: exportMeshBegin(cellMm) → N× exportMeshBody(i) (loop +
  // progress) → exportMeshStl()/exportMesh3mf() + exportMeshReport()
  // → exportMeshEnd(). Any document mutation drops the session.

  /** Start an export meshing pass; returns the body count to loop.
   *  `method` selects the generic mesher (`"adaptive"`, `"surface-nets"`,
   *  `"lichen"`, …); `optionsJson` carries the method-specific knobs beyond
   *  the cell size (camelCase, see `ExportMethodOptions`; `""` = defaults). */
  exportMeshBegin(
    cellSizeMm: number,
    method: string,
    optionsJson: string,
  ): number {
    return this.inner.exportMeshBegin(cellSizeMm, method, optionsJson);
  }

  /** Pre-mesh estimate (dense-grid methods only): finest achievable cell
   *  size + rough raw-grid triangle count at `cellSizeMm`, as JSON
   *  (`ExportEstimate`). Read-only + cheap, for a live dialog hint. */
  exportEstimate(cellSizeMm: number, method: string): string {
    return this.inner.exportEstimate(cellSizeMm, method);
  }

  /** 3MF archive from raw triangle buffers (server-mesh format conversion). */
  mesh3mfFromBuffers(positions: Float32Array, indices: Uint32Array): Uint8Array {
    return this.inner.mesh3mfFromBuffers(positions, indices);
  }

  /** Debug: sample the scene SDF on a `resolution`×`resolution` grid over a
   *  plane at `origin` with `normal`, spanning ±`halfExtent`. Row-major
   *  (v outer, u inner) mm distances — the same field the meshers see. */
  sdfPlaneSample(
    origin: [number, number, number],
    normal: [number, number, number],
    halfExtent: number,
    resolution: number,
  ): Float32Array {
    return this.inner.sdfPlaneSample(
      origin[0],
      origin[1],
      origin[2],
      normal[0],
      normal[1],
      normal[2],
      halfExtent,
      resolution,
    );
  }

  /** Debug ▸ Mesher: narrow-band adaptive octree over the scene. Returns leaf
   *  cell centres packed `[x,y,z, sdf, nx,ny,nz, cellSize]` (8 floats each). */
  mesherPoints(optionsJson: string): Float32Array {
    return this.inner.mesherPoints(optionsJson);
  }

  /** Debug ▸ Mesher Lichen: the same flat overlay format the PASM mesher
   *  emits — `[nPoints, nTris, nFallbackTris, nPoints×9 floats, indices]`. */
  lichenMesh(json: string): Float32Array {
    return this.inner.lichenMesh(json);
  }

  /** Debug ▸ Voxel (#369): grid geometry for `resolution` voxels along the
   *  longest scene-AABB axis, `[nx, ny, nz, ox, oy, oz, spacing]` or `[]`. */
  voxelGridInfo(resolution: number): Float32Array {
    return this.inner.voxelGridInfo(resolution);
  }

  /** Debug ▸ Voxel (#369): row-major occupancy (1 inside / 0 outside), x
   *  fastest then y then z, over the grid of {@link Kernel.voxelGridInfo}. */
  voxelizeOccupancy(resolution: number): Uint8Array {
    return this.inner.voxelizeOccupancy(resolution);
  }

  /** Mesh + validate body `i`; returns its report JSON ("" without a
   *  running session). */
  exportMeshBody(i: number): string {
    return this.inner.exportMeshBody(i);
  }

  /** Binary STL of the session's export meshes. */
  exportMeshStl(): Uint8Array {
    return this.inner.exportMeshStl();
  }

  /** 3MF archive of the session's export meshes. */
  exportMesh3mf(): Uint8Array {
    return this.inner.exportMesh3mf();
  }

  /** Merged export-mesh vertex positions (xyz-interleaved) for the
   *  in-app solid preview. Pairs with `exportMeshPreviewIndices`. */
  exportMeshPreviewPositions(): Float32Array {
    return this.inner.exportMeshPreviewPositions();
  }

  /** Merged export-mesh triangle indices (rebased) for the preview. */
  exportMeshPreviewIndices(): Uint32Array {
    return this.inner.exportMeshPreviewIndices();
  }

  /** Aggregate session report as JSON (see ExportReport in
   *  exportTypes.ts). */
  exportMeshReport(): string {
    return this.inner.exportMeshReport();
  }

  /** Drop the session and free its meshes. */
  exportMeshEnd(): void {
    this.inner.exportMeshEnd();
  }

  /** M14b: classify features for STEP export capability. */
  classifyForStep(): string {
    return this.inner.classifyForStep();
  }

  /** M14d: build the StepCommand[] JSON from the feature spine. */
  stepCommands(): string {
    return this.inner.stepCommands();
  }

  /**
   * K6b.3b: replace an existing sketch's plane and sub_calls in
   * place, keeping the feature id stable. Used by the edit
   * reentry path — Finish on a re-opened sketch lands here
   * instead of `addSketch` so the outliner row doesn't move.
   */
  replaceSketch(
    id: number,
    planeQuery: string,
    subCalls: ReadonlyArray<{ type: string; params: Record<string, unknown> }>,
  ): void {
    const prepared = subCalls.map((c, i) => ({
      type: c.type,
      id: String(i + 1),
      params: c.params,
    }));
    this.inner.replaceSketch(id, planeQuery, JSON.stringify(prepared));
  }

  // ── mutations ───────────────────────────────────────────────────────

  /**
   * SI-W: instantiate a feature by type name — a built-in or a document
   * FieldSkript — with its parameter definition as JSON. Returns the new
   * feature id. The regenerate triggered here can reach a scripted body, so
   * the main-thread build host must already be synced (it is, via every
   * `upsertScript` and `loadJson`).
   */
  addFeatureCall(typeName: string, paramsJson: string): number {
    return this.guardMut(() => this.inner.addFeatureCall(typeName, paramsJson));
  }

  addBox(
    op: BoolOpTagValue,
    translate: [number, number, number],
    halfExtents: [number, number, number],
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Box,
      op,
      translate[0],
      translate[1],
      translate[2],
      halfExtents[0],
      halfExtents[1],
      halfExtents[2],
    );
  }

  addSphere(
    op: BoolOpTagValue,
    translate: [number, number, number],
    radius: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Sphere,
      op,
      translate[0],
      translate[1],
      translate[2],
      radius,
      0,
      0,
    );
  }

  addCylinder(
    op: BoolOpTagValue,
    translate: [number, number, number],
    radius: number,
    halfHeight: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Cylinder,
      op,
      translate[0],
      translate[1],
      translate[2],
      radius,
      halfHeight,
      0,
    );
  }

  addTorus(
    op: BoolOpTagValue,
    translate: [number, number, number],
    majorRadius: number,
    minorRadius: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Torus,
      op,
      translate[0],
      translate[1],
      translate[2],
      majorRadius,
      minorRadius,
      0,
    );
  }

  addThread(
    op: BoolOpTagValue,
    translate: [number, number, number],
    nominalRadius: number,
    pitch: number,
    depth: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Thread,
      op,
      translate[0],
      translate[1],
      translate[2],
      nominalRadius,
      pitch,
      depth,
    );
  }

  addCone(
    op: BoolOpTagValue,
    translate: [number, number, number],
    rBottom: number,
    rTop: number,
    halfHeight: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Cone,
      op,
      translate[0],
      translate[1],
      translate[2],
      rBottom,
      rTop,
      halfHeight,
    );
  }

  addCapsule(
    op: BoolOpTagValue,
    translate: [number, number, number],
    radius: number,
    halfHeight: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Capsule,
      op,
      translate[0],
      translate[1],
      translate[2],
      radius,
      halfHeight,
      0,
    );
  }

  addEllipsoid(
    op: BoolOpTagValue,
    translate: [number, number, number],
    rx: number,
    ry: number,
    rz: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.Ellipsoid,
      op,
      translate[0],
      translate[1],
      translate[2],
      rx,
      ry,
      rz,
    );
  }

  addHexPrism(
    op: BoolOpTagValue,
    translate: [number, number, number],
    apothem: number,
    halfHeight: number,
  ): number {
    return this.inner.addPrimitive(
      PrimitiveKind.HexPrism,
      op,
      translate[0],
      translate[1],
      translate[2],
      apothem,
      halfHeight,
      0,
    );
  }

  removeFeature(id: number): boolean {
    return this.inner.removeFeature(id);
  }

  setFeatureOp(id: number, op: BoolOpTagValue): boolean {
    return this.inner.setFeatureOp(id, op);
  }

  /** K13.1: import triangle mesh as a CSG-participating solid. */
  addImportedMesh(
    positions: Float32Array,
    indices: Uint32Array,
    op: BoolOpTagValue,
  ): number {
    return this.inner.addImportedMesh(positions, indices, op);
  }

  /**
   * Add a mate connector anchored to a B-Rep element. `anchor` is an element
   * query (`qFace`/`qEdge`/`qVertex`); the anchor defines +Z (edge direction /
   * face normal). `t` positions an edge anchor (0.5 = midpoint). `secondary` is
   * a reference FACE query orienting the secondary axes (edge) or defining +Z
   * (vertex); empty → auto-picked. `reorient` rotates the secondary axes in 90°
   * steps about +Z (0..3). `flip` reverses +Z. CSYS is kernel-resolved (C2).
   */
  addMateConnector(
    anchor: string,
    t = 0.5,
    secondary = "",
    reorient = 0,
    flip = false,
  ): number {
    return this.inner.addMateConnector(anchor, t, secondary, reorient, flip);
  }

  /**
   * C2: resolved CSYS of every placed `mate_connector`, parsed from the
   * kernel's JSON. Each entry is `{ id, origin, normal, x_axis }`.
   */
  resolvedMateConnectors(): {
    id: number;
    origin: [number, number, number];
    normal: [number, number, number];
    x_axis: [number, number, number];
  }[] {
    const json = this.inner.resolvedMateConnectors();
    try {
      return JSON.parse(json);
    } catch {
      return [];
    }
  }

  /** DL-61: add a fillet selecting an edge by query (e.g. `qEdge(<idHash>)`). */
  addFilletEdge(edge: string, blendRadius: number): number {
    return this.inner.addFilletEdge(edge, blendRadius);
  }

  /** #414: add a chamfer selecting an edge by query (e.g. `qEdge(<idHash>)`).
   *  `distanceMm` is the bevel leg length — how far the cut sets back from the
   *  edge on EACH adjacent face (Onshape's EQUAL_OFFSETS). The wasm binding
   *  writes it in `Length` wire form, so callers pass plain millimetres. */
  addChamferEdge(edge: string, distanceMm: number): number {
    return this.inner.addChamferEdge(edge, distanceMm);
  }

  /** Add a shell feature hollowing the body of the picked planar face(s).
   *  `openFaces` is a comma-separated list of face queries; empty creates the
   *  feature so its dialog can be filled. */
  addShell(openFaces: string, thicknessMm: number): number {
    return this.inner.addShell(openFaces, thicknessMm);
  }

  /** d: round every Boolean-junction edge globally with `radiusMm` (0 clears). */
  addRoundAll(radiusMm: number): number {
    return this.inner.addRoundAll(radiusMm);
  }

  /** #384 offset_tree: signed body offset. `targets` is a comma-separated list
   *  of `qCreatedBy(...)` body queries (empty creates the feature so its dialog
   *  can be filled); `deltaMm > 0` dilates, `< 0` erodes. */
  addOffset(targets: string, deltaMm: number): number {
    return this.inner.addOffset(targets, deltaMm);
  }

  /** Extrude one or more coplanar planar faces (P1). `faces` is a
   *  comma-separated list of `qFace(<idHash>)` queries; `symmetric=false`
   *  is the blind/two-sided Onshape default growing outward from the face.
   *  `suppressed=true` creates the call as a pending ghost (ticket x7af):
   *  the body keeps its pre-extrude shape until the feature is confirmed. */
  addExtrudeFaces(
    faces: string,
    depthMm: number,
    symmetric: boolean,
    depthBackMm: number,
    suppressed = false,
  ): number {
    return this.inner.addExtrudeFaces(
      faces,
      depthMm,
      symmetric,
      depthBackMm,
      suppressed,
    );
  }

  /** Thicken one or more curved faces into a thin solid wall offset along the
   *  normal. A closed NON-analytic face (a revolved / swept blob) goes through
   *  the general SDF field-offset band `Shell(Offset(carrier, out), out+in)`; an
   *  analytic face (sphere / cylinder / torus / cone) folds back to the exact
   *  concentric surfaces as a fast-path. `faces` is a comma-separated list of
   *  `qFace(<idHash>)` queries; `symmetric=false` grows `thicknessMm` outward
   *  (Onshape default). `upToFace` is an optional `qFace(<idHash>)` target: when
   *  non-empty the wall grows up to that coaxial face and the thickness
   *  arguments are ignored. */
  addThicken(
    faces: string,
    thicknessMm: number,
    symmetric: boolean,
    thicknessBackMm: number,
    upToFace = "",
  ): number {
    return this.inner.addThicken(
      faces,
      thicknessMm,
      symmetric,
      thicknessBackMm,
      upToFace,
    );
  }

  /** PERSISTENT-NAMING N4: re-canonicalise every stored element reference
   *  (fresh hash + fingerprint) against the current woven B-Reps. Called by
   *  the persistence funnel right before a save; returns the heal report
   *  (`[{feature, from, to, rehashed}]`, empty when nothing changed). */
  healElementRefs(): {
    feature: string;
    from: string;
    to: string;
    rehashed: boolean;
  }[] {
    return JSON.parse(this.inner.healElementRefs()) as {
      feature: string;
      from: string;
      to: string;
      rehashed: boolean;
    }[];
  }

  /** Resolve a canonical query string against the woven B-Reps
   *  (Onshape-Import: uniqueness check before storing a translated plane
   *  query). Each match carries the fingerprint-carrying canonical
   *  element form plus the body-local centroid for disambiguation. A
   *  parse error yields an empty list (the importer treats it as
   *  unresolvable). */
  resolveQuery(query: string): {
    kind: "face" | "edge" | "vertex";
    idHash: string;
    query: string;
    centroid: number[];
  }[] {
    const parsed = JSON.parse(this.inner.resolveQuery(query)) as unknown;
    return Array.isArray(parsed)
      ? (parsed as {
          kind: "face" | "edge" | "vertex";
          idHash: string;
          query: string;
          centroid: number[];
        }[])
      : [];
  }

  /** K9.1: overall scene AABB in mm (conservative CSG bound). */
  sceneAabb(): { min: number[]; max: number[]; size: number[] } {
    return JSON.parse(this.inner.sceneAabb()) as {
      min: number[];
      max: number[];
      size: number[];
    };
  }

  /** Ticket f88u: world-space centre of the *visible* solid's tight AABB — the
   *  orbit pivot. `[x,y,z]`, or `[]` when the kernel can't tighten the field
   *  (empty / unbounded scene). Memoised kernel-side on the geometry version. */
  sceneVisibleCenter(): number[] {
    return Array.from(this.inner.sceneVisibleCenter());
  }

  /** K8.7: toggle the suppressed flag of a feature call. */
  setFeatureSuppressed(id: number, suppressed: boolean): void {
    this.inner.setFeatureSuppressed(id, suppressed);
  }

  setRollbackIndex(index: number): void {
    this.inner.setRollbackIndex(index);
  }

  getRollbackIndex(): number {
    return this.inner.getRollbackIndex();
  }

  /** K8.10 / 8ar9: upsert a document-level variable. `kind` controls how
   *  the value is presented (number/length/angle); pass "" to keep an
   *  existing variable's kind (or default to number on insert). */
  setVariable(
    name: string,
    value: number,
    kind: VarKind | "" = "",
    formula?: string,
  ): void {
    this.inner.setVariable(name, value, kind, formula);
  }

  /** K8.10: remove a variable. Returns true when something
   *  was actually removed. */
  removeVariable(name: string): boolean {
    return this.inner.removeVariable(name);
  }

  /** Ticket 8ar9: rename a variable, keeping its value. Throws on
   *  an empty new name or a collision with an existing variable. */
  renameVariable(oldName: string, newName: string): void {
    this.inner.renameVariable(oldName, newName);
  }

  /** K8.10 / 8ar9: list every variable as `{name, value, kind}` entries,
   *  sorted by name. */
  listVariables(): VariableEntry[] {
    return JSON.parse(this.inner.listVariables()) as VariableEntry[];
  }

  /** K8.8: duplicate a feature call with a +20mm X nudge on
   *  its translate, and return the new feature id. */
  duplicateFeature(id: number): number {
    return this.inner.duplicateFeature(id);
  }

  reorderFeature(id: number, targetIndex: number): boolean {
    return this.inner.reorderFeature(id, targetIndex);
  }

  setFeatureTranslate(id: number, x: number, y: number, z: number): boolean {
    return this.inner.setFeatureTranslate(id, x, y, z);
  }

  setFeatureRotation(
    id: number,
    rx: number,
    ry: number,
    rz: number,
    rw: number,
  ): boolean {
    return this.inner.setFeatureRotation(id, rx, ry, rz, rw);
  }

  setFeatureScale(id: number, sx: number, sy: number, sz: number): boolean {
    return this.inner.setFeatureScale(id, sx, sy, sz);
  }

  applyGizmoTranslate(id: number, tx: number, ty: number, tz: number): boolean {
    return this.inner.applyGizmoTranslate(id, tx, ty, tz);
  }

  applyGizmoRotate(
    id: number,
    rx: number,
    ry: number,
    rz: number,
    rw: number,
  ): boolean {
    return this.inner.applyGizmoRotate(id, rx, ry, rz, rw);
  }

  applyGizmoScale(id: number, sx: number, sy: number, sz: number): boolean {
    return this.inner.applyGizmoScale(id, sx, sy, sz);
  }

  nudgeFeature(id: number, dx: number, dy: number, dz: number): boolean {
    return this.inner.nudgeFeature(id, dx, dy, dz);
  }

  // ── undo / redo ─────────────────────────────────────────────────────

  undo(): boolean {
    return this.inner.undo();
  }

  redo(): boolean {
    return this.inner.redo();
  }

  canUndo(): boolean {
    return this.inner.canUndo();
  }

  canRedo(): boolean {
    return this.inner.canRedo();
  }
  undoDepth(): number {
    return this.inner.undoDepth();
  }
  redoDepth(): number {
    return this.inner.redoDepth();
  }

  // ── document I/O ────────────────────────────────────────────────────

  toJson(): string {
    return this.inner.toJson();
  }

  loadJson(json: string): number {
    // Refresh the main-thread build host's compiled-`build()` table BEFORE
    // loading: the load regenerates the spine, and any scripted instance it
    // reaches calls back through the runner. This single seam covers every
    // store loadJson call site (rehydrate, undo/redo, import).
    syncMainScriptHost(scriptRecordsFrom(json));
    return this.inner.loadJson(json);
  }

  // ── FieldSkript records (S1) ────────────────────────────────────────

  /** List the document's FieldSkript records. */
  listScripts(): FieldSkriptRecord[] {
    return JSON.parse(this.inner.listScripts()) as FieldSkriptRecord[];
  }

  /**
   * Insert or replace a FieldSkript record (keyed by `id`). Returns
   * `true` when it replaced an existing record, `false` when inserted.
   */
  upsertScript(record: FieldSkriptRecord): boolean {
    const replaced = this.guardMut(() =>
      this.inner.upsertScript(JSON.stringify(record)),
    );
    // The host must know the new/edited compiled `build()` before any later
    // instance of this script regenerates on the main thread.
    syncMainScriptHost(this.listScripts());
    return replaced;
  }

  /** Remove a FieldSkript record by id. Returns `true` when removed. */
  removeScript(id: string): boolean {
    const removed = this.guardMut(() => this.inner.removeScript(id));
    syncMainScriptHost(this.listScripts());
    return removed;
  }
}
