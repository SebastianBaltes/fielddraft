/**
 * Generic FeatureDialog (K6a + K6a.1).
 *
 * One React component that renders the editor panel for any
 * feature type — no hand-written dialog code per feature. The
 * component takes a parsed [`FeatureSchema`], the current
 * definition blob, and an `onChange` callback; it dispatches
 * each parameter to the matching widget from `./widgets.tsx`,
 * runs the shared validator for per-field error messages, and
 * emits updated definition blobs through `onChange` as the user
 * types.
 *
 * K6a.1 additions:
 *
 * * Header chrome with optional confirm/cancel callbacks —
 *   matches the Onshape green-check / red-X pattern.
 * * Recursive rendering of collapsible groups driven by a
 *   sibling boolean parameter.
 * * Conditional visibility via `visible_when`, so an enum can
 *   drive which sub-fields appear under it (e.g. the extrude
 *   end-condition drop-down swapping depth / face / entity
 *   fields in and out).
 * * Footer slot for view-state controls like the transparency
 *   slider at the bottom of the Onshape extrude dialog — those
 *   live outside the definition blob, so the parent owns them.
 *
 * The dialog itself owns no state — it's a pure function of
 * `(schema, definition, onChange)` — so a parent component
 * (typically the Outliner or a feature-tree panel) can drive
 * it from its own store and persist the definition back to the
 * kernel on commit.
 */

import {
  isGroupExpanded,
  isVisible,
  readParam,
  updateParam,
  validateParamValue,
  type FeatureParamDescriptor,
  type FeatureSchema,
} from "./schema";
import {
  AngleWidget,
  BoolWidget,
  EnumWidget,
  FlipButton,
  GroupHeader,
  LengthWidget,
  NumberWidget,
  MultiQueryWidget,
  QueryWidget,
  SliderWidget,
  StringWidget,
  ColorWidget,
  UnknownWidget,
  Vec2Widget,
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
  /** Optional confirm handler. When set, the header grows a ✓
   *  button that invokes it. */
  onCommit?: () => void;
  /** Optional cancel handler. When set, the header grows a ✗
   *  button that invokes it. */
  onCancel?: () => void;
  /** Optional footer slot for view-state controls that don't
   *  belong in the definition (e.g. transparency slider). */
  footer?: React.ReactNode;
}

export const FEATURE_TYPE_LABELS: Record<string, string> = {
  box: "Box",
  sphere: "Sphere",
  cylinder: "Cylinder",
  hex_prism: "Hex Prism",
  ellipsoid: "Ellipsoid",
  capsule: "Capsule",
  cone: "Cone",
  torus: "Torus",
  sketch: "Sketch",
  extrude: "Extrude",
  revolve: "Revolve",
  hole: "Hole",
  thread: "Thread",
  gyroid: "Gyroid",
  linear_pattern: "Linear Pattern",
  circular_pattern: "Circular Pattern",
  mirror: "Mirror",
  mate_connector: "Mate Connector",
  fillet_edge: "Fillet Edge",
  sweep: "Sweep",
  boolean: "Boolean",
  material: "Material",
  transform: "Transform",
  imported_mesh: "Imported Mesh",
  sketch_line: "Line",
  sketch_rectangle: "Rectangle",
  sketch_circle: "Circle",
  sketch_polygon: "Polygon",
  sketch_arc: "Arc",
  sketch_point: "Point",
  sketch_mirror: "Mirror",
  sketch_trim: "Trim",
  sketch_offset: "Offset",
};

