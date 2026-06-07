/**
 * PlaneGCS wrapper (K5a web side).
 *
 * Thin bridge between FieldDraft's sketch state wire format
 * (shared with the Rust `crates/solver` crate) and the
 * `@salusoft89/planegcs` WASM module. The wrapper lives on the
 * TypeScript side of the separate-WASM integration path
 * (README §4.4 option 1): PlaneGCS runs as a second WebAssembly
 * module inside the Regeneration Worker, the Rust kernel doesn't
 * link to it, and every solve-call goes through this file.
 *
 * ## Responsibilities
 *
 * * Lazy-load the PlaneGCS WASM module once per process via
 *   `make_gcs_wrapper`. Subsequent calls reuse the live module
 *   handle — loading the WASM is the expensive step and we
 *   amortise it across every solve.
 * * Translate FieldDraft [`Entity`] objects into PlaneGCS
 *   `SketchPrimitive`s. Points become `SketchPoint`s, lines
 *   become `SketchLine`s referencing already-pushed points,
 *   circles become `SketchCircle`s, arcs compute their start /
 *   end angles from the start / end point positions.
 * * Translate FieldDraft [`Constraint`] objects into PlaneGCS
 *   constraints. The mapping is documented inline at each
 *   `translateConstraint` arm.
 * * Call `solve`, pull updated entity parameters out of the
 *   solver, and return a fresh [`SolveResult`] with the new
 *   state plus [`SketchDiagnostics`] derived from the solver's
 *   conflicting / redundant / partially-redundant sets.
 *
 * ## Not covered yet
 *
 * * `Constraint.Symmetric`: PlaneGCS has no single "symmetric"
 *   primitive — the adapter layer (K5b) will decompose it into
 *   midpoint + perpendicular. For K5a the translator throws a
 *   descriptive error; a sketch that uses `symmetric` won't
 *   solve through this path.
 * * Live drag: [`DragController`] re-solves the sketch every pointer
 *   frame, feeding the dragged geometry as the initial guess so the
 *   solver settles each entity onto the nearest valid manifold
 *   (continuation). It drives this same `solveSketch` entry point —
 *   no separate `temporary`-flag API is needed.
 * * Two-WASM packaging / bundle split: the file imports from
 *   `@salusoft89/planegcs` directly so Vite handles the worker
 *   chunk emission. Sub-splitting the PlaneGCS binary into its
 *   own lazy chunk is a later optimisation.
 */

// eslint-disable-next-line import/no-unresolved
import {
  init_planegcs_module,
  GcsWrapper,
  type ModuleStatic,
} from "@salusoft89/planegcs";
// eslint-disable-next-line import/no-unresolved
import type { SketchPrimitive } from "@salusoft89/planegcs";

import type {
  Constraint,
  Entity,
  EntityId,
  SketchDiagnostics,
  SketchState,
  SolveResult,
} from "./types";
import { emptyDiagnostics } from "./types";
import { bsplinePoints } from "../sketchSpline";

// ─── module cache ────────────────────────────────────────────────

/**
 * Cached wrapper handle. PlaneGCS' wrapper carries mutable state
 * (the `GcsSystem` bound to a live WASM instance), so we keep one
 * shared instance across solves and call `clear_data` between
 * requests instead of re-loading the module every time. Tests
 * can reset the cache via [`resetPlanegcsModuleForTests`].
 */
let wrapperPromise: Promise<GcsWrapper> | null = null;

/**
 * The raw WASM module behind the cached wrapper. B-splines are not
 * part of the declarative `SketchPrimitive` layer, so they're built
 * directly on `wrapper.gcs` (see [`injectBSplines`]), which needs the
 * module's `IntVector` constructor — `make_gcs_wrapper` hides it, so
 * we instantiate the module ourselves and keep a handle.
 */
let planegcsModule: ModuleStatic | null = null;

/**
 * Load or reuse the PlaneGCS wrapper. First call in a process
 * instantiates the WASM module; every subsequent call returns
 * the same wrapper after clearing its internal state. Mirrors the
 * package's `make_gcs_wrapper`, but retains the module handle for
 * low-level B-spline construction.
 */
