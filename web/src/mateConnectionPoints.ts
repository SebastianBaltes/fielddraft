/**
 * Mate-connector inference — enumerate the candidate connection points the
 * Mate Connector tool snaps to while hovering (Onshape parity).
 *
 * Connection points are the model's salient local coordinate origins:
 *   - **vertices** — B-Rep corners
 *   - **edge midpoints** — the arc-length centre of every edge
 *   - **face centres** — the centroid of every planar face
 *
 * Each point carries one or more **orientation variants** (option b): the
 * connector's +Z can align to any adjacent face's normal, and the user cycles
 * through them. A variant records the *primary* reference face query so the
 * placement is fully query-driven (kernel-resolved, regen-stable — C2): the
 * kernel re-derives the exact same CSYS from `anchorQuery` + `t` + the
 * variant's `primaryQuery`. This module is pure (no THREE, no DOM) so it is
 * unit-testable; it only mirrors the kernel's CSYS convention so the live
 * preview matches the committed connector to f32.
 */

import {
  localToWorld,
  type BRepFace,
  type BRepVertex,
  type BRepEdge,
  type Placement,
} from "./kernel";
import { rotateByPlacement } from "./faceGeometry";

type Vec3 = [number, number, number];

/** The kind of geometry a connection point sits on (drives the marker glyph). */
export type MatePointKind = "vertex" | "edge-mid" | "face-center";

/** One orientation the connector's +Z can take at a connection point. */
export interface MateOrientation {
  /** Primary axis (+Z), world unit. */
  z: Vec3;
  /** Secondary in-plane axis (+X), world unit. */
  x: Vec3;
  /** `qFace(idHash)` of the face whose normal defines +Z, or `null` when the
   *  anchor IS that face (a face-centre point — the kernel uses the anchor
   *  face's own normal, so `primary` stays empty). */
  primaryQuery: string | null;
  /** Surface kind of the orienting face, for the HUD. */
  label: string;
}

/** A candidate the inference overlay can snap to. */
export interface MateConnectionPoint {
  kind: MatePointKind;
  /** Connector origin in world space. */
  world: Vec3;
  /** Kernel anchor query: `qVertex`/`qEdge`/`qFace(idHash)`. */
  anchorQuery: string;
  /** Edge arc-length param `[0,1]` (0.5 = midpoint); ignored for non-edges. */
  t: number;
  /** Orientation variants to cycle through (≥1). */
  orientations: MateOrientation[];
}

/** The minimal kernel surface this module reads (so tests can fake it). */
export interface ConnectionKernel {
  bodyCount(): number;
  bodyBrepFaces(i: number): BRepFace[];
  bodyBrepVertices(i: number): BRepVertex[];
  bodyBrepEdges(i: number): BRepEdge[];
  bodyPlacement(i: number): Placement;
}

const VERTEX_TOL = 1e-3; // mm; vertex↔edge-endpoint coincidence.

function sub(a: Vec3, b: Vec3): Vec3 {
  return [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
}
function dot(a: Vec3, b: Vec3): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}
function len(a: Vec3): number {
  return Math.hypot(a[0], a[1], a[2]);
}
function normalize(a: Vec3): Vec3 {
  const n = len(a);
  return n > 1e-9 ? [a[0] / n, a[1] / n, a[2] / n] : [0, 0, 1];
}
function cross(a: Vec3, b: Vec3): Vec3 {
  return [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ];
}

/** In-plane +X for a normal `n`: cross `n` with the world axis least aligned
 *  with it. Matches the kernel's `face_plane_frame` / `seed_cross_x`. */
export function seedCrossX(n: Vec3): Vec3 {
  const ax = Math.abs(n[0]);
  const ay = Math.abs(n[1]);
  const az = Math.abs(n[2]);
  const seed: Vec3 =
    ax <= ay && ax <= az ? [1, 0, 0] : ay <= az ? [0, 1, 0] : [0, 0, 1];
  return normalize(cross(seed, n));
}

/** Project `v` onto the plane ⟂ `n`, normalise; fall back to seed-cross when
 *  `v` is parallel to `n`. Matches the kernel's `project_perp`. */
function projectPerp(v: Vec3, n: Vec3): Vec3 {
  const d = dot(v, n);
  const p: Vec3 = [v[0] - d * n[0], v[1] - d * n[1], v[2] - d * n[2]];
  return len(p) > 1e-6 ? normalize(p) : seedCrossX(n);
}

/** Arc-length midpoint + unit tangent of a flat `[x,y,z,…]` world polyline. */
function polylineMidpoint(points: number[]): { p: Vec3; tangent: Vec3 } | null {
  const n = Math.floor(points.length / 3);
  if (n < 2) return null;
  const pt = (i: number): Vec3 => [
    points[i * 3],
    points[i * 3 + 1],
    points[i * 3 + 2],
  ];
  let total = 0;
  for (let i = 0; i < n - 1; i++) total += len(sub(pt(i + 1), pt(i)));
  if (total <= 1e-9) return { p: pt(0), tangent: [1, 0, 0] };
  const target = total * 0.5;
  let acc = 0;
  for (let i = 0; i < n - 1; i++) {
    const a = pt(i);
    const b = pt(i + 1);
    const l = len(sub(b, a));
    if (acc + l >= target || i === n - 2) {
      const local = l > 1e-9 ? (target - acc) / l : 0;
      return {
        p: [
          a[0] + (b[0] - a[0]) * local,
          a[1] + (b[1] - a[1]) * local,
          a[2] + (b[2] - a[2]) * local,
        ],
        tangent: normalize(sub(b, a)),
      };
    }
    acc += l;
  }
  return null;
}

