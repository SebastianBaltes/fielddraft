/**
 * FeatureDialog widget kit (K6a).
 *
 * Each widget is a controlled React component — it takes the
 * current value plus an `onChange(next)` callback and renders a
 * little editor for one parameter. The FeatureDialog stitches
 * them together into a full panel based on the parsed schema.
 *
 * Widgets avoid owning local state beyond what the keyboard
 * focus needs: the definition blob in the parent dialog is the
 * single source of truth, every edit flows through `onChange`,
 * and the parent re-renders with the new value as the next
 * prop. That's the standard React controlled-input pattern;
 * it keeps the dialog commit path simple (no diffing, no
 * sync-loop management) and makes the widgets trivially
 * replaceable once K6b wants more specialised behaviour.
 *
 * The K6a kit covers the eight known widget kinds — number,
 * vec3, bool, string, bool_op, length, angle, query — plus a
 * fallback JSON text editor for `unknown` fields so novel
 * Rust-side types (e.g. a custom Enum via the generic derive
 * fallback) still get a usable editor with zero extra code.
 */

import { useId } from "react";
import {
  ANGLE_UNITS,
  BOOL_OPS,
  LENGTH_UNITS,
  type AngleUnit,
  type BoolOpWire,
  type LengthUnit,
} from "./schema";

// ─── shared widget props ────────────────────────────────────────

interface WidgetProps<T> {
  label: string;
  value: T;
  onChange: (next: T) => void;
  /** Optional per-field error from the dialog's validator. Shown
   *  beneath the widget as a red hint when set. */
  error?: string | null;
  required?: boolean;
}

/** Shared label + error chrome around a single widget body. */
function Field({
  label,
  htmlFor,
  error,
  required,
  children,
}: {
  label: string;
  htmlFor: string;
  error?: string | null;
  required?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="field">
      <label className="label" htmlFor={htmlFor}>
        {label}
        {required ? <span className="required"> *</span> : null}
      </label>
      {children}
      {error ? <div className="error">{error}</div> : null}
    </div>
  );
}

// ─── number ─────────────────────────────────────────────────────

export function NumberWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<number>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <input
        id={id}
        className="input"
        type="number"
        inputMode="decimal"
        value={Number.isFinite(value) ? value : 0}
        onChange={(e) => {
          const n = Number(e.currentTarget.value);
          if (Number.isFinite(n)) {
            onChange(n);
          }
        }}
      />
    </Field>
  );
}

// ─── vec3 ───────────────────────────────────────────────────────

export function Vec3Widget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<[number, number, number]>) {
  const id = useId();
  const updateIndex = (index: 0 | 1 | 2, next: number) => {
    const copy: [number, number, number] = [value[0], value[1], value[2]];
    copy[index] = next;
    onChange(copy);
  };
  return (
    <Field label={label} htmlFor={`${id}-x`} error={error} required={required}>
      <div className="vec3-row">
        {(["x", "y", "z"] as const).map((axis, i) => (
          <input
            key={axis}
            id={`${id}-${axis}`}
            className="input"
            type="number"
            inputMode="decimal"
            aria-label={`${label} ${axis}`}
            value={Number.isFinite(value[i]) ? value[i] : 0}
            onChange={(e) => {
              const n = Number(e.currentTarget.value);
              if (Number.isFinite(n)) {
                updateIndex(i as 0 | 1 | 2, n);
              }
            }}
          />
        ))}
      </div>
    </Field>
  );
}

// ─── bool ───────────────────────────────────────────────────────

export function BoolWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<boolean>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <input
        id={id}
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.currentTarget.checked)}
      />
    </Field>
  );
}

// ─── string ─────────────────────────────────────────────────────

export function StringWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<string>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <input
        id={id}
        className="input"
        type="text"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
      />
    </Field>
  );
}

// ─── length ─────────────────────────────────────────────────────