async function getWrapper(): Promise<GcsWrapper> {
  if (!wrapperPromise) {
    wrapperPromise = (async () => {
      const module = await init_planegcs_module();
      planegcsModule = module;
      return new GcsWrapper(new module.GcsSystem());
    })();
  }
  const wrapper = await wrapperPromise;
  wrapper.clear_data();
  return wrapper;
}

/**
 * Drop the cached wrapper so the next [`solveSketch`] loads a
 * fresh WASM instance. Used by tests that need hermetic isolation
 * between cases; production code should never call this.
 */
export function resetPlanegcsModuleForTests(): void {
  wrapperPromise = null;
  planegcsModule = null;
}

// ─── id namespaces ───────────────────────────────────────────────
//
// PlaneGCS identifies sketch primitives by string `oid`s. We
// namespace entity ids and constraint ids so there's no risk of
// collision when the same numeric id is reused across the two
// domains. Every `oid` we emit is deterministic from the FD id,
// which makes diagnostics round-tripping trivial on the way back.

function entityOid(id: EntityId): string {
  return `e_${id}`;
}

function constraintOid(id: number): string {
  return `c_${id}`;
}

/** Inverse of [`constraintOid`]. Returns `null` if the tag
 *  doesn't match the format we emit — PlaneGCS sometimes reports
 *  internal constraint ids (radius parameters, arc rules) that
 *  weren't authored by the caller, and we don't want those to
 *  show up in the diagnostics arrays. */
function parseConstraintOid(oid: string | undefined): number | null {
  // Raw B-spline constraints (see `injectBSplines`) use high integer
  // tags that aren't in the wrapper's sketch_index, so a flagged one
  // maps back to `undefined` here — guard rather than throw.
  if (typeof oid !== "string" || !oid.startsWith("c_")) return null;
  const n = Number(oid.slice(2));
  return Number.isFinite(n) ? n : null;
}

// ─── translators ─────────────────────────────────────────────────

/**
 * Convert a FieldDraft entity into zero-or-more PlaneGCS
 * primitives. Most entities emit exactly one primitive, but
 * `Origin` emits a fixed point that pins the sketch; later
 * slices may expand this to emit helper constructs for arcs.
 *
 * `entityIndex` is a lookup from entity id to entity — arcs need
 * it to resolve their centre, start, and end point positions so
 * they can compute start / end angles before handing control to
 * PlaneGCS.
 */
