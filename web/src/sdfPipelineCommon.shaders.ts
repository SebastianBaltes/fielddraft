/**
 * Shared TSL shader building blocks of both SDF pipelines (VM single-pass
 * and deferred): the displaced SDF eval wrapper, the canvas depth write,
 * the union sphere-trace and the hit shading (normal → PBR → AO →
 * selection tint → wireframe). Kept separate from sdfPipelineCommon.ts so
 * the @ts-nocheck below stays confined to actual shader-emitting code.
 */

// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import {
  Break,
  If,
  Loop,
  abs,
  clamp,
  cos,
  cross,
  depth,
  float,
  fract,
  int,
  max,
  min,
  mix,
  normalize,
  select,
  sin,
  smoothstep,
  vec3,
  vec4,
  viewZToOrthographicDepth,
  viewZToPerspectiveDepth,
} from "three/tsl";
import {
  MAT_STRIDE,
  MAT_COLOR_R,
  MAT_COLOR_G,
  MAT_COLOR_B,
  MAT_ROUGHNESS,
  MAT_METALNESS,
  MAT_EMISSIVE_R,
  MAT_EMISSIVE_G,
  MAT_EMISSIVE_B,
  MAT_OPACITY,
  MAT_WIREFRAME,
  MAT_DISPLACE_DEPTH,
  MAT_DISPLACE_FREQ,
  SURFACE_EPS,
} from "./sdfPipelineCommon";
import { pbrShade } from "./pbrShading";
import { NORMAL_EPS } from "./sdfRenderer";

// ── Shared shader constants ────────────────────────────────────────

export const WIREFRAME_GRID_SIZE = 10;
export const WIREFRAME_GRID_THRESHOLD = 0.03;
export const FRESNEL_EDGE_MIN = 0.65;
export const FRESNEL_EDGE_MAX = 0.85;
export const DISPLACE_FREQ_Y = 1.3;
export const DISPLACE_FREQ_Z = 0.7;

// ── Shared TSL building blocks ─────────────────────────────────────

/**
 * Wrap a raw VM SDF eval with the per-body sine displacement from the
 * material buffer (MAT_DISPLACE_DEPTH/FREQ). Callers that pass no `bodyIdx`
 * get the raw distance — displacement is a per-body material property.
 * Identical wrapper for both pipelines (VM material + deferred shade pass).
 */
export function makeDisplacedSdfEval(sdfEvalRaw: any, materialBuf: any) {
  return (args: {
    p: any;
    baseOffset: any;
    volumeBaseIdx: any;
    bodyIdx?: any;
  }) => {
    const d = sdfEvalRaw({
      p: args.p,
      baseOffset: args.baseOffset,
      volumeBaseIdx: args.volumeBaseIdx,
    });
    if (!args.bodyIdx) return d;
    const matBase = args.bodyIdx.mul(MAT_STRIDE);
    const dispDepth = materialBuf.element(matBase.add(MAT_DISPLACE_DEPTH));
    const freq = materialBuf.element(matBase.add(MAT_DISPLACE_FREQ));
    const disp = args.p.x
      .mul(freq)
      .sin()
      .mul(args.p.y.mul(freq).mul(DISPLACE_FREQ_Y).sin())
      .mul(args.p.z.mul(freq).mul(DISPLACE_FREQ_Z).sin());
    return d.sub(dispDepth.mul(disp));
  };
}

/**
 * Write per-pixel canvas depth for a raymarched world-space hit, so the
 * B-Rep edge overlay can occlude in "visible" mode. Both SDF pipelines draw
 * their final shading with an ortho fullscreen quad, so the rasterizer's own
 * depth is meaningless — the depth is reconstructed from the SCENE camera's
 * view matrix + near/far instead. Perspective depth is reciprocal in view-Z,
 * orthographic is linear; `isOrthoU` picks the matching encoder. Emits into
 * the calling `Fn()`/`If()` scope.
 *
 * The depth is the TRUE surface depth. It used to be scaled 0.1% of viewZ
 * BEHIND the surface (`DEPTH_EDGE_BIAS`, ticket b9xz) so the exactly-on-
 * surface B-Rep edges would win their z-fight in "visible" edge mode — but
 * that skewed every depth comparison against rasterized geometry: where a
 * solid meets the SDF plane at a grazing angle the offset amplifies to a
 * view-dependent millimetre-scale crescent along the intersection (ticket
 * iw6f). The edge overlay now pulls ITSELF forward via polygonOffset
 * instead (brepEdgeOverlays.tsx).
 */
