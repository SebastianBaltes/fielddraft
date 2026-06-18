/**
 * Module bridge carrying the workspace length unit into the sketch overlay.
 *
 * The overlay's dimension drawing (`drawLinearDimension`, the live readouts)
 * runs in plain module functions with no React context, so the `SketchCanvas2d`
 * component pushes the current unit + decimals here once per change and the
 * drawing code reads it back. Mirrors the other sketch bridges
 * (sketchCameraBridge, sketchLabelBridge, …).
 */

import { formatLength, mmToUnit, unitToMm } from "./lengthUnits";

interface SketchUnitState {
  unit: string;
  decimals: number;
}

const state: SketchUnitState = { unit: "mm", decimals: 2 };

export function setSketchLengthUnit(unit: string, decimals: number): void {
  state.unit = unit;
  state.decimals = decimals;
}

export function getSketchLengthUnit(): Readonly<SketchUnitState> {
  return state;
}

/** Format a millimetre length in the active workspace unit, e.g. "15 cm". */
export function formatSketchLength(mm: number): string {
  return formatLength(mm, state.unit, state.decimals);
}

/** The active unit's display value for `mm` (number only, no label). */
export function sketchDisplayLength(mm: number): number {
  return mmToUnit(mm, state.unit);
}

/** Convert a value typed in the active unit back to millimetres. */
export function sketchLengthToMm(value: number): number {
  return unitToMm(value, state.unit);
}
