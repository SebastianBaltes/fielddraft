/**
 * NumberInput — the app-wide relaxed numeric input.
 *
 * `<input type="number">` fights the user: it wipes the field on an
 * intermediate keystroke (a lone "-", a trailing "."), refuses a decimal comma,
 * clamps mid-word, and bolts on a spinner nobody asked for. The feature dialogs
 * already worked around this with a text-based buffered input; this promotes
 * that proven pattern to ONE generic component so every raw number field in the
 * app can drop the native control.
 *
 * While focused it keeps a local text buffer, so ANY intermediate state (`""`,
 * `"-"`, `"1."`, `"0,3"`, `".5"`) is allowed as you type. It commits the parsed
 * number to `onChange` (clamped to `[min, max]`, optionally rounded) whenever
 * the text currently reads as a finite number; a still-incomplete field simply
 * doesn't commit. On blur / Enter it snaps the text to the committed value, or
 * reverts to the last good value if the field was left unparseable. Arrow
 * up/down nudge by `step`.
 */

import { useEffect, useState } from "react";
import type React from "react";

/**
 * Parse a relaxed numeric string → number, or `null` for empty / intermediate /
 * invalid input (`""`, `"-"`, `"1e"`, `"abc"`). Accepts a decimal comma and
 * surrounding whitespace.
 */
export function parseRelaxedNumber(raw: string): number | null {
  const s = raw.trim().replace(",", ".");
  if (s === "") return null;
  const n = Number(s);
  return Number.isFinite(n) ? n : null;
}

/** Clamp to `[min, max]` (either bound optional) and optionally round to int. */
export function clampNumber(
  n: number,
  opts: { min?: number; max?: number; integer?: boolean },
): number {
  let v = opts.integer ? Math.round(n) : n;
  if (opts.min != null && v < opts.min) v = opts.min;
  if (opts.max != null && v > opts.max) v = opts.max;
  return v;
}

function formatNumber(v: number): string {
  return Number.isFinite(v) ? String(v) : "";
}

export interface NumberInputProps
  extends Omit<
    React.InputHTMLAttributes<HTMLInputElement>,
    "value" | "onChange" | "type" | "min" | "max" | "step" | "defaultValue"
  > {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  /** Arrow-up / arrow-down increment (default 1). */
  step?: number;
  /** Round to the nearest integer before committing. */
  integer?: boolean;
  /**
   * Commit a parsed value on every valid keystroke (live preview) — the
   * default. Set `false` to commit only on blur / Enter, for fields whose
   * change is expensive (e.g. a sketch constraint that re-solves the sketch).
   */
  commitOnChange?: boolean;
}

export function NumberInput({
  value,
  onChange,
  min,
  max,
  step = 1,
  integer,
  commitOnChange = true,
  onFocus,
  onBlur,
  onKeyDown,
  ...rest
}: NumberInputProps) {
  const clampOpts = { min, max, integer };
  const [raw, setRaw] = useState(() => formatNumber(value));
  const [editing, setEditing] = useState(false);

  // Reflect external value changes while idle; never fight the user's text.
  useEffect(() => {
    if (!editing) setRaw(formatNumber(value));
  }, [value, editing]);

  // Parse `text`, clamp, and commit — returns whether it committed.
  const tryCommit = (text: string): boolean => {
    const parsed = parseRelaxedNumber(text);
    if (parsed == null) return false;
    onChange(clampNumber(parsed, clampOpts));
    return true;
  };

  // Blur / Enter: commit the current text and normalise it, or revert.
  const settle = () => {
    const parsed = parseRelaxedNumber(raw);
    if (parsed == null) {
      setRaw(formatNumber(value));
      return;
    }
    const c = clampNumber(parsed, clampOpts);
    onChange(c);
    setRaw(formatNumber(c));
  };

  return (
    <input
      {...rest}
      type="text"
      inputMode="decimal"
      value={raw}
      onChange={(e) => {
        const s = e.currentTarget.value;
        setRaw(s);
        if (commitOnChange) tryCommit(s);
      }}
      onFocus={(e) => {
        setEditing(true);
        onFocus?.(e);
      }}
      onBlur={(e) => {
        setEditing(false);
        settle();
        onBlur?.(e);
      }}
      onKeyDown={(e) => {
        onKeyDown?.(e);
        if (e.defaultPrevented) return;
        if (e.key === "Enter") {
          settle();
        } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          const base = parseRelaxedNumber(raw) ?? value;
          const next = clampNumber(
            base + (e.key === "ArrowUp" ? step : -step),
            clampOpts,
          );
          onChange(next);
          setRaw(formatNumber(next));
        }
      }}
    />
  );
}