export function FeatureDialog({
  schema,
  definition,
  onChange,
  onCommit,
  onCancel,
  footer,
}: FeatureDialogProps) {
  const handleParamChange = (key: string, next: unknown) => {
    onChange(updateParam(definition, key, next));
  };
  return (
    <div className="fd-feature-dialog" data-feature-type={schema.type}>
      <div className="header">
        <div className="title">{FEATURE_TYPE_LABELS[schema.type] ?? schema.type}</div>
        {onCommit || onCancel ? (
          <div className="header-actions">
            {onCommit ? (
              <button
                type="button"
                className="header-action commit"
                aria-label="Confirm"
                onClick={onCommit}
              >
                ✓
              </button>
            ) : null}
            {onCancel ? (
              <button
                type="button"
                className="header-action cancel"
                aria-label="Cancel"
                onClick={onCancel}
              >
                ✗
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
      <div className="body">
        <ParamList
          params={schema.params}
          definition={definition}
          onParamChange={handleParamChange}
        />
      </div>
      {footer ? <div className="footer">{footer}</div> : null}
    </div>
  );
}

/**
 * Render a list of params, skipping hidden ones and recursing
 * into groups. Kept as a nested component so groups can reuse
 * the exact same dispatch path as the top-level body.
 */
function ParamList({
  params,
  definition,
  onParamChange,
}: {
  params: FeatureParamDescriptor[];
  definition: Record<string, unknown>;
  onParamChange: (key: string, next: unknown) => void;
}) {
  return (
    <>
      {params.map((p) => {
        if (!isVisible(p, definition)) return null;
        if (p.widget === "group" || p.widget === "expandable") {
          return (
            <Group
              key={p.key}
              group={p}
              definition={definition}
              onParamChange={onParamChange}
            />
          );
        }
        if (p.widget === "tabs" && p.tabs) {
          return (
            <TabsWidget
              key={p.key}
              param={p}
              definition={definition}
              onParamChange={onParamChange}
            />
          );
        }
        return (
          <WidgetFor
            key={p.key}
            param={p}
            definition={definition}
            onParamChange={onParamChange}
          />
        );
      })}
    </>
  );
}

/**
 * Collapsible group: renders the header row with the chevron +
 * driving checkbox, and (when expanded) the nested param list
 * underneath.
 */
function Group({
  group,
  definition,
  onParamChange,
}: {
  group: FeatureParamDescriptor;
  definition: Record<string, unknown>;
  onParamChange: (key: string, next: unknown) => void;
}) {
  const expanded = isGroupExpanded(group, definition);
  const handleToggle = (next: boolean) => {
    if (group.drivingParam) {
      onParamChange(group.drivingParam, next);
    }
  };
  return (
    <div
      className={"group" + (expanded ? " is-open" : "")}
      data-group-key={group.key}
    >
      <GroupHeader
        label={group.label}
        expanded={expanded}
        onToggle={handleToggle}
      />
      {expanded && group.children ? (
        <div className="group-body">
          <ParamList
            params={group.children}
            definition={definition}
            onParamChange={onParamChange}
          />
        </div>
      ) : null}
    </div>
  );
}

/**
 * Dispatch to the matching widget for one leaf parameter. Pure
 * presentational wiring — the actual state lives in the parent
 * `definition` prop.
 */
/**
 * K7.21g: Tabbed widget — renders a tab bar and shows the active
 * tab's children. The selected tab is stored in the definition
 * under the param's key.
 */
function TabsWidget({
  param,
  definition,
  onParamChange,
}: {
  param: FeatureParamDescriptor;
  definition: Record<string, unknown>;
  onParamChange: (key: string, next: unknown) => void;
}) {
  const tabs = param.tabs ?? [];
  const activeTab = (definition[param.key] as string) ?? tabs[0]?.label ?? "";
  const active = tabs.find((t) => t.label === activeTab) ?? tabs[0];

  return (
    <div className="tabs-widget">
      <div className="tab-bar">
        {tabs.map((tab) => (
          <button
            key={tab.label}
            type="button"
            className={"tab" + (tab.label === activeTab ? " is-active" : "")}
            onClick={() => onParamChange(param.key, tab.label)}
          >
            {tab.label}
          </button>
        ))}
      </div>
      {active && (
        <div className="tab-body">
          <ParamList
            params={active.children}
            definition={definition}
            onParamChange={onParamChange}
          />
        </div>
      )}
    </div>
  );
}

function WidgetFor({
  param,
  definition,
  onParamChange,
}: {
  param: FeatureParamDescriptor;
  definition: Record<string, unknown>;
  onParamChange: (key: string, next: unknown) => void;
}) {
  const value = readParam(definition, param);
  const error = validateParamValue(param, value);
  const emit = (next: unknown) => onParamChange(param.key, next);

  switch (param.widget) {
    case "number":
      return (
        <NumberWidget
          label={param.label}
          value={value as number}
          required={param.required}
          error={error}
          onChange={emit}
          unit={param.unit}
        />
      );
    case "vec2":
      return (
        <Vec2Widget
          label={param.label}
          value={value as [number, number]}
          required={param.required}
          error={error}
          onChange={emit}
          unit={param.unit}
        />
      );
    case "vec3":
      return (
        <Vec3Widget
          label={param.label}
          value={value as [number, number, number]}
          required={param.required}
          error={error}
          onChange={emit}
          unit={param.unit}
        />
      );
    case "color":
      return (
        <ColorWidget
          label={param.label}
          value={value as [number, number, number]}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "bool":
      return (
        <BoolWidget
          label={param.label}
          value={value as boolean}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "flip":
      return (
        <FlipButton
          label={param.label}
          value={value as boolean}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "string":
      return (
        <StringWidget
          label={param.label}
          value={value as string}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "length":
      return (
        <LengthWidget
          label={param.label}
          value={value as LengthValue}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "angle":
      return (
        <AngleWidget
          label={param.label}
          value={value as AngleValue}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "enum":
      return (
        <EnumWidget
          label={param.label}
          value={value as string}
          variants={param.variants ?? []}
          variant={param.uiHint === "horizontal_enum" ? "tabs" : "dropdown"}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "slider":
      return (
        <SliderWidget
          label={param.label}
          value={value as number}
          min={param.min ?? 0}
          max={param.max ?? 1}
          step={param.step ?? 0.01}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "query":
      return (
        <QueryWidget
          label={param.label}
          value={value as string}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "multi_query":
      return (
        <MultiQueryWidget
          label={param.label}
          value={value as string}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
    case "group":
      // Group nodes are handled by the caller — this arm is
      // unreachable but keeps the switch exhaustive for TS.
      return null;
    case "unknown":
      return (
        <UnknownWidget
          label={param.label}
          value={value}
          required={param.required}
          error={error}
          onChange={emit}
        />
      );
  }
}
