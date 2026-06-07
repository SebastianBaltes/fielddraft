/**
 * Shared sketch solve-replay.
 *
 * After any structural change to the session sub-spine (a new line,
 * a new constraint, an edited dimension) the geometry must be
 * re-solved and the solved coordinates written back into the
 * sub-calls. Both the 2D canvas commit path and the constraint
 * palette go through `solveAndReplace` so there is exactly one
 * solve pipeline.
 */

import { buildDraftSketchState, ellipseParamsFromFocus } from "../sketchSession";
import { solveSketch } from "./planegcs";
import type { SketchDiagnostics, SketchState } from "./types";

/** Minimal shape of a sketch sub-call (mirrors the session slice). */
export interface SubCall {
  type: string;
  params: Record<string, unknown>;
}

/** Minimal store surface `solveAndReplace` needs — kept structural
 *  so it doesn't drag the full KernelStore type into the solver
 *  layer (and so tests can pass a stub). */
export interface SketchSolveHost {
  getState(): {
    sketchSession: { active: boolean; subCalls: ReadonlyArray<SubCall> };
  };
  replaceSketchSubCalls(calls: ReadonlyArray<SubCall>): void;
  /** F20: stash the latest solve diagnostics on the session so the
   *  Outliner can flag conflicting / redundant constraint rows. */
  setSketchDiagnostics(diagnostics: SketchDiagnostics): void;
}

/**
 * Walk a sub_calls list alongside a solved `SketchState` and return
 * a new list where every geometric call carries the solved
 * coordinates. The mapping replicates the kernel's deterministic
 * entity allocation order: id 0 is the origin, then ids 1+ are
 * allocated one per geometry call in sub_calls order. Constraint
 * calls are left untouched — their entity-id references don't change
 * under a pure coordinate solve.
 */
