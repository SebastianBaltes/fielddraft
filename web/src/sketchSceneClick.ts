/**
 * Ticket 4anf: context-dependent routing of a 3D-SCENE click while a sketch
 * session is open.
 *
 * The problem: clicking into the 3D scene during a sketch (e.g. to draw a
 * polygon) must NOT deselect / commit the sketch feature or activate some other
 * feature — the sketch ends ONLY via Accept (✓) / Cancel (✗). At the same time
 * a few sketch contexts legitimately need to pick scene geometry:
 *
 *  - the sketch-PLANE field has click focus (a query picker is armed): the click
 *    may select a default geometry plane OR a B-Rep face to lock the plane;
 *  - the PROJECT tool (`sketch_project`) is active: the click may pick B-Rep
 *    faces / edges to project into the sketch.
 *
 * In every OTHER sketch context the scene bodies are INERT: the click resolves
 * to nothing in 3D — no feature/element/datum selection, no mode activation, no
 * session change.
 *
 * The decision is a pure function of the sketch context so it is exhaustively
 * unit-testable and lives in exactly one place. The Viewport's click handler
 * consults it BEFORE running any of its default selection / mode branches — the
 * ticket explicitly rules out an overlay div that swallows clicks; the routing
 * is what changes behaviour, not the DOM stacking.
 *
 * NOTE this governs the click ROUTING only. Whether a query picker ultimately
 * consumes the click (a valid plane/face under the cursor) is decided by the
 * picker itself (`tryQueryPick`); when it doesn't, an armed-picker click is a
 * no-op — never a fall-through to feature selection. This module's job is to say
 * which of those code paths the click is even allowed to enter.
 */

/** The sketch-relevant context of a single 3D-scene click. */
export interface SketchSceneContext {
  /** A sketch session is currently active (the Detail view is open). */
  sketchActive: boolean;
  /** A query-by-click picker is armed AND its filter accepts a sketch plane
   *  (a default geometry plane or a B-Rep face) — i.e. the sketch-plane field
   *  holds the viewport click focus. */
  planeQueryArmed: boolean;
  /** The active sketch tool is the Project tool (`sketch_project`), which picks
   *  scene B-Rep edges/faces to project into the 2D sketch. */
  projectToolActive: boolean;
}

/**
 * What a 3D-scene click is allowed to do, given the sketch context.
 *
 *  - `"default"` — no sketch session; the normal viewport selection / mode
 *    routing applies unchanged.
 *  - `"plane-pick"` — a sketch session is active and the sketch-plane field is
 *    armed; the click may resolve to a default plane or a B-Rep face (the
 *    query-picker path). It must NOT fall through to feature selection.
 *  - `"project-pick"` — a sketch session is active and the Project tool is on;
 *    the click may pick B-Rep faces/edges for projection. (In practice the 2D
 *    overlay owns this pick, but the routing makes the intent explicit and
 *    keeps the scene non-selecting.)
 *  - `"inert"` — a sketch session is active in any other context; the scene is
 *    completely inert: the click selects nothing, activates nothing, and never
 *    ends the session.
 */
export type SketchSceneRouting =
  | "default"
  | "plane-pick"
  | "project-pick"
  | "inert";

export function routeSketchSceneClick(
  ctx: SketchSceneContext,
): SketchSceneRouting {
  // No session → the viewport behaves exactly as it does outside sketch mode.
  if (!ctx.sketchActive) return "default";
  // Sketch-plane field armed → the click may pick the plane (default plane or
  // B-Rep face). Takes precedence over the Project tool: arming the plane field
  // is an explicit, focused intent.
  if (ctx.planeQueryArmed) return "plane-pick";
  // Project tool → the click may pick B-Rep geometry to project.
  if (ctx.projectToolActive) return "project-pick";
  // Every other sketch context: the scene is inert.
  return "inert";
}

/**
 * Convenience predicate: may a 3D-scene click select / activate scene
 * features, bodies, elements or datums right now?
 *
 * True only outside a sketch session. Inside a session every routing keeps
 * the scene from changing the feature/element/datum selection — the
 * query-picker paths feed a field, they don't select — so the answer is
 * always false while a sketch is open.
 */
export function sceneClickMaySelectFeatures(ctx: SketchSceneContext): boolean {
  return routeSketchSceneClick(ctx) === "default";
}