function translateEntity(
  entity: Entity,
  entityIndex: Map<EntityId, Entity>,
  fixedPoints: Set<EntityId>,
): SketchPrimitive[] {
  switch (entity.kind) {
    case "point":
      // A `fix` constraint pins a point in place by flagging the
      // point itself `fixed: true` (the same mechanism the origin
      // uses) — there's no separate PlaneGCS primitive for it.
      return [
        {
          type: "point",
          id: entityOid(entity.id),
          x: entity.at.x,
          y: entity.at.y,
          fixed: fixedPoints.has(entity.id),
        },
      ];
    case "origin":
      // Fixed point at (0, 0). The `fixed: true` flag tells
      // PlaneGCS that this point has zero DOFs, which anchors
      // every sketch that includes it.
      return [
        {
          type: "point",
          id: entityOid(entity.id),
          x: 0,
          y: 0,
          fixed: true,
        },
      ];
    case "line":
      return [
        {
          type: "line",
          id: entityOid(entity.id),
          p1_id: entityOid(entity.start),
          p2_id: entityOid(entity.end),
        },
      ];
    case "circle":
      return [
        {
          type: "circle",
          id: entityOid(entity.id),
          c_id: entityOid(entity.center),
          radius: entity.radius,
        },
      ];
    case "arc": {
      // PlaneGCS arcs need start_angle / end_angle in radians,
      // measured CCW from the +X axis relative to the centre.
      // Compute them from the FD arc's start and end point
      // positions so the caller doesn't have to maintain angle
      // fields manually.
      const center = requirePoint(entityIndex, entity.center, "arc.center");
      const start = requirePoint(entityIndex, entity.start, "arc.start");
      const end = requirePoint(entityIndex, entity.end, "arc.end");
      const startAngle = Math.atan2(start.y - center.y, start.x - center.x);
      const endAngle = Math.atan2(end.y - center.y, end.x - center.x);
      return [
        {
          type: "arc",
          id: entityOid(entity.id),
          c_id: entityOid(entity.center),
          start_id: entityOid(entity.start),
          end_id: entityOid(entity.end),
          radius: entity.radius,
          start_angle: startAngle,
          end_angle: endAngle,
        },
      ];
    }
    case "ellipse":
      // PlaneGCS models the ellipse as centre + one focus + the
      // semi-minor radius. Both the centre and focus are separate
      // FieldDraft point entities (pushed by their own `point` cases
      // before this ellipse, since they hold lower ids), so we only
      // reference them by oid here. `push_ellipse` re-pushes them
      // idempotently.
      return [
        {
          type: "ellipse",
          id: entityOid(entity.id),
          c_id: entityOid(entity.center),
          focus1_id: entityOid(entity.focus1),
          radmin: entity.radmin,
        },
      ];
    case "bspline":
      // B-splines are not part of the declarative SketchPrimitive
      // layer — they're built directly on the raw `GcsSystem` after all
      // declarative primitives are pushed (see `injectBSplines`), since
      // their poles must already have GCS params. Emit nothing here.
      return [];
  }
}

function requirePoint(
  index: Map<EntityId, Entity>,
  id: EntityId,
  context: string,
): { x: number; y: number } {
  const e = index.get(id);
  if (!e) {
    throw new Error(
      `solver: ${context} references unknown entity id ${id}`,
    );
  }
  if (e.kind === "point") return e.at;
  if (e.kind === "origin") return { x: 0, y: 0 };
  throw new Error(
    `solver: ${context} must reference a point, got ${e.kind} (id ${id})`,
  );
}

/**
 * Convert a FieldDraft constraint into a PlaneGCS constraint
 * primitive. The mapping is documented per case — most are
 * one-to-one, but `Distance` picks between P2P and P2L based on
 * the referenced entity shape, `Concentric` is modelled as a
 * coincidence between two circle centres, and `Symmetric` is
 * unsupported in K5a (throws).
 *
 * Returns `null` for `fix`: the constraint has no own PlaneGCS
 * primitive — it's realised through the target point's
 * `fixed: true` flag (see [`translateEntity`]). The caller filters
 * the nulls out of the primitive stream.
 */