export function emitSceneDepthWrite({
  hitPos,
  viewMatU,
  nearU,
  farU,
  isOrthoU,
}: any): void {
  depth
    .assign(sceneDepthValue({ hitPos, viewMatU, nearU, farU, isOrthoU }))
    .toStack();
}

/**
 * The NDC depth value `emitSceneDepthWrite` writes — as a node, for pipelines
 * that must route the depth somewhere other than the frag-depth builtin. The
 * deferred renderer shades into an off-screen target and hands the depth on as
 * a second colour attachment (see `createShadeMaterial`), because sampling a
 * WebGPU depth attachment back out of a render target reads 0 in three r183
 * (ticket #344).
 */
export function sceneDepthValue({
  hitPos,
  viewMatU,
  nearU,
  farU,
  isOrthoU,
}: any): any {
  const viewZ = viewMatU.mul(vec4(hitPos, 1.0)).z;
  return select(
    isOrthoU.greaterThan(float(0.5)),
    viewZToOrthographicDepth(viewZ, nearU, farU),
    viewZToPerspectiveDepth(viewZ, nearU, farU),
  );
}

/**
 * Emit the SINGLE adaptive union sphere-trace both SDF pipelines march with:
 * each step lets `evalBodies` evaluate every candidate body's own SDF at the
 * current point and keep the SMALLEST distance (`minD`) plus which body
 * produced it (`closestBody`); the ray then steps by that conservative
 * distance, scaled by the per-body `marchRelax` factor (Ticket 1wnm: a
 * thin-walled revolve/shell declares < 1 so grazing rays don't overshoot its
 * wall; simple primitives keep 1.0). On a hit the pixel belongs to
 * `hitBody` — the body whose own surface is nearest along the ray, exactly
 * the depth-buffer occlusion two separate opaque solids must have.
 *
 * NOTE on the "implicit boolean" concern (tickets deeg/9ljc): for OPAQUE
 * primary visibility this union march and a per-body nearest-hit march
 * produce the SAME image — both show the nearest surface and record
 * `closestBody`, so separate solids occlude, they do NOT fuse. The earlier
 * per-body NESTED march (Loop(MAX_BODIES){Loop(MARCH_STEPS)}, commit
 * e4e5727) gave no visual benefit but sliced bodies (each body got its own
 * capped step budget and a shared `bestT` early-out cut marches short) and
 * — in the deferred renderer — exploded the shader so it never finished
 * compiling. Reverted to this single union march (ticket 6yin).
 *
 * @param ro          ray origin (vec3 node)
 * @param rd          ray direction (vec3 node)
 * @param tStart      initial ray parameter (number; deferred backs off 1 mm)
 * @param steps       max iterations (number, SDF_MARCH_STEPS)
 * @param marchRelaxBuf per-body conservative step factors (storage node)
 * @param tEnd        abort criterion: march stops once `t` exceeds this node
 *                    (VM: camera far plane; deferred: back-face exit depth)
 * @param evalBodies  `(p, minD, closestBody) => void` — emits the per-body
 *                    SDF evals of this pipeline and folds the running min
 * @returns `{ t, hit, hitPos, hitBody }` node vars for the caller's shading
 */
export function emitUnionMarch({
  ro,
  rd,
  tStart,
  steps,
  marchRelaxBuf,
  tEnd,
  evalBodies,
}: any): { t: any; hit: any; hitPos: any; hitBody: any } {
  const t = float(tStart).toVar();
  const hit = float(0).toVar();
  const hitPos = vec3(0, 0, 0).toVar();
  const hitBody = int(0).toVar();
  Loop(int(steps), () => {
    const p = ro.add(rd.mul(t));
    const minD = float(1e30).toVar();
    const closestBody = int(0).toVar();
    evalBodies(p, minD, closestBody);
    If(minD.lessThan(float(SURFACE_EPS)), () => {
      hit.assign(1);
      hitPos.assign(p);
      hitBody.assign(closestBody);
      Break();
    });
    t.addAssign(
      max(minD.mul(marchRelaxBuf.element(closestBody)), float(SURFACE_EPS)),
    );
    If(t.greaterThan(tEnd), () => {
      Break();
    });
  });
  return { t, hit, hitPos, hitBody };
}

