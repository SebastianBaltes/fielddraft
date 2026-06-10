/**
 * Pure math + resolution logic for the extrude depth gizmo
 * (ticket jn3t). The React/three component lives in
 * `ExtrudeGizmo.tsx`; everything here is DOM-free and unit-tested.
 *
 * Anchor model: a committed extrude owns the prism faces it created
 * (BRepFace.featureId === extrude feature id). Its TIP face — the cap
 * the arrow sits on — is the planar owned face most of whose sibling
 * owned faces are perpendicular to it (the prism side walls). The
 * original base face is consumed by the boolean, so it cannot anchor
 * the gizmo; the tip works for create AND re-select.
 */

import {
  localToWorld,
  type BRepEdge,
  type BRepFace,
  type Placement,
} from "./kernel";

export type Vec3 = [number, number, number];

const dot = (a: Vec3, b: Vec3): number =>
  a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

/** Rotate a body-local direction into world space (placement rotation
 *  only — normals ignore translate, and the placement scale is uniform). */
export function rotateToWorld(n: Vec3, pl: Placement): Vec3 {
  const [qx, qy, qz, qw] = pl.rotation;
  const tx = 2 * (qy * n[2] - qz * n[1]);
  const ty = 2 * (qz * n[0] - qx * n[2]);
  const tz = 2 * (qx * n[1] - qy * n[0]);
  return [
    n[0] + qw * tx + (qy * tz - qz * ty),
    n[1] + qw * ty + (qz * tx - qx * tz),
    n[2] + qw * tz + (qx * ty - qy * tx),
  ];
}

export interface TipFaceCandidate {
  /** World-space unit normal. */
  normal: Vec3;
  /** World-space centroid. */
  centroid: Vec3;
  /** Index back into the caller's face list. */
  index: number;
}

/**
 * Pick the tip face among the planar faces an extrude feature owns:
 * the candidate with the most perpendicular siblings (a quad prism cap
 * sees 4 side walls; each side wall only sees 3). Ties (cap vs. base of
 * an unmerged "New" prism) break toward the larger `centroid·normal` —
 * the outward-most cap. Returns `null` for an empty list.
 */
export function pickTipFace(
  candidates: ReadonlyArray<TipFaceCandidate>,
): TipFaceCandidate | null {
  if (candidates.length === 0) return null;
  if (candidates.length === 1) return candidates[0];
  let best: TipFaceCandidate | null = null;
  let bestPerp = -1;
  let bestOut = -Infinity;
  for (const c of candidates) {
    let perp = 0;
    for (const o of candidates) {
      if (o === c) continue;
      if (Math.abs(dot(c.normal, o.normal)) < 0.3) perp++;
    }
    const out = dot(c.centroid, c.normal);
    if (perp > bestPerp || (perp === bestPerp && out > bestOut)) {
      best = c;
      bestPerp = perp;
      bestOut = out;
    }
  }
  return best;
}

/**
 * Parameter (signed mm) along the gizmo axis closest to a pointer ray —
 * the classic line–line closest-point solve with unit directions.
 * Returns `null` when ray and axis are near-parallel (denominator → 0),
 * where the param explodes and the drag should hold its last value.
 */
export function axisDepthParam(
  axisOrigin: Vec3,
  axisDir: Vec3,
  rayOrigin: Vec3,
  rayDir: Vec3,
): number | null {
  const b = dot(axisDir, rayDir);
  const denom = 1 - b * b;
  if (Math.abs(denom) < 1e-6) return null;
  const w: Vec3 = [
    axisOrigin[0] - rayOrigin[0],
    axisOrigin[1] - rayOrigin[1],
    axisOrigin[2] - rayOrigin[2],
  ];
  const e = dot(rayDir, w);
  const f = dot(axisDir, w);
  return (b * e - f) / denom;
}

// ── Length wire form ({"value": 10, "unit": "mm"}) ───────────────────

const UNIT_TO_MM: Record<string, number> = {
  mm: 1,
  cm: 10,
  m: 1000,
  in: 25.4,
};

/** Read a feature-param Length (wire object, bare number = mm, or a
 *  numeric string) as millimetres. `null` when unparseable. */
export function lengthParamToMm(v: unknown): number | null {
  if (typeof v === "number") return Number.isFinite(v) ? v : null;
  if (typeof v === "string") {
    const n = Number.parseFloat(v);
    return Number.isFinite(n) ? n : null;
  }
  if (v !== null && typeof v === "object") {
    const o = v as { value?: unknown; unit?: unknown };
    const n = Number(o.value);
    const scale = UNIT_TO_MM[String(o.unit)] ?? null;
    if (Number.isFinite(n) && scale !== null) return n * scale;
  }
  return null;
}

/** Re-encode `mm` in the same wire shape (and unit) as the original
 *  param value, so a drag never silently rewrites the user's unit. */
export function lengthParamWithMm(orig: unknown, mm: number): unknown {
  if (orig !== null && typeof orig === "object") {
    const unit = String((orig as { unit?: unknown }).unit ?? "mm");
    const scale = UNIT_TO_MM[unit] ?? 1;
    return { value: mm / scale, unit: UNIT_TO_MM[unit] ? unit : "mm" };
  }
  return { value: mm, unit: "mm" };
}

// ── anchor resolution against the live kernel ────────────────────────

/** The kernel slice the anchor resolver reads. */
export interface ExtrudeAnchorKernel {
  bodyCount(): number;
  bodyBrepFaces(i: number): BRepFace[];
  bodyBrepEdges(i: number): BRepEdge[];
  bodyPlacement(i: number): Placement;
}

export interface ExtrudeAnchor {
  /** Tip-face centroid, world space. */
  origin: Vec3;
  /** Outward unit normal of the tip face, world space. */
  normal: Vec3;
  /** Tip-face boundary polylines (world space, flat xyz). */
  outline: number[][];
}

/**
 * Locate the tip face of a committed extrude in the current B-Rep.
 * Returns `null` while the feature contributes no resolvable planar
 * face (suppressed, rolled back, consumed by later booleans, …).
 */
export function resolveExtrudeAnchor(
  kernel: ExtrudeAnchorKernel,
  featureId: number,
): ExtrudeAnchor | null {
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let placement: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      placement = kernel.bodyPlacement(i);
    } catch {
      continue;
    }
    const owned = faces.filter(
      (f) => f.featureId === featureId && f.surfaceKind === "plane",
    );
    if (owned.length === 0) continue;
    const candidates: TipFaceCandidate[] = owned.map((f, idx) => {
      const n = rotateToWorld(f.normalLocal, placement);
      const len = Math.hypot(n[0], n[1], n[2]) || 1;
      return {
        normal: [n[0] / len, n[1] / len, n[2] / len] as Vec3,
        centroid: localToWorld(f.centroidLocal, placement),
        index: idx,
      };
    });
    const tip = pickTipFace(candidates);
    if (!tip) continue;
    const tipFace = owned[tip.index];
    let edges: BRepEdge[] = [];
    try {
      edges = kernel.bodyBrepEdges(i);
    } catch {
      // outline optional — arrow still works
    }
    const outline = edges
      .filter((e) => e.a === tipFace.branchId || e.b === tipFace.branchId)
      .map((e) => e.points)
      .filter((p) => p.length >= 6);
    return { origin: tip.centroid, normal: tip.normal, outline };
  }
  return null;
}
