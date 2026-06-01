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
  connectorA: { partId: string; connectorFeatureId: number };
  connectorB: { partId: string; connectorFeatureId: number };
  angle?: number;
  offset?: number;
  /** P2: joint limits for revolute mates (degrees). */
  minAngle?: number;
  maxAngle?: number;
}

// ── Assembly document ───────────────────────────────────────────

export interface AssemblyDocument {
  schemaVersion: number;
  name: string;
  parts: AssemblyPartInstance[];
  mates: Mate[];
  /** P2.8: FieldScript source code for kinematic controller. */
  script?: string;
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

function solvePrismatic(
  partA: AssemblyPartInstance,
  connA: MateConnectorDef,
  connB: MateConnectorDef,
  offsetMm: number,
): AssemblyTransform {
  const worldA = worldConnectorOrigin(partA, connA);
  const n = connA.normal;
  return {
    translate: [
      worldA[0] - connB.origin[0] + n[0] * offsetMm,
      worldA[1] - connB.origin[1] + n[1] * offsetMm,
      worldA[2] - connB.origin[2] + n[2] * offsetMm,
    ],
    rotate: [...partA.transform.rotate],
  };
}

function solveCylindrical(
  partA: AssemblyPartInstance,
  connA: MateConnectorDef,
  connB: MateConnectorDef,
  angleDeg: number,
  offsetMm: number,
): AssemblyTransform {
  const worldA = worldConnectorOrigin(partA, connA);
  const n = connA.normal;
  return {
    translate: [
      worldA[0] - connB.origin[0] + n[0] * offsetMm,
      worldA[1] - connB.origin[1] + n[1] * offsetMm,
      worldA[2] - connB.origin[2] + n[2] * offsetMm,
    ],
    rotate: [
      partA.transform.rotate[0],
      partA.transform.rotate[1] + angleDeg,
      partA.transform.rotate[2],
    ],
  };
}

function solveBall(
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
    rotate: [0, 0, 0],
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
      case "prismatic":
        newTransform = solvePrismatic(partA, connA, connB, mate.offset ?? 0);
        break;
      case "cylindrical":
        newTransform = solveCylindrical(partA, connA, connB, mate.angle ?? 0, mate.offset ?? 0);
        break;
      case "ball":
        newTransform = solveBall(partA, connA, connB);
        break;
      default: {
        // Exhaustiveness guard: a new MateType must add a case above.
        const _exhaustive: never = mate.type;
        void _exhaustive;
        continue;
      }
    }

    partB.transform = newTransform;
  }

  return parts;
}

// ── P2: Inverse Kinematics (CCD) ────────────────────────────────

export interface IKTarget {
  position: [number, number, number];
  endEffectorPartId: string;
  endEffectorConnectorId: number;
}

/**
 * CCD (Cyclic Coordinate Descent) IK solver for a serial chain of
 * revolute mates. Iteratively adjusts each joint angle to bring the
 * end-effector closer to the target position.
 *
 * Returns updated mates with solved joint angles, or null if no
 * revolute chain was found.
 */
export function solveIK(
  doc: AssemblyDocument,
  target: IKTarget,
  maxIterations = 20,
  tolerance = 0.5,
): Mate[] | null {
  // Find the revolute chain leading to the end-effector part.
  const revMates = doc.mates.filter((m) => m.type === "revolute");
  if (revMates.length === 0) return null;

  const mates = doc.mates.map((m) => ({ ...m }));
  const revIndices = mates
    .map((m, i) => (m.type === "revolute" ? i : -1))
    .filter((i) => i >= 0);

  for (let iter = 0; iter < maxIterations; iter++) {
    // Solve the assembly with current angles to get world positions.
    const parts = solveAssembly({ ...doc, mates });
    const partMap = new Map(parts.map((p) => [p.id, p]));

    // Find end-effector world position.
    const eePart = partMap.get(target.endEffectorPartId);
    if (!eePart) return null;
    const eeConn = eePart.mateConnectors.find(
      (c) => c.featureId === target.endEffectorConnectorId,
    );
    if (!eeConn) return null;
    const ee: [number, number, number] = [
      eePart.transform.translate[0] + eeConn.origin[0],
      eePart.transform.translate[1] + eeConn.origin[1],
      eePart.transform.translate[2] + eeConn.origin[2],
    ];

    const dist = Math.hypot(
      target.position[0] - ee[0],
      target.position[1] - ee[1],
      target.position[2] - ee[2],
    );
    if (dist < tolerance) break;

    // CCD: iterate joints from end-effector back to base.
    for (let ji = revIndices.length - 1; ji >= 0; ji--) {
      const mi = revIndices[ji];
      const mate = mates[mi];

      // Find the pivot point (connector A's world position).
      const pivotPart = partMap.get(mate.connectorA.partId);
      if (!pivotPart) continue;
      const pivotConn = pivotPart.mateConnectors.find(
        (c) => c.featureId === mate.connectorA.connectorFeatureId,
      );
      if (!pivotConn) continue;

      const pivot: [number, number, number] = [
        pivotPart.transform.translate[0] + pivotConn.origin[0],
        pivotPart.transform.translate[1] + pivotConn.origin[1],
        pivotPart.transform.translate[2] + pivotConn.origin[2],
      ];

      // Compute angle adjustment in the XZ plane (simplified: Y-axis rotation).
      const toEE = [ee[0] - pivot[0], ee[2] - pivot[2]];
      const toTarget = [target.position[0] - pivot[0], target.position[2] - pivot[2]];

      const angleEE = Math.atan2(toEE[1], toEE[0]);
      const angleTarget = Math.atan2(toTarget[1], toTarget[0]);
      let delta = ((angleTarget - angleEE) * 180) / Math.PI;

      // Wrap to [-180, 180]
      while (delta > 180) delta -= 360;
      while (delta < -180) delta += 360;

      let newAngle = (mate.angle ?? 0) + delta;

      // Apply joint limits
      if (mate.minAngle !== undefined) newAngle = Math.max(newAngle, mate.minAngle);
      if (mate.maxAngle !== undefined) newAngle = Math.min(newAngle, mate.maxAngle);

      mate.angle = newAngle;
    }
  }

  return mates;
}

let nextId = 1;
export function allocateAssemblyId(): string {
  return `asm_${nextId++}`;
}
