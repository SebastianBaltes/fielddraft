/**
 * FeatureDialog widget kit (K6a + K6a.1).
 *
 * Each widget is a controlled React component: it takes the
 * current value plus an `onChange(next)` callback and renders a
 * little editor for one parameter. The FeatureDialog stitches
 * them together into a full panel based on the parsed schema.
 *
 * K6a.1 widened the kit with the four Onshape-style widgets the
 * extrude dialog needs on top of the original eight:
 *
 * * `EnumWidget` — dispatches between a segmented tab row
 *   (`ui_hint: "horizontal_enum"`) and a plain `<select>`.
 *   Replaces the K6a `BoolOpWidget`, which was a hardcoded
 *   special case of the same pattern.
 * * `FlipButton` — a bool rendered as an inline ±-flip button,
 *   used for Onshape's "reverse direction" affordance next to
 *   length fields. Triggered by `ui_hint: "opposite_direction"`
 *   on a `bool` parameter.
 * * `SliderWidget` — a number rendered as a range slider,
 *   useful for transparency/opacity-style params.
 * * `GroupHeader` — the clickable chevron + checkbox title row
 *   that owns a collapsible group. The body of the group is
 *   rendered by `FeatureDialog` itself so the recursion stays
 *   in one place.
 */

import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  register as registerQueryPicker,
  isActive as isQueryPickerActive,
  getActive as getActiveQueryPicker,
  subscribe as subscribeQueryPicker,
  deactivate as deactivateQueryPicker,
} from "../queryPickerBridge";
import { useWorkspaceUnits, decimalPlaces } from "../SettingsMenu";
import { LENGTH_TO_MM, isLengthUnit } from "../lengthUnits";
import { useKernelStore } from "../store";
import { formatQueryDisplay } from "../queryLabels";
import { queryAcceptedByFilter } from "../elementQuery";
import { logUserAction } from "../userActionLog";
import {
  ANGLE_UNITS,
  type AngleUnit,
  type EnumVariant,
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

// ─── buffered number input ─────────────────────────────────────
// type="number" swallows the minus sign on intermediate keystrokes
// ("-" → empty string → controlled value resets to 0). This
// component uses type="text" with local state so "-10" works.

function BufferedNumberInput({
  value,
  onCommit,
  ...rest
}: {
  value: number;
  onCommit: (n: number) => void;
} & Omit<
  React.InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange" | "type"
>) {
  const display = Number.isFinite(value) ? value : 0;
  const [raw, setRaw] = useState(String(display));
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) setRaw(String(display));
  }, [display, editing]);

  return (
    <input
      {...rest}
      type="text"
      inputMode="decimal"
      value={raw}
      onChange={(e) => {
        const s = e.currentTarget.value;
        setRaw(s);
        const n = Number(s);
        if (s.length > 0 && Number.isFinite(n)) {
          onCommit(n);
        }
      }}
      onFocus={(e) => {
        setEditing(true);
        rest.onFocus?.(e);
      }}
      onBlur={(e) => {
        setEditing(false);
        const n = Number(raw);
        if (Number.isFinite(n)) {
          onCommit(n);
          setRaw(String(n));
        } else {
          setRaw(String(display));
        }
        rest.onBlur?.(e);
      }}
    />
  );
}

// ─── unit conversion helpers ──────────────────────────────────

const ANGLE_TO_DEG: Record<string, number> = {
  deg: 1,
  "°": 1,
  rad: 180 / Math.PI,
};

function convertUnit(value: number, from: string, to: string): number | null {
  const f = from.toLowerCase();
  const t = to.toLowerCase();
  if (f === t) return value;
  const fl = LENGTH_TO_MM[f];
  const tl = LENGTH_TO_MM[t];
  if (fl !== undefined && tl !== undefined) return (value * fl) / tl;
  const fa = ANGLE_TO_DEG[f];
  const ta = ANGLE_TO_DEG[t];
  if (fa !== undefined && ta !== undefined) return (value * fa) / ta;
  return null;
}

