import type { Kernel, FeatureSummary } from "./kernel";
import type { DeferredPipelineState } from "./sdfDeferredRenderer";
import type { VmPipelineState } from "./sdfVmRenderer";
import { BODY_COLORS } from "./sdfDeferredRenderer";
import { DEFAULT_ROUGHNESS, DEFAULT_METALNESS, DEFAULT_BASE_COLOR } from "./pbrShading";

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

export function resolveMaterials(
  kernel: Kernel,
  features: FeatureSummary[],
): Map<number, MaterialProps> {
  const result = new Map<number, MaterialProps>();

  const materialFeatures = features.filter((f) => f.kind === "material");
  if (materialFeatures.length === 0) return result;

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

  return result;
}

export function applyMaterialsToDeferred(
  pipeline: DeferredPipelineState,
  materials: Map<number, MaterialProps>,
  bodyCount: number,
): void {
  for (let i = 0; i < bodyCount; i++) {
    const mat = materials.get(i);
    if (mat) {
      pipeline.colorData[i * 3] = mat.color[0];
      pipeline.colorData[i * 3 + 1] = mat.color[1];
      pipeline.colorData[i * 3 + 2] = mat.color[2];
      pipeline.roughnessData[i] = mat.roughness;
      pipeline.metalnessData[i] = mat.metalness;
      pipeline.emissiveData[i * 3] = mat.emissive[0];
      pipeline.emissiveData[i * 3 + 1] = mat.emissive[1];
      pipeline.emissiveData[i * 3 + 2] = mat.emissive[2];
      pipeline.opacityData[i] = mat.opacity;
      pipeline.wireframeData[i] = mat.wireframe ? 1 : 0;
      pipeline.displaceDepthData[i] = mat.displaceDepth;
      pipeline.displaceFreqData[i] = mat.displaceFreq;
    } else {
      const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
      pipeline.colorData[i * 3] = r;
      pipeline.colorData[i * 3 + 1] = g;
      pipeline.colorData[i * 3 + 2] = b;
      pipeline.roughnessData[i] = DEFAULTS.roughness;
      pipeline.metalnessData[i] = DEFAULTS.metalness;
      pipeline.emissiveData[i * 3] = 0;
      pipeline.emissiveData[i * 3 + 1] = 0;
      pipeline.emissiveData[i * 3 + 2] = 0;
      pipeline.opacityData[i] = 1;
      pipeline.wireframeData[i] = 0;
      pipeline.displaceDepthData[i] = 0;
      pipeline.displaceFreqData[i] = 0;
    }
  }
}

export function applyMaterialsToVm(
  state: VmPipelineState,
  materials: Map<number, MaterialProps>,
  bodyCount: number,
): void {
  for (let i = 0; i < bodyCount; i++) {
    const mat = materials.get(i);
    if (mat) {
      state.colorData[i * 3] = mat.color[0];
      state.colorData[i * 3 + 1] = mat.color[1];
      state.colorData[i * 3 + 2] = mat.color[2];
      state.roughnessData[i] = mat.roughness;
      state.metalnessData[i] = mat.metalness;
      state.emissiveData[i * 3] = mat.emissive[0];
      state.emissiveData[i * 3 + 1] = mat.emissive[1];
      state.emissiveData[i * 3 + 2] = mat.emissive[2];
      state.opacityData[i] = mat.opacity;
      state.wireframeData[i] = mat.wireframe ? 1 : 0;
      state.displaceDepthData[i] = mat.displaceDepth;
      state.displaceFreqData[i] = mat.displaceFreq;
    } else {
      const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
      state.colorData[i * 3] = r;
      state.colorData[i * 3 + 1] = g;
      state.colorData[i * 3 + 2] = b;
      state.roughnessData[i] = DEFAULTS.roughness;
      state.metalnessData[i] = DEFAULTS.metalness;
      state.emissiveData[i * 3] = 0;
      state.emissiveData[i * 3 + 1] = 0;
      state.emissiveData[i * 3 + 2] = 0;
      state.opacityData[i] = 1;
      state.wireframeData[i] = 0;
      state.displaceDepthData[i] = 0;
      state.displaceFreqData[i] = 0;
    }
  }
}
