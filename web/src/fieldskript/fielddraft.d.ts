/**
 * Ambient typings for the `fielddraft` authoring module that
 * FieldSkript scripts import (S1).
 *
 * A script writes `import { fd, length, query, ... } from "fielddraft"`.
 * In the Monaco editor (S4) this file is fed to the TypeScript worker
 * as an `extraLib`, giving authors IntelliSense, hover types and live
 * diagnostics against the real `fd.*` API. It declares the full
 * **authoring** (parameter) surface that ships in S1, and the
 * **build** (geometry-emission + query) surface as the forthcoming
 * S2 contract — declared now so author-facing types are stable and a
 * script body type-checks before the Rust callback lands.
 *
 * Marken-Regel (README §11): the API is `fd.*`, never Onshape terms.
 */

declare module "fielddraft" {
  // ─── value types ──────────────────────────────────────────────

  /** A length value (numeric magnitude + unit). */
  export interface Length {
    value: number | string;
    unit: string;
  }

  /** An angle value (numeric magnitude + unit). */
  export interface Angle {
    value: number;
    unit: string;
  }

  /** A lazy, regeneration-stable reference to geometry (a query AST). */
  export type Query = string;

  // ─── parameter builders (authoring side, S1) ──────────────────

  /** A declared parameter; `TValue` is its resolved `def` value type. */
  export type Param<TValue = unknown> = {
    readonly key: string;
    readonly __value?: TValue;
  };

  /** Derive the typed `def` object from a list of params. */
  export type DefOf<P extends readonly Param[]> = {
    [K in P[number] as K extends { key: infer Key }
      ? Key & string
      : never]: K extends Param<infer V> ? V : unknown;
  };

  interface BaseOpts {
    name?: string;
    required?: boolean;
  }
  interface NumberOpts extends BaseOpts {
    default?: number;
    min?: number;
    max?: number;
    step?: number;
    unit?: string;
  }

  export function length(key: string, opts?: NumberOpts): Param<Length>;
  export function angle(key: string, opts?: NumberOpts): Param<Angle>;
  export function real(
    key: string,
    opts?: NumberOpts & { uiHint?: "slider" },
  ): Param<number>;
  export function bool(
    key: string,
    opts?: BaseOpts & { default?: boolean; uiHint?: "opposite_direction" },
  ): Param<boolean>;
  export function str(
    key: string,
    opts?: BaseOpts & { default?: string },
  ): Param<string>;
  export function vec2(
    key: string,
    opts?: BaseOpts & { default?: [number, number]; unit?: string },
  ): Param<[number, number]>;
  export function vec3(
    key: string,
    opts?: BaseOpts & {
      default?: [number, number, number];
      unit?: string;
      uiHint?: "color";
    },
  ): Param<[number, number, number]>;

  export interface Variant {
    key: string;
    label: string;
    reveals?: string[];
  }
  export function enumParam(
    key: string,
    variants: Variant[] | string[] | Record<string, string>,
    opts?: BaseOpts & { default?: string; uiHint?: "horizontal_enum" },
  ): Param<string>;
  export function query(
    key: string,
    opts?: BaseOpts & { filter?: string; maxPicks?: number },
  ): Param<Query>;
  export function group(
    name: string,
    opts: { key?: string; drivingParam?: string },
    children: Param[],
  ): Param<undefined>;
  export function visibleWhen<T>(
    paramName: string,
    equals: string | number | boolean,
    wrapped: Param<T>,
  ): Param<T>;

  export interface FeatureSpec {
    name: string;
    nameTemplate?: string;
    parameters: Param[];
  }
  export interface FeatureDefinition {
    readonly name: string;
    readonly nameTemplate?: string;
    readonly parameters: Param[];
  }
  export function feature(spec: FeatureSpec): FeatureDefinition;

  // ─── build side (geometry emission + queries, S2 contract) ────

  /** Entity classes a query can target. */
  export enum EntityType {
    VERTEX = "vertex",
    EDGE = "edge",
    FACE = "face",
    BODY = "body",
  }

  /** Hierarchical feature id; `sub()` extends it for sub-operations. */
  export interface Id {
    sub(name: string): Id;
    toString(): string;
  }

  /**
   * The mutable build context. `fd.op*` commit geometry into it and
   * `fd.query.evaluate` reads its current state — the Onshape
   * `context` model (S2).
   */
  export interface Context {
    readonly _brand: "fd.Context";
  }

  /** Resolved parameter values for a script instance (see {@link DefOf}). */
  export type Def = Record<string, unknown>;

  /** A handle to geometry emitted by an `fd.op*` call, queryable later. */
  export interface OpHandle {
    readonly id: string;
  }

  /** The `fd.*` namespace a script imports. */
  export const fd: {
    feature: typeof feature;
    length: typeof length;
    angle: typeof angle;
    real: typeof real;
    bool: typeof bool;
    str: typeof str;
    vec2: typeof vec2;
    vec3: typeof vec3;
    enumParam: typeof enumParam;
    query: typeof query;
    group: typeof group;
    visibleWhen: typeof visibleWhen;

    /** Lazy query builders + evaluation (S2/S3). */
    readonly q: {
      createdBy(id: Id, kind: EntityType): Query;
      evaluate(context: Context, q: Query): OpHandle[];
    };

    // Geometry-emission ops resolve in S2; declared for author types.
    opBox(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opSphere(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opCylinder(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opExtrude(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opRevolve(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opHole(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opFillet(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opChamfer(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opShell(context: Context, id: Id, def: Record<string, unknown>): OpHandle;
    opBoolean(context: Context, id: Id, def: Record<string, unknown>): OpHandle;

    /** Call another feature (script or built-in) into this context (S2). */
    call(type: string, def: Record<string, unknown>): OpHandle;
  };
}
