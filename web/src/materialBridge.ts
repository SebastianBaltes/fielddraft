import type { Kernel, FeatureSummary } from "./kernel";
import { BODY_COLORS } from "./bodyColors";
import { DEFAULT_ROUGHNESS, DEFAULT_METALNESS, DEFAULT_BASE_COLOR } from "./pbrShading";
import {
  MAT_STRIDE,
  MAT_COLOR_R, MAT_COLOR_G, MAT_COLOR_B,
  MAT_ROUGHNESS, MAT_METALNESS,
  MAT_EMISSIVE_R, MAT_EMISSIVE_G, MAT_EMISSIVE_B,
  MAT_OPACITY, MAT_WIREFRAME,
  MAT_DISPLACE_DEPTH, MAT_DISPLACE_FREQ,
} from "./sdfPipelineCommon";

export interface MaterialProps {
  color: [number, number, number];
  roughness: number;
  metalness: number;
  emissive: [number, number, number];
  opacity: number;
  wireframe: boolean;
  displaceDepth: number;
  displaceFreq: number;
}

const DEFAULTS: MaterialProps = {
  color: DEFAULT_BASE_COLOR,
  roughness: DEFAULT_ROUGHNESS,
  metalness: DEFAULT_METALNESS,
  emissive: [0, 0, 0],
  opacity: 1,
  wireframe: false,
  displaceDepth: 0,
  displaceFreq: 0,
};

/// Translucent red preview for a body whose feature-implicit boolean has no
/// valid merge scope (see the kernel `feature_warnings`). Overrides any user
/// material so the unconnected body is unmistakable, mirroring Onshape.
const WARNING_MATERIAL: MaterialProps = {
  color: [0.78, 0.12, 0.12],
  roughness: 0.5,
  metalness: 0,
  emissive: [0.12, 0.0, 0.0],
  opacity: 0.45,
  wireframe: false,
  displaceDepth: 0,
  displaceFreq: 0,
};

export function resolveMaterials(
  kernel: Kernel,
  features: FeatureSummary[],
): Map<number, MaterialProps> {
  const result = new Map<number, MaterialProps>();

  const materialFeatures = features.filter((f) => f.kind === "material");
  const warningFeatures = features.filter((f) => f.warning && !f.suppressed);
  if (materialFeatures.length === 0 && warningFeatures.length === 0) return result;

  const bodyCount = kernel.bodyCount();
  const seedToBody = new Map<number, number>();
  for (let i = 0; i < bodyCount; i++) {
    seedToBody.set(kernel.bodySeedFeatureId(i), i);
  }

  for (const mf of materialFeatures) {
    if (mf.suppressed) continue;
    let params: Record<string, unknown>;
    try {
      params = kernel.getFeatureCallParams(mf.id);
    } catch {
      continue;
    }

    const raw = params.targets as string | string[] | undefined;
    const queries: string[] = Array.isArray(raw)
      ? raw
      : typeof raw === "string" && raw.length > 0
        ? raw.split(",").map((s) => s.trim()).filter(Boolean)
        : [];

    const targetBodyIndices: number[] = [];
    for (const q of queries) {
      const m = typeof q === "string" && q.match(/^qCreatedBy\((\d+)\)$/);
      if (!m) continue;
      const featureId = Number(m[1]);
      const bodyIdx = seedToBody.get(featureId);
      if (bodyIdx !== undefined) targetBodyIndices.push(bodyIdx);
    }

    if (targetBodyIndices.length === 0) continue;

    const color = (params.color ?? params.Color) as [number, number, number] | undefined;
    const roughness = (params.roughness ?? params.Roughness) as number | undefined;
    const metalness = (params.metalness ?? params.Metalness) as number | undefined;
    const emissive = (params.emissive ?? params.Emissive) as [number, number, number] | undefined;
    const opacity = (params.opacity ?? params.Opacity) as number | undefined;
    const wireframe = (params.wireframe ?? params.Wireframe) as boolean | undefined;
    const displaceDepth = (params.displace_depth ?? params["Displace depth"]) as number | undefined;
    const displaceFreq = (params.displace_freq ?? params["Displace freq"]) as number | undefined;

    const mat: MaterialProps = {
      color: color ?? DEFAULTS.color,
      roughness: roughness ?? DEFAULTS.roughness,
      metalness: metalness ?? DEFAULTS.metalness,
      emissive: emissive ?? DEFAULTS.emissive,
      opacity: opacity ?? DEFAULTS.opacity,
      wireframe: wireframe ?? DEFAULTS.wireframe,
      displaceDepth: displaceDepth ?? DEFAULTS.displaceDepth,
      displaceFreq: displaceFreq ?? DEFAULTS.displaceFreq,
    };

    for (const bi of targetBodyIndices) {
      result.set(bi, mat);
    }
  }

  // Merge-scope warnings win over any user material: paint the unconnected
  // body translucent red so the missing scope is obvious in the viewport.
  for (const wf of warningFeatures) {
    const bodyIdx = seedToBody.get(wf.id);
    if (bodyIdx !== undefined) result.set(bodyIdx, WARNING_MATERIAL);
  }

  return result;
}

export interface MaterialBufferTarget {
  materialData: Float32Array;
  materialAttr: { needsUpdate: boolean };
}

export function applyMaterials(
  target: MaterialBufferTarget,
  materials: Map<number, MaterialProps>,
  bodyCount: number,
  /** Global opacity multiplier applied to every body (Debug ▸ Mesher part-
   *  visibility slider; 1 = unchanged). */
  opacityScale = 1,
): void {
  const d = target.materialData;
  for (let i = 0; i < bodyCount; i++) {
    const base = i * MAT_STRIDE;
    const mat = materials.get(i);
    if (mat) {
      d[base + MAT_COLOR_R] = mat.color[0];
      d[base + MAT_COLOR_G] = mat.color[1];
      d[base + MAT_COLOR_B] = mat.color[2];
      d[base + MAT_ROUGHNESS] = mat.roughness;
      d[base + MAT_METALNESS] = mat.metalness;
      d[base + MAT_EMISSIVE_R] = mat.emissive[0];
      d[base + MAT_EMISSIVE_G] = mat.emissive[1];
      d[base + MAT_EMISSIVE_B] = mat.emissive[2];
      d[base + MAT_OPACITY] = mat.opacity * opacityScale;
      d[base + MAT_WIREFRAME] = mat.wireframe ? 1 : 0;
      d[base + MAT_DISPLACE_DEPTH] = mat.displaceDepth;
      d[base + MAT_DISPLACE_FREQ] = mat.displaceFreq;
    } else {
      const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
      d[base + MAT_COLOR_R] = r;
      d[base + MAT_COLOR_G] = g;
      d[base + MAT_COLOR_B] = b;
      d[base + MAT_ROUGHNESS] = DEFAULTS.roughness;
      d[base + MAT_METALNESS] = DEFAULTS.metalness;
      d[base + MAT_EMISSIVE_R] = 0;
      d[base + MAT_EMISSIVE_G] = 0;
      d[base + MAT_EMISSIVE_B] = 0;
      d[base + MAT_OPACITY] = opacityScale;
      d[base + MAT_WIREFRAME] = 0;
      d[base + MAT_DISPLACE_DEPTH] = 0;
      d[base + MAT_DISPLACE_FREQ] = 0;
    }
  }
  target.materialAttr.needsUpdate = true;
}
