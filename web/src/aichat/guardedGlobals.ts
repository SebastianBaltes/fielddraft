/**
 * FieldDraft's debug globals — blocked for the assistant (ticket #347).
 *
 * They exist only in dev builds, but baseaichat's guard is a BLOCKLIST since
 * v0.2.0: a free `__fdStore` resolves, and the model would be holding the raw
 * kernel with no curation, no validation and no undo semantics — straight past
 * `fieldDraftApi.ts`, which is the actual security boundary. There is nothing in
 * them the API does not offer safely.
 *
 * The SAME list guards agent-written FieldSkript (scriptProposals.ts): a script
 * runs in our realm too, so the escape hatch would be identical.
 *
 * Pinned by guardPolicy.test.ts.
 */
export const GUARDED_APP_GLOBALS = [
  "__fdStore",
  "__fdKernel",
  "__threeRenderer",
  "__fdInvalidate",
  "__fdSketchCamera",
  "__fdSketchToScreen",
  "__fdPerfMonitor",
  "__fdBenchmark",
  "__fdTelemetry",
  "__fdActionLog",
] as const;