function parseValueWithUnit(
  text: string,
  defaultUnit: string,
): { value: number; unit: string } | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const normalized = trimmed.replace(/,/g, ".");
  const match = normalized.match(/^([+-]?\d*\.?\d+)\s*(.*)$/);
  if (!match) return null;
  const value = parseFloat(match[1]);
  if (!Number.isFinite(value)) return null;
  const unitText = match[2].trim();
  return { value, unit: unitText || defaultUnit };
}

function unitDisplayLabel(unit: string): string {
  if (unit === "deg") return "°";
  return unit;
}

function isAngleUnit(unit: string): boolean {
  return unit.toLowerCase() in ANGLE_TO_DEG || unit === "°";
}

// ─── unit-aware input ─────────────────────────────────────────

function UnitInput({
  value,
  onCommit,
  unit,
  ...rest
}: {
  value: number;
  onCommit: (n: number) => void;
  unit?: string;
} & Omit<
  React.InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange" | "type"
>) {
  const ws = useWorkspaceUnits();

  if (!unit) {
    return <BufferedNumberInput value={value} onCommit={onCommit} {...rest} />;
  }

  const displayUnit = isLengthUnit(unit)
    ? ws.lengthUnit
    : isAngleUnit(unit)
      ? ws.angleUnit
      : unit;
  const decimals = isLengthUnit(unit)
    ? decimalPlaces(ws.lengthDecimals)
    : isAngleUnit(unit)
      ? decimalPlaces(ws.angleDecimals)
      : 3;
  const label = unitDisplayLabel(displayUnit);

  const internalVal = Number.isFinite(value) ? value : 0;
  const displayVal = convertUnit(internalVal, unit, displayUnit) ?? internalVal;
  const formatted = `${displayVal.toFixed(decimals)} ${label}`;

  const [raw, setRaw] = useState(formatted);
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) setRaw(formatted);
  }, [formatted, editing]);

  const tryCommit = (text: string) => {
    const parsed = parseValueWithUnit(text, displayUnit);
    if (parsed) {
      const converted = convertUnit(parsed.value, parsed.unit, unit);
      if (converted !== null && Number.isFinite(converted)) {
        onCommit(converted);
      }
    }
  };

  return (
    <input
      {...rest}
      type="text"
      value={raw}
      onChange={(e) => {
        const text = e.currentTarget.value;
        setRaw(text);
        tryCommit(text);
      }}
      onFocus={(e) => {
        setEditing(true);
        rest.onFocus?.(e);
      }}
      onBlur={(e) => {
        setEditing(false);
        const parsed = parseValueWithUnit(raw, displayUnit);
        if (parsed) {
          const converted = convertUnit(parsed.value, parsed.unit, unit);
          if (converted !== null && Number.isFinite(converted)) {
            onCommit(converted);
            const newDisplay =
              convertUnit(converted, unit, displayUnit) ?? converted;
            setRaw(`${newDisplay.toFixed(decimals)} ${label}`);
          } else {
            setRaw(formatted);
          }
        } else {
          setRaw(formatted);
        }
        rest.onBlur?.(e);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          (e.currentTarget as HTMLInputElement).blur();
        }
        rest.onKeyDown?.(e);
      }}
    />
  );
}

// ─── number ─────────────────────────────────────────────────────

/** A scalar value that may be a plain number or a `"=…"` formula string
 *  (resolved kernel-side against the document variables). Ticket x2m7. */
type Scalar = number | string;

/**
 * A single numeric cell that can flip to a formula (`=Seitenlänge`) via a
 * ƒx toggle — the per-component building block behind NumberWidget and the
 * vector widgets (ticket x2m7). In numeric mode it's a unit-aware input; in
 * formula mode a free-text field bound to the wire-form string. The kernel
 * resolves the formula against the variables table at regen time, so a bound
 * cell persists as a string element in the param's JSON.
 */
