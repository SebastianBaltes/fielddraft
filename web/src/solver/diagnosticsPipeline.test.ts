import { describe, expect, it } from "vitest";
import {
  SketchElementState,
  classifySketch,
  entitiesReferencedBy,
} from "./diagnosticsPipeline";
import { emptyDiagnostics, sketchWithOrigin, type SketchState } from "./types";

function baseSketch(): SketchState {
  const s = sketchWithOrigin();
  s.entities.push({ kind: "point", id: 1, at: { x: 0, y: 0 } });
  s.entities.push({ kind: "point", id: 2, at: { x: 10, y: 0 } });
  s.entities.push({ kind: "line", id: 3, start: 1, end: 2 });
  s.next_entity_id = 4;
  s.constraints.push({ kind: "horizontal", id: 1, line: 3 });
  s.next_constraint_id = 2;
  return s;
}

describe("classifySketch: overall state", () => {
  it("reports FullyConstrained when DOF is zero and no errors", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      dof: 0,
    };
    const out = classifySketch(state, diagnostics);
    expect(out.overall).toBe(SketchElementState.FullyConstrained);
  });

  it("reports UnderConstrained when DOF > 0", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      dof: 3,
    };
    const out = classifySketch(state, diagnostics);
    expect(out.overall).toBe(SketchElementState.UnderConstrained);
  });

  it("reports OverConstrained when DOF < 0", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      dof: -1,
    };
    const out = classifySketch(state, diagnostics);
    expect(out.overall).toBe(SketchElementState.OverConstrained);
  });

  it("reports OverConstrained when there are conflicting constraints", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: false,
      dof: 0,
      conflicting: [1],
    };
    const out = classifySketch(state, diagnostics);
    expect(out.overall).toBe(SketchElementState.OverConstrained);
  });

  it("reports Malformed when there are malformed constraints", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: false,
      dof: 0,
      malformed: [1],
    };
    const out = classifySketch(state, diagnostics);
    expect(out.overall).toBe(SketchElementState.Malformed);
  });

  it("Malformed outranks conflicting", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: false,
      dof: 0,
      conflicting: [1],
      malformed: [1],
    };
    const out = classifySketch(state, diagnostics);
    expect(out.overall).toBe(SketchElementState.Malformed);
  });
});

describe("classifySketch: per-constraint state", () => {
  it("marks conflicting constraints as OverConstrained", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      conflicting: [1],
    };
    const out = classifySketch(state, diagnostics);
    expect(out.constraints.get(1)).toBe(SketchElementState.OverConstrained);
  });

  it("marks malformed constraints as Malformed", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: false,
      malformed: [1],
    };
    const out = classifySketch(state, diagnostics);
    expect(out.constraints.get(1)).toBe(SketchElementState.Malformed);
  });

  it("marks other constraints as FullyConstrained", () => {
    const state = baseSketch();
    state.constraints.push({ kind: "vertical", id: 2, line: 3 });
    state.next_constraint_id = 3;
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      conflicting: [1],
    };
    const out = classifySketch(state, diagnostics);
    expect(out.constraints.get(1)).toBe(SketchElementState.OverConstrained);
    expect(out.constraints.get(2)).toBe(SketchElementState.FullyConstrained);
  });
});

describe("classifySketch: per-entity state propagation", () => {
  it("propagates OverConstrained from a conflicting constraint to its entities", () => {
    const state = baseSketch();
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      conflicting: [1],
    };
    const out = classifySketch(state, diagnostics);
    // The horizontal constraint binds line 3.
    expect(out.entities.get(3)).toBe(SketchElementState.OverConstrained);
  });

  it("worse-of semantics: malformed binding wins over over-constrained", () => {
    const state = baseSketch();
    state.constraints.push({ kind: "vertical", id: 2, line: 3 });
    state.next_constraint_id = 3;
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      conflicting: [1],
      malformed: [2],
    };
    const out = classifySketch(state, diagnostics);
    // Line 3 is bound by both a conflicting and a malformed
    // constraint — malformed wins.
    expect(out.entities.get(3)).toBe(SketchElementState.Malformed);
  });

  it("unbound entities inherit the overall state", () => {
    // Freshly dropped point has no constraints, sketch is
    // under-constrained overall → point shows as under too.
    const state = sketchWithOrigin();
    state.entities.push({ kind: "point", id: 1, at: { x: 5, y: 5 } });
    state.next_entity_id = 2;
    const diagnostics = {
      ...emptyDiagnostics(),
      converged: true,
      dof: 2,
    };
    const out = classifySketch(state, diagnostics);
    expect(out.entities.get(1)).toBe(SketchElementState.UnderConstrained);
  });
});

describe("entitiesReferencedBy", () => {
  it("returns both endpoints for a coincident constraint", () => {
    const ids = entitiesReferencedBy({
      kind: "coincident",
      id: 1,
      a: 7,
      b: 8,
    });
    expect(ids).toEqual([7, 8]);
  });

  it("returns just the line for horizontal / vertical", () => {
    const h = entitiesReferencedBy({
      kind: "horizontal",
      id: 1,
      line: 5,
    });
    expect(h).toEqual([5]);
    const v = entitiesReferencedBy({
      kind: "vertical",
      id: 2,
      line: 6,
    });
    expect(v).toEqual([6]);
  });

  it("returns line + curve for a tangent", () => {
    const ids = entitiesReferencedBy({
      kind: "tangent",
      id: 1,
      line: 3,
      curve: 4,
    });
    expect(ids).toEqual([3, 4]);
  });

  it("returns all three entities for symmetric", () => {
    const ids = entitiesReferencedBy({
      kind: "symmetric",
      id: 1,
      a: 2,
      b: 3,
      axis: 4,
    });
    expect(ids).toEqual([2, 3, 4]);
  });
});
