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
 * Pick the tip face among the planar faces an extrude feature owns.
 *
 * Preferred signal — `extrudeLenMm`, the prism's axial extent (depth, or
 * depth+depth_back for one-sided): the two caps are the antiparallel face
 * pair separated by exactly that length along their shared normal, while
 * every wall pair is separated by a profile dimension. This is purely
 * relative geometry, so it holds wherever the prism sits in world space.
 * When a body keeps BOTH caps (an "Add" that produced a standalone solid,
 * not a boolean that consumed the base), the perpendicular-sibling count
 * is a 6-way tie and the old `centroid·normal` tie-break silently picks
 * the world-outermost face — the wrong axis for any off-origin prism
 * (ticket zzl1). The tip is the cap on the grown side: the larger
 * `centroid·normal` of the matched pair.
 *
 * Fallback (no length, or no pair matches it — e.g. the base was consumed
 * so only one cap survives): the candidate with the most perpendicular
 * siblings (a quad prism cap sees 4 side walls; each side wall only 3),
 * ties broken toward the outward-most `centroid·normal`.
 *
 * Returns `null` for an empty list.
 */
export function pickTipFace(
  candidates: ReadonlyArray<TipFaceCandidate>,
  extrudeLenMm?: number,
): TipFaceCandidate | null {
  if (candidates.length === 0) return null;
  if (candidates.length === 1) return candidates[0];

  // Cap-pair-by-depth: find the antiparallel pair whose plane separation
  // best matches the extrude length, then take its grown-side cap.
  if (extrudeLenMm !== undefined && extrudeLenMm > 0) {
    const tol = Math.max(0.05 * extrudeLenMm, 0.01);
    let capTip: TipFaceCandidate | null = null;
    let bestErr = tol;
    for (let i = 0; i < candidates.length; i++) {
      for (let j = i + 1; j < candidates.length; j++) {
        const a = candidates[i];
        const b = candidates[j];
        if (dot(a.normal, b.normal) > -0.99) continue; // not antiparallel
        const sep = Math.abs(
          (a.centroid[0] - b.centroid[0]) * a.normal[0] +
            (a.centroid[1] - b.centroid[1]) * a.normal[1] +
            (a.centroid[2] - b.centroid[2]) * a.normal[2],
        );
        const err = Math.abs(sep - extrudeLenMm);
        if (err < bestErr) {
          bestErr = err;
          capTip = dot(a.centroid, a.normal) >= dot(b.centroid, b.normal) ? a : b;
        }
      }
    }
    if (capTip) return capTip;
  }

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

// ── drag preview bands (ticket qqgo) ─────────────────────────────────
//
// While the arrow is dragged, NOTHING is committed to the kernel — the
// preview is drawn locally as translucent prism "bands": slices of the
// extrusion volume between two signed offsets along the tip normal,
// measured from the drag-start tip (0 = old tip, negative = into the
// body). The full preview prism shows the would-be result; delta bands
// highlight what gets added (grow) or removed (shrink).

export interface PreviewBands {
  /** The whole would-be prism, [from, to] offsets along the normal. */
  prism: [number, number];
  /** Changed slices vs. the drag-start prism (0–2 entries; the back
   *  band only exists for symmetric extrudes, which grow both ways). */
  delta: Array<[number, number]>;
  /** true = previewed depth ≥ drag-start depth (adding material). */
  grow: boolean;
}

export function previewBandRanges(
  startDepthMm: number,
  previewDepthMm: number,
  symmetric: boolean,
): PreviewBands {
  // Symmetric extrudes grow ±depth/2 about the sketch plane, so the tip
  // only travels half of a depth change.
  const factor = symmetric ? 2 : 1;
  const tip = (previewDepthMm - startDepthMm) / factor;
  const back = tip - previewDepthMm;
  const delta: Array<[number, number]> = [];
  const front: [number, number] = [Math.min(0, tip), Math.max(0, tip)];
  if (front[1] - front[0] > 1e-9) delta.push(front);
  const rear: [number, number] = [
    Math.min(-startDepthMm, back),
    Math.max(-startDepthMm, back),
  ];
  if (rear[1] - rear[0] > 1e-9) delta.push(rear);
  return { prism: [back, tip], delta, grow: previewDepthMm >= startDepthMm };
}

/**
 * Triangle soup (flat xyz) for one prism band: the tip outline shifted
 * by `offA`/`offB` along the anchor normal, side walls between the two
 * rings plus centroid-fan caps on both ends. The fan is exact for
 * profiles star-shaped around the centroid (every primitive profile).
 */
export function prismBandTriangles(
  anchor: ExtrudeAnchor,
  offA: number,
  offB: number,
): number[] {
  const [nx, ny, nz] = anchor.normal;
  const ax = nx * offA, ay = ny * offA, az = nz * offA;
  const bx = nx * offB, by = ny * offB, bz = nz * offB;
  const [cx, cy, cz] = anchor.origin;
  const tris: number[] = [];
  for (const pts of anchor.outline) {
    for (let i = 0; i + 5 < pts.length; i += 3) {
      const px = pts[i], py = pts[i + 1], pz = pts[i + 2];
      const qx = pts[i + 3], qy = pts[i + 4], qz = pts[i + 5];
      // Side wall quad between the two rings.
      tris.push(px + ax, py + ay, pz + az, qx + ax, qy + ay, qz + az, qx + bx, qy + by, qz + bz);
      tris.push(px + ax, py + ay, pz + az, qx + bx, qy + by, qz + bz, px + bx, py + by, pz + bz);
      // Caps (centroid fan) on both ends.
      tris.push(cx + ax, cy + ay, cz + az, px + ax, py + ay, pz + az, qx + ax, qy + ay, qz + az);
      tris.push(cx + bx, cy + by, cz + bz, px + bx, py + by, pz + bz, qx + bx, qy + by, qz + bz);
    }
  }
  return tris;
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

/** Parse the idHashes out of a multi-query of `qFace(<idHash>)` terms. */
export function faceQueryHashes(faces: unknown): string[] {
  if (typeof faces !== "string") return [];
  const out: string[] = [];
  for (const m of faces.matchAll(/qFace\(([^)]+)\)/g)) out.push(m[1]);
  return out;
}

/**
 * Anchor for a PENDING (suppressed) face-extrude (ticket x7af). The ghost
 * contributes no B-Rep faces of its own, so the anchor derives from the
 * PROFILE faces it was created from — which still exist unchanged on the
 * pre-extrude body; that body is the extrude's true reference, so the
 * preview measures from the original face rather than from a previously
 * committed partial extrude. The returned anchor mirrors the
 * committed-tip convention (origin on the would-be tip plane, normal
 * pointing tip-out, outline shifted onto that plane) so the band/drag
 * math is shared. `tipOffsetMm` is the SIGNED depth along the profile
 * normal — negative grows the prism in the −normal direction.
 */
export function resolvePendingExtrudeAnchor(
  kernel: ExtrudeAnchorKernel,
  faceIdHashes: string[],
  tipOffsetMm: number,
): ExtrudeAnchor | null {
  if (faceIdHashes.length === 0) return null;
  const wanted = new Set(faceIdHashes);
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
    const matches = faces.filter((f) => wanted.has(f.idHash));
    if (matches.length === 0) continue;
    const n = rotateToWorld(matches[0].normalLocal, placement);
    const len = Math.hypot(n[0], n[1], n[2]) || 1;
    const profileNormal: Vec3 = [n[0] / len, n[1] / len, n[2] / len];
    // Average centroid keeps the band's fan centre inside a coplanar
    // multi-face profile — fine for the translucent preview.
    const centroid: Vec3 = [0, 0, 0];
    for (const f of matches) {
      const c = localToWorld(f.centroidLocal, placement);
      centroid[0] += c[0] / matches.length;
      centroid[1] += c[1] / matches.length;
      centroid[2] += c[2] / matches.length;
    }
    const shift: Vec3 = [
      profileNormal[0] * tipOffsetMm,
      profileNormal[1] * tipOffsetMm,
      profileNormal[2] * tipOffsetMm,
    ];
    let edges: BRepEdge[] = [];
    try {
      edges = kernel.bodyBrepEdges(i);
    } catch {
      // outline optional — arrow still works
    }
    // Face-precise boundary via the edge→face arena association (ticket
    // b91c) — the picked profile may be ONE of two coplanar split faces
    // sharing a branch signature, and only its own boundary may outline.
    const arenas = new Set(matches.map((f) => f.faceIndex));
    const outline = edges
      .filter((e) => e.faces.some((fi) => arenas.has(fi)))
      .map((e) => {
        const pts = new Array<number>(e.points.length);
        for (let k = 0; k + 2 < e.points.length; k += 3) {
          pts[k] = e.points[k] + shift[0];
          pts[k + 1] = e.points[k + 1] + shift[1];
          pts[k + 2] = e.points[k + 2] + shift[2];
        }
        return pts;
      })
      .filter((p) => p.length >= 6);
    const sign = tipOffsetMm < 0 ? -1 : 1;
    return {
      origin: [
        centroid[0] + shift[0],
        centroid[1] + shift[1],
        centroid[2] + shift[2],
      ],
      normal: [
        profileNormal[0] * sign,
        profileNormal[1] * sign,
        profileNormal[2] * sign,
      ],
      outline,
    };
  }
  return null;
}

/**
 * Locate the tip face of a committed extrude in the current B-Rep.
 * Returns `null` while the feature contributes no resolvable planar
 * face (suppressed, rolled back, consumed by later booleans, …).
 */
export function resolveExtrudeAnchor(
  kernel: ExtrudeAnchorKernel,
  featureId: number,
  extrudeLenMm?: number,
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
    const tip = pickTipFace(candidates, extrudeLenMm);
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
