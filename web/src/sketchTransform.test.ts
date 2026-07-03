import { describe, expect, it } from "vitest";
import {
  enterSketchSession,
  setSketchSelection,
  replaceSketchSubCallAt,
  subCallsForSelection,
  entityOwnerSubCalls,
  INACTIVE_SKETCH_SESSION,
} from "./sketchSession";
import { curveOfSubCall, subCallOfCurve } from "./sketchTrim";
import { translateCurve, rotateCurve } from "./sketchPattern";
import type { SketchSessionState } from "./sketchSession";

/**
 * F17 (Move / Copy / Rotate): the store applies a selection transform by
 * (1) mapping the entity-id selection back to its geometry sub-calls via
 * `subCallsForSelection`, (2) reading each via `curveOfSubCall`, (3)
 * transforming with `translateCurve` / `rotateCurve`, then either replacing
 * each in place (`replaceSketchSubCallAt`) or appending copies. These tests
 * exercise exactly that composition at module level — no DOM / kernel — and
 * assert the two load-bearing invariants: an equal-count in-place replace
 * keeps the sub-call count (and hence entity ids) stable, and the moved
 * coordinates round-trip correctly.
 */

const carry = (params: Record<string, unknown>): Record<string, unknown> =>
  params.construction !== undefined ? { construction: params.construction } : {};

/** A session with a single line from (0,0) → (10,0) and a circle at
 *  (20,20) r5, both committed to the spine. */
function twoGeometrySession(): SketchSessionState {
  let s = enterSketchSession(INACTIVE_SKETCH_SESSION, "qXY");
  s = {
    ...s,
    subCalls: [
      { type: "sketch_line", params: { start: [0, 0], end: [10, 0] } },
      { type: "sketch_circle", params: { center: [20, 20], radius: 5 } },
    ],
  };
  return s;
}

