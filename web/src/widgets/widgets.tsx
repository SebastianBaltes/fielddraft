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

import { useEffect, useId, useState, useSyncExternalStore } from "react";
import {
  register as registerQueryPicker,
  isActive as isQueryPickerActive,
  subscribe as subscribeQueryPicker,
} from "../queryPickerBridge";
import {
  ANGLE_UNITS,
  LENGTH_UNITS,
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
      <BufferedNumberInput
        id={id}
        className="input"
        value={Number.isFinite(value) ? value : 0}
        onCommit={onChange}
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
}: WidgetProps<[number, number]>) {
  const id = useId();
  const updateIndex = (index: 0 | 1, next: number) => {
    const copy: [number, number] = [value[0], value[1]];
    copy[index] = next;
    onChange(copy);
  };
  return (
    <Field label={label} htmlFor={`${id}-x`} error={error} required={required}>
      <div className="vec-row">
        {(["x", "y"] as const).map((axis, i) => (
          <div key={axis} className="vec-cell">
            <span className="axis-tag">{axis.toUpperCase()}</span>
            <BufferedNumberInput
              id={`${id}-${axis}`}
              className="input"
              aria-label={`${label} ${axis}`}
              value={Number.isFinite(value[i]) ? value[i] : 0}
              onCommit={(n) => updateIndex(i as 0 | 1, n)}
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
}: WidgetProps<[number, number, number]>) {
  const id = useId();
  const updateIndex = (index: 0 | 1 | 2, next: number) => {
    const copy: [number, number, number] = [value[0], value[1], value[2]];
    copy[index] = next;
    onChange(copy);
  };
  return (
    <Field label={label} htmlFor={`${id}-x`} error={error} required={required}>
      <div className="vec-row">
        {(["x", "y", "z"] as const).map((axis, i) => (
          <div key={axis} className="vec-cell">
            <span className="axis-tag">{axis.toUpperCase()}</span>
            <BufferedNumberInput
              id={`${id}-${axis}`}
              className="input"
              aria-label={`${label} ${axis}`}
              value={Number.isFinite(value[i]) ? value[i] : 0}
              onCommit={(n) => updateIndex(i as 0 | 1 | 2, n)}
            />
          </div>
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
  value: number | string;
  unit: LengthUnit;
}

function isFormula(v: number | string): v is string {
  return typeof v === "string" && v.startsWith("=");
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
    <Field label={label} htmlFor={`${id}-value`} error={error} required={required}>
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
          <BufferedNumberInput
            id={`${id}-value`}
            className="input"
            value={Number.isFinite(value.value as number) ? (value.value as number) : 0}
            onCommit={(n) => onChange({ value: n, unit: value.unit })}
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
        <BufferedNumberInput
          id={`${id}-value`}
          className="input"
          value={Number.isFinite(value.value) ? value.value : 0}
          onCommit={(n) => onChange({ value: n, unit: value.unit })}
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
const QUERY_DISPLAY_LABELS: Record<string, string> = {
  "qCreatedBy(origin_plane_xy)": "XY Plane",
  "qCreatedBy(origin_plane_xz)": "XZ Plane",
  "qCreatedBy(origin_plane_yz)": "YZ Plane",
};

function formatQueryDisplay(raw: string): string {
  if (QUERY_DISPLAY_LABELS[raw]) return QUERY_DISPLAY_LABELS[raw];
  const match = raw.match(/^qCreatedBy\((.+)\)$/);
  if (match) return match[1].replace(/_/g, " ");
  return raw;
}

export function QueryWidget({
  label,
  value,
  onChange,
  error,
  required,
}: WidgetProps<string>) {
  const id = useId();
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

  // Register with the bridge while `active` is true. The register
  // function returns an unregister closure that we wire up as
  // the effect's cleanup.
  useEffect(() => {
    if (!active) return;
    return registerQueryPicker({
      ownerId: id,
      onPick: (query: string) => {
        setText(query);
        onChange(query);
        return true;
      },
    });
  }, [active, id, onChange]);

  // Auto-activate on an empty required field so a freshly-opened
  // dialog already has the viewport click focus on its primary
  // selection input — matches the Onshape `status: "Active/Highlight"`
  // convention.
  const [autoArmed, setAutoArmed] = useState<boolean>(
    Boolean(required) && value === "",
  );
  useEffect(() => {
    if (autoArmed && value === "" && !active) {
      // Flip `autoArmed` off before registering to avoid re-arming
      // after the first pick.
      setAutoArmed(false);
      registerQueryPicker({
        ownerId: id,
        onPick: (query: string) => {
          setText(query);
          onChange(query);
          return true;
        },
      });
    }
  }, [autoArmed, value, active, id, onChange]);

  const activate = () => {
    registerQueryPicker({
      ownerId: id,
      onPick: (query: string) => {
        setText(query);
        onChange(query);
        return true;
      },
    });
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
          onClick={activate}
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
          onFocus={() => setFocused(true)}
          onBlur={() => { setFocused(false); commitText(); }}
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