export interface LengthValue {
  value: number;
  unit: LengthUnit;
}

export function LengthWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<LengthValue>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={`${id}-value`} error={error} required={required}>
      <div className="unit-row">
        <input
          id={`${id}-value`}
          className="input"
          type="number"
          inputMode="decimal"
          value={Number.isFinite(value.value) ? value.value : 0}
          onChange={(e) => {
            const n = Number(e.currentTarget.value);
            if (Number.isFinite(n)) {
              onChange({ value: n, unit: value.unit });
            }
          }}
        />
        <select
          id={`${id}-unit`}
          className="select"
          aria-label={`${label} unit`}
          value={value.unit}
          onChange={(e) =>
            onChange({
              value: value.value,
              unit: e.currentTarget.value as LengthUnit,
            })
          }
        >
          {LENGTH_UNITS.map((u) => (
            <option key={u} value={u}>
              {u}
            </option>
          ))}
        </select>
      </div>
    </Field>
  );
}

// ─── angle ──────────────────────────────────────────────────────

export interface AngleValue {
  value: number;
  unit: AngleUnit;
}

export function AngleWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<AngleValue>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={`${id}-value`} error={error} required={required}>
      <div className="unit-row">
        <input
          id={`${id}-value`}
          className="input"
          type="number"
          inputMode="decimal"
          value={Number.isFinite(value.value) ? value.value : 0}
          onChange={(e) => {
            const n = Number(e.currentTarget.value);
            if (Number.isFinite(n)) {
              onChange({ value: n, unit: value.unit });
            }
          }}
        />
        <select
          id={`${id}-unit`}
          className="select"
          aria-label={`${label} unit`}
          value={value.unit}
          onChange={(e) =>
            onChange({
              value: value.value,
              unit: e.currentTarget.value as AngleUnit,
            })
          }
        >
          {ANGLE_UNITS.map((u) => (
            <option key={u} value={u}>
              {u}
            </option>
          ))}
        </select>
      </div>
    </Field>
  );
}

// ─── bool_op ────────────────────────────────────────────────────

export function BoolOpWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<BoolOpWire>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <select
        id={id}
        className="select"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value as BoolOpWire)}
      >
        {BOOL_OPS.map((op) => (
          <option key={op} value={op}>
            {op}
          </option>
        ))}
      </select>
    </Field>
  );
}

// ─── query ──────────────────────────────────────────────────────

/**
 * K6a: a minimal text-input for query strings. K6b will swap
 * this out for a real `QueryPicker` that lets the user click
 * into the viewport and resolves the click into a `qFaceOf` or
 * `qEdgeBetween` expression via the B-Rep proxy. Until then the
 * author can hand-type any K4.5.7 query form.
 */
export function QueryWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<string>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <input
        id={id}
        className="input query-input"
        type="text"
        spellCheck={false}
        placeholder="qCreatedBy(…)"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
      />
    </Field>
  );
}

// ─── unknown / generic fallback ────────────────────────────────

/**
 * Escape hatch for schema fields the K6a widget kit doesn't
 * recognise natively (custom Rust enums via the `generic`
 * derive fallback, Option-wrapped types once they ship, …).
 * Renders a JSON text editor so the author can still author
 * the field by hand — the dialog's validator swallows invalid
 * JSON and keeps the last good value until the user types
 * something parseable.
 */
export function UnknownWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<unknown>) {
  const id = useId();
  const serialised = stringifyPretty(value);
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <textarea
        id={id}
        className="textarea"
        rows={3}
        value={serialised}
        onChange={(e) => {
          const text = e.currentTarget.value;
          try {
            onChange(JSON.parse(text));
          } catch {
            // Parse failure: leave the last good value alone.
            // The visible text is re-rendered from the last
            // good value on the next render, which is the
            // standard React-uncontrolled-inside-controlled
            // pattern.
          }
        }}
      />
    </Field>
  );
}

function stringifyPretty(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}