/** Mirror of store.moveSketchSelection's in-place branch. */
function applyMoveInPlace(
  session: SketchSessionState,
  dx: number,
  dy: number,
): SketchSessionState {
  const sources = subCallsForSelection(
    session.subCalls,
    session.selection,
  ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
  if (sources.length === 0) return session;
  let next = session;
  for (const i of [...sources].sort((a, b) => b - a)) {
    const cv = curveOfSubCall(session.subCalls[i]);
    if (!cv) continue;
    next = replaceSketchSubCallAt(next, i, [
      subCallOfCurve(translateCurve(cv, dx, dy), carry(session.subCalls[i].params)),
    ]);
  }
  return setSketchSelection(next, session.selection);
}

/** Mirror of store.moveSketchSelection's copy branch (append). */
function applyMoveCopy(
  session: SketchSessionState,
  dx: number,
  dy: number,
): SketchSessionState {
  const sources = subCallsForSelection(
    session.subCalls,
    session.selection,
  ).filter((i) => curveOfSubCall(session.subCalls[i]) !== null);
  if (sources.length === 0) return session;
  const copies = sources
    .map((i) => {
      const cv = curveOfSubCall(session.subCalls[i]);
      return cv
        ? subCallOfCurve(translateCurve(cv, dx, dy), carry(session.subCalls[i].params))
        : null;
    })
    .filter((c): c is NonNullable<typeof c> => c !== null);
  return { ...session, subCalls: [...session.subCalls, ...copies] };
}

describe("F17 selection transform application", () => {
  it("subCallsForSelection + translateCurve round-trips a moved line", () => {
    const s = twoGeometrySession();
    // The line owns the first entities allocated (start + end + line).
    const owner = entityOwnerSubCalls(s.subCalls);
    const lineEntity = [...owner.entries()].find(([, idx]) => idx === 0)![0];
    const sel = setSketchSelection(s, [lineEntity]);

    const sources = subCallsForSelection(sel.subCalls, sel.selection);
    expect(sources).toEqual([0]);

    const cv = curveOfSubCall(sel.subCalls[0]);
    expect(cv).not.toBeNull();
    const moved = translateCurve(cv!, 3, -4);
    expect(moved).toMatchObject({ kind: "line", a: [3, -4], b: [13, -4] });
  });

  it("in-place Move preserves sub-call count and entity ids (id stability)", () => {
    const s = twoGeometrySession();
    const before = entityOwnerSubCalls(s.subCalls);
    const beforeNextId = Math.max(...before.keys()) + 1;

    // Select the line (sub-call 0).
    const lineEntity = [...before.entries()].find(([, idx]) => idx === 0)![0];
    const sel = setSketchSelection(s, [lineEntity]);

    const moved = applyMoveInPlace(sel, 5, 5);

    // Same number of sub-calls → in place, not appended.
    expect(moved.subCalls.length).toBe(s.subCalls.length);
    // Same entity-id allocation map (count unchanged → ids stable).
    const after = entityOwnerSubCalls(moved.subCalls);
    expect(Math.max(...after.keys()) + 1).toBe(beforeNextId);
    expect([...after.entries()].sort()).toEqual([...before.entries()].sort());

    // The line moved.
    const cv = curveOfSubCall(moved.subCalls[0]);
    expect(cv).toMatchObject({ kind: "line", a: [5, 5], b: [15, 5] });
    // The circle is untouched.
    expect(curveOfSubCall(moved.subCalls[1])).toMatchObject({
      kind: "circle",
      c: [20, 20],
      r: 5,
    });
    // Selection is preserved (ids stable).
    expect(moved.selection).toEqual(sel.selection);
  });

  it("Copy appends a translated copy and keeps the originals", () => {
    const s = twoGeometrySession();
    const owner = entityOwnerSubCalls(s.subCalls);
    const circleEntity = [...owner.entries()].find(([, idx]) => idx === 1)![0];
    const sel = setSketchSelection(s, [circleEntity]);

    const out = applyMoveCopy(sel, 0, 100);

    // One new sub-call appended; originals intact.
    expect(out.subCalls.length).toBe(s.subCalls.length + 1);
    expect(out.subCalls.filter((c) => c.type === "sketch_circle").length).toBe(2);
    // The appended copy is the translated circle.
    expect(curveOfSubCall(out.subCalls[2])).toMatchObject({
      kind: "circle",
      c: [20, 120],
      r: 5,
    });
    // The original circle is unchanged.
    expect(curveOfSubCall(out.subCalls[1])).toMatchObject({
      kind: "circle",
      c: [20, 20],
      r: 5,
    });
  });

  it("empty selection is a no-op for both Move and Copy", () => {
    const s = setSketchSelection(twoGeometrySession(), []);
    expect(applyMoveInPlace(s, 5, 5).subCalls).toEqual(s.subCalls);
    expect(applyMoveCopy(s, 5, 5).subCalls).toEqual(s.subCalls);
  });

  it("in-place Rotate about the origin preserves count and rotates coords", () => {
    const s = twoGeometrySession();
    const owner = entityOwnerSubCalls(s.subCalls);
    const lineEntity = [...owner.entries()].find(([, idx]) => idx === 0)![0];
    const sel = setSketchSelection(s, [lineEntity]);

    // 90° CCW about origin: (0,0)→(0,0), (10,0)→(0,10).
    const sources = subCallsForSelection(sel.subCalls, sel.selection);
    let next: SketchSessionState = sel;
    for (const i of [...sources].sort((a, b) => b - a)) {
      const cv = curveOfSubCall(sel.subCalls[i])!;
      next = replaceSketchSubCallAt(next, i, [
        subCallOfCurve(rotateCurve(cv, [0, 0], Math.PI / 2), {}),
      ]);
    }

    expect(next.subCalls.length).toBe(s.subCalls.length);
    const cv = curveOfSubCall(next.subCalls[0]);
    expect(cv).not.toBeNull();
    expect(cv!.kind).toBe("line");
    if (cv && cv.kind === "line") {
      expect(cv.a[0]).toBeCloseTo(0);
      expect(cv.a[1]).toBeCloseTo(0);
      expect(cv.b[0]).toBeCloseTo(0);
      expect(cv.b[1]).toBeCloseTo(10);
    }
  });
});