function translateConstraint(
  constraint: Constraint,
  entityIndex: Map<EntityId, Entity>,
): SketchPrimitive | null {
  const id = constraintOid(constraint.id);
  switch (constraint.kind) {
    case "fix":
      // No primitive: realised via the entity's `fixed` flag.
      return null;
    case "point_on_object": {
      // Constrain a point onto a curve, dispatched by the curve's
      // kind: a line uses `point_on_line_pl`, a circle
      // `point_on_circle`, an arc `point_on_arc`.
      const curve = entityIndex.get(constraint.curve);
      if (!curve) {
        throw new Error(
          `solver: point_on_object references unknown curve id ${constraint.curve}`,
        );
      }
      if (curve.kind === "line") {
        return {
          type: "point_on_line_pl",
          id,
          p_id: entityOid(constraint.point),
          l_id: entityOid(constraint.curve),
        };
      }
      if (curve.kind === "circle") {
        return {
          type: "point_on_circle",
          id,
          p_id: entityOid(constraint.point),
          c_id: entityOid(constraint.curve),
        };
      }
      if (curve.kind === "arc") {
        return {
          type: "point_on_arc",
          id,
          p_id: entityOid(constraint.point),
          a_id: entityOid(constraint.curve),
        };
      }
      if (curve.kind === "ellipse") {
        return {
          type: "point_on_ellipse",
          id,
          p_id: entityOid(constraint.point),
          e_id: entityOid(constraint.curve),
        };
      }
      if (curve.kind === "bspline") {
        // No declarative primitive — point_on_bspline is added on the
        // raw GcsSystem after the B-spline geometry is built (see
        // `injectBSplines`). Skip it in the declarative stream.
        return null;
      }
      throw new Error(
        `solver: point_on_object curve must be a line, circle, arc, ellipse, or bspline, got ${curve.kind}`,
      );
    }
    case "coincident":
      return {
        type: "p2p_coincident",
        id,
        p1_id: entityOid(constraint.a),
        p2_id: entityOid(constraint.b),
      };
    case "parallel":
      return {
        type: "parallel",
        id,
        l1_id: entityOid(constraint.a),
        l2_id: entityOid(constraint.b),
      };
    case "horizontal":
      return {
        type: "horizontal_l",
        id,
        l_id: entityOid(constraint.line),
      };
    case "vertical":
      return {
        type: "vertical_l",
        id,
        l_id: entityOid(constraint.line),
      };
    case "perpendicular":
      return {
        type: "perpendicular_ll",
        id,
        l1_id: entityOid(constraint.a),
        l2_id: entityOid(constraint.b),
      };
    case "distance":
      return {
        type: "p2p_distance",
        id,
        p1_id: entityOid(constraint.a),
        p2_id: entityOid(constraint.b),
        distance: constraint.value_mm,
        driving: !(constraint.driven ?? false),
      };
    case "angle": {
      // PlaneGCS `l2l_angle_ll` expects the angle in radians.
      const radians = (constraint.value_deg * Math.PI) / 180;
      return {
        type: "l2l_angle_ll",
        id,
        l1_id: entityOid(constraint.a),
        l2_id: entityOid(constraint.b),
        angle: radians,
        driving: !(constraint.driven ?? false),
      };
    }
    case "tangent": {
      // Dispatch by curve kind: line-to-circle → tangent_lc,
      // line-to-arc → tangent_la.
      const curve = entityIndex.get(constraint.curve);
      if (!curve) {
        throw new Error(
          `solver: tangent references unknown curve id ${constraint.curve}`,
        );
      }
      if (curve.kind === "circle") {
        return {
          type: "tangent_lc",
          id,
          l_id: entityOid(constraint.line),
          c_id: entityOid(constraint.curve),
        };
      }
      if (curve.kind === "arc") {
        return {
          type: "tangent_la",
          id,
          l_id: entityOid(constraint.line),
          a_id: entityOid(constraint.curve),
        };
      }
      if (curve.kind === "ellipse") {
        return {
          type: "tangent_le",
          id,
          l_id: entityOid(constraint.line),
          e_id: entityOid(constraint.curve),
        };
      }
      throw new Error(
        `solver: tangent curve must be a circle, arc, or ellipse, got ${curve.kind}`,
      );
    }
    case "concentric": {
      // PlaneGCS has no named concentric primitive; the canonical
      // decomposition is a point-to-point coincidence between the
      // two circles' centres. Walk the entity index to resolve
      // the centre ids.
      const a = entityIndex.get(constraint.a);
      const b = entityIndex.get(constraint.b);
      if (
        !a ||
        !b ||
        !(a.kind === "circle" || a.kind === "arc") ||
        !(b.kind === "circle" || b.kind === "arc")
      ) {
        throw new Error(
          "solver: concentric requires two circles or arcs",
        );
      }
      return {
        type: "p2p_coincident",
        id,
        p1_id: entityOid(a.center),
        p2_id: entityOid(b.center),
      };
    }
    case "equal": {
      // For two circles / arcs, PlaneGCS has `equal_radius_cc`
      // / `equal_radius_aa` / `equal_radius_ca`. For two lines
      // the equivalent is a distance equality between the two
      // pairs of endpoints, which PlaneGCS models via `equal`
      // on the p2p_distance parameters — but that requires
      // pushing helper P2PDistance constraints first. K5a
      // handles circle-circle and circle-arc; line-line equal
      // is deferred to K5b's adapter layer.
      const a = entityIndex.get(constraint.a);
      const b = entityIndex.get(constraint.b);
      if (!a || !b) {
        throw new Error("solver: equal references unknown entities");
      }
      if (a.kind === "circle" && b.kind === "circle") {
        return {
          type: "equal_radius_cc",
          id,
          c1_id: entityOid(constraint.a),
          c2_id: entityOid(constraint.b),
        };
      }
      if (a.kind === "arc" && b.kind === "arc") {
        return {
          type: "equal_radius_aa",
          id,
          a1_id: entityOid(constraint.a),
          a2_id: entityOid(constraint.b),
        };
      }
      if (
        (a.kind === "circle" && b.kind === "arc") ||
        (a.kind === "arc" && b.kind === "circle")
      ) {
        return {
          type: "equal_radius_ca",
          id,
          c1_id: entityOid(a.kind === "circle" ? constraint.a : constraint.b),
          a2_id: entityOid(a.kind === "arc" ? constraint.a : constraint.b),
        };
      }
      throw new Error(
        `solver: equal between ${a.kind} and ${b.kind} is not yet supported`,
      );
    }
    case "midpoint": {
      // FieldDraft's `Midpoint { point, line }` means "the point lies at
      // the midpoint of the line". A point M is the midpoint of segment
      // (A, B) exactly when A and B are symmetric about M — which is
      // PlaneGCS' `p2p_symmetric_ppp` (p1, p2 symmetric about p). So we
      // resolve the line's two endpoints and pin `point` as their
      // symmetry centre. One constraint, no helper geometry.
      const line = entityIndex.get(constraint.line);
      if (!line || line.kind !== "line") {
        throw new Error("solver: midpoint requires a line");
      }
      return {
        type: "p2p_symmetric_ppp",
        id,
        p1_id: entityOid(line.start),
        p2_id: entityOid(line.end),
        p_id: entityOid(constraint.point),
      };
    }
    case "symmetric": {
      // `Symmetric { a, b, axis }`: points a and b mirror across the
      // axis line. Maps one-to-one to PlaneGCS' `p2p_symmetric_ppl`.
      const axis = entityIndex.get(constraint.axis);
      if (!axis || axis.kind !== "line") {
        throw new Error("solver: symmetric axis must be a line");
      }
      return {
        type: "p2p_symmetric_ppl",
        id,
        p1_id: entityOid(constraint.a),
        p2_id: entityOid(constraint.b),
        l_id: entityOid(constraint.axis),
      };
    }
    case "radius": {
      // Dispatch by entity kind: circle → circle_radius, arc →
      // arc_radius. The stored value is the radius in mm.
      const e = entityIndex.get(constraint.entity);
      if (!e || (e.kind !== "circle" && e.kind !== "arc")) {
        throw new Error("solver: radius requires a circle or arc");
      }
      const driving = !(constraint.driven ?? false);
      return e.kind === "circle"
        ? {
            type: "circle_radius",
            id,
            c_id: entityOid(constraint.entity),
            radius: constraint.value_mm,
            driving,
          }
        : {
            type: "arc_radius",
            id,
            a_id: entityOid(constraint.entity),
            radius: constraint.value_mm,
            driving,
          };
    }
    case "diameter": {
      // Dispatch by entity kind: circle → circle_diameter, arc →
      // arc_diameter. The stored value is the diameter in mm.
      const e = entityIndex.get(constraint.entity);
      if (!e || (e.kind !== "circle" && e.kind !== "arc")) {
        throw new Error("solver: diameter requires a circle or arc");
      }
      const driving = !(constraint.driven ?? false);
      return e.kind === "circle"
        ? {
            type: "circle_diameter",
            id,
            c_id: entityOid(constraint.entity),
            diameter: constraint.value_mm,
            driving,
          }
        : {
            type: "arc_diameter",
            a_id: entityOid(constraint.entity),
            id,
            diameter: constraint.value_mm,
            driving,
          };
    }
  }
}

