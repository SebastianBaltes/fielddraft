/**
 * Element-selection → outliner linkage (ticket r36f).
 *
 * A B-Rep element selected in the viewport knows which CSG branches carry
 * it; this module folds those branch ids back onto the features and the
 * body that own the geometry, so the sidebar can render the matching
 * Features rows and Parts row as selected.
 *
 * Branch ids pack a feature id with its per-feature fields (see `BranchId` in
 * crates/kernel/src/csg.rs); stripping the low {@link FEATURE_SHIFT} bits
 * yields the flat feature id — the same id space as `FeatureSummary.id`, which
 * is what the Outliner keys its rows on.
 */

import type { BRepEdge, BRepFace, BRepVertex } from "./kernel";
import type { PickedElement } from "./elementQuery";

/**
 * Mirrors the `BranchId` field layout in crates/kernel/src/csg.rs:
 * `feature (20) | instance (12) | occurrence (5) | sub (12)`, 49
 * significant bits. The kernel caps the layout well under 53 bits precisely so
 * a packed branch stays an EXACT JavaScript integer (2^49 ≪ 2^53) and can
 * cross the wasm boundary as a plain JSON number.
 *
 * Never decode a branch with `>>` / `&`: JavaScript bit operators truncate to
 * 32 bits and would silently return garbage for any feature id ≥ 256.
 */
export const SUB_BITS = 12;
export const OCCURRENCE_BITS = 5;
export const OCCURRENCE_SHIFT = SUB_BITS;
export const INSTANCE_BITS = 12;
export const INSTANCE_SHIFT = SUB_BITS + OCCURRENCE_BITS;
export const FEATURE_SHIFT = INSTANCE_SHIFT + INSTANCE_BITS;
export const FEATURE_BITS = 20;

/** Flat feature id of a packed branch id. */
export function branchFeatureId(branchId: number): number {
  return Math.floor(branchId / 2 ** FEATURE_SHIFT) % 2 ** FEATURE_BITS;
}

/** Wall-occurrence ordinal of a packed branch id; 0 unless one sketch curve
 *  fenced several of the sweep's walls (#424). */
export function branchOccurrence(branchId: number): number {
  return Math.floor(branchId / 2 ** OCCURRENCE_SHIFT) % 2 ** OCCURRENCE_BITS;
}

/** Pattern instance ordinal of a packed branch id; 0 is the original (#429/E1). */
export function branchInstance(branchId: number): number {
  return Math.floor(branchId / 2 ** INSTANCE_SHIFT) % 2 ** INSTANCE_BITS;
}

/** Per-feature sub-face index of a packed branch id. */
export function branchSub(branchId: number): number {
  return branchId % 2 ** SUB_BITS;
}

/** The slice of the kernel API this module reads. */
export interface SelectionLinkKernel {
  bodyCount(): number;
  bodyBrepFaces(i: number): BRepFace[];
  bodyBrepEdges(i: number): BRepEdge[];
  bodyBrepVertices(i: number): BRepVertex[];
}

/**
 * Feature ids owning the selected elements, plus the body index the primary
 * element lives on. Scans the kernel's analytic B-Rep lists (tiny) for the
 * idHashes; unknown hashes (mid-regen) simply contribute nothing.
 */
export function resolveSelectionLinks(
  kernel: SelectionLinkKernel,
  elements: readonly PickedElement[],
): { featureIds: Set<number>; bodyIndex: number | null } {
  const featureIds = new Set<number>();
  let bodyIndex: number | null = null;
  if (elements.length === 0) return { featureIds, bodyIndex };

  const wanted = new Map<string, PickedElement>();
  for (const el of elements) wanted.set(`${el.kind}:${el.idHash}`, el);

  const count = kernel.bodyCount();
  for (let i = 0; i < count && wanted.size > 0; i++) {
    let faces: BRepFace[];
    let edges: BRepEdge[];
    let vertices: BRepVertex[];
    try {
      faces = kernel.bodyBrepFaces(i);
      edges = kernel.bodyBrepEdges(i);
      vertices = kernel.bodyBrepVertices(i);
    } catch {
      continue;
    }
    for (const f of faces) {
      if (!wanted.delete(`face:${f.idHash}`)) continue;
      featureIds.add(f.featureId);
      bodyIndex ??= i;
    }
    for (const e of edges) {
      if (!wanted.delete(`edge:${e.idHash}`)) continue;
      featureIds.add(branchFeatureId(e.a));
      featureIds.add(branchFeatureId(e.b));
      bodyIndex ??= i;
    }
    for (const v of vertices) {
      if (!wanted.delete(`vertex:${v.idHash}`)) continue;
      for (const b of v.branches) featureIds.add(branchFeatureId(b));
      bodyIndex ??= i;
    }
  }
  return { featureIds, bodyIndex };
}

/**
 * Feature ids contributing to one body — the reverse link for a selected
 * BODY (Parts row / body escalation): every feature whose branches appear
 * on the body's faces.
 */
export function bodyFeatureIds(
  kernel: SelectionLinkKernel,
  bodyIndex: number,
): Set<number> {
  const out = new Set<number>();
  if (bodyIndex < 0 || bodyIndex >= kernel.bodyCount()) return out;
  try {
    for (const f of kernel.bodyBrepFaces(bodyIndex)) out.add(f.featureId);
  } catch {
    // body mid-regen — no links
  }
  return out;
}
