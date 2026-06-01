/**
 * P1 goldfile tests — assembly mate solver verification.
 *
 * Three canonical test scenes from the ROADMAP:
 * 1. Robot arm (serial revolute chain)
 * 2. Linear axis (prismatic mate)
 * 3. Scissor mechanism (mixed mate types)
 */

import { describe, expect, it } from "vitest";
import {
  type AssemblyDocument,
  type AssemblyPartInstance,
  solveAssembly,
  solveIK,
} from "./assembly";

function makePart(
  id: string,
  name: string,
  connectors: { featureId: number; origin: [number, number, number]; normal: [number, number, number] }[] = [],
  translate: [number, number, number] = [0, 0, 0],
): AssemblyPartInstance {
  return {
    id,
    name,
    documentJson: "{}",
    transform: { translate, rotate: [0, 0, 0] },
    meshBodies: [],
    mateConnectors: connectors.map((c) => ({
      featureId: c.featureId,
      origin: c.origin,
      normal: c.normal,
    })),
  };
}

describe("P1 goldfile: robot arm (revolute chain)", () => {
  const base = makePart("base", "Base", [
    { featureId: 1, origin: [0, 50, 0], normal: [0, 1, 0] },
  ]);
  const upperArm = makePart("upper", "Upper Arm", [
    { featureId: 2, origin: [0, 0, 0], normal: [0, 1, 0] },
    { featureId: 3, origin: [0, 100, 0], normal: [0, 1, 0] },
  ]);
  const forearm = makePart("fore", "Forearm", [
    { featureId: 4, origin: [0, 0, 0], normal: [0, 1, 0] },
    { featureId: 5, origin: [0, 80, 0], normal: [0, 1, 0] },
  ]);

  it("solves a 2-joint arm at zero angles", () => {
    const doc: AssemblyDocument = {
      schemaVersion: 1,
      name: "Robot Arm",
      parts: [base, upperArm, forearm],
      mates: [
        {
          id: "m1", name: "shoulder", type: "revolute",
          connectorA: { partId: "base", connectorFeatureId: 1 },
          connectorB: { partId: "upper", connectorFeatureId: 2 },
          angle: 0,
        },
        {
          id: "m2", name: "elbow", type: "revolute",
          connectorA: { partId: "upper", connectorFeatureId: 3 },
          connectorB: { partId: "fore", connectorFeatureId: 4 },
          angle: 0,
        },
      ],
    };

    const solved = solveAssembly(doc);
    expect(solved[0].transform.translate).toEqual([0, 0, 0]);
    expect(solved[1].transform.translate[1]).toBeCloseTo(50, 0);
    expect(solved[2].transform.translate[1]).toBeCloseTo(150, 0);
  });

  it("shoulder rotation at 90° moves upper arm", () => {
    const doc: AssemblyDocument = {
      schemaVersion: 1,
      name: "Robot Arm",
      parts: [base, upperArm, forearm],
      mates: [
        {
          id: "m1", name: "shoulder", type: "revolute",
          connectorA: { partId: "base", connectorFeatureId: 1 },
          connectorB: { partId: "upper", connectorFeatureId: 2 },
          angle: 90,
        },
        {
          id: "m2", name: "elbow", type: "revolute",
          connectorA: { partId: "upper", connectorFeatureId: 3 },
          connectorB: { partId: "fore", connectorFeatureId: 4 },
          angle: 0,
        },
      ],
    };

    const solved = solveAssembly(doc);
    expect(solved[1].transform.rotate[1]).toBeCloseTo(90, 0);
  });
});

describe("P1 goldfile: linear axis (prismatic)", () => {
  const rail = makePart("rail", "Rail", [
    { featureId: 1, origin: [0, 0, 0], normal: [1, 0, 0] },
  ]);
  const carriage = makePart("car", "Carriage", [
    { featureId: 2, origin: [0, 0, 0], normal: [1, 0, 0] },
  ]);

  it("prismatic offset moves carriage along normal", () => {
    const doc: AssemblyDocument = {
      schemaVersion: 1,
      name: "Linear Axis",
      parts: [rail, carriage],
      mates: [
        {
          id: "m1", name: "slide", type: "prismatic",
          connectorA: { partId: "rail", connectorFeatureId: 1 },
          connectorB: { partId: "car", connectorFeatureId: 2 },
          offset: 50,
        },
      ],
    };

    const solved = solveAssembly(doc);
    expect(solved[1].transform.translate[0]).toBeCloseTo(50, 0);
    expect(solved[1].transform.translate[1]).toBeCloseTo(0, 0);
  });
});

describe("P1 goldfile: scissor mechanism (cylindrical)", () => {
  const arm1 = makePart("a1", "Arm 1", [
    { featureId: 1, origin: [0, 0, 0], normal: [0, 1, 0] },
  ]);
  const arm2 = makePart("a2", "Arm 2", [
    { featureId: 2, origin: [0, 0, 0], normal: [0, 1, 0] },
  ]);

  it("cylindrical mate allows rotation + offset", () => {
    const doc: AssemblyDocument = {
      schemaVersion: 1,
      name: "Scissor",
      parts: [arm1, arm2],
      mates: [
        {
          id: "m1", name: "pivot", type: "cylindrical",
          connectorA: { partId: "a1", connectorFeatureId: 1 },
          connectorB: { partId: "a2", connectorFeatureId: 2 },
          angle: 45,
          offset: 10,
        },
      ],
    };

    const solved = solveAssembly(doc);
    expect(solved[1].transform.rotate[1]).toBeCloseTo(45, 0);
    expect(solved[1].transform.translate[1]).toBeCloseTo(10, 0);
  });
});

describe("P1 goldfile: ball joint", () => {
  const socket = makePart("s", "Socket", [
    { featureId: 1, origin: [0, 50, 0], normal: [0, 1, 0] },
  ]);
  const ball = makePart("b", "Ball", [
    { featureId: 2, origin: [0, 0, 0], normal: [0, 1, 0] },
  ]);

  it("ball mate aligns origins with free rotation", () => {
    const doc: AssemblyDocument = {
      schemaVersion: 1,
      name: "Ball Joint",
      parts: [socket, ball],
      mates: [
        {
          id: "m1", name: "ball", type: "ball",
          connectorA: { partId: "s", connectorFeatureId: 1 },
          connectorB: { partId: "b", connectorFeatureId: 2 },
        },
      ],
    };

    const solved = solveAssembly(doc);
    expect(solved[1].transform.translate[1]).toBeCloseTo(50, 0);
    expect(solved[1].transform.rotate).toEqual([0, 0, 0]);
  });
});

describe("P2 IK solver", () => {
  it("returns null when no revolute mates exist", () => {
    const doc: AssemblyDocument = {
      schemaVersion: 1,
      name: "No joints",
      parts: [makePart("a", "A")],
      mates: [],
    };
    expect(solveIK(doc, { position: [10, 10, 10], endEffectorPartId: "a", endEffectorConnectorId: 1 })).toBeNull();
  });
});
