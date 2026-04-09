// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally
// loose; the framework expects you to script TSL in plain JS. We keep
// this single file un-typed to follow that contract, while every other
// file in the project remains strict-typed.

/**
 * SDF raymarching pass via Three.js TSL.
 *
 * The kernel maintains a packed `Float32Array` of features (4 vec4s
 * per feature, see `FEATURE_STRIDE` and the layout doc in
 * `crates/kernel/src/lib.rs`). On each frame this module copies the
 * view into a `uniformArray<vec4>` and lets a TSL `Fn` evaluate the
 * composite SDF on the GPU using sphere tracing.
 *
 * The pipeline:
 *   1. A `BackSide` cube the size of the build volume is mounted in
 *      the world. Its `NodeMaterial.colorNode` overrides the fragment
 *      shading to sphere-trace the SDF from the camera through the
 *      fragment.
 *   2. The mesh is rendered with `depthTest: false` so the camera can
 *      sit inside it.
 *   3. Conceptual reference: `three/addons/tsl/utils/Raymarching.js`,
 *      adapted from volume marching to SDF sphere tracing.
 */

import * as THREE from "three";
import {
  Break,
  Fn,
  If,
  Loop,
  abs,
  cameraPosition,
  float,
  int,
  length,
  max,
  min,
  modelWorldMatrix,
  normalize,
  positionGeometry,
  uniform,
  uniformArray,
  vec2,
  vec3,
  vec4,
} from "three/tsl";
import { MeshBasicNodeMaterial } from "three/webgpu";
import { FEATURE_STRIDE } from "./kernel";

export const MAX_RAYMARCH_FEATURES = 32;
const VEC4S_PER_FEATURE = 4; // FEATURE_STRIDE / 4 = 16 / 4
const MAX_VEC4S = MAX_RAYMARCH_FEATURES * VEC4S_PER_FEATURE;

/** World-space size of the raymarched bounding cube (mm). The build
 *  volume is 300 mm cubed; we double that to give some breathing room
 *  for translated features. */
export const RAYMARCH_VOLUME_SIZE = 600;

// Sphere tracing: in practice <40 steps converge for any reasonable
// SDF. We keep this conservative because headless WebGPU on swiftshader
// is *very* slow in CI / Playwright runs, and every step matters there.
const MAX_STEPS = 48;
const SURFACE_EPS = 0.1;
const MAX_DIST = RAYMARCH_VOLUME_SIZE * 2;
const NORMAL_EPS = 0.5;

export interface RaymarchHandle {
  material: MeshBasicNodeMaterial;
  /** Mutable vec4 slots backing the uniform array. */
  sceneSlots: THREE.Vector4[];
  /** Single uniform tracking how many features are live. */
  featureCount: { value: number };
}

