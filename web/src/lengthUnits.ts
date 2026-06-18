/**
 * Length-unit conversion — the single source of truth for turning the
 * kernel's internal millimetres into a display unit and back.
 *
 * The kernel works in millimetres everywhere; only the UI shows other
 * units (the workspace `lengthUnit` setting). Keep this table the one
 * place that knows the factors — `widgets.tsx` and the sketch overlay
 * both consume it.
 */

/** Millimetres per one unit (mm is the kernel/base unit). */
export const LENGTH_TO_MM: Record<string, number> = {
  mm: 1,
  cm: 10,
  m: 1000,
  in: 25.4,
  ft: 304.8,
};

export function isLengthUnit(unit: string): boolean {
  return unit.toLowerCase() in LENGTH_TO_MM;
}

/** Convert a value in millimetres to `unit`. Unknown unit → unchanged. */
export function mmToUnit(mm: number, unit: string): number {
  const f = LENGTH_TO_MM[unit.toLowerCase()];
  return f === undefined ? mm : mm / f;
}

/** Convert a value given in `unit` back to millimetres. Unknown unit → unchanged. */
export function unitToMm(value: number, unit: string): number {
  const f = LENGTH_TO_MM[unit.toLowerCase()];
  return f === undefined ? value : value * f;
}

/** Format a millimetre length in `unit` with `decimals` places, e.g. "15 cm". */
export function formatLength(mm: number, unit: string, decimals: number): string {
  return `${mmToUnit(mm, unit).toFixed(decimals)} ${unit}`;
}