// ─── main entry point ───────────────────────────────────────────

/**
 * Solve a sketch. Loads the PlaneGCS WASM module on first call,
 * translates every entity and constraint, runs the solver,
 * reads the updated parameters back into a fresh
 * [`SketchState`], and returns it alongside
 * [`SketchDiagnostics`] derived from the solver's conflicting /
 * redundant output.
 *
 * The input sketch is not mutated; the returned state is a deep
 * copy with updated point positions and circle / arc radii.
 */
/**
 * Serializes solves. The PlaneGCS wrapper is a shared singleton with
 * mutable state (cleared via `clear_data` at the start of each solve),
 * so two solves that overlap — e.g. the solve fired when a dimension
 * is applied and the one fired immediately after when its value is
 * edited — would interleave their `clear_data` / `push_primitive`
 * calls and corrupt the system ("object with id e_0 already exists").
 * Chaining every solve through one promise keeps them strictly
 * sequential. A rejected solve is swallowed on the chain so a single
 * failure can't wedge all later solves.
 */
let solveQueue: Promise<unknown> = Promise.resolve();

/** Param index of a pushed point entity: `[posX, posY]` (push_point
 * lays x then y consecutively). Throws if the point wasn't pushed. */
function pointParamIndex(
  wrapper: GcsWrapper,
  id: EntityId,
  context: string,
): [number, number] {
  const px = wrapper.p_param_index.get(entityOid(id));
  if (px === undefined) {
    throw new Error(`solver: ${context} point ${id} has no GCS params`);
  }
  return [px, px + 1];
}

