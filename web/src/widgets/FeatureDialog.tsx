/**
 * Generic FeatureDialog (K6a).
 *
 * One React component that renders the editor panel for any
 * feature type — no hand-written dialog code per feature. The
 * component takes a parsed [`FeatureSchema`], the current
 * definition blob, and an `onChange` callback; it dispatches
 * each parameter to the matching widget from
 * `./widgets.tsx`, runs the shared validator for per-field
 * error messages, and emits updated definition blobs through
 * `onChange` as the user types.
 *
 * The dialog itself owns no state — it's a pure function of
 * `(schema, def, onChange)` — so a parent component (typically
 * the Outliner or a feature-tree panel) can drive it from its
 * own store and persist the definition back to the kernel on
 * commit.
 */

import {
  readParam,
  updateParam,
  validateParamValue,
  type FeatureParamDescriptor,
  type FeatureSchema,
} from "./schema";
import {
  AngleWidget,
  BoolOpWidget,
  BoolWidget,
  LengthWidget,
  NumberWidget,
  QueryWidget,
  StringWidget,
  UnknownWidget,
  Vec3Widget,
  type AngleValue,
  type LengthValue,
} from "./widgets";

export interface FeatureDialogProps {
  /** The parsed schema for the feature type being edited. */
  schema: FeatureSchema;
  /** Current definition blob — the JSON the kernel consumes. */
  definition: Record<string, unknown>;
  /** Called with the updated definition on every edit. */
  onChange: (next: Record<string, unknown>) => void;
}

export function FeatureDialog({
  schema,
  definition,
  onChange,
}: FeatureDialogProps) {
  return (
    <div className="fd-feature-dialog" data-feature-type={schema.type}>
      <div className="header">
        <div className="title">{schema.type}</div>
      </div>
      <div className="body">
        {schema.params.map((p) => (
          <WidgetFor
            key={p.key}
            param={p}
            definition={definition}
            onParamChange={(next) =>
              onChange(updateParam(definition, p.key, next))
            }
          />
        ))}
      </div>
    </div>
  );
}

/**
 * Dispatch to the matching widget for one parameter. Pure
 * presentational wiring — the actual state lives in the parent
 * `definition` prop.
 */
function WidgetFor({
  param,
  definition,
  onParamChange,
}: {
  param: FeatureParamDescriptor;
  definition: Record<string, unknown>;
  onParamChange: (next: unknown) => void;
}) {
  const value = readParam(definition, param);
  const error = validateParamValue(param, value);

  switch (param.widget) {
    case "number":
      return (
        <NumberWidget
          label={param.label}
          value={value as number}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "vec3":
      return (
        <Vec3Widget
          label={param.label}
          value={value as [number, number, number]}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "bool":
      return (
        <BoolWidget
          label={param.label}
          value={value as boolean}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "string":
      return (
        <StringWidget
          label={param.label}
          value={value as string}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "length":
      return (
        <LengthWidget
          label={param.label}
          value={value as LengthValue}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "angle":
      return (
        <AngleWidget
          label={param.label}
          value={value as AngleValue}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "bool_op":
      return (
        <BoolOpWidget
          label={param.label}
          value={value as "union" | "difference" | "intersect"}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "query":
      return (
        <QueryWidget
          label={param.label}
          value={value as string}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
    case "unknown":
      return (
        <UnknownWidget
          label={`${param.label} (raw)`}
          value={value}
          required={param.required}
          error={error}
          onChange={onParamChange}
        />
      );
  }
}
