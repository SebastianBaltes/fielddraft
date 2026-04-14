/**
 * P2.8 — FieldScript runtime for assembly kinematic controllers.
 *
 * A FieldScript is a plain JavaScript function body that receives
 * a context object with access to assembly joints (mates). The
 * script declares parameters via a `@param` header that the UI
 * renders as sliders/buttons.
 *
 * Example script:
 * ```
 * // @param angle: number = 0 [min: -90, max: 90]
 * // @param speed: number = 1 [min: 0.1, max: 10]
 *
 * ctx.setJointAngle("joint_0", params.angle * Math.sin(ctx.time * params.speed));
 * ```
 *
 * The runtime evaluates scripts deterministically (no async,
 * no DOM access). Parameters are extracted from header comments
 * and rendered as form controls by the host UI.
 */

// ── Parameter parsing ───────────────────────────────────────────

export interface ScriptParam {
  name: string;
  type: "number" | "boolean";
  defaultValue: number | boolean;
  min?: number;
  max?: number;
}

const PARAM_RE = /^\/\/\s*@param\s+(\w+)\s*:\s*(number|boolean)\s*=\s*([^\[]+?)(?:\[(.+)\])?\s*$/;

export function parseScriptParams(source: string): ScriptParam[] {
  const params: ScriptParam[] = [];
  for (const line of source.split("\n")) {
    const m = PARAM_RE.exec(line.trim());
    if (!m) continue;
    const [, name, type, rawDefault, rawMeta] = m;
    const param: ScriptParam = {
      name,
      type: type as "number" | "boolean",
      defaultValue: type === "boolean" ? rawDefault.trim() === "true" : Number(rawDefault.trim()),
    };
    if (rawMeta) {
      const minMatch = /min:\s*([\d.\-]+)/.exec(rawMeta);
      const maxMatch = /max:\s*([\d.\-]+)/.exec(rawMeta);
      if (minMatch) param.min = Number(minMatch[1]);
      if (maxMatch) param.max = Number(maxMatch[1]);
    }
    params.push(param);
  }
  return params;
}

// ── Script context ──────────────────────────────────────────────

export interface ScriptContext {
  time: number;
  dt: number;
  setJointAngle: (mateId: string, angleDeg: number) => void;
  getJointAngle: (mateId: string) => number;
}

// ── Script execution ────────────────────────────────────────────

export interface ScriptResult {
  success: boolean;
  error?: string;
  jointUpdates: Map<string, number>;
}

export function executeScript(
  source: string,
  params: Record<string, number | boolean>,
  time: number,
  dt: number,
  currentAngles: Map<string, number>,
): ScriptResult {
  const jointUpdates = new Map<string, number>();

  const ctx: ScriptContext = {
    time,
    dt,
    setJointAngle: (mateId: string, angleDeg: number) => {
      jointUpdates.set(mateId, angleDeg);
    },
    getJointAngle: (mateId: string) => {
      return currentAngles.get(mateId) ?? 0;
    },
  };

  try {
    const fn = new Function("ctx", "params", source);
    fn(ctx, params);
    return { success: true, jointUpdates };
  } catch (e) {
    return {
      success: false,
      error: e instanceof Error ? e.message : String(e),
      jointUpdates,
    };
  }
}

// ── Default script template ─────────────────────────────────────

export const DEFAULT_SCRIPT = `// @param angle: number = 0 [min: -180, max: 180]
// @param speed: number = 1 [min: 0.1, max: 10]

// Oscillate the first revolute joint
const mates = Object.keys(params);
if (mates.length > 0) {
  ctx.setJointAngle(mates[0], params.angle * Math.sin(ctx.time * params.speed));
}
`;