interface FaceWorld {
  faceIndex: number;
  idHash: string;
  surfaceKind: BRepFace["surfaceKind"];
  centroid: Vec3;
  normal: Vec3;
}

/** Build orientation variants from a set of adjacent faces, deterministically
 *  ordered by idHash so the cycle order is stable across regenerations. */
function orientationsFromFaces(
  faces: FaceWorld[],
  tangent: Vec3 | null,
): MateOrientation[] {
  return [...faces]
    .sort((a, b) => a.idHash.localeCompare(b.idHash))
    .map((f) => ({
      z: f.normal,
      x: tangent ? projectPerp(tangent, f.normal) : seedCrossX(f.normal),
      primaryQuery: `qFace(${f.idHash})`,
      label: f.surfaceKind,
    }));
}

/**
 * Enumerate every connection point across all bodies. Pure: pass any object
 * implementing {@link ConnectionKernel}. A body that throws on read is skipped.
 */
export function enumerateConnectionPoints(
  kernel: ConnectionKernel,
): MateConnectionPoint[] {
  const out: MateConnectionPoint[] = [];
  const count = kernel.bodyCount();
  for (let i = 0; i < count; i++) {
    let faces: BRepFace[];
    let verts: BRepVertex[];
    let edges: BRepEdge[];
    let pl: Placement;
    try {
      faces = kernel.bodyBrepFaces(i);
      verts = kernel.bodyBrepVertices(i);
      edges = kernel.bodyBrepEdges(i);
      pl = kernel.bodyPlacement(i);
    } catch {
      continue;
    }

    // Fold faces to world; index by arena faceIndex (== BRepEdge.faces).
    const faceByIndex = new Map<number, FaceWorld>();
    for (const f of faces) {
      faceByIndex.set(f.faceIndex, {
        faceIndex: f.faceIndex,
        idHash: f.idHash,
        surfaceKind: f.surfaceKind,
        centroid: localToWorld(f.centroidLocal, pl),
        normal: normalize(rotateByPlacement(f.normalLocal, pl)),
      });
    }

    // ── Face centres (planar only) ──────────────────────────────
    for (const fw of faceByIndex.values()) {
      if (fw.surfaceKind !== "plane") continue;
      out.push({
        kind: "face-center",
        world: fw.centroid,
        anchorQuery: `qFace(${fw.idHash})`,
        t: 0,
        // The anchor IS the orienting face → primaryQuery stays null.
        orientations: [
          {
            z: fw.normal,
            x: seedCrossX(fw.normal),
            primaryQuery: null,
            label: fw.surfaceKind,
          },
        ],
      });
    }

    // ── Edge midpoints ──────────────────────────────────────────
    for (const e of edges) {
      const mid = polylineMidpoint(e.points);
      if (!mid) continue;
      const adj = e.faces
        .map((fi) => faceByIndex.get(fi))
        .filter((f): f is FaceWorld => !!f);
      const orientations = orientationsFromFaces(adj, mid.tangent);
      if (orientations.length === 0) continue; // can't orient → skip
      out.push({
        kind: "edge-mid",
        world: mid.p,
        anchorQuery: `qEdge(${e.idHash})`,
        t: 0.5,
        orientations,
      });
    }

    // ── Vertices ────────────────────────────────────────────────
    const vWorld = verts.map((v) => ({
      idHash: v.idHash,
      world: localToWorld(v.positionLocal, pl) as Vec3,
    }));
    for (const v of vWorld) {
      // Adjacent faces = union over edges touching this vertex.
      const adjIdx = new Set<number>();
      for (const e of edges) {
        const n = Math.floor(e.points.length / 3);
        if (n < 1) continue;
        const first: Vec3 = [e.points[0], e.points[1], e.points[2]];
        const last: Vec3 = [
          e.points[(n - 1) * 3],
          e.points[(n - 1) * 3 + 1],
          e.points[(n - 1) * 3 + 2],
        ];
        if (
          len(sub(first, v.world)) < VERTEX_TOL ||
          len(sub(last, v.world)) < VERTEX_TOL
        ) {
          for (const fi of e.faces) adjIdx.add(fi);
        }
      }
      const adj = [...adjIdx]
        .map((fi) => faceByIndex.get(fi))
        .filter((f): f is FaceWorld => !!f);
      const orientations = orientationsFromFaces(adj, null);
      if (orientations.length === 0) continue;
      out.push({
        kind: "vertex",
        world: v.world,
        anchorQuery: `qVertex(${v.idHash})`,
        t: 0,
        orientations,
      });
    }
  }
  return out;
}