function ScalarFormulaInput({
  value,
  onCommit,
  unit,
  id,
  className,
  ariaLabel,
}: {
  value: Scalar;
  onCommit: (next: Scalar) => void;
  unit?: string;
  id?: string;
  className?: string;
  ariaLabel?: string;
}) {
  const formula = isFormula(value);
  return (
    <div className="formula-cell">
      {formula ? (
        <input
          id={id}
          className={(className ?? "input") + " is-formula"}
          type="text"
          value={value as string}
          aria-label={ariaLabel ? `${ariaLabel} formula` : undefined}
          onChange={(e) => onCommit(e.currentTarget.value)}
        />
      ) : (
        <UnitInput
          id={id}
          className={className ?? "input"}
          aria-label={ariaLabel}
          value={Number.isFinite(value as number) ? (value as number) : 0}
          onCommit={onCommit}
          unit={unit}
        />
      )}
      <button
        type="button"
        className={"formula-toggle" + (formula ? " is-active" : "")}
        title={formula ? "Switch to numeric" : "Switch to formula"}
        tabIndex={-1}
        onClick={() => onCommit(formula ? 0 : "=")}
      >
        ƒx
      </button>
    </div>
  );
}

export function NumberWidget({
  label,
  value,
  onChange,
  error,
  required,
  unit,
}: WidgetProps<Scalar> & { unit?: string }) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <ScalarFormulaInput
        id={id}
        value={value}
        onCommit={onChange}
        unit={unit}
      />
    </Field>
  );
}

// ─── vec2 ───────────────────────────────────────────────────────

export function Vec2Widget({
  label,
  value,
  onChange,
  error,
  required,
  unit,
}: WidgetProps<[Scalar, Scalar]> & { unit?: string }) {
  const id = useId();
  const updateIndex = (index: 0 | 1, next: Scalar) => {
    const copy: [Scalar, Scalar] = [value[0], value[1]];
    copy[index] = next;
    onChange(copy);
  };
  return (
    <Field label={label} htmlFor={`${id}-x`} error={error} required={required}>
      <div className="vec-row">
        {(["x", "y"] as const).map((axis, i) => (
          <div key={axis} className="vec-cell">
            <span className="axis-tag">{axis.toUpperCase()}</span>
            <ScalarFormulaInput
              id={`${id}-${axis}`}
              ariaLabel={`${label} ${axis}`}
              value={value[i]}
              onCommit={(n) => updateIndex(i as 0 | 1, n)}
              unit={unit}
            />
          </div>
        ))}
      </div>
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
  unit,
}: WidgetProps<[Scalar, Scalar, Scalar]> & { unit?: string }) {
  const id = useId();
  const updateIndex = (index: 0 | 1 | 2, next: Scalar) => {
    const copy: [Scalar, Scalar, Scalar] = [value[0], value[1], value[2]];
    copy[index] = next;
    onChange(copy);
  };
  return (
    <Field label={label} htmlFor={`${id}-x`} error={error} required={required}>
      <div className="vec-row">
        {(["x", "y", "z"] as const).map((axis, i) => (
          <div key={axis} className="vec-cell">
            <span className="axis-tag">{axis.toUpperCase()}</span>
            <ScalarFormulaInput
              id={`${id}-${axis}`}
              ariaLabel={`${label} ${axis}`}
              value={value[i]}
              onCommit={(n) => updateIndex(i as 0 | 1 | 2, n)}
              unit={unit}
            />
          </div>
        ))}
      </div>
    </Field>
  );
}

// ─── color (vec3 with color picker) ─────────────────────────────

function floatToHex(rgb: [number, number, number]): string {
  const r = Math.round(Math.max(0, Math.min(1, rgb[0])) * 255);
  const g = Math.round(Math.max(0, Math.min(1, rgb[1])) * 255);
  const b = Math.round(Math.max(0, Math.min(1, rgb[2])) * 255);
  return `#${r.toString(16).padStart(2, "0")}${g.toString(16).padStart(2, "0")}${b.toString(16).padStart(2, "0")}`;
}

