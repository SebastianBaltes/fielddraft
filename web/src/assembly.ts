/**
 * P1 — Assembly data model and constraint solver.
 *
 * An assembly is a scene graph of Part Studio instances connected by
 * mechanical mates. Each part instance carries its own document JSON
 * and cached mesh; the assembly layer manages transforms and mate
 * constraints.
 *
 * Architecture: assemblies are a frontend concept. Each part gets its
 * own WASM kernel for meshing. The assembly positions cached mesh
 * bodies using Three.js group transforms. Mates constrain these
 * transforms via a simple constraint solver.
 */

// ── Data model ──────────────────────────────────────────────────

export interface AssemblyPartInstance {
  id: string;
  name: string;
  documentJson: string;
  transform: AssemblyTransform;
  /** Cached mesh bodies from the part's kernel. */
  meshBodies: AssemblyMeshedBody[];
  /** Mate connectors defined in this part (origin + normal). */
  mateConnectors: MateConnectorDef[];
}

export interface AssemblyTransform {
  translate: [number, number, number];
  rotate: [number, number, number]; // Euler angles in degrees (XYZ order)
}

export const IDENTITY_TRANSFORM: AssemblyTransform = {
  translate: [0, 0, 0],
  rotate: [0, 0, 0],
};

export interface AssemblyMeshedBody {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
}

export interface MateConnectorDef {
  featureId: number;
  origin: [number, number, number];
  normal: [number, number, number];
}

// ── Mate types ──────────────────────────────────────────────────

export type MateType = "fixed" | "revolute" | "prismatic" | "cylindrical" | "ball";

export interface Mate {
  id: string;
  name: string;
  type: MateType;
  /** Part instance ID + mate connector feature ID for side A. */
  connectorA: { partId: string; connectorFeatureId: number };
  /** Part instance ID + mate connector feature ID for side B. */
  connectorB: { partId: string; connectorFeatureId: number };
  /** For revolute: current angle in degrees. */
  angle?: number;
  /** For prismatic: current offset in mm. */
  offset?: number;
}

// ── Assembly document ───────────────────────────────────────────

export interface AssemblyDocument {
  schemaVersion: number;
  name: string;
  parts: AssemblyPartInstance[];
  mates: Mate[];
}

export function createEmptyAssembly(name = "Assembly 1"): AssemblyDocument {
  return {
    schemaVersion: 1,
    name,
    parts: [],
    mates: [],
  };
}

// ── Assembly state for the store ────────────────────────────────

export interface AssemblyState {
  active: boolean;
  document: AssemblyDocument;
  selectedPartId: string | null;
}

export const INACTIVE_ASSEMBLY: AssemblyState = {
  active: false,
  document: createEmptyAssembly(),
  selectedPartId: null,
};

// ── Mate constraint solver ──────────────────────────────────────

/**
 * Solve Fixed mates: part B's transform is computed so its mate
 * connector aligns with part A's mate connector.
 *
 * For a Fixed mate, the two connectors must coincide in world space
 * (same origin, same orientation). This sets part B's position to:
 *   partB.translate = partA.translate + connA.origin - connB.origin
 *
 * This is a simplified solver — a full solver would handle chains
 * of mates and kinematic loops.
 */
export function solveFixedMate(
  partA: AssemblyPartInstance,
  connA: MateConnectorDef,
  _partB: AssemblyPartInstance,
  _connB: MateConnectorDef,
): AssemblyTransform {
  const worldA = [
    partA.transform.translate[0] + connA.origin[0],
    partA.transform.translate[1] + connA.origin[1],
    partA.transform.translate[2] + connA.origin[2],
  ] as [number, number, number];

  return {
    translate: [
      worldA[0] - _connB.origin[0],
      worldA[1] - _connB.origin[1],
      worldA[2] - _connB.origin[2],
    ],
    rotate: partA.transform.rotate,
  };
}

let nextId = 1;
export function allocateAssemblyId(): string {
  return `asm_${nextId++}`;
}