export function rebuildSubCallsFromSolved(
  original: ReadonlyArray<SubCall>,
  solved: SketchState,
): SubCall[] {
  const pointAt = (id: number): [number, number] | null => {
    const e = solved.entities.find((x) => x.id === id);
    if (!e) return null;
    if (e.kind === "point") return [e.at.x, e.at.y];
    if (e.kind === "origin") return [0, 0];
    return null;
  };
  let nextId = 1; // origin is at id 0
  return original.map((call): SubCall => {
    if (call.type === "sketch_point") {
      const at = pointAt(nextId++);
      if (!at) return call;
      return { ...call, params: { ...call.params, at } };
    }
    if (call.type === "sketch_line") {
      const start = pointAt(nextId++);
      const end = pointAt(nextId++);
      nextId += 1; // line entity itself
      if (!start || !end) return call;
      return { ...call, params: { ...call.params, start, end } };
    }
    if (call.type === "sketch_circle") {
      const center = pointAt(nextId++);
      const circleId = nextId++;
      if (!center) return call;
      const circle = solved.entities.find((e) => e.id === circleId);
      const radius =
        circle && circle.kind === "circle"
          ? circle.radius
          : Number(call.params.radius);
      return { ...call, params: { ...call.params, center, radius } };
    }
    if (call.type === "sketch_rectangle") {
      // Match the bl/br/tr/tl allocation order — corner_a maps to
      // bl, corner_b maps to tr.
      const bl = pointAt(nextId++);
      nextId += 1; // br
      const tr = pointAt(nextId++);
      nextId += 1; // tl
      nextId += 4; // bottom, right, top, left
      if (!bl || !tr) return call;
      return {
        ...call,
        params: { ...call.params, corner_a: bl, corner_b: tr },
      };
    }
    if (call.type === "sketch_polygon") {
      const verts = call.params.vertices;
      if (Array.isArray(verts)) {
        const numPoints = Math.floor(verts.length / 2);
        const newVerts: number[] = [];
        for (let i = 0; i < numPoints; i++) {
          const pt = pointAt(nextId++);
          if (pt) {
            newVerts.push(pt[0], pt[1]);
          } else {
            newVerts.push(Number(verts[i * 2]), Number(verts[i * 2 + 1]));
          }
        }
        return { ...call, params: { ...call.params, vertices: newVerts } };
      }
      return call;
    }
    if (call.type === "sketch_arc") {
      const center = pointAt(nextId++);
      if (!center) return call;
      return { ...call, params: { ...call.params, center } };
    }
    if (call.type === "sketch_ellipse") {
      // Mirrors buildDraftSketchState: centre point, focus point, then
      // the ellipse entity. Read the solved centre + focus + radmin and
      // recover (radius_x, radius_y, rotation) so the rendered outline
      // tracks the solver after a constraint reshapes it. Canonical
      // form puts the semi-major radius on radius_x.
      const center = pointAt(nextId++);
      const focus = pointAt(nextId++);
      const ellipseId = nextId++;
      if (!center || !focus) return call;
      const ell = solved.entities.find((e) => e.id === ellipseId);
      if (!ell || ell.kind !== "ellipse") {
        return { ...call, params: { ...call.params, center } };
      }
      const { radiusX, radiusY, rotation } = ellipseParamsFromFocus(
        center[0],
        center[1],
        focus[0],
        focus[1],
        ell.radmin,
      );
      return {
        ...call,
        params: {
          ...call.params,
          center,
          radius_x: radiusX,
          radius_y: radiusY,
          rotation,
        },
      };
    }
    if (call.type === "sketch_slot") {
      // Mirrors buildDraftSketchState: TWO end-cap centres in order.
      const ca = pointAt(nextId++);
      const cb = pointAt(nextId++);
      if (!ca || !cb) return call;
      return {
        ...call,
        params: { ...call.params, center_a: ca, center_b: cb },
      };
    }
    if (call.type === "sketch_spline") {
      // Mirrors buildDraftSketchState: N control-point entities, then
      // (for any open spline) the endpoint leg line(s). Write back the
      // solved control points (so spline constraints are visible) and
      // advance nextId past the legs (so later sub-calls stay
      // id-aligned). Scope MUST match the kernel / session mirror.
      const points = call.params.points;
      if (Array.isArray(points)) {
        const numPoints = Math.floor(points.length / 2);
        const newPoints: number[] = [];
        for (let i = 0; i < numPoints; i++) {
          const pt = pointAt(nextId++);
          if (pt) {
            newPoints.push(pt[0], pt[1]);
          } else {
            newPoints.push(Number(points[i * 2]), Number(points[i * 2 + 1]));
          }
        }
        // Endpoint legs: any open spline, N>=2 → start leg always;
        // N>=3 → also end leg. (N==2 → exactly one leg.)
        if (call.params.closed !== true && numPoints >= 2) {
          nextId += 1; // start leg
          if (numPoints >= 3) nextId += 1; // end leg
        }
        // B-spline entity: control mode + open only (matches the kernel /
        // session mirror), allocated LAST. Advance past it so later
        // sub-calls stay id-aligned.
        if (
          call.params.mode === "control" &&
          call.params.closed !== true &&
          numPoints >= 2
        ) {
          nextId += 1;
        }
        return { ...call, params: { ...call.params, points: newPoints } };
      }
      return call;
    }
    if (call.type === "sketch_path") {
      // Mirrors buildDraftSketchState / the kernel BYTE-FOR-BYTE:
      // CANONICAL order per contour = start, then per segment
      // (Quad → ctrl, to; Line → to). Advance nextId over EVERY vertex
      // entity and write back solved coords into start/segment-to/
      // quad-ctrl so solver moves are reflected. Closing edge adds none.
      const contours = call.params.contours;
      if (Array.isArray(contours)) {
        const newContours = contours.map((c) => {
          const cc = c as {
            start?: unknown;
            segments?: unknown;
          };
          let newStart = cc?.start;
          if (Array.isArray(cc?.start) && cc.start.length === 2) {
            const pt = pointAt(nextId++);
            if (pt) newStart = pt;
          }
          let newSegments = cc?.segments;
          if (Array.isArray(cc?.segments)) {
            newSegments = cc.segments.map((seg) => {
              const s = seg as {
                kind?: unknown;
                ctrl?: unknown;
                to?: unknown;
              };
              const next: Record<string, unknown> = { ...s };
              if (s?.kind === "quad" && Array.isArray(s.ctrl)) {
                const cp = pointAt(nextId++);
                if (cp) next.ctrl = cp;
              }
              if (Array.isArray(s?.to)) {
                const tp = pointAt(nextId++);
                if (tp) next.to = tp;
              }
              return next;
            });
          }
          return { ...(c as object), start: newStart, segments: newSegments };
        });
        return { ...call, params: { ...call.params, contours: newContours } };
      }
      return call;
    }
    return call;
  });
}

/**
 * Re-solve the host's current sketch session and write the solved
 * coordinates back. A no-op when no session is active. Bails if the
 * session changed underneath the async solve (the snapshot identity
 * guard) so a stale solve can't clobber newer edits.
 */
export async function solveAndReplace(host: SketchSolveHost): Promise<void> {
  const before = host.getState().sketchSession;
  if (!before.active) return;
  const snapshot = before.subCalls;
  try {
    const state = buildDraftSketchState(snapshot);
    const result = await solveSketch(state);
    const rebuilt = rebuildSubCallsFromSolved(snapshot, result.state);
    const after = host.getState().sketchSession;
    if (!after.active || after.subCalls !== snapshot) return;
    host.replaceSketchSubCalls(rebuilt);
    // Diagnostics' constraint ids are in the session's id space:
    // `buildDraftSketchState(snapshot)` is the solve input and uses
    // the same monotonic constraint-id allocation the Outliner reads
    // back via `constraintIdsOfSubCall`. Stash them under the same
    // snapshot-identity guard so a stale solve can't clobber.
    host.setSketchDiagnostics(result.diagnostics);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.error("fielddraft: solveSketch failed", err);
  }
}