function hexToFloat(hex: string): [number, number, number] {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  return [r, g, b];
}

export function ColorWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<[number, number, number]>) {
  const id = useId();
  const hex = floatToHex(value);
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <div className="color-row">
        <input
          id={id}
          type="color"
          className="color-swatch"
          value={hex}
          onChange={(e) => onChange(hexToFloat(e.currentTarget.value))}
        />
        <input
          type="text"
          className="color-hex"
          value={draft ?? hex}
          onChange={(e) => {
            const v = e.currentTarget.value;
            setDraft(v);
            if (/^#[0-9a-fA-F]{6}$/.test(v)) onChange(hexToFloat(v));
          }}
          onBlur={() => setDraft(null)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              setDraft(null);
              e.currentTarget.blur();
            }
          }}
        />
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
    <div className="field field-inline">
      <input
        id={id}
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.currentTarget.checked)}
      />
      <label className="label" htmlFor={id}>
        {label}
        {required ? <span className="required"> *</span> : null}
      </label>
      {error ? <div className="error">{error}</div> : null}
    </div>
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
  value: number | string;
  unit: LengthUnit;
}

function isFormula(v: number | string): v is string {
  return typeof v === "string" && v.startsWith("=");
}

function LengthInlineInput({
  storedValue,
  storedUnit,
  onCommit,
  ...rest
}: {
  storedValue: number;
  storedUnit: string;
  onCommit: (value: number, unit: string) => void;
} & Omit<
  React.InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange" | "type"
>) {
  const ws = useWorkspaceUnits();
  const displayUnit = isLengthUnit(storedUnit) ? storedUnit : ws.lengthUnit;
  const decimals = decimalPlaces(ws.lengthDecimals);
  const unitLabel = unitDisplayLabel(displayUnit);

  const formatted = `${storedValue.toFixed(decimals)} ${unitLabel}`;
  const [raw, setRaw] = useState(formatted);
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) setRaw(formatted);
  }, [formatted, editing]);

  const tryCommit = (text: string) => {
    const parsed = parseValueWithUnit(text, displayUnit);
    if (parsed && Number.isFinite(parsed.value) && isLengthUnit(parsed.unit)) {
      onCommit(parsed.value, parsed.unit);
    }
  };

  return (
    <input
      {...rest}
      type="text"
      value={raw}
      onChange={(e) => {
        setRaw(e.currentTarget.value);
        tryCommit(e.currentTarget.value);
      }}
      onFocus={(e) => {
        setEditing(true);
        rest.onFocus?.(e);
      }}
      onBlur={(e) => {
        setEditing(false);
        const parsed = parseValueWithUnit(raw, displayUnit);
        if (
          parsed &&
          Number.isFinite(parsed.value) &&
          isLengthUnit(parsed.unit)
        ) {
          onCommit(parsed.value, parsed.unit);
          setRaw(
            `${parsed.value.toFixed(decimals)} ${unitDisplayLabel(parsed.unit)}`,
          );
        } else {
          setRaw(formatted);
        }
        rest.onBlur?.(e);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          (e.currentTarget as HTMLInputElement).blur();
        }
      }}
    />
  );
}