/** Seed the curve parameter `u` for a point-on-spline by projecting the
 * point onto the sampled curve and taking the nearest sample's
 * parameter. `u` ranges over the clamped knot domain `[0, n − degree]`.
 * A close seed matters: the DogLeg solver (the wrapper default) can fail
 * to converge from a poor initial `u`, where a nearby one converges. */
function nearestCurveU(
  curve: Extract<Entity, { kind: "bspline" }>,
  entityIndex: Map<EntityId, Entity>,
  pointId: EntityId,
): number {
  const p = requirePoint(entityIndex, pointId, "point_on_bspline.point");
  const n = curve.poles.length;
  if (n < 2) return 0;
  const poles: [number, number][] = curve.poles.map((id) => {
    const q = requirePoint(entityIndex, id, "bspline.pole");
    return [q.x, q.y];
  });
  const pts = bsplinePoints(poles, curve.degree, curve.periodic, 64);
  if (pts.length < 2) return 0;
  let bestJ = 0;
  let bestD = Infinity;
  pts.forEach((q, j) => {
    const d = (q[0] - p.x) ** 2 + (q[1] - p.y) ** 2;
    if (d < bestD) {
      bestD = d;
      bestJ = j;
    }
  });
  // Samples span the parameter domain uniformly; map sample fraction to
  // the open clamped domain [0, n − degree] (periodic is always false in
  // our wiring).
  return (bestJ / (pts.length - 1)) * (n - curve.degree);
}

/**
 * Build every B-spline entity directly on the raw `GcsSystem` and add
 * any point-on-spline constraints that target them. B-splines aren't
 * part of the declarative SketchPrimitive layer, so this runs AFTER all
 * declarative primitives are pushed (the poles already have GCS params)
 * and BEFORE `solve()` (which doesn't clear data).
 *
 * `make_bspline` takes parameter *indices* for the poles/weights/knots
 * (recovered from the wrapper's `p_param_index`) plus integer knot
 * multiplicities. We synthesize the clamped-uniform knot vector that
 * matches the curve the kernel draws (`bspline_quad_beziers`): distinct
 * knots `[0,1,…,n−degree]`, end multiplicity `degree+1`, interior 1.
 * Weights are all 1 (non-rational). Every param we push here is fixed,
 * so the B-spline adds no DOF beyond what its poles already have.
 */