// ── Ambient occlusion ──────────────────────────────────────────────
//
// Hemisphere AO over the SDF. A single ray along the normal cannot
// darken WIDE cavities (straight out of a vase cavity is free space —
// the occlusion comes from the walls all around), so we sample four
// directions: the normal plus three rays tilted ~53° towards it,
// spread 120° apart in azimuth. Tap distances are in model units (mm)
// and reach feature scale so concave interiors read as interiors —
// without AO a shell cavity is indistinguishable from a flat lid.
// Per-tap weights sum to 1 PER RAY, and ray contributions add up
// without renormalizing: a fully open hemisphere scores 0, while a
// single fully-blocked ray already darkens by SDF_AO_STRENGTH·1.
// Dividing by the ray count instead would need every ray of the
// hemisphere blocked for full effect — real cavities (vase floor:
// occ ≈ 0.4) would never get past a ~10% dim.
// Finer, more closely-spaced tap distances than the original [4,12,36]: with
// only 3 taps the per-tap occlusion contribution switches on/off at discrete
// depths as a ray starts/stops hitting the opposite bore wall or the concave
// rim, quantising a smooth cylindrical bore into visible horizontal AO bands
// (ticket km43). More taps spread those transitions out into a smooth gradient.
// Cheap: the AO loop is a single runtime `Loop` (one sdfEval call site), so
// extra taps are just loop iterations, NOT extra inlined VM copies. Weights
// sum to 1 per ray.
export const SDF_AO_TAPS: [number, number][] = [
  [3, 0.12],
  [6, 0.16],
  [11, 0.18],
  [18, 0.18],
  [28, 0.18],
  [44, 0.18],
];
export const SDF_AO_STRENGTH = 0.85;
// Brighter cavity floor (was 0.2): the floor multiplies the ambient term too,
// so 0.2 drove deep bores to ~ambient·0.2 ≈ 5% → near black (ticket km43). 0.5
// keeps cavities readable while still reading as interiors.
export const SDF_AO_FLOOR = 0.5;

// ── Shared hit shading ─────────────────────────────────────────────

/**
 * Shade a raymarched SDF surface hit: central-difference normal → PBR lighting
 * → selection tint and wireframe-grid alpha. Emits its nodes into the calling
 * `Fn()` body (it uses `If`), and returns the final `vec4(color, alpha)`.
 * Shared by the single-pass VM renderer and the deferred renderer so the
 * lighting/material/wireframe logic lives in one place.
 *
 * @param p              — world-space hit position (vec3 node)
 * @param bi             — body index (int node)
 * @param offsetBuf      — per-body opcode (offset,length) storage buffer
 * @param volumeBaseBuf  — per-body volume-texture base-index storage buffer
 * @param materialBuf    — per-body material storage buffer (MAT_* layout)
 * @param viewMatU       — view-matrix uniform (world → view)
 * @param viewDirWorld   — world-space direction toward the camera (vec3 node)
 * @param sdfEval        — compiled SDF eval node-fn
 * @param selectionBuf   — optional per-body selection-flag buffer; when given,
 *                         selected bodies (>0.5) get a blue highlight tint
 */