export function LengthWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<LengthValue>) {
  const id = useId();
  const formula = isFormula(value.value);
  return (
    <Field
      label={label}
      htmlFor={`${id}-value`}
      error={error}
      required={required}
    >
      <div className="unit-row">
        {formula ? (
          <input
            id={`${id}-value`}
            className="input is-formula"
            type="text"
            value={value.value as string}
            aria-label={`${label} formula`}
            onChange={(e) =>
              onChange({ value: e.currentTarget.value, unit: value.unit })
            }
          />
        ) : (
          <LengthInlineInput
            id={`${id}-value`}
            className="input"
            storedValue={
              Number.isFinite(value.value as number)
                ? (value.value as number)
                : 0
            }
            storedUnit={value.unit}
            onCommit={(v, u) => onChange({ value: v, unit: u as LengthUnit })}
          />
        )}
        <button
          type="button"
          className={"formula-toggle" + (formula ? " is-active" : "")}
          title={formula ? "Switch to numeric" : "Switch to formula"}
          onClick={() => {
            if (formula) {
              onChange({ value: 0, unit: value.unit });
            } else {
              onChange({ value: "=", unit: value.unit });
            }
          }}
        >
          ƒx
        </button>
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
  const unitLabel = value.unit === "deg" ? "°" : "rad";
  return (
    <Field
      label={label}
      htmlFor={`${id}-value`}
      error={error}
      required={required}
    >
      <div className="unit-row">
        <UnitInput
          id={`${id}-value`}
          className="input"
          value={Number.isFinite(value.value) ? value.value : 0}
          onCommit={(n) => onChange({ value: n, unit: value.unit })}
          unit={unitLabel}
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

// ─── enum (tabs or dropdown) ────────────────────────────────────

/**
 * Dispatches between a segmented tab row and a `<select>` based
 * on the parameter's `ui_hint`. Variant list is passed in
 * verbatim from the schema — this widget never guesses defaults
 * or mutates the list.
 */
export function EnumWidget({
  label,
  value,
  onChange,
  error,
  required,
  variants,
  variant = "dropdown",
}: WidgetProps<string> & {
  variants: EnumVariant[];
  variant?: "dropdown" | "tabs";
}) {
  const id = useId();
  if (variant === "tabs") {
    return (
      <Field label={label} htmlFor={id} error={error} required={required}>
        <div className="tab-row" role="tablist" aria-label={label}>
          {variants.map((v) => {
            const active = v.key === value;
            return (
              <button
                key={v.key}
                id={v.key === value ? id : undefined}
                type="button"
                role="tab"
                aria-selected={active}
                className={"tab" + (active ? " is-active" : "")}
                onClick={() => onChange(v.key)}
              >
                {v.label}
              </button>
            );
          })}
        </div>
      </Field>
    );
  }
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <select
        id={id}
        className="select"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
      >
        {variants.map((v) => (
          <option key={v.key} value={v.key}>
            {v.label}
          </option>
        ))}
      </select>
    </Field>
  );
}

// ─── flip button (bool with opposite_direction hint) ────────────

/**
 * Inline ±-flip toggle, the Onshape "reverse direction" pattern.
 * Renders compact enough to sit next to a length input in the
 * same row — the dialog is responsible for the layout.
 */
export function FlipButton({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<boolean>) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <button
        id={id}
        type="button"
        className={"flip-button" + (value ? " is-flipped" : "")}
        aria-pressed={value}
        title={value ? "Flip direction (currently reversed)" : "Flip direction"}
        onClick={() => onChange(!value)}
      >
        {value ? "−" : "+"}
      </button>
    </Field>
  );
}

// ─── slider (number with slider hint) ──────────────────────────

export function SliderWidget({
  label,
  value,
  onChange,
  error,
  required,
  min,
  max,
  step,
}: WidgetProps<number> & {
  min: number;
  max: number;
  step: number;
}) {
  const id = useId();
  const safe = Number.isFinite(value) ? value : min;
  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <div className="slider-row">
        <input
          id={id}
          className="slider"
          type="range"
          min={min}
          max={max}
          step={step}
          value={safe}
          onChange={(e) => {
            const n = Number(e.currentTarget.value);
            if (Number.isFinite(n)) onChange(n);
          }}
        />
        <output className="slider-value" htmlFor={id}>
          {safe}
        </output>
      </div>
    </Field>
  );
}

// ─── group header (collapsible, bound to a driving bool) ───────

/**
 * Header row for a collapsible group: a chevron, a checkbox that
 * toggles the driving bool, and the group's label. The dialog
 * stitches this together with the group's body so the recursion
 * stays in one place.
 */
