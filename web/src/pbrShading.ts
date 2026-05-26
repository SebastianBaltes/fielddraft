// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

/**
 * Shared PBR shading for all FieldDraft renderers (SDF + mesh).
 *
 * Cook-Torrance BRDF with GGX NDF, Smith-Schlick geometry,
 * and Fresnel-Schlick — the same model as Three.js MeshStandardMaterial.
 *
 * All inputs/outputs are in view space (camera-fixed lights).
 */

import {
  Fn,
  float,
  max,
  normalize,
  vec3,
} from "three/tsl";

const PI = 3.141592653589793;

const ROUGHNESS = 0.35;
const F0_DIELECTRIC = 0.04;

const LIGHTS = [
  { dir: [0.2, 0.3, 1.0], radiance: 3.0 },
  { dir: [-0.5, 0.1, 0.6], radiance: 0.5 },
  { dir: [0.0, -0.2, -1.0], radiance: 0.3 },
];

const AMBIENT = 0.25;

/**
 * PBR shade a surface point.
 *
 * @param nView    — view-space surface normal (normalized)
 * @param viewDir  — view-space direction from surface toward camera (normalized)
 * @param baseColor — albedo (linear RGB)
 * @returns lit linear RGB colour (vec3)
 */
export const pbrShade = Fn(({ nView, viewDir, baseColor }: any) => {
  const roughness = float(ROUGHNESS);
  const alpha = roughness.mul(roughness);
  const alpha2 = alpha.mul(alpha);

  const f0 = vec3(F0_DIELECTRIC);

  const NdotV = max(nView.dot(viewDir), float(0.001));
  const Lo = vec3(0).toVar();

  const r1 = roughness.add(1);
  const k = r1.mul(r1).div(8);

  for (const { dir, radiance } of LIGHTS) {
    const L = normalize(vec3(dir[0], dir[1], dir[2]));
    const rad = vec3(radiance);
    const H = normalize(L.add(viewDir));
    const NdotL = max(nView.dot(L), float(0));
    const NdotH = max(nView.dot(H), float(0));
    const VdotH = max(viewDir.dot(H), float(0));

    // D — GGX (Trowbridge-Reitz) normal distribution
    const NdotH2 = NdotH.mul(NdotH);
    const dTerm = NdotH2.mul(alpha2.sub(1)).add(1);
    const D = alpha2.div(float(PI).mul(dTerm).mul(dTerm));

    // G — Smith-Schlick geometry
    const G1V = NdotV.div(NdotV.mul(float(1).sub(k)).add(k));
    const G1L = NdotL.div(NdotL.mul(float(1).sub(k)).add(k));
    const G = G1V.mul(G1L);

    // F — Fresnel-Schlick
    const t = float(1).sub(VdotH).clamp(0, 1);
    const t2 = t.mul(t);
    const t5 = t2.mul(t2).mul(t);
    const F = f0.add(vec3(1).sub(f0).mul(t5));

    // Cook-Torrance specular
    const specDenom = float(4).mul(NdotV).mul(NdotL).max(0.001);
    const spec = D.mul(G).mul(F).div(specDenom);

    // Energy-conserving diffuse (Lambertian)
    const kD = vec3(1).sub(F);
    const diffuse = baseColor.div(float(PI));

    Lo.addAssign(kD.mul(diffuse).add(spec).mul(rad).mul(NdotL));
  }

  const ambient = baseColor.mul(AMBIENT);
  return Lo.add(ambient);
});
