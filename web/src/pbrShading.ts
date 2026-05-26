// @ts-nocheck — TSL's TypeScript surface in three r183 is intentionally loose.

import {
  Fn,
  float,
  max,
  mix,
  normalize,
  vec3,
} from "three/tsl";

const PI = 3.141592653589793;

export const DEFAULT_ROUGHNESS = 0.35;
export const DEFAULT_METALNESS = 0.0;
export const DEFAULT_BASE_COLOR: [number, number, number] = [0.55, 0.65, 0.85];

export const LIGHTS = [
  { dir: [0.2, 0.3, 1.0], radiance: 3.0 },
  { dir: [-0.5, 0.1, 0.6], radiance: 0.5 },
  { dir: [0.0, -0.2, -1.0], radiance: 0.3 },
];

export const AMBIENT = 0.25;

/**
 * PBR shade a surface point (Cook-Torrance BRDF).
 *
 * @param nView     — view-space surface normal (normalized)
 * @param viewDir   — view-space direction toward camera (normalized)
 * @param baseColor — albedo (linear RGB)
 * @param roughness — perceptual roughness [0..1]
 * @param metalness — metallic factor [0..1]
 * @param emissive  — emissive colour (linear RGB, added after lighting)
 */
export const pbrShade = Fn(({ nView, viewDir, baseColor, roughness, metalness, emissive }: any) => {
  const rough = roughness ?? float(DEFAULT_ROUGHNESS);
  const metal = metalness ?? float(DEFAULT_METALNESS);
  const emis = emissive ?? vec3(0);

  const alpha = rough.mul(rough);
  const alpha2 = alpha.mul(alpha);

  // Metallic workflow: dielectric F0 = 0.04, metal F0 = baseColor
  const f0 = mix(vec3(0.04), baseColor, metal);

  const NdotV = max(nView.dot(viewDir), float(0.001));
  const Lo = vec3(0).toVar();

  const r1 = rough.add(1);
  const k = r1.mul(r1).div(8);

  for (const { dir, radiance } of LIGHTS) {
    const L = normalize(vec3(dir[0], dir[1], dir[2]));
    const rad = vec3(radiance);
    const H = normalize(L.add(viewDir));
    const NdotL = max(nView.dot(L), float(0));
    const NdotH = max(nView.dot(H), float(0));
    const VdotH = max(viewDir.dot(H), float(0));

    const NdotH2 = NdotH.mul(NdotH);
    const dTerm = NdotH2.mul(alpha2.sub(1)).add(1);
    const D = alpha2.div(float(PI).mul(dTerm).mul(dTerm));

    const G1V = NdotV.div(NdotV.mul(float(1).sub(k)).add(k));
    const G1L = NdotL.div(NdotL.mul(float(1).sub(k)).add(k));
    const G = G1V.mul(G1L);

    const t = float(1).sub(VdotH).clamp(0, 1);
    const t2 = t.mul(t);
    const t5 = t2.mul(t2).mul(t);
    const F = f0.add(vec3(1).sub(f0).mul(t5));

    const specDenom = float(4).mul(NdotV).mul(NdotL).max(0.001);
    const spec = D.mul(G).mul(F).div(specDenom);

    // Metals have no diffuse; dielectrics attenuate by (1-F)
    const kD = vec3(1).sub(F).mul(float(1).sub(metal));
    const diffuse = baseColor.div(float(PI));

    Lo.addAssign(kD.mul(diffuse).add(spec).mul(rad).mul(NdotL));
  }

  const ambient = baseColor.mul(AMBIENT);
  return Lo.add(ambient).add(emis);
});
