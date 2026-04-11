/**
 * `fd` main-thread Outer API (K6a).
 *
 * Narrow, typed façade the React UI and the FeatureDialog use
 * to talk to the kernel. Wraps the existing main-thread
 * `Kernel` class from `web/src/kernel.ts` and exposes the
 * method surface the FeatureDialog generator needs:
 *
 *   * `listFeatureTypes()` — every registered feature type name.
 *   * `schemaForType(type)` — parsed schema ready for dialog
 *     rendering.
 *   * `listFeatures()` — current feature spine summaries.
 *
 * This file is the main-thread complement to the Outer API
 * client in `web/src/outerApi.ts` (K4.5.6), which speaks to
 * the regeneration worker via `postMessage`. Today's main
 * thread owns the authoritative kernel instance, so the
 * FeatureDialog commits flow through this synchronous façade.
 * A later slice will migrate the read path to the async worker
 * client once the kernel instance moves into the worker fully;
 * the method surface is deliberately kept narrow so that
 * migration is a one-file swap at the call sites.
 */

import type { Kernel, FeatureSummary } from "../kernel";
import {
  parseFeatureSchema,
  type FeatureSchema,
} from "../widgets/schema";

/**
 * The API object the React tree consumes. Fresh instances per
 * kernel are fine — construction is cheap and the kernel
 * holds all the state.
 */
export interface FdApi {
  /** All feature types the kernel registry knows about. */
  listFeatureTypes(): string[];
  /** Parsed schema for a single feature type. Throws if the
   *  type name isn't registered. */
  schemaForType(type: string): FeatureSchema;
  /** Current feature-spine summaries for the outliner. */
  listFeatures(): FeatureSummary[];
  /** K6b.2: every sketch-tool type the kernel knows. Sketch
   *  tools live in a separate registry from top-level features. */
  listSketchToolTypes(): string[];
  /** K6b.2: parsed schema for a single sketch tool. Throws if
   *  the name isn't a registered sketch tool. */
  schemaForSketchToolType(type: string): FeatureSchema;
}

export function createFdApi(kernel: Kernel): FdApi {
  return {
    listFeatureTypes() {
      return kernel.listFeatureTypes();
    },
    schemaForType(type) {
      return parseFeatureSchema(kernel.schemaForType(type));
    },
    listFeatures() {
      return kernel.listFeatures();
    },
    listSketchToolTypes() {
      return kernel.listSketchToolTypes();
    },
    schemaForSketchToolType(type) {
      return parseFeatureSchema(kernel.schemaForSketchToolType(type));
    },
  };
}
