/**
 * Element-selection → outliner linkage (ticket r36f).
 *
 * A B-Rep element selected in the viewport knows which CSG branches carry
 * it; this module folds those branch ids back onto the features and the
 * body that own the geometry, so the sidebar can render the matching
 * Features rows and Parts row as selected.
 *
 * Branch ids pack `feature << SUB_BITS | sub` (see `BranchId` in
 * crates/kernel/src/csg.rs); stripping the low {@link SUB_BITS} bits yields
 * the flat feature id — the same id space as `FeatureSummary.id`, which is
 * what the Outliner keys its rows on.
 */

import type { BRepEdge, BRepFace, BRepVertex } from "./kernel";
import type { PickedElement } from "./elementQuery";

/** Mirrors `BranchId::SUB_BITS` in crates/kernel/src/csg.rs. */
export const SUB_BITS = 12;

/** Flat feature id of a packed branch id. */
export function branchFeatureId(branchId: number): number {
  return Math.floor(branchId / 2 ** SUB_BITS);
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
