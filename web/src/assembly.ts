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

function findConnector(
  part: AssemblyPartInstance,
  featureId: number,
): MateConnectorDef | undefined {
  return part.mateConnectors.find((c) => c.featureId === featureId);
}

function worldConnectorOrigin(
  part: AssemblyPartInstance,
  conn: MateConnectorDef,
): [number, number, number] {
  return [
    part.transform.translate[0] + conn.origin[0],
    part.transform.translate[1] + conn.origin[1],
    part.transform.translate[2] + conn.origin[2],
  ];
}

function solveFixed(
  partA: AssemblyPartInstance,
  connA: MateConnectorDef,
  connB: MateConnectorDef,
): AssemblyTransform {
  const worldA = worldConnectorOrigin(partA, connA);
  return {
    translate: [
      worldA[0] - connB.origin[0],
      worldA[1] - connB.origin[1],
      worldA[2] - connB.origin[2],
    ],
    rotate: [...partA.transform.rotate],
  };
}

function solveRevolute(
  partA: AssemblyPartInstance,
  connA: MateConnectorDef,
  connB: MateConnectorDef,
  angleDeg: number,
): AssemblyTransform {
  const worldA = worldConnectorOrigin(partA, connA);
  // Revolute: part B can rotate around the connector normal axis.
  // The rotation is applied around the connector origin in part B's
  // local space. For simplicity, we add the angle to the Y-axis
  // rotation (assuming connectors are Y-up aligned).
  return {
    translate: [
      worldA[0] - connB.origin[0],
      worldA[1] - connB.origin[1],
      worldA[2] - connB.origin[2],
    ],
    rotate: [
      partA.transform.rotate[0],
      partA.transform.rotate[1] + angleDeg,
      partA.transform.rotate[2],
    ],
  };
}

/**
 * Run the constraint solver over all mates in the assembly.
 * Returns a new parts array with updated transforms for parts
 * constrained by mates. Part A of each mate is treated as the
 * grounded/fixed side; part B is repositioned.
 */
export function solveAssembly(doc: AssemblyDocument): AssemblyPartInstance[] {
  const parts = doc.parts.map((p) => ({ ...p }));
  const partMap = new Map(parts.map((p) => [p.id, p]));

  for (const mate of doc.mates) {
    const partA = partMap.get(mate.connectorA.partId);
    const partB = partMap.get(mate.connectorB.partId);
    if (!partA || !partB) continue;

    const connA = findConnector(partA, mate.connectorA.connectorFeatureId);
    const connB = findConnector(partB, mate.connectorB.connectorFeatureId);
    if (!connA || !connB) continue;

    let newTransform: AssemblyTransform;
    switch (mate.type) {
      case "fixed":
        newTransform = solveFixed(partA, connA, connB);
        break;
      case "revolute":
        newTransform = solveRevolute(partA, connA, connB, mate.angle ?? 0);
        break;
      default:
        continue;
    }

    partB.transform = newTransform;
  }

  return parts;
}

let nextId = 1;
export function allocateAssemblyId(): string {
  return `asm_${nextId++}`;
}