export function createRaymarchMaterial(): RaymarchHandle {
  // Allocate the GPU-backing array once. We mutate the same Vector4
  // instances every frame; UniformArrayNode picks up the changes
  // because its updateType is "render".
  const sceneSlots: THREE.Vector4[] = [];
  for (let i = 0; i < MAX_VEC4S; i++) {
    sceneSlots.push(new THREE.Vector4(0, 0, 0, 0));
  }
  const sceneArray = uniformArray(sceneSlots, "vec4");
  const featureCount = uniform(0, "int");

  // ── per-primitive SDFs (in primitive-local space) ─────────────────
  const sdfBox = Fn(({ p, halfExtents }) => {
    const q = abs(p).sub(halfExtents).toVar();
    const outside = max(q, vec3(0)).length();
    const inside = min(max(q.x, max(q.y, q.z)), float(0));
    return outside.add(inside);
  });

  const sdfSphere = Fn(({ p, radius }) => length(p).sub(radius));

  const sdfCylinder = Fn(({ p, radius, halfHeight }) => {
    const radial = length(vec2(p.x, p.z)).sub(radius);
    const axial = abs(p.y).sub(halfHeight);
    const outside = length(
      vec2(max(radial, float(0)), max(axial, float(0))),
    );
    const inside = min(max(radial, axial), float(0));
    return outside.add(inside);
  });

  // Inverse-rotate `v` by quaternion `q` (using its conjugate).
  // Mirrors `quat_inverse_rotate` in crates/kernel/src/lib.rs.
  const quatInverseRotate = Fn(({ q, v }) => {
    const qx = q.x.negate();
    const qy = q.y.negate();
    const qz = q.z.negate();
    const qw = q.w;
    const tx = qy.mul(v.z).sub(qz.mul(v.y)).mul(2);
    const ty = qz.mul(v.x).sub(qx.mul(v.z)).mul(2);
    const tz = qx.mul(v.y).sub(qy.mul(v.x)).mul(2);
    return vec3(
      v.x.add(qw.mul(tx)).add(qy.mul(tz)).sub(qz.mul(ty)),
      v.y.add(qw.mul(ty)).add(qz.mul(tx)).sub(qx.mul(tz)),
      v.z.add(qw.mul(tz)).add(qx.mul(ty)).sub(qy.mul(tx)),
    );
  });

  // Eval one feature's SDF at a world point.
  const sdfFeature = Fn(({ p, v0, v1, v2, v3 }) => {
    const kind = v0.x;
    const center = vec3(v0.y, v0.z, v0.w);
    const params = vec3(v1.x, v1.y, v1.z);
    const translate = vec3(v2.x, v2.y, v2.z);
    const scale = v2.w;
    const rotation = v3;

    // World → primitive-local
    const pTranslated = p.sub(translate);
    const pRotated = quatInverseRotate({ q: rotation, v: pTranslated });
    const pLocal = pRotated.div(scale);
    const pPrim = pLocal.sub(center);

    const dBox = sdfBox({ p: pPrim, halfExtents: params });
    const dSphere = sdfSphere({ p: pPrim, radius: params.x });
    const dCylinder = sdfCylinder({
      p: pPrim,
      radius: params.x,
      halfHeight: params.y,
    });

    const result = float(0).toVar();
    If(kind.lessThan(0.5), () => {
      result.assign(dBox);
    })
      .ElseIf(kind.lessThan(1.5), () => {
        result.assign(dSphere);
      })
      .Else(() => {
        result.assign(dCylinder);
      });
    return result.mul(scale);
  });

  // Composite SDF over the feature spine.
  const sceneSdf = Fn(({ p }) => {
    const acc = float(1e9).toVar();
    Loop(featureCount, ({ i }) => {
      const base = i.mul(VEC4S_PER_FEATURE);
      const v0 = sceneArray.element(base);
      const v1 = sceneArray.element(base.add(1));
      const v2 = sceneArray.element(base.add(2));
      const v3 = sceneArray.element(base.add(3));
      const di = sdfFeature({ p, v0, v1, v2, v3 });
      const op = v1.w;

      If(op.lessThan(0.5), () => {
        // Union
        acc.assign(min(acc, di));
      })
        .ElseIf(op.lessThan(1.5), () => {
          // Difference: max(acc, -di)
          acc.assign(max(acc, di.negate()));
        })
        .Else(() => {
          // Intersect
          acc.assign(max(acc, di));
        });
    });
    return acc;
  });

  // Surface normal via central differences.
  const sceneNormal = Fn(({ p }) => {
    const ex = vec3(NORMAL_EPS, 0, 0);
    const ey = vec3(0, NORMAL_EPS, 0);
    const ez = vec3(0, 0, NORMAL_EPS);
    return normalize(
      vec3(
        sceneSdf({ p: p.add(ex) }).sub(sceneSdf({ p: p.sub(ex) })),
        sceneSdf({ p: p.add(ey) }).sub(sceneSdf({ p: p.sub(ey) })),
        sceneSdf({ p: p.add(ez) }).sub(sceneSdf({ p: p.sub(ez) })),
      ),
    );
  });

  // ── colorNode ─────────────────────────────────────────────────────
  const colorNode = Fn(() => {
    // World-space ray from camera through this fragment.
    const worldPos = modelWorldMatrix.mul(vec4(positionGeometry, 1)).xyz;
    const ro = cameraPosition;
    const rd = normalize(worldPos.sub(ro));

    const t = float(0).toVar();
    const hit = float(0).toVar();
    const hitPos = vec3(0).toVar();

    Loop(int(MAX_STEPS), () => {
      const p = ro.add(rd.mul(t));
      const d = sceneSdf({ p });
      If(d.lessThan(SURFACE_EPS), () => {
        hit.assign(1);
        hitPos.assign(p);
        Break();
      });
      t.addAssign(d);
      If(t.greaterThan(MAX_DIST), () => {
        Break();
      });
    });

    // Background — soft warm light-mode wash, matches the CSS palette.
    const bg = vec3(0.964, 0.964, 0.953);

    const finalColor = bg.toVar();
    If(hit.greaterThan(0.5), () => {
      const n = sceneNormal({ p: hitPos });
      const lightDir = normalize(vec3(0.4, 1, 0.5));
      const diffuse = max(n.dot(lightDir), float(0));
      const ambient = float(0.35);
      const baseColor = vec3(0.55, 0.65, 0.85);
      const lit = baseColor.mul(diffuse.add(ambient));
      finalColor.assign(lit);
    });

    return vec4(finalColor, 1);
  });

  const material = new MeshBasicNodeMaterial();
  material.colorNode = colorNode();
  material.side = THREE.BackSide;
  material.transparent = false;
  material.depthWrite = false;
  material.depthTest = false;
  material.toneMapped = false;

  return { material, sceneSlots, featureCount };
}

/**
 * Copy the kernel's packed feature buffer (16 floats per feature) into
 * the uniform-array slots (4 vec4s per feature).
 */
export function uploadFeatures(
  handle: RaymarchHandle,
  view: Float32Array,
  count: number,
): void {
  const cap = Math.min(count, MAX_RAYMARCH_FEATURES);
  for (let i = 0; i < cap; i++) {
    const base = i * FEATURE_STRIDE;
    const slot = i * VEC4S_PER_FEATURE;
    handle.sceneSlots[slot + 0].set(
      view[base + 0],
      view[base + 1],
      view[base + 2],
      view[base + 3],
    );
    handle.sceneSlots[slot + 1].set(
      view[base + 4],
      view[base + 5],
      view[base + 6],
      view[base + 7],
    );
    handle.sceneSlots[slot + 2].set(
      view[base + 8],
      view[base + 9],
      view[base + 10],
      view[base + 11],
    );
    handle.sceneSlots[slot + 3].set(
      view[base + 12],
      view[base + 13],
      view[base + 14],
      view[base + 15],
    );
  }
  handle.featureCount.value = cap;
}