export function GroupHeader({
  label,
  expanded,
  onToggle,
  error,
}: {
  label: string;
  expanded: boolean;
  onToggle: (next: boolean) => void;
  error?: string | null;
}) {
  const id = useId();
  return (
    <div className={"group-header" + (expanded ? " is-open" : "")}>
      <span className="chevron" aria-hidden="true">
        {expanded ? "▾" : "▸"}
      </span>
      <input
        id={id}
        type="checkbox"
        checked={expanded}
        onChange={(e) => onToggle(e.currentTarget.checked)}
      />
      <label className="label" htmlFor={id}>
        {label}
      </label>
      {error ? <div className="error">{error}</div> : null}
    </div>
  );
}

// ─── query ──────────────────────────────────────────────────────

/**
 * Query picker widget (K7.22 UX-fix, onshape-ux `Selection-Input`).
 *
 * The widget starts in click-focus mode automatically when it
 * receives the empty string — matching the Onshape convention
 * where the Profiles field of a freshly-opened Extrude dialog
 * already has the viewport click focus. A small ⊕ button toggles
 * the focus manually; the raw expression is also editable via the
 * text input for power users who want to type `qCreatedBy(…)`
 * directly.
 *
 * When focused, the module-level `queryPickerBridge` carries a
 * registration over to the Viewport click handler. The next
 * click in the 3D scene that hits a proxy entity gets converted
 * into a `qContributor(…)` expression and written back into the
 * dialog's definition via `onChange`. The widget then drops its
 * focus automatically.
 */