export function shadeSdfHit({
  p,
  bi,
  offsetBuf,
  volumeBaseBuf,
  materialBuf,
  viewMatU,
  viewDirWorld,
  sdfEval,
  selectionBuf,
}: any) {
  const baseOff = int(offsetBuf.element(bi.mul(2)));
  const volBase = int(volumeBaseBuf.element(bi));

  // Normal via central differences.
  const eps = float(NORMAL_EPS);
  const ex = vec3(eps, 0, 0);
  const ey = vec3(0, eps, 0);
  const ez = vec3(0, 0, eps);
  const n = normalize(
    vec3(
      sdfEval({
        p: p.add(ex),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      }).sub(
        sdfEval({
          p: p.sub(ex),
          baseOffset: baseOff,
          volumeBaseIdx: volBase,
          bodyIdx: bi,
        }),
      ),
      sdfEval({
        p: p.add(ey),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      }).sub(
        sdfEval({
          p: p.sub(ey),
          baseOffset: baseOff,
          volumeBaseIdx: volBase,
          bodyIdx: bi,
        }),
      ),
      sdfEval({
        p: p.add(ez),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      }).sub(
        sdfEval({
          p: p.sub(ez),
          baseOffset: baseOff,
          volumeBaseIdx: volBase,
          bodyIdx: bi,
        }),
      ),
    ),
  );

  const nView = normalize(viewMatU.mul(vec4(n, 0.0)).xyz);
  const viewDir = normalize(viewMatU.mul(vec4(viewDirWorld, 0.0)).xyz);

  const matBase = bi.mul(MAT_STRIDE);
  const cr = materialBuf.element(matBase.add(MAT_COLOR_R));
  const cg = materialBuf.element(matBase.add(MAT_COLOR_G));
  const cb = materialBuf.element(matBase.add(MAT_COLOR_B));
  const baseColor = vec3(cr, cg, cb);
  const roughness = materialBuf.element(matBase.add(MAT_ROUGHNESS));
  const metalness = materialBuf.element(matBase.add(MAT_METALNESS));
  const er = materialBuf.element(matBase.add(MAT_EMISSIVE_R));
  const eg = materialBuf.element(matBase.add(MAT_EMISSIVE_G));
  const eb = materialBuf.element(matBase.add(MAT_EMISSIVE_B));
  const emissive = vec3(er, eg, eb);
  const color = pbrShade({
    nView,
    viewDir,
    baseColor,
    roughness,
    metalness,
    emissive,
  }).toVar();

  // Hemisphere AO (see SDF_AO_TAPS): accumulate, per ray and tap, how
  // far the SDF falls short of the UNOCCLUDED expectation. Over an
  // infinite flat surface a ray tilted by θ from the normal sees
  // sdf = dist·cos(θ), not dist — so that (not dist itself) is the
  // zero-occlusion baseline; comparing against dist would darken
  // every flat surface by a constant ~40% and wash out the cavity
  // contrast this pass exists for.
  //
  // Both loops MUST be runtime TSL Loops so there is exactly ONE
  // sdfEval call site here: every call site inlines the entire VM
  // interpreter into the shader, and unrolling 4×3 copies blows the
  // WGSL module up far enough to hang the shader compiler (observed
  // as "WebGPU Device Lost" mid-compile on the vase).
  const helper = select(
    abs(n.y).greaterThan(float(0.9)),
    vec3(1, 0, 0),
    vec3(0, 1, 0),
  );
  const t1 = normalize(cross(n, helper));
  const t2 = cross(n, t1);
  const occ = float(0).toVar();
  const rayCount = 4;
  Loop(int(rayCount), ({ i: ray }) => {
    const az = float(ray.sub(int(1))).mul((2 * Math.PI) / 3);
    const tilted = normalize(
      n
        .mul(0.6)
        .add(t1.mul(cos(az).mul(0.8)))
        .add(t2.mul(sin(az).mul(0.8))),
    );
    const dir = select(ray.equal(int(0)), n, tilted).toVar();
    Loop(int(SDF_AO_TAPS.length), ({ i: tap }) => {
      const [dist, weight] = SDF_AO_TAPS.reduceRight(
        ([accDist, accWeight], [tDist, tWeight], idx) => [
          select(tap.equal(int(idx)), float(tDist), accDist),
          select(tap.equal(int(idx)), float(tWeight), accWeight),
        ],
        [float(0), float(0)],
      );
      const d = sdfEval({
        p: p.add(dir.mul(dist)),
        baseOffset: baseOff,
        volumeBaseIdx: volBase,
        bodyIdx: bi,
      });
      const expected = dist.mul(dir.dot(n));
      occ.addAssign(
        clamp(expected.sub(d).div(expected), float(0), float(1)).mul(weight),
      );
    });
  });
  const ao = clamp(
    float(1).sub(occ.mul(SDF_AO_STRENGTH)),
    float(SDF_AO_FLOOR),
    float(1),
  );
  color.assign(color.mul(ao));

  if (selectionBuf) {
    const sel = selectionBuf.element(bi);
    If(sel.greaterThan(float(0.5)), () => {
      color.assign(mix(color, vec3(0.541, 0.706, 0.973), float(0.35)));
    });
  }

  const bodyOpacity = materialBuf.element(matBase.add(MAT_OPACITY));
  const wf = materialBuf.element(matBase.add(MAT_WIREFRAME));
  const alpha = bodyOpacity.toVar();
  If(wf.greaterThan(float(0.5)), () => {
    const fresnel = float(1).sub(abs(n.dot(viewDirWorld)));
    const edgeFactor = smoothstep(
      float(FRESNEL_EDGE_MIN),
      float(FRESNEL_EDGE_MAX),
      fresnel,
    );
    const gridSize = float(WIREFRAME_GRID_SIZE);
    const gx = min(
      fract(p.x.div(gridSize)),
      float(1).sub(fract(p.x.div(gridSize))),
    );
    const gy = min(
      fract(p.y.div(gridSize)),
      float(1).sub(fract(p.y.div(gridSize))),
    );
    const gz = min(
      fract(p.z.div(gridSize)),
      float(1).sub(fract(p.z.div(gridSize))),
    );
    const minGrid = min(gx, min(gy, gz));
    const gridAlpha = float(1).sub(
      smoothstep(float(0), float(WIREFRAME_GRID_THRESHOLD), minGrid),
    );
    alpha.assign(max(edgeFactor, gridAlpha).mul(bodyOpacity));
  });

  return vec4(color, alpha);
}
