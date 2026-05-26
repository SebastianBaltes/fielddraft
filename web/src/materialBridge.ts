import type { Kernel, FeatureSummary } from "./kernel";
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

export interface MaterialBufferTarget {
  colorData: Float32Array;
  roughnessData: Float32Array;
  metalnessData: Float32Array;
  emissiveData: Float32Array;
  opacityData: Float32Array;
  wireframeData: Float32Array;
  displaceData: Float32Array;
}

export function applyMaterials(
  target: MaterialBufferTarget,
  materials: Map<number, MaterialProps>,
  bodyCount: number,
): void {
  for (let i = 0; i < bodyCount; i++) {
    const mat = materials.get(i);
    if (mat) {
      target.colorData[i * 3] = mat.color[0];
      target.colorData[i * 3 + 1] = mat.color[1];
      target.colorData[i * 3 + 2] = mat.color[2];
      target.roughnessData[i] = mat.roughness;
      target.metalnessData[i] = mat.metalness;
      target.emissiveData[i * 3] = mat.emissive[0];
      target.emissiveData[i * 3 + 1] = mat.emissive[1];
      target.emissiveData[i * 3 + 2] = mat.emissive[2];
      target.opacityData[i] = mat.opacity;
      target.wireframeData[i] = mat.wireframe ? 1 : 0;
      target.displaceData[i * 2] = mat.displaceDepth;
      target.displaceData[i * 2 + 1] = mat.displaceFreq;
    } else {
      const [r, g, b] = BODY_COLORS[i % BODY_COLORS.length];
      target.colorData[i * 3] = r;
      target.colorData[i * 3 + 1] = g;
      target.colorData[i * 3 + 2] = b;
      target.roughnessData[i] = DEFAULTS.roughness;
      target.metalnessData[i] = DEFAULTS.metalness;
      target.emissiveData[i * 3] = 0;
      target.emissiveData[i * 3 + 1] = 0;
      target.emissiveData[i * 3 + 2] = 0;
      target.opacityData[i] = 1;
      target.wireframeData[i] = 0;
      target.displaceData[i * 2] = 0;
      target.displaceData[i * 2 + 1] = 0;
    }
  }
}