function injectBSplines(
  wrapper: GcsWrapper,
  module: ModuleStatic,
  input: SketchState,
  entityIndex: Map<EntityId, Entity>,
): void {
  const splines = input.entities.filter(
    (e): e is Extract<Entity, { kind: "bspline" }> => e.kind === "bspline",
  );
  if (splines.length === 0) return;

  const gcs = wrapper.gcs;
  const intVec = (arr: number[]): InstanceType<ModuleStatic["IntVector"]> => {
    const v = new module.IntVector();
    for (const x of arr) v.push_back(x);
    return v;
  };

  const handles = new Map<EntityId, ReturnType<typeof gcs.make_bspline>>();

  for (const e of splines) {
    const n = e.poles.length;
    const degree = e.degree;
    const polesXY: number[] = [];
    for (const p of e.poles) {
      const [px, py] = pointParamIndex(wrapper, p, "bspline.pole");
      polesXY.push(px, py);
    }
    // weights all 1 (fixed → no DOF).
    const weights = e.poles.map(() => gcs.push_p_param(1, true));
    // clamped-uniform knots: distinct values [0,…,n−degree].
    const last = n - degree;
    const knots: number[] = [];
    const mult: number[] = [];
    for (let k = 0; k <= last; k++) {
      knots.push(gcs.push_p_param(k, true));
      mult.push(k === 0 || k === last ? degree + 1 : 1);
    }
    const polesVec = intVec(polesXY);
    const weightsVec = intVec(weights);
    const knotsVec = intVec(knots);
    const multVec = intVec(mult);
    const bs = gcs.make_bspline(
      polesXY[0],
      polesXY[1],
      polesXY[polesXY.length - 2],
      polesXY[polesXY.length - 1],
      polesVec,
      weightsVec,
      knotsVec,
      multVec,
      degree,
      e.periodic,
    );
    // make_bspline copies the vectors by value; free our handles.
    polesVec.delete();
    weightsVec.delete();
    knotsVec.delete();
    multVec.delete();
    handles.set(e.id, bs);
  }

  // High tag offset keeps these raw constraints from colliding with the
  // wrapper's small sequential declarative tags; a flagged one maps back
  // to `undefined` in diagnostics and is filtered (see parseConstraintOid).
  const RAW_TAG_OFFSET = 1_000_000;
  for (const c of input.constraints) {
    if (c.kind !== "point_on_object") continue;
    const curve = entityIndex.get(c.curve);
    if (!curve || curve.kind !== "bspline") continue;
    const bs = handles.get(c.curve);
    if (!bs) continue;
    const [px, py] = pointParamIndex(wrapper, c.point, "point_on_bspline");
    const P = gcs.make_point(px, py);
    const ui = gcs.push_p_param(nearestCurveU(curve, entityIndex, c.point), false);
    gcs.add_constraint_point_on_bspline(P, bs, ui, RAW_TAG_OFFSET + c.id, true, 1);
  }
}

export function solveSketch(input: SketchState): Promise<SolveResult> {
  const run = solveQueue.then(() => solveSketchInner(input));
  solveQueue = run.catch(() => undefined);
  return run;
}

async function solveSketchInner(input: SketchState): Promise<SolveResult> {
  const wrapper = await getWrapper();

  const entityIndex = new Map<EntityId, Entity>();
  for (const e of input.entities) entityIndex.set(e.id, e);

  // Collect every point id targeted by a `fix` constraint before
  // translating entities — a fixed point emits `fixed: true` so the
  // solver treats it as having zero DOFs.
  const fixedPoints = new Set<EntityId>();
  for (const c of input.constraints) {
    if (c.kind === "fix") fixedPoints.add(c.point);
  }

  // Push entities first so later constraints can reference them
  // by oid without ordering surprises.
  for (const entity of input.entities) {
    for (const primitive of translateEntity(entity, entityIndex, fixedPoints)) {
      wrapper.push_primitive(primitive);
    }
  }

  for (const constraint of input.constraints) {
    const primitive = translateConstraint(constraint, entityIndex);
    if (primitive !== null) wrapper.push_primitive(primitive);
  }

  // B-splines and their point-on-spline constraints aren't part of the
  // declarative layer; build them directly on the raw GcsSystem now
  // that every pole has GCS params, and before solve (which doesn't
  // clear data). No-op when the sketch has no B-splines.
  if (planegcsModule) {
    injectBSplines(wrapper, planegcsModule, input, entityIndex);
  }

  const status = wrapper.solve();
  // PlaneGCS Algorithm.DogLeg / SolveStatus.Success maps to 0.
  // Non-zero means "didn't fully converge"; we still pull
  // updated values because PlaneGCS reports the best-effort
  // configuration even on partial failure.
  wrapper.apply_solution();

  const converged = status === 0;
  const diagnostics = buildDiagnostics(wrapper, converged);
  const solvedState = readBackState(input, wrapper);
  return { state: solvedState, diagnostics };
}