export function QueryWidget({
  label,
  value,
  onChange,
  error,
  required,
  entityFilter,
  // Ticket e9il: a single-value query field is COMPLETE after one pick, so the
  // picker releases the viewport by default (one-shot, like the sketch-plane
  // field — ticket dxrq). Leaving it armed forever meant the next click in the
  // scene was swallowed (fs23 guard) and the hover highlight kept following the
  // cursor — the "camera/everything feels stuck" symptom after picking a
  // Revolve axis. Pass `deactivateOnPick={false}` to opt back into re-pick mode.
  deactivateOnPick = true,
}: WidgetProps<string> & {
  entityFilter?: string;
  deactivateOnPick?: boolean;
}) {
  const id = useId();
  // NB: the single-query picker is intentionally NOT scoped to
  // `selectableFeatureIds` (unlike MultiQueryWidget). It drives sketch-plane
  // / single-face selection, which references datums or earlier faces anyway,
  // so the snapshot risk is low — and subscribing here (useKernelStore) would
  // pull the async store into every dialog widget. The body-selection path
  // (Boolean tools/targets, merge scope) IS scoped via MultiQueryWidget.
  const [text, setText] = useState<string>(value);
  const [focused, setFocused] = useState(false);

  // Keep the text input in sync when the definition changes
  // externally (e.g. a viewport pick wrote a new query string
  // through `onChange`).
  useEffect(() => {
    setText(value);
  }, [value]);

  // Subscribe to the bridge so the widget re-renders when
  // another picker claims focus (deactivating this one).
  const active = useSyncExternalStore(
    subscribeQueryPicker,
    () => isQueryPickerActive(id),
    () => false,
  );

  // Single source of truth for what a viewport pick does for this widget.
  // One-shot pickers (the sketch-plane field) release the viewport after
  // a single pick so the next click reaches the sketch tools instead of
  // re-picking the plane (ticket dxrq).
  const onPick = useCallback(
    (query: string) => {
      // Reject picks whose kind the field's filter doesn't permit (e.g. a
      // datum plane or body for an "EDGE,SKETCHLINE" axis field) — the single
      // chokepoint that enforces the filter regardless of which path emitted
      // the pick. Returning false leaves the picker armed for another try.
      if (!queryAcceptedByFilter(query, entityFilter)) return false;
      logUserAction("pick", "query", { field: label, query });
      setText(query);
      onChange(query);
      if (deactivateOnPick) deactivateQueryPicker();
      return true;
    },
    [onChange, deactivateOnPick, entityFilter, label],
  );

  // The bridge holds at most one active picker. Registering flips this
  // widget's `active` to true, which runs the effect below — that effect is
  // the single owner of the registration's lifecycle: it re-registers and
  // returns the unregister closure as its cleanup, so the picker is always
  // torn down on unmount or when another widget claims focus.
  useEffect(() => {
    if (!active) return;
    return registerQueryPicker({ ownerId: id, onPick, entityFilter });
  }, [active, id, onPick, entityFilter]);

  // Auto-activate on an empty required field so a freshly-opened
  // dialog already has the viewport click focus on its primary
  // selection input — matches the Onshape `status: "Active/Highlight"`
  // convention.
  const [autoArmed, setAutoArmed] = useState<boolean>(
    Boolean(required) && value === "",
  );
  useEffect(() => {
    if (autoArmed && value === "" && !active && !getActiveQueryPicker()) {
      setAutoArmed(false);
      registerQueryPicker({ ownerId: id, onPick, entityFilter });
    }
  }, [autoArmed, value, active, id, onPick, entityFilter]);

  // True toggle: arming twice releases the viewport again. Without
  // this, a picker whose target can't be picked (or one armed by
  // accident) kept click-focus forever and every viewport click
  // silently vanished — the button's own title already promised
  // "click here to cancel".
  const toggleActive = () => {
    if (active) {
      deactivateQueryPicker();
      return;
    }
    registerQueryPicker({ ownerId: id, onPick, entityFilter });
  };

  const commitText = () => {
    if (text !== value) onChange(text);
  };

  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <div className={"query-picker" + (active ? " is-active" : "")}>
        <button
          type="button"
          className="query-focus-toggle"
          aria-pressed={active}
          title={
            active
              ? "Click in the viewport to select, or click here to cancel"
              : "Click here, then click in the viewport to select"
          }
          onClick={toggleActive}
        >
          {active ? "◎" : "⊕"}
        </button>
        <input
          id={id}
          className="input query-input"
          type="text"
          spellCheck={false}
          placeholder={
            active ? "Click in the viewport…" : "qCreatedBy(…) — or click ⊕"
          }
          value={focused ? text : formatQueryDisplay(text)}
          onChange={(e) => setText(e.currentTarget.value)}
          onFocus={() => {
            setFocused(true);
            // Ticket bg64: focusing the field arms the viewport picker
            // (Onshape Selection-Input convention — a focused selection
            // input already takes the next scene click). Without this only
            // the ⊕ button armed it, so a user who clicked into the field
            // and then clicked a datum plane in the scene saw nothing happen.
            if (!active && !getActiveQueryPicker()) {
              registerQueryPicker({ ownerId: id, onPick, entityFilter });
            }
          }}
          onClick={() => {
            // Ticket sqif: a one-shot picker (deactivateOnPick — the sketch
            // plane field) releases the viewport after its pick, but the input
            // keeps DOM focus, so a SECOND plane pick never re-fires `onFocus`
            // and the field stays disarmed. Re-arm on a click into the already
            // focused input whenever no picker is active.
            if (!active && !getActiveQueryPicker()) {
              registerQueryPicker({ ownerId: id, onPick, entityFilter });
            }
          }}
          onBlur={() => {
            setFocused(false);
            commitText();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              (e.currentTarget as HTMLInputElement).blur();
            }
          }}
        />
      </div>
    </Field>
  );
}

// ─── multi-query (set of query expressions) ───────────────────