function buildDiagnostics(
  wrapper: GcsWrapper,
  converged: boolean,
): SketchDiagnostics {
  const d = emptyDiagnostics();
  d.converged = converged;

  // PlaneGCS returns constraint oids as strings; we filter by
  // the `c_<id>` pattern so internal rule-type constraints that
  // the wrapper pushes for us (arc_rules, circle_radius helpers)
  // don't pollute the diagnostics arrays.
  const collect = (list: string[], into: number[]): void => {
    for (const oid of list) {
      const parsed = parseConstraintOid(oid);
      if (parsed !== null) into.push(parsed);
    }
  };
  collect(wrapper.get_gcs_conflicting_constraints(), d.conflicting);
  collect(wrapper.get_gcs_redundant_constraints(), d.redundant);
  collect(
    wrapper.get_gcs_partially_redundant_constraints(),
    d.partially_redundant,
  );

  // Authoritative degrees-of-freedom straight from the solver.
  // GcsWrapper doesn't surface `dof()` on its own API, but its
  // `gcs` handle (the live `GcsSystem` bound to WASM) is public,
  // so we read the real post-solve number rather than relying on
  // the naive Jacobi tally in `dofPreflight`. PlaneGCS reports
  // the *remaining* DoF (always >= 0); over-constraint is flagged
  // separately through the conflicting/redundant arrays above.
  d.dof = wrapper.gcs.dof();
  return d;
}

/**
 * Walk the wrapper's `sketch_index` to produce an updated
 * [`SketchState`] after a solve. Only the mutable fields change
 * — point positions, circle radii, arc radii — everything else
 * (ids, counters, structural references) is copied verbatim from
 * the input.
 */
function readBackState(
  input: SketchState,
  wrapper: GcsWrapper,
): SketchState {
  const entities: Entity[] = input.entities.map((e) => {
    const oid = entityOid(e.id);
    const updated = wrapper.sketch_index.get_primitive(oid);
    if (!updated) return { ...e };
    switch (e.kind) {
      case "point":
        if (updated.type === "point") {
          return { ...e, at: { x: updated.x, y: updated.y } };
        }
        return { ...e };
      case "circle":
        if (updated.type === "circle") {
          return { ...e, radius: updated.radius };
        }
        return { ...e };
      case "arc":
        if (updated.type === "arc") {
          return { ...e, radius: updated.radius };
        }
        return { ...e };
      case "ellipse":
        // The centre and focus points are separate `point` entities
        // and get their positions back through their own cases; only
        // the semi-minor radius lives on the ellipse primitive.
        if (updated.type === "ellipse") {
          return { ...e, radmin: updated.radmin };
        }
        return { ...e };
      case "line":
      case "origin":
      // A B-spline carries no mutable scalar of its own — its poles are
      // separate `point` entities that read their positions back through
      // the `point` case (and it's never pushed declaratively anyway).
      case "bspline":
        return { ...e };
    }
  });

  return {
    name: input.name,
    next_entity_id: input.next_entity_id,
    next_constraint_id: input.next_constraint_id,
    entities,
    constraints: input.constraints.map((c) => ({ ...c })),
  };
}