function parseMultiQuery(raw: string): string[] {
  if (!raw) return [];
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function serializeMultiQuery(items: string[]): string {
  return items.join(",");
}

export function MultiQueryWidget({
  label,
  value,
  onChange,
  error,
  required,
  entityFilter,
}: WidgetProps<string> & { entityFilter?: string }) {
  const id = useId();
  const { state } = useKernelStore();
  const featureNames = useMemo(() => {
    if (!state) return undefined;
    const m = new Map<number, string>();
    for (const f of state.features) m.set(f.id, f.name);
    return m;
  }, [state]);
  // Snapshot semantics: when editing one feature, only features BEFORE it in
  // the tree may be referenced. Hand the picker the set of allowed owner ids
  // so later geometry stays unpickable (e.g. a Boolean can't take a body that
  // was created after it).
  const selectableFeatureIds = useMemo<ReadonlySet<number> | undefined>(() => {
    if (!state || state.selection.size !== 1) return undefined;
    const editedId = [...state.selection][0];
    const idx = state.features.findIndex((f) => f.id === editedId);
    if (idx < 0) return undefined;
    return new Set(state.features.slice(0, idx).map((f) => f.id));
  }, [state]);
  const itemsRef = useRef<string[]>(parseMultiQuery(value));

  useEffect(() => {
    itemsRef.current = parseMultiQuery(value);
  }, [value]);

  const active = useSyncExternalStore(
    subscribeQueryPicker,
    () => isQueryPickerActive(id),
    () => false,
  );

  const onPickRef = useRef<(query: string) => boolean>(() => false);
  onPickRef.current = (query: string) => {
    // Same filter chokepoint as QueryWidget — a BODY merge-scope picker
    // rejects an edge/face pick, etc.
    if (!queryAcceptedByFilter(query, entityFilter)) return false;
    const items = itemsRef.current;
    if (items.includes(query)) return true;
    const next = [...items, query];
    itemsRef.current = next;
    onChange(serializeMultiQuery(next));
    return true;
  };

  useEffect(() => {
    if (!active) return;
    return registerQueryPicker({
      ownerId: id,
      onPick: (q: string) => onPickRef.current(q),
      entityFilter,
      selectableFeatureIds,
    });
  }, [active, id, entityFilter, selectableFeatureIds]);

  const [autoArmed, setAutoArmed] = useState<boolean>(
    Boolean(required) && value === "",
  );
  useEffect(() => {
    if (autoArmed && value === "" && !active && !getActiveQueryPicker()) {
      setAutoArmed(false);
      registerQueryPicker({
        ownerId: id,
        onPick: (q: string) => onPickRef.current(q),
        entityFilter,
        selectableFeatureIds,
      });
    }
  }, [autoArmed, value, active, id, entityFilter, selectableFeatureIds]);

  // Same toggle semantics as QueryWidget: a second click stops picking.
  const activate = () => {
    if (active) {
      deactivateQueryPicker();
      return;
    }
    registerQueryPicker({
      ownerId: id,
      onPick: (q: string) => onPickRef.current(q),
      entityFilter,
      selectableFeatureIds,
    });
  };

  const removeItem = (query: string) => {
    const next = itemsRef.current.filter((q) => q !== query);
    itemsRef.current = next;
    onChange(serializeMultiQuery(next));
  };

  const items = parseMultiQuery(value);

  return (
    <Field label={label} htmlFor={id} error={error} required={required}>
      <div
        className={"query-picker multi" + (active ? " is-active" : "")}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest(".chip-remove")) return;
          if ((e.target as HTMLElement).closest(".query-focus-toggle")) return;
          if (!active) activate();
        }}
        style={{ cursor: "pointer" }}
      >
        <button
          type="button"
          className="query-focus-toggle"
          aria-pressed={active}
          title={
            active
              ? "Picking — click objects in viewport, or click here to stop"
              : "Click to start picking objects in viewport"
          }
          onClick={activate}
        >
          {active ? "◎" : "⊕"}
        </button>
        <div className="query-chips">
          {items.length === 0 && (
            <span className="query-chips-empty">
              {active ? "Click objects in viewport…" : "None selected"}
            </span>
          )}
          {items.map((q) => (
            <span key={q} className="query-chip">
              {formatQueryDisplay(q, featureNames)}
              <button
                type="button"
                className="chip-remove"
                title={`Remove ${formatQueryDisplay(q, featureNames)}`}
                onClick={() => removeItem(q)}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      </div>
    </Field>
  );
}

// ─── unknown / generic fallback ────────────────────────────────

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
